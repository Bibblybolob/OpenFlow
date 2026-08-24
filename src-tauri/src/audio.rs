use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use thiserror::Error;

/// Upload sample rate. Speech STT models operate at 16 kHz natively; feeding
/// them resampled mono audio cuts the payload ~3x versus 48 kHz with no
/// transcription quality loss.
const TARGET_SAMPLE_RATE: u32 = 16_000;

/// A recording whose voiced spans total less than this is treated as an
/// accidental/noisy press, not dictation.
const MIN_VOICED_MS: u32 = 300;

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
    /// Slow-tracking estimate of ambient noise (raw RMS). Falls quickly,
    /// rises barely and is capped — used only for the voiced decision.
    floor: f32,
    /// Peak-decay envelope of raw RMS feeding the bar display.
    env: f32,
}

/// Raw RMS that maps to a full-scale bar. Chosen so conversational speech
/// at default mic gain fills the waveform; quiet rooms don't shrink it and
/// loud ones don't clip it into uselessness below shouting.
const BAR_FULL_SCALE: f32 = 0.18;
/// Envelope decay per callback (~30/s): peaks hold, then fall in ~200ms.
const ENV_DECAY: f32 = 0.86;
/// Below this absolute RMS nothing counts as voice, whatever the floor.
const VOICED_ABS_MIN: f32 = 0.008;
/// Voice must clear the tracked floor by this multiple.
const VOICED_FLOOR_MULT: f32 = 2.5;
/// Floor growth per callback: ~0.0001/s — minutes to drift up, so long
/// takes never sag. Hard cap keeps noisy rooms from eating the gate.
const FLOOR_RISE: f32 = 0.000004;
const FLOOR_CAP: f32 = 0.03;

fn next_floor(floor: f32, rms: f32) -> f32 {
    if rms < floor {
        floor * 0.95 + rms * 0.05
    } else {
        (floor + FLOOR_RISE).min(FLOOR_CAP)
    }
}

fn is_voiced(floor: f32, rms: f32) -> bool {
    rms > VOICED_ABS_MIN && rms > floor * VOICED_FLOOR_MULT
}

/// A finished dictation capture, ready for upload.
pub struct Recording {
    pub duration_ms: i64,
    /// Mono 16 kHz 16-bit PCM WAV, held in memory (no temp file).
    pub wav: Vec<u8>,
    /// Loudest 20ms frame RMS of the *raw* input, before suppression.
    /// Feeds the hallucination guard: near-silent captures that still
    /// produce transcript text are treated as model confabulation.
    pub max_frame_rms: f32,
}

/// How far above the tracked noise floor audio must climb to count as
/// voice. Maps to the Settings sensitivity preset (Low/Medium/High).
pub const VAD_MULT_LOW: f32 = 2.5;
pub const VAD_MULT_MEDIUM: f32 = 3.5;
pub const VAD_MULT_HIGH: f32 = 5.0;

/// Segments of a recording that cleared the adaptive voice gate.
pub struct VoiceRegions {
    /// (start, end) sample indices of voiced spans, padded ±120ms and merged.
    pub segments: Vec<(usize, usize)>,
    /// Peak frame RMS of the whole buffer (raw scale).
    pub max_rms: f32,
    /// Total voiced duration across segments (pre-padding), ms.
    pub voiced_ms: u32,
}

pub struct AudioEngine {
    shared: Arc<Mutex<Shared>>,
    stream: Option<cpal::Stream>,
    started_at: Option<Instant>,
    on_level: Option<Arc<dyn Fn(f32, bool) + Send + Sync>>,
    device_pref: Option<String>,
    noise_suppression: bool,
    vad_mult: f32,
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
                floor: 0.01,
                env: 0.0,
            })),
            stream: None,
            started_at: None,
            on_level: None,
            device_pref: None,
            noise_suppression: true,
            vad_mult: VAD_MULT_MEDIUM,
        }
    }

    /// Configures per-session conditioning: RNNoise-style suppression on/off
    /// and the voice-gate sensitivity multiplier.
    pub fn set_processing(&mut self, noise_suppression: bool, vad_mult: f32) {
        self.noise_suppression = noise_suppression;
        self.vad_mult = vad_mult;
    }

    /// Selects a preferred input device by name. `None` (or the name of a
    /// device that later disappears) falls back to the system default.
    pub fn set_device(&mut self, name: Option<String>) {
        self.device_pref = name.filter(|n| !n.trim().is_empty());
    }

    /// Registers a callback receiving (bar, voiced) roughly 30x per second
    /// while recording. `bar` is a peak-decay envelope of the raw RMS
    /// normalized to a fixed speech reference — lively regardless of room
    /// noise. `voiced` mirrors the offline gate's floor-relative decision
    /// and drives silence detection. Persists across sessions.
    pub fn set_level_callback(&mut self, cb: impl Fn(f32, bool) + Send + Sync + 'static) {
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

        // Stage A — neural noise suppression (RNNoise family): keyboard,
        // fan and room noise drop before anything downstream sees them.
        let mut work = samples;
        let mut work_rate = capture_rate;
        if self.noise_suppression {
            work = suppress_noise(&work, capture_rate);
            work_rate = 48_000;
        }

        // Stage B — adaptive voice isolation: hysteresis gate around the
        // tracked noise floor keeps only voiced spans. Recordings where
        // nothing was actually spoken are dropped here, which is what
        // stops STT models from hallucinating text out of silence.
        let regions = voice_regions(&work, work_rate, self.vad_mult);
        if regions.voiced_ms < MIN_VOICED_MS {
            eprintln!(
                "voice isolation: discarded {}ms session ({}ms voiced below floor)",
                elapsed_ms, regions.voiced_ms
            );
            return Ok(None);
        }
        let cropped: Vec<f32> = regions
            .segments
            .iter()
            .flat_map(|(a, b)| work[*a..*b].iter().copied())
            .collect();

        let wav = encode_wav(&resample_to_16k(&cropped, work_rate), TARGET_SAMPLE_RATE)?;
        Ok(Some(Recording {
            duration_ms: elapsed_ms,
            wav,
            max_frame_rms: regions.max_rms,
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
    level_cb: &Option<Arc<dyn Fn(f32, bool) + Send + Sync>>,
    iter: impl Iterator<Item = f32>,
    frame_count: usize,
) {
    let mut s = shared.lock().unwrap();
    s.samples.extend(iter);
    if let Some(cb) = level_cb {
        let start = s.samples.len().saturating_sub(frame_count);
        let recent = &s.samples[start..];
        let rms = (recent.iter().map(|x| x * x).sum::<f32>() / recent.len().max(1) as f32).sqrt();
        s.env = rms.max(s.env * ENV_DECAY);
        s.floor = next_floor(s.floor, rms);
        cb((s.env / BAR_FULL_SCALE).min(1.0), is_voiced(s.floor, rms));
    }
}

/// Adaptive voice-isolation gate. Tracks the recording's own noise floor
/// (10th-percentile 20ms-frame RMS) and marks spans as voiced with
/// hysteresis — audio enters at `floor x mult` and holds until it falls
/// below ~45% of that for a 250ms hangover, so short plosives and
/// mid-word dips don't split segments. Voiced spans are padded ±120ms;
/// everything between them is discarded.
fn voice_regions(samples: &[f32], sample_rate: u32, mult: f32) -> VoiceRegions {
    let frame_len = ((sample_rate as f32 * 0.02) as usize).max(1); // 20 ms
    let n_frames = samples.len() / frame_len;
    if n_frames == 0 {
        return VoiceRegions {
            segments: Vec::new(),
            max_rms: 0.0,
            voiced_ms: 0,
        };
    }
    let mut rms: Vec<f32> = Vec::with_capacity(n_frames);
    for frame in 0..n_frames {
        let seg = &samples[frame * frame_len..(frame + 1) * frame_len];
        rms.push((seg.iter().map(|x| x * x).sum::<f32>() / seg.len() as f32).sqrt());
    }
    let max_rms = rms.iter().copied().fold(0.0f32, f32::max);
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[sorted.len() / 10];
    let enter = (floor * mult).max(0.006);
    let exit = enter * 0.45;
    let hangover_frames = 12; // ~250ms

    let pad = 6; // ±120ms
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut in_voice = false;
    let mut seg_start = 0usize;
    let mut last_loud = 0usize;
    let mut raw_voiced_ms = 0u32;
    for (i, &v) in rms.iter().enumerate() {
        if v > exit {
            last_loud = i;
        }
        match (in_voice, v > enter) {
            (false, true) => {
                in_voice = true;
                seg_start = i.saturating_sub(pad);
                raw_voiced_ms += 20;
                last_loud = i;
            }
            (true, true) => raw_voiced_ms += 20,
            (true, false) => {
                if i - last_loud > hangover_frames {
                    in_voice = false;
                    push_segment(
                        &mut segments,
                        seg_start,
                        (last_loud + 1 + pad).min(n_frames),
                    );
                }
            }
            (false, false) => {}
        }
    }
    if in_voice {
        push_segment(
            &mut segments,
            seg_start,
            (last_loud + 1 + pad).min(n_frames),
        );
    }
    let to_samples = |f: usize| (f * frame_len).min(samples.len());
    let segments = segments
        .into_iter()
        .map(|(a, b)| (to_samples(a), to_samples(b)))
        .filter(|(a, b)| b > a)
        .collect();
    VoiceRegions {
        segments,
        max_rms,
        voiced_ms: raw_voiced_ms,
    }
}

fn push_segment(segments: &mut Vec<(usize, usize)>, start: usize, end: usize) {
    if let Some(last) = segments.last_mut() {
        if start <= last.1 {
            last.1 = last.1.max(end);
            return;
        }
    }
    segments.push((start, end));
}

/// RNNoise-family suppression (via pure-Rust `nnnoiseless`). Returns audio
/// at a fixed 48 kHz regardless of input rate — callers must treat the
/// result as 48 kHz from here on. The model consumes 480-sample frames of
/// 16-bit-range samples, so input is resampled/scaled in and processed
/// through one stateful pass. A zeroed warm-up frame primes the filter so
/// no real sample loses its output to fade-in.
fn suppress_noise(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    use nnnoiseless::DenoiseState;
    const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 @ 48kHz

    let upsampled = if sample_rate == 48_000 {
        samples.to_vec()
    } else {
        resample_linear(samples, sample_rate, 48_000)
    };
    if upsampled.is_empty() {
        return upsampled;
    }

    let mut denoise = DenoiseState::new();
    let mut output: Vec<f32> = Vec::with_capacity(upsampled.len());
    let mut out_buf = [0.0f32; FRAME];
    // Warm-up: prime the stateful features with silence, discard output.
    denoise.process_frame(&mut out_buf, &[0.0f32; FRAME]);

    for chunk in upsampled.chunks(FRAME) {
        let mut input = [0.0f32; FRAME];
        // nnnoiseless expects 16-bit-range PCM, not unit floats.
        for (dst, src) in input.iter_mut().zip(chunk) {
            *dst = *src * 32768.0;
        }
        denoise.process_frame(&mut out_buf, &input);
        output.extend_from_slice(&out_buf[..chunk.len()]);
    }
    output
        .into_iter()
        .map(|v| (v / 32768.0).clamp(-1.0, 1.0))
        .collect()
}

/// Linear-interpolation resample between arbitrary rates (used to reach
/// nnnoiseless's fixed 48 kHz operating rate).
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = f64::from(from_rate) / f64::from(to_rate);
    let out_len = ((samples.len() as f64) / ratio).floor() as usize;
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

    #[test]
    fn voice_regions_find_tone_between_silence() {
        // 0.5 s silence, 0.3 s tone, 0.5 s silence at 48 kHz.
        let rate = 48_000u32;
        let mut samples = vec![0.0f32; rate as usize / 2];
        samples.extend((0..rate as usize * 3 / 10).map(|i| ((i as f32) / 50.0).sin() * 0.5));
        samples.resize(samples.len() + rate as usize / 2, 0.0);

        let regions = voice_regions(&samples, rate, VAD_MULT_MEDIUM);
        assert_eq!(regions.segments.len(), 1, "one voiced span");
        let (start, end) = regions.segments[0];
        assert!(
            (start as i64 - rate as i64 / 2).abs() < (rate as f32 * 0.2) as i64,
            "start {start}"
        );
        assert!(end > start);
        assert!((300..=500).contains(&regions.voiced_ms));
        assert!(regions.max_rms > 0.3);
    }

    #[test]
    fn voice_regions_all_quiet_yields_nothing_voiced() {
        let samples = vec![0.001f32; 48_000]; // 1s of near-silence
        let regions = voice_regions(&samples, 48_000, VAD_MULT_MEDIUM);
        assert!(regions.segments.is_empty());
        assert!(regions.voiced_ms < 300, "voiced {}ms", regions.voiced_ms);
        assert!(regions.max_rms < 0.005);
    }

    #[test]
    fn voice_regions_hangover_keeps_plosive_tail_together() {
        // Two short bursts 150ms apart must merge into one segment.
        let rate = 48_000u32;
        let mut samples = vec![0.0f32; rate as usize];
        for burst in [0, 0] {
            let at = rate as usize / 4 + burst * rate as usize / 100;
            for i in 0..rate as usize / 20 {
                samples[at + i] = ((i as f32) / 40.0).sin() * 0.6;
            }
        }
        let gap = rate as usize / 100 * 3 / 2; // 150ms
        let _ = gap;
        let regions = voice_regions(&samples, rate, VAD_MULT_MEDIUM);
        assert_eq!(regions.segments.len(), 1, "{:?}", regions.segments);
    }

    #[test]
    fn suppress_noise_attenuates_stationary_noise() {
        // Stationary broadband noise is RNNoise's core target; pitched
        // content is deliberately preserved by its harmonic filter.
        let rate = 48_000u32;
        let mut lcg = 0x2545F4914F6CDD1Du64;
        let input: Vec<f32> = (0..rate as usize)
            .map(|_| {
                lcg = lcg
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((lcg >> 33) as f32 / u32::MAX as f32 - 0.5) * 0.1
            })
            .collect();
        let out = suppress_noise(&input, rate);
        let energy_in: f32 = input.iter().map(|x| x * x).sum();
        let energy_out: f32 = out.iter().map(|x| x * x).sum();
        assert!(
            energy_out < energy_in * 0.5,
            "stationary noise survived: {energy_out} vs {energy_in}"
        );
    }

    #[test]
    fn suppress_noise_preserves_gated_speech_like_tone() {
        // A tone gated on/off every ~250ms has speech-like transients;
        // RNNoise's VAD should keep most of it audible.
        let rate = 48_000u32;
        let period = rate as usize / 4;
        let input: Vec<f32> = (0..rate as usize)
            .map(|i| {
                let on = (i / (period / 2)).is_multiple_of(2);
                if on {
                    ((i as f32) / 60.0).sin() * 0.4
                } else {
                    0.0
                }
            })
            .collect();
        let out = suppress_noise(&input, rate);
        assert!(out.iter().all(|v| v.is_finite()));
        let peak_out = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak_out > 0.08, "gated tone crushed, peak {peak_out}");
    }

    #[test]
    fn suppress_noise_resamples_non_48k_input() {
        let input: Vec<f32> = (0..44_100)
            .map(|i| ((i as f32) / 80.0).sin() * 0.3)
            .collect();
        let out = suppress_noise(&input, 44_100);
        // Contract: output is 48 kHz — expect the proportional length.
        let expected = (input.len() as f64 * 48_000.0 / 44_100.0) as i64;
        assert!(
            (out.len() as i64 - expected).abs() < 500,
            "len {} want ~{expected}",
            out.len()
        );
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn envelope_holds_peaks_and_decays() {
        let mut env = 0.0f32;
        for _ in 0..5 {
            env = 0.3f32.max(env * ENV_DECAY);
        }
        assert!((env - 0.3).abs() < 1e-6);
        for _ in 0..40 {
            env = 0.0f32.max(env * ENV_DECAY);
        }
        assert!(env < 0.3 * 0.86f32.powi(30), "decayed too slowly: {env}");
    }

    #[test]
    fn bar_scale_maps_speech_near_full() {
        assert!((0.18f32 / BAR_FULL_SCALE).min(1.0) >= 1.0);
        assert!(
            0.09 / BAR_FULL_SCALE > 0.45,
            "half-speech should be visible"
        );
    }

    #[test]
    fn voiced_needs_floor_clearance_or_absolute_minimum() {
        assert!(!is_voiced(0.02, 0.005), "below absolute min");
        assert!(!is_voiced(0.02, 0.04), "inside the floor margin");
        assert!(is_voiced(0.02, 0.06));
        // Capped floor keeps very noisy rooms honest.
        assert!(!is_voiced(FLOOR_CAP, FLOOR_CAP * VOICED_FLOOR_MULT * 0.9));
        assert!(is_voiced(FLOOR_CAP, FLOOR_CAP * VOICED_FLOOR_MULT * 1.2));
    }

    #[test]
    fn floor_drift_is_negligible_during_speech() {
        let mut floor = 0.01f32;
        for _ in 0..1800 {
            floor = next_floor(floor, 0.2); // one minute of continuous speech
        }
        assert!(floor < 0.02, "floor crept too high during speech: {floor}");
        assert!(floor <= FLOOR_CAP);
    }

    #[test]
    fn resample_linear_changes_length_proportionally() {
        let input: Vec<f32> = vec![0.0; 48_000];
        let out = resample_linear(&input, 48_000, 24_000);
        assert_eq!(out.len(), 24_000);
    }
}
