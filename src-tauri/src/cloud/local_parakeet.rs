//! Optional on-device Parakeet TDT transcription model assets.
//!
//! The recognizer is kept behind the `parakeet` feature while the model
//! downloader is validated. The archive is extracted into a staging
//! directory and atomically renamed only after every required file is
//! present and non-empty, so an interrupted download can never look usable.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use bzip2::read::BzDecoder;
use serde::Serialize;
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use tauri::{AppHandle, Emitter};

use crate::store::{Result, StoreError};

pub const MODEL_ID: &str = "parakeet-tdt-0.6b-v3";

const MODEL_ARCHIVE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";
const REQUIRED_FILES: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

static DOWNLOAD_IN_FLIGHT: OnceLock<Mutex<bool>> = OnceLock::new();
static RECOGNIZER_CACHE: OnceLock<Mutex<Option<std::sync::Arc<OfflineRecognizer>>>> =
    OnceLock::new();

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    model: String,
    downloaded_mb: u64,
    total_mb: u64,
}

pub fn model_dir() -> PathBuf {
    super::local_stt::models_dir().join(MODEL_ID)
}

pub fn is_downloaded() -> bool {
    bundle_is_complete(&model_dir())
}

/// Transcribes a mono 16 kHz WAV using the official Parakeet TDT bundle.
/// Parakeet is an offline recognizer, so the prompt and language hint used by
/// Whisper are deliberately not applied here.
pub fn transcribe_local(
    wav_bytes: &[u8],
    _language: &str,
    _prompt: Option<&str>,
) -> Result<crate::cloud::stt::TranscriptionResult> {
    if !is_downloaded() {
        return Err(StoreError::Other(format!(
            "on-device model \"{MODEL_ID}\" not downloaded yet — get it in Settings → Transcription"
        )));
    }

    let samples = super::local_stt::decode_wav_mono(wav_bytes)?;
    if samples.is_empty() {
        return Err(StoreError::Other(
            "Parakeet transcription received an empty audio capture".to_string(),
        ));
    }
    let recognizer = load_recognizer()?;
    let stream = recognizer.create_stream();
    stream.accept_waveform(16_000, &samples);
    recognizer.decode(&stream);
    let result = stream.get_result().ok_or_else(|| {
        StoreError::Other("Parakeet transcription did not return a result".to_string())
    })?;
    let text = result.text.trim().to_string();
    Ok(crate::cloud::stt::TranscriptionResult {
        text: text.clone(),
        raw_text: text,
    })
}

fn load_recognizer() -> Result<std::sync::Arc<OfflineRecognizer>> {
    let cache = RECOGNIZER_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some(recognizer) = guard.as_ref() {
        return Ok(std::sync::Arc::clone(recognizer));
    }

    let dir = model_dir();
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(dir.join("encoder.int8.onnx").to_string_lossy().into_owned()),
        decoder: Some(dir.join("decoder.int8.onnx").to_string_lossy().into_owned()),
        joiner: Some(dir.join("joiner.int8.onnx").to_string_lossy().into_owned()),
    };
    config.model_config.tokens = Some(dir.join("tokens.txt").to_string_lossy().into_owned());
    config.model_config.model_type = Some("nemo_transducer".to_string());

    let started = std::time::Instant::now();
    let recognizer = OfflineRecognizer::create(&config)
        .ok_or_else(|| StoreError::Other("failed to load Parakeet model bundle".to_string()))?;
    eprintln!(
        "Parakeet model loaded in {}ms",
        started.elapsed().as_millis()
    );
    let recognizer = std::sync::Arc::new(recognizer);
    *guard = Some(std::sync::Arc::clone(&recognizer));
    Ok(recognizer)
}

/// Downloads and validates the official Parakeet int8 archive.
pub fn download_model(app: &AppHandle) -> Result<PathBuf> {
    if is_downloaded() {
        let _ = app.emit(
            "local-parakeet-progress",
            serde_json::json!({ "type": "done", "model": MODEL_ID }),
        );
        return Ok(model_dir());
    }

    let in_flight = DOWNLOAD_IN_FLIGHT.get_or_init(|| Mutex::new(false));
    let mut guard = in_flight.lock().unwrap();
    if *guard {
        return Err(StoreError::Other(
            "Parakeet is already downloading".to_string(),
        ));
    }
    *guard = true;
    drop(guard);

    let result = download_model_inner(app);
    *in_flight.lock().unwrap() = false;
    result
}

fn download_model_inner(app: &AppHandle) -> Result<PathBuf> {
    let models_dir = super::local_stt::models_dir();
    fs::create_dir_all(&models_dir)?;

    let archive_path = models_dir.join(format!(".{MODEL_ID}.tar.bz2.part"));
    let staging_dir = models_dir.join(format!(".{MODEL_ID}.part"));
    let destination = model_dir();
    let previous_dir = models_dir.join(format!(".{MODEL_ID}.previous"));

    // A previous failed extraction is never reused. The final destination is
    // left intact until a complete replacement is ready.
    let _ = fs::remove_file(&archive_path);
    let _ = fs::remove_dir_all(&staging_dir);
    let _ = fs::remove_dir_all(&previous_dir);

    let result = (|| {
        let mut output = File::create(&archive_path)?;
        let mut downloaded = 0u64;
        let mut last_emit_mb = 0u64;
        let (expected_size, _downloaded) = super::stream_download(MODEL_ARCHIVE_URL, |chunk| {
            output.write_all(chunk)?;
            downloaded += chunk.len() as u64;
            let downloaded_mb = downloaded / (1024 * 1024);
            if downloaded_mb > last_emit_mb {
                last_emit_mb = downloaded_mb;
                let _ = app.emit(
                    "local-parakeet-progress",
                    DownloadProgress {
                        model: MODEL_ID.to_string(),
                        downloaded_mb,
                        total_mb: 0,
                    },
                );
            }
            Ok(())
        })?;
        output.flush()?;
        let archive_size = output
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        drop(output);
        if archive_size == 0 || expected_size.is_some_and(|expected| archive_size != expected) {
            return Err(StoreError::Other(
                "Parakeet download looks truncated — please retry".to_string(),
            ));
        }

        let archive_file = File::open(&archive_path)?;
        extract_archive(BzDecoder::new(archive_file), &staging_dir)?;
        if !bundle_is_complete(&staging_dir) {
            return Err(StoreError::Other(
                "Parakeet archive is missing required model files".to_string(),
            ));
        }

        // Keep the previous valid bundle until the complete staging bundle
        // is in place. Restore it if the final rename fails.
        if destination.exists() {
            fs::rename(&destination, &previous_dir)?;
        }
        if let Err(error) = fs::rename(&staging_dir, &destination) {
            if previous_dir.exists() {
                let _ = fs::rename(&previous_dir, &destination);
            }
            return Err(error.into());
        }
        let _ = fs::remove_dir_all(&previous_dir);
        let _ = app.emit(
            "local-parakeet-progress",
            serde_json::json!({ "type": "done", "model": MODEL_ID }),
        );
        Ok(destination)
    })();

    let _ = fs::remove_file(&archive_path);
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging_dir);
    }
    result
}

fn bundle_is_complete(dir: &Path) -> bool {
    REQUIRED_FILES.iter().all(|name| {
        let path = dir.join(name);
        path.is_file()
            && path
                .metadata()
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false)
    })
}

fn extract_archive<R: Read>(reader: R, staging_dir: &Path) -> Result<()> {
    fs::create_dir_all(staging_dir)?;
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !REQUIRED_FILES.contains(&name) {
            continue;
        }
        // Only the allow-listed basename is used, so archive directory names
        // and traversal components cannot escape the staging directory.
        let mut output = File::create(staging_dir.join(name))?;
        std::io::copy(&mut entry, &mut output)?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::write::BzEncoder;
    use bzip2::Compression;
    use std::io::Cursor;
    use tar::{Builder, Header};

    fn archive_fixture(include: &[&str]) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let encoder = BzEncoder::new(&mut compressed, Compression::fast());
            let mut builder = Builder::new(encoder);
            for name in include {
                let contents = format!("fixture-{name}");
                let mut header = Header::new_gnu();
                header.set_size(contents.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, format!("model/{name}"), contents.as_bytes())
                    .unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap();
        }
        compressed
    }

    #[test]
    fn archive_extraction_requires_all_model_files() {
        let root = std::env::temp_dir().join(format!(
            "flowclone-parakeet-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        extract_archive(
            BzDecoder::new(Cursor::new(archive_fixture(&REQUIRED_FILES))),
            &root,
        )
        .unwrap();
        assert!(bundle_is_complete(&root));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incomplete_archive_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "flowclone-parakeet-incomplete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        extract_archive(
            BzDecoder::new(Cursor::new(archive_fixture(&REQUIRED_FILES[..3]))),
            &root,
        )
        .unwrap();
        assert!(!bundle_is_complete(&root));
        let _ = fs::remove_dir_all(root);
    }

    /// Manual hardware gate: point FLOWCLONE_PARAKEET_TEST_WAV at a 16 kHz
    /// mono wav and FLOWCLONE_PARAKEET_MODELS_DIR at a directory containing
    /// the extracted Parakeet bundle, then run with --ignored --nocapture.
    #[test]
    #[ignore = "manual: requires a downloaded Parakeet bundle and test wav"]
    fn parakeet_model_transcribes_real_speech() {
        let wav_path =
            std::env::var("FLOWCLONE_PARAKEET_TEST_WAV").expect("FLOWCLONE_PARAKEET_TEST_WAV set");
        let models_dir = std::env::var("FLOWCLONE_PARAKEET_MODELS_DIR")
            .map(PathBuf::from)
            .expect("FLOWCLONE_PARAKEET_MODELS_DIR set");
        super::super::local_stt::init_models_dir(models_dir);
        assert!(is_downloaded(), "Parakeet model bundle missing on disk");

        let wav = fs::read(wav_path).expect("read test wav");
        let started = std::time::Instant::now();
        let result = transcribe_local(&wav, "en", None).expect("Parakeet transcription");
        eprintln!(
            "Parakeet cold end-to-end in {:?}: {:?}",
            started.elapsed(),
            result.text
        );
        assert!(!result.text.trim().is_empty(), "expected some transcript");

        let started = std::time::Instant::now();
        let warm_result = transcribe_local(&wav, "en", None).expect("warm Parakeet transcription");
        eprintln!(
            "Parakeet warm end-to-end in {:?}: {:?}",
            started.elapsed(),
            warm_result.text
        );
        assert_eq!(warm_result.text, result.text);
    }
}
