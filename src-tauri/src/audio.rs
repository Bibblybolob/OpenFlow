use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use thiserror::Error;

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

pub struct AudioEngine {
    shared: Arc<Mutex<Shared>>,
    stream: Option<cpal::Stream>,
    started_at: Option<Instant>,
    on_level: Option<Arc<dyn Fn(f32) + Send + Sync>>,
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
        }
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
        let device = host.default_input_device().ok_or(AudioError::NoDevice)?;
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

    /// Stops capture; writes captured audio as 16-bit PCM WAV to `path`.
    /// Returns the session duration in milliseconds, or `None` for taps too
    /// short to be a real dictation.
    pub fn stop(&mut self, path: &Path) -> Result<Option<i64>, AudioError> {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        let started_at = match self.started_at.take() {
            Some(t) => t,
            None => return Ok(None),
        };
        let mut shared = self.shared.lock().unwrap();
        let samples = std::mem::take(&mut shared.samples);
        let sample_rate = shared.sample_rate;
        drop(shared);

        let elapsed_ms = started_at.elapsed().as_millis() as i64;
        // Ignore accidental taps shorter than ~250ms.
        if elapsed_ms < 250 || samples.is_empty() {
            return Ok(None);
        }

        encode_wav(&samples, sample_rate, path)?;
        Ok(Some(elapsed_ms))
    }

    /// Stops capture and throws the audio away (Esc-cancel).
    pub fn discard(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        self.started_at = None;
        self.shared.lock().unwrap().samples.clear();
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

fn encode_wav(samples: &[f32], sample_rate: u32, path: &Path) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let file = File::create(path)?;
    let mut writer = hound::WavWriter::new(BufWriter::new(file), spec)?;
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        writer.write_sample((clamped * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_roundtrip() {
        let dir = std::env::temp_dir().join(format!("flowclone-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.wav");
        let samples: Vec<f32> = (0..4_800).map(|i| ((i as f32) / 100.0).sin()).collect();
        encode_wav(&samples, 48_000, &path).unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }
}
