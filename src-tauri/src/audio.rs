use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use thiserror::Error;

/// Upload sample rate. Speech STT models operate at 16 kHz natively; feeding
/// them resampled mono audio cuts the payload ~3x versus 48 kHz with no
/// transcription quality loss.
const TARGET_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no input device found")]
    NoDevice,
    #[error("audio stream error: {0}")]
    Stream(String),
    #[error("wav encode error: {0}")]
    Wav(#[from] hound::Error),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
}

struct Shared {
    samples: Vec<f32>,
    sample_rate: u32,
}

/// A finished dictation capture, ready for upload.
pub struct Recording {
    pub duration_ms: i64,
    /// Mono 16 kHz 16-bit PCM WAV, held in memory (no temp file).
    pub wav: Vec<u8>,
}

pub struct AudioEngine {
    shared: Arc<Mutex<Shared>>,
    stream: Option<cpal::Stream>,
    started_at: Option<Instant>,
    on_level: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    device_pref: Option<String>,
}

/// Names of every input device on the default host, for the Settings picker.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect::<Vec<_>>())
        .unwrap_or_default()
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                samples: Vec::new(),
                sample_rate: 48_000,
            })),
            stream: None,
            started_at: None,
            on_level: None,
            device_pref: None,
        }
    }

    /// Selects a preferred input device by name. `None` (or the name of a
    /// device that later disappears) falls back to the system default.
    pub fn set_device(&mut self, name: Option<String>) {
        self.device_pref = name.filter(|n| !n.trim().is_empty());
    }

    /// Registers a callback receiving input loudness (0.0..1.0) roughly
    /// 30x per second while recording. Persists across sessions.
    pub fn set_level_callback(&mut self, cb: impl Fn(f32) + Send + Sync + 'static) {
        self.on_level = Some(Arc::new(cb));
    }

    pub fn start(&mut self) -> Result<(), AudioError> {
        if self.stream.is_some() {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = match &self.device_pref {
            Some(name) => host
                .input_devices()
                .ok()
                .and_then(|mut devices| {
                    devices.find(|d| d.name().map(|n| &n == name).unwrap_or(false))
                })
                .or_else(|| {
                    eprintln!("mic \"{name}\" not found — using system default");
                    host.default_input_device()
                }),
            None => host.default_input_device(),
        }
        .ok_or(AudioError::NoDevice)?;
        let config = device
            .default_input_config()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        let sample_rate = config.sample_rate().0;
        let format = config.sample_format();

        let shared = Arc::clone(&self.shared);
        {
            let mut s = shared.lock().unwrap();
            s.samples.clear();
            s.sample_rate = sample_rate;
        }
        let err_fn = |e: cpal::StreamError| eprintln!("audio stream error: {e}");
        let cb_a = self.on_level.clone();
        let cb_b = self.on_level.clone();
        let cb_c = self.on_level.clone();

        let stream = match format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    push_levelled(&shared, &cb_a, data.iter().copied(), data.len())
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _| {
                    push_levelled(
                        &shared,
                        &cb_b,
                        data.iter().map(|&s| s.to_sample::<f32>()),
                        data.len(),
                    )
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &config.into(),
                move |data: &[u16], _| {
                    push_levelled(
                        &shared,
                        &cb_c,
                        data.iter().map(|&s| s.to_sample::<f32>()),
                        data.len(),
                    )
                },
                err_fn,
                None,
            ),
            other => {
                return Err(AudioError::Stream(format!(
                    "unsupported sample format: {other}"
                )))
            }
        }
        .map_err(|e| AudioError::Stream(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        self.stream = Some(stream);
        self.started_at = Some(Instant::now());
        Ok(())
    }

    /// Stops capture and encodes the audio in memory as a mono 16 kHz 16-bit
    /// PCM WAV, ready to upload. Returns `None` for taps too short to be a
    /// real dictation.
    pub fn stop(&mut self) -> Result<Option<Recording>, AudioError> {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        let started_at = match self.started_at.take() {
            Some(t) => t,
            None => return Ok(None),
        };
        let (samples, capture_rate) = {
            let mut shared = self.shared.lock().unwrap();
            let samples = std::mem::take(&mut shared.samples);
            (samples, shared.sample_rate)
        };

        let elapsed_ms = started_at.elapsed().as_millis() as i64;
        // Ignore accidental taps shorter than ~250ms.
        if elapsed_ms < 250 || samples.is_empty() {
            return Ok(None);
        }

        let wav = encode_wav(&resample_to_16k(&samples, capture_rate), TARGET_SAMPLE_RATE)?;
        Ok(Some(Recording {
            duration_ms: elapsed_ms,
            wav,
        }))
    }

    /// Stops capture and throws the audio away (Esc-cancel).
    pub fn discard(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        self.started_at = None;
        self.shared.lock().unwrap().samples.clear();
    }

    /// Suspends the capture stream without ending the session. Elapsed
    /// wall-clock time keeps counting, so paused stretches still count
    /// toward the session duration (v1 tradeoff).
    pub fn pause(&mut self) {
        if let Some(stream) = self.stream.as_ref() {
            let _ = stream.pause();
        }
    }

    /// Resumes a paused capture stream.
    pub fn resume(&mut self) {
        if let Some(stream) = self.stream.as_ref() {
            let _ = stream.play();
        }
    }

    /// Opens a short-lived capture stream to (a) trigger macOS's mic
    /// permission prompt on first use and (b) verify an input device works.
    pub fn probe(&mut self) -> Result<(), AudioError> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or(AudioError::NoDevice)?;
        let config = device
            .default_input_config()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        let format = config.sample_format();
        let err_fn = |e: cpal::StreamError| eprintln!("audio probe error: {e}");
        let stream = match format {
            cpal::SampleFormat::F32 => {
                device.build_input_stream(&config.into(), |_: &[f32], _| {}, err_fn, None)
            }
            cpal::SampleFormat::I16 => {
                device.build_input_stream(&config.into(), |_: &[i16], _| {}, err_fn, None)
            }
            cpal::SampleFormat::U16 => {
                device.build_input_stream(&config.into(), |_: &[u16], _| {}, err_fn, None)
            }
            other => {
                return Err(AudioError::Stream(format!(
                    "unsupported sample format: {other}"
                )))
            }
        }
        .map_err(|e| AudioError::Stream(e.to_string()))?;
        stream
            .play()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        std::thread::sleep(Duration::from_millis(120));
        drop(stream);
        Ok(())
    }
}

fn push_levelled(
    shared: &Arc<Mutex<Shared>>,
    level_cb: &Option<Arc<dyn Fn(f32) + Send + Sync>>,
    iter: impl Iterator<Item = f32>,
    frame_count: usize,
) {
    let mut s = shared.lock().unwrap();
    s.samples.extend(iter);
    if let Some(cb) = level_cb {
        let start = s.samples.len().saturating_sub(frame_count);
        let recent = &s.samples[start..];
        let rms = (recent.iter().map(|x| x * x).sum::<f32>() / recent.len().max(1) as f32).sqrt();
        cb((rms * 4.0).clamp(0.0, 1.0));
    }
}

/// Resamples mono audio to 16 kHz. Integer-rate inputs (48k, 44.1k is not —
/// handled by linear path) use average pooling, which doubles as a crude
/// anti-alias filter; other rates fall back to linear interpolation.
fn resample_to_16k(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == TARGET_SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    if sample_rate.is_multiple_of(TARGET_SAMPLE_RATE) {
        let factor = (sample_rate / TARGET_SAMPLE_RATE) as usize;
        return samples
            .chunks(factor)
            .map(|chunk| chunk.iter().sum::<f32>() / chunk.len() as f32)
            .collect();
    }
    let ratio = f64::from(sample_rate) / f64::from(TARGET_SAMPLE_RATE);
    let out_len = (samples.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = samples[idx];
        let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

/// Encodes mono samples as a 16-bit PCM WAV held entirely in memory.
/// The 44-byte RIFF header is written directly — cheaper than routing
/// through a writer abstraction for what is a fixed-layout buffer.
fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, hound::Error> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let v = (clamped * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn wav_roundtrip_in_memory() {
        let samples: Vec<f32> = (0..4_800).map(|i| ((i as f32) / 100.0).sin()).collect();
        let bytes = encode_wav(&samples, 48_000).unwrap();
        assert!(bytes.len() > 44, "wav payload missing");

        let mut reader = hound::WavReader::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        let decoded: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap())
            .map(|s| s.to_sample::<f32>())
            .collect();
        assert_eq!(decoded.len(), samples.len());
        assert!((decoded[0] - samples[0]).abs() < 0.001);
    }

    #[test]
    fn resample_integer_factor_average_pools() {
        // 3x decimation: each output sample is the mean of three inputs.
        let input: Vec<f32> = vec![0.0, 0.3, 0.6, 0.9, 1.2, 1.5];
        let out = resample_to_16k(&input, 48_000);
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.3).abs() < 1e-6);
        assert!((out[1] - 1.2).abs() < 1e-6);
    }

    #[test]
    fn resample_linear_path_handles_44k() {
        let input: Vec<f32> = (0..44_100).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_to_16k(&input, 44_100);
        // ~16000 output frames with a small floor-truncation margin.
        assert!((out.len() as i64 - 16_000).abs() < 10);
        assert!(
            out.iter().all(|s| s.is_finite()),
            "resampler produced NaN/inf"
        );
    }

    #[test]
    fn resample_passthrough_at_target_rate() {
        let input: Vec<f32> = vec![0.1, -0.2, 0.3];
        assert_eq!(resample_to_16k(&input, 16_000), input);
    }
}
