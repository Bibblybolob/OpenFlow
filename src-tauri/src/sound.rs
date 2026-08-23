//! Synthesized start/stop chimes played through the default output device.
//! Generated in-process (two short sine blips with fade edges) so there are
//! no binary assets to bundle, and no codec dependency.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Debug, Clone, Copy)]
pub enum Chime {
    /// Rising blip when a recording starts.
    Start,
    /// Falling blip when a session ends.
    Stop,
}

pub fn enabled(db: &crate::store::Store) -> bool {
    db.get_setting("soundEffects")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true)
}

/// Fires the chime on its own thread so callers never block the pipeline.
pub fn play(chime: Chime) {
    std::thread::spawn(move || play_blocking(chime));
}

fn play_blocking(chime: Chime) {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return;
    };
    let Ok(config) = device.default_output_config() else {
        return;
    };

    let segments: &[(f32, f32)] = match chime {
        Chime::Start => &[(660.0, 0.055), (880.0, 0.075)],
        Chime::Stop => &[(880.0, 0.055), (587.33, 0.085)],
    };
    let sample_rate = config.sample_rate().0;
    let samples = render(segments, sample_rate);
    let total_len = samples.len();

    let shared = Arc::new((samples, AtomicUsize::new(0)));
    let err_fn = |e: cpal::StreamError| eprintln!("chime stream error: {e}");

    macro_rules! build {
        ($ty:ty, $conv:expr) => {
            device
                .build_output_stream(
                    &config.into(),
                    move |data: &mut [$ty], _: &cpal::OutputCallbackInfo| {
                        let (samples, cursor) = &*shared;
                        let start = cursor.fetch_add(data.len(), Ordering::Relaxed);
                        for (i, frame) in data.iter_mut().enumerate() {
                            let v = samples.get(start + i).copied().unwrap_or(0.0);
                            *frame = $conv(v);
                        }
                    },
                    err_fn,
                    None,
                )
                .ok()
        };
    }

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build!(f32, |v: f32| v),
        cpal::SampleFormat::I16 => {
            build!(i16, |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        }
        cpal::SampleFormat::U16 => build!(u16, |v: f32| ((v.clamp(-1.0, 1.0) + 1.0) / 2.0
            * u16::MAX as f32) as u16),
        _ => return,
    };
    let Some(stream) = stream else { return };
    if stream.play().is_err() {
        return;
    }
    // Hold the stream alive until the buffer has been consumed.
    std::thread::sleep(Duration::from_secs_f32(
        total_len as f32 / sample_rate as f32 + 0.05,
    ));
    drop(stream);
}

fn render(segments: &[(f32, f32)], sample_rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    for &(freq, dur) in segments {
        let n = (sample_rate as f32 * dur) as usize;
        let fade = (sample_rate as f32 * 0.008).max(1.0); // ~8 ms edges
        for i in 0..n {
            let t = i as f32 / sample_rate as f32;
            let env = (i as f32 / fade)
                .min(1.0)
                .min(((n - 1 - i) as f32 / fade).min(1.0));
            out.push((2.0 * std::f32::consts::PI * freq * t).sin() * 0.22 * env);
        }
        // 15 ms gap between the two blips.
        let gap = (sample_rate as f32 * 0.015) as usize;
        out.resize(out.len() + gap, 0.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_chimes_are_short_and_bounded() {
        for segs in [
            &[(660.0f32, 0.055), (880.0, 0.075)][..],
            &[(880.0, 0.055), (587.33, 0.085)][..],
        ] {
            let s = render(segs, 48_000);
            assert!(s.len() < 48_000, "chime must stay under one second");
            assert!(
                s.iter().all(|v| v.is_finite() && v.abs() <= 0.23),
                "samples must be finite and quiet"
            );
            assert!(s.iter().any(|v| v.abs() > 0.01), "chime must be audible");
            // Ends silent thanks to the fade-out envelope.
            assert!(s.last().unwrap().abs() < 1e-3);
        }
    }
}
