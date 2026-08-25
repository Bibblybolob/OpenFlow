//! On-device transcription via whisper.cpp. Models are quantized ggml files
//! downloaded once from the official whisper.cpp Hugging Face mirror; the
//! loaded context is cached process-wide (model load dominates local
//! latency, ~200-500ms) while each dictation gets a cheap fresh state.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::cloud::stt::TranscriptionResult;
use crate::store::{Result, Store, StoreError};

pub struct LocalModel {
    pub id: &'static str,
    pub label: &'static str,
    /// Approximate download size in MB, for UI display.
    pub approx_mb: u64,
    /// Exact ggml filename on the whisper.cpp Hugging Face mirror. Most
    /// models ship quantized as q5_1, but large-v3-turbo only exists as
    /// q5_0 / q8_0 upstream.
    pub file_name: &'static str,
}

/// Multilingual quantized models from ggerganov/whisper.cpp. Multilingual
/// variants keep the app's 19-language support intact.
pub const LOCAL_MODELS: &[LocalModel] = &[
    LocalModel {
        id: "tiny",
        label: "Tiny — fastest, lower accuracy",
        approx_mb: 33,
        file_name: "ggml-tiny-q5_1.bin",
    },
    LocalModel {
        id: "base",
        label: "Base — balanced (recommended)",
        approx_mb: 60,
        file_name: "ggml-base-q5_1.bin",
    },
    LocalModel {
        id: "small",
        label: "Small — middle ground, slower",
        approx_mb: 188,
        file_name: "ggml-small-q5_1.bin",
    },
    LocalModel {
        id: "large-v3-turbo",
        label: "Large v3 Turbo — highest accuracy",
        approx_mb: 574,
        file_name: "ggml-large-v3-turbo-q5_0.bin",
    },
];

fn catalog_model(model_id: &str) -> Option<&'static LocalModel> {
    LOCAL_MODELS.iter().find(|m| m.id == model_id)
}

const MODEL_URL_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
static MODELS_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Cache of the loaded whisper context keyed by model id. Model loading is
/// by far the slowest part of local transcription; reuse it across sessions.
type ContextCache = Mutex<Option<(String, Arc<WhisperContext>)>>;
static CONTEXT_CACHE: OnceLock<ContextCache> = OnceLock::new();
static DOWNLOADS_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Called once at startup with the platform app-data dir.
pub fn init_models_dir(dir: PathBuf) {
    let _ = MODELS_DIR.set(dir);
}

pub(crate) fn models_dir() -> PathBuf {
    MODELS_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("flowclone-models"))
}

pub fn model_path(model_id: &str) -> PathBuf {
    match catalog_model(model_id) {
        Some(m) => models_dir().join(m.file_name),
        // Unknown ids fall back to the legacy q5_1 naming.
        None => models_dir().join(format!("ggml-{model_id}-q5_1.bin")),
    }
}

pub fn resolve_model_id(db: &Store) -> String {
    db.get_setting("sttLocalModel")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .filter(|m| LOCAL_MODELS.iter().any(|known| known.id == m))
        .unwrap_or_else(|| "base".to_string())
}

pub fn is_downloaded(model_id: &str) -> bool {
    model_path(model_id).try_exists().unwrap_or(false)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    model: String,
    downloaded_mb: u64,
    total_mb: u64,
}

/// Streams a model file to disk, emitting `local-model-progress` events so
/// the Hub can show a progress bar. Returns the finished path.
pub fn download_model(app: &AppHandle, model_id: &str) -> Result<PathBuf> {
    if !LOCAL_MODELS.iter().any(|m| m.id == model_id) {
        return Err(StoreError::Other(format!(
            "unknown local model: {model_id}"
        )));
    }
    let in_flight = DOWNLOADS_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = in_flight.lock().unwrap();
    if !guard.insert(model_id.to_string()) {
        return Err(StoreError::Other(
            "this model is already downloading".to_string(),
        ));
    }
    drop(guard);

    let result = download_model_inner(app, model_id);
    in_flight.lock().unwrap().remove(model_id);
    result
}

fn download_model_inner(app: &AppHandle, model_id: &str) -> Result<PathBuf> {
    let dest = model_path(model_id);
    if is_downloaded(model_id) {
        return Ok(dest);
    }

    let url = match catalog_model(model_id) {
        Some(m) => format!("{MODEL_URL_BASE}/{}?download=true", m.file_name),
        None => format!("{MODEL_URL_BASE}/ggml-{model_id}-q5_1.bin?download=true"),
    };
    let mut resp = super::http_client()?
        .get(&url)
        .send()
        .map_err(|e| StoreError::Other(format!("model download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(StoreError::Other(format!(
            "model download failed ({})",
            resp.status()
        )));
    }

    std::fs::create_dir_all(models_dir())?;
    let partial = dest.with_extension("part");
    let mut file = File::create(&partial)?;

    // Content-Length may be absent; fall back to the catalog approximation.
    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: usize = 0;
    let mut last_emit_mb: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| StoreError::Other(format!("model download failed: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n;

        let mb = (downloaded / (1024 * 1024)) as u64;
        if mb > last_emit_mb {
            last_emit_mb = mb;
            let _ = app.emit(
                "local-model-progress",
                DownloadProgress {
                    model: model_id.to_string(),
                    downloaded_mb: mb,
                    total_mb: total / (1024 * 1024),
                },
            );
        }
    }
    file.flush()?;
    drop(file);

    std::fs::rename(&partial, &dest)?;
    let _ = app.emit(
        "local-model-progress",
        serde_json::json!({ "type": "done", "model": model_id }),
    );
    Ok(dest)
}

/// Transcribes WAV bytes on-device. Expects the mono 16 kHz PCM produced by
/// AudioEngine; decodes through hound regardless of source layout.
pub fn transcribe_local(
    db: &Store,
    wav_bytes: &[u8],
    language: &str,
    prompt: Option<&str>,
) -> Result<TranscriptionResult> {
    let model_id = resolve_model_id(db);
    let path = model_path(&model_id);
    if !is_downloaded(&model_id) {
        return Err(StoreError::Other(format!(
            "on-device model \"{model_id}\" not downloaded yet — get it in Settings → Transcription"
        )));
    }

    let samples = decode_wav_mono(wav_bytes)?;
    let ctx = load_context(&model_id, &path)?;
    let mut state = ctx
        .create_state()
        .map_err(|e| StoreError::Other(format!("failed to start transcription: {e}")))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_translate(false);
    params.set_language(if language.is_empty() || language == "auto" {
        None
    } else {
        Some(language)
    });
    params.set_no_context(true);
    // Blank-result suppression: without it whisper emits empty/"you" style
    // segments over silence.
    params.set_suppress_blank(true);
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    if let Some(p) = prompt {
        let p = p.trim();
        if !p.is_empty() {
            params.set_initial_prompt(p);
        }
    }

    state
        .full(params, &samples)
        .map_err(|e| StoreError::Other(format!("local transcription failed: {e}")))?;

    let count = state
        .full_n_segments()
        .map_err(|e| StoreError::Other(format!("local transcription failed: {e}")))?;
    let mut text = String::new();
    for i in 0..count {
        match state.full_get_segment_text(i) {
            Ok(seg) => text.push_str(&seg),
            Err(e) => {
                return Err(StoreError::Other(format!(
                    "local transcription failed: {e}"
                )))
            }
        }
    }

    Ok(TranscriptionResult {
        text: text.trim().to_string(),
        raw_text: text.trim().to_string(),
    })
}

fn load_context(model_id: &str, path: &Path) -> Result<Arc<WhisperContext>> {
    let cache = CONTEXT_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some((cached_id, ctx)) = guard.as_ref() {
        if cached_id == model_id {
            return Ok(Arc::clone(ctx));
        }
    }
    let started = std::time::Instant::now();
    let ctx = WhisperContext::new_with_params(
        &path.to_string_lossy(),
        WhisperContextParameters {
            use_gpu: true,
            ..Default::default()
        },
    )
    .map_err(|e| StoreError::Other(format!("failed to load model: {e}")))?;
    eprintln!(
        "whisper model \"{model_id}\" loaded in {}ms",
        started.elapsed().as_millis()
    );
    *guard = Some((model_id.to_string(), Arc::new(ctx)));
    Ok(Arc::clone(&guard.as_ref().expect("just inserted").1))
}

/// Decodes 16-bit PCM WAV bytes into f32 samples in [-1, 1].
fn decode_wav_mono(wav_bytes: &[u8]) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::new(Cursor::new(wav_bytes))
        .map_err(|e| StoreError::Other(format!("bad audio capture: {e}")))?;
    Ok(reader
        .samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / i16::MAX as f32)
        .collect())
}

/// Manual hardware gate for a model: point FLOWCLONE_TURBO_TEST_WAV at a
/// 16 kHz mono wav and run
///   cargo test turbo_model_transcribes_real_speech -- --ignored --nocapture
/// Skips silently when the env var is absent so normal `cargo test` stays hermetic.
#[test]
#[ignore = "manual: set FLOWCLONE_TURBO_TEST_WAV to a 16 kHz mono wav"]
fn turbo_model_transcribes_real_speech() {
    let Ok(wav_path) = std::env::var("FLOWCLONE_TURBO_TEST_WAV") else {
        eprintln!("skipping: FLOWCLONE_TURBO_TEST_WAV not set");
        return;
    };
    // Default per-platform install layouts; override with FLOWCLONE_MODELS_DIR.
    let dir = std::env::var("FLOWCLONE_MODELS_DIR").map_or_else(
        |_| {
            #[cfg(target_os = "macos")]
            {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join("Library/Application Support/com.flowclone.app/models")
            }
            #[cfg(not(target_os = "macos"))]
            {
                std::env::var("APPDATA")
                    .map(|v| PathBuf::from(v).join("com.flowclone.app/models"))
                    .unwrap_or_else(|_| std::env::temp_dir().join("flowclone-models"))
            }
        },
        PathBuf::from,
    );
    init_models_dir(dir);
    assert!(
        is_downloaded("large-v3-turbo"),
        "turbo model missing on disk"
    );

    let db_path =
        std::env::temp_dir().join(format!("flowclone-turbo-gate-{}.db", std::process::id()));
    let db = Store::open(&db_path).expect("open scratch store");
    let _ = std::fs::remove_file(&db_path);
    // Point the resolver at turbo; resolve_model_id reads this setting.
    db.set_setting("sttLocalModel", &serde_json::json!("large-v3-turbo"))
        .expect("set model");

    let wav = std::fs::read(&wav_path).expect("read test wav");
    let started = std::time::Instant::now();
    let result = transcribe_local(&db, &wav, "en", None).expect("turbo transcription");
    eprintln!(
        "turbo end-to-end in {:?}: {:?}",
        started.elapsed(),
        result.text
    );
    assert!(!result.text.trim().is_empty(), "expected some transcript");
}
