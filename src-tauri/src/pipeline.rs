use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioEngine;
use crate::cloud::{llm, stt};
use crate::hotkey::{HotkeyEvent, SharedHotkeyConfig};
use crate::inject;
use crate::store::{Store, Transcript};

const MAX_SESSION_SECS: u64 = 360;
/// Two taps within this window (each under the tap threshold) enter
/// hands-free. Tight on purpose: accidental shift-taps while typing must
/// not open the mic.
const DOUBLE_TAP_WINDOW_MS: u64 = 350;
/// After a quick release, wait this long for a possible second tap before
/// judging the session finished. Keeps double-tap detection possible
/// without ever leaving the mic open indefinitely.
const TAP_JUDGE_WAIT_MS: u64 = 380;
/// Hands-free sessions end themselves after this much silence, so walking
/// away from the mic doesn't leave a giant accidental transcript behind.
/// Natural-speech endpointing: stop soon after the voice trails off —
/// sentence chunks have already been shipping during the session, so this
/// only ends the tail.
const HANDS_FREE_SILENCE_STOP_MS: u64 = 1500;
/// Raw-RMS ceiling under which a transcript is suspect: below this the mic
/// captured essentially silence/room tone, so whisper-style models tend to
/// confabulate stock phrases rather than transcribe speech.
const ARTIFACT_RAW_RMS: f32 = 0.02;
/// Live previews are phrase-level rather than per-audio-frame. This keeps
/// local model work bounded while still putting text on screen during a
/// natural pause in dictation.
const LIVE_PREVIEW_TICK_MS: u64 = 250;
const LIVE_PREVIEW_SILENCE_MS: u64 = 550;
const LIVE_PREVIEW_MIN_SEGMENT_MS: u64 = 400;
const LIVE_PREVIEW_MAX_SEGMENT_MS: u64 = 2200;

fn noise_suppression_enabled(db: &Store) -> bool {
    db.get_setting("noiseSuppression")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true)
}

fn vad_sensitivity_mult(db: &Store) -> f32 {
    match db
        .get_setting("voiceSensitivity")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .as_deref()
    {
        Some("low") => crate::audio::VAD_MULT_LOW,
        Some("high") => crate::audio::VAD_MULT_HIGH,
        _ => crate::audio::VAD_MULT_MEDIUM,
    }
}

/// Classic whisper-family hallucinations on near-silence. Exact matches only,
/// and gated on a low-energy recording, so genuine quiet dictation of these
/// words still passes.
fn is_whisper_artifact(text: &str) -> bool {
    let normalized: String = text
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    matches!(
        normalized.as_str(),
        "" | "thank you"
            | "thank you bye"
            | "thanks for watching"
            | "thank you for watching"
            | "thanks for watching bye"
            | "bye"
            | "bye bye"
            | "music"
            | "noise"
            | "silence"
            | "you"
    )
}

fn is_non_speech_label(label: &str) -> bool {
    let normalized = label
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        normalized.as_str(),
        "music"
            | "silence"
            | "noise"
            | "background noise"
            | "applause"
            | "laughter"
            | "inaudible"
            | "blank audio"
            | "no audio"
            | "no speech"
    )
}

/// Whisper and caption-trained models can return a sound label instead of
/// speech. Drop only results made entirely from those labels or music notes.
fn is_non_speech_annotation(text: &str) -> bool {
    let mut rest = text.trim();
    let mut found = false;
    while !rest.is_empty() {
        let Some(first) = rest.chars().next() else {
            break;
        };
        if matches!(first, '♪' | '♫' | '♬' | '♩' | '\u{fe0f}') {
            rest = rest[first.len_utf8()..].trim_start();
            found = true;
            continue;
        }

        let closing = match first {
            '[' => ']',
            '(' => ')',
            _ => return false,
        };
        let Some(end) = rest.find(closing) else {
            return false;
        };
        if !is_non_speech_label(&rest[first.len_utf8()..end]) {
            return false;
        }
        rest = rest[end + closing.len_utf8()..].trim_start();
        found = true;
    }
    found
}

fn should_drop_transcript(text: &str, max_frame_rms: f32) -> bool {
    is_non_speech_annotation(text)
        || (max_frame_rms < ARTIFACT_RAW_RMS && is_whisper_artifact(text))
}

fn filter_preview_text(text: String, max_frame_rms: f32) -> Option<String> {
    (!text.trim().is_empty() && !should_drop_transcript(&text, max_frame_rms)).then_some(text)
}

/// Broadcasts hotkey-watcher lifecycle to the webviews so the Hub can show
/// "waiting for permission / ready / unavailable" instead of a silent dead
/// hotkey. Also mirrored into stderr for headless debugging.
pub(crate) fn emit_hotkey_status(app: &AppHandle, status: &str, detail: Option<String>) {
    eprintln!(
        "hotkey watcher: {status}{}",
        detail
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default()
    );
    let _ = app.emit(
        "hotkey-status",
        serde_json::json!({ "status": status, "detail": detail }),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PipelineState {
    Idle,
    Recording,
    Transcribing,
    Injecting,
    Paused,
}

impl PipelineState {
    const fn as_u8(self) -> u8 {
        match self {
            PipelineState::Idle => 0,
            PipelineState::Recording => 1,
            PipelineState::Transcribing => 2,
            PipelineState::Injecting => 3,
            PipelineState::Paused => 4,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => PipelineState::Recording,
            2 => PipelineState::Transcribing,
            3 => PipelineState::Injecting,
            4 => PipelineState::Paused,
            _ => PipelineState::Idle,
        }
    }
}

impl fmt::Display for PipelineState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PipelineState::Idle => "idle",
            PipelineState::Recording => "recording",
            PipelineState::Transcribing => "transcribing",
            PipelineState::Injecting => "injecting",
            PipelineState::Paused => "paused",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineEvent {
    #[serde(rename = "type")]
    state: PipelineState,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<Transcript>,
}

enum Msg {
    Hotkey(HotkeyEvent),
    /// UI/tray stop action. Unlike a physical key release, this must stop in
    /// both toggle and push-to-talk activation modes.
    Stop,
    Cancel,
    PauseToggle,
    Retry {
        recording: crate::audio::Recording,
        target_app: String,
        reply: mpsc::Sender<bool>,
    },
    /// Sent by the worker thread once post-processing (STT/LLM/inject)
    /// completes, so the hotkey handler can accept new dictations again.
    SessionDone,
    /// Sent TAP_JUDGE_WAIT_MS after a quick release that kept the session
    /// alive for double-tap detection; if no second tap arrived by then,
    /// the session ends like a normal release would.
    TapJudge {
        generation: u64,
    },
    /// Stops an active recording from a watchdog or the hard session limit.
    /// Generation tagging prevents an old timer from stopping a later session.
    AutoStop {
        generation: u64,
    },
    /// Checks whether a completed speech phrase is ready for a best-effort
    /// live transcription. Generation tagging disarms old timers.
    PreviewTick {
        generation: u64,
    },
    /// Returns a phrase preview to the handler. The final full-session
    /// transcription remains authoritative for paste and history.
    PreviewDone {
        generation: u64,
        text: Option<String>,
    },
}

#[derive(Clone)]
struct WorkerFlags {
    busy: Arc<std::sync::atomic::AtomicBool>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

pub struct Pipeline {
    state: Arc<AtomicU8>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    control_tx: mpsc::Sender<Msg>,
}

impl Pipeline {
    pub fn start(
        app: AppHandle,
        db: Arc<Store>,
        hotkey_config: SharedHotkeyConfig,
        watcher_status: std::sync::Arc<std::sync::RwLock<String>>,
        mic_level: Arc<std::sync::atomic::AtomicU32>,
        mic_voiced: Arc<std::sync::atomic::AtomicU8>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let state = Arc::new(AtomicU8::new(PipelineState::Idle.as_u8()));
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_flags = WorkerFlags {
            busy: Arc::clone(&busy),
            cancelled: worker_cancelled,
        };

        let (hk_tx, hk_rx) = mpsc::channel::<HotkeyEvent>();
        {
            let config = Arc::clone(&hotkey_config);
            let status_app = app.clone();
            let status_cell = Arc::clone(&watcher_status);
            #[cfg(target_os = "macos")]
            std::thread::spawn(move || {
                // Native listen-only event tap: reports its own lifecycle
                // (waiting-accessibility / waiting-input-monitoring / ready)
                // instead of failing invisibly like the polling backend.
                let cb: crate::hotkey_tap::TapCallback =
                    Arc::new(move |state: &str, reason: Option<String>| {
                        *status_cell.write().unwrap() = state.to_string();
                        emit_hotkey_status(&status_app, state, reason);
                    });
                crate::hotkey_tap::run(config, hk_tx, cb);
            });
            #[cfg(not(target_os = "macos"))]
            std::thread::spawn(move || {
                use crate::hotkey::{HotkeyWatcher, PushToTalkWatcher, WatcherStatus};
                PushToTalkWatcher {
                    config,
                    poll_interval_ms: 20,
                    on_status: Some(Arc::new(move |status| match status {
                        WatcherStatus::Ready => {
                            *status_cell.write().unwrap() = "ready".to_string();
                            emit_hotkey_status(&status_app, "ready", None);
                        }
                        WatcherStatus::Unavailable(reason) => {
                            let s = format!("unavailable:{reason}");
                            emit_hotkey_status(&status_app, &s, Some(reason));
                            *status_cell.write().unwrap() = s;
                        }
                    })),
                }
                .spawn(hk_tx);
            });
        }
        let fwd_tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = hk_rx.recv() {
                if fwd_tx.send(Msg::Hotkey(event)).is_err() {
                    return;
                }
            }
        });

        {
            let state = Arc::clone(&state);
            let worker_flags = worker_flags.clone();
            let timer_tx = tx.clone();
            let metering = Metering {
                mic_level,
                mic_voiced,
            };
            std::thread::spawn(move || {
                handler_loop(app, db, rx, state, worker_flags, timer_tx, metering)
            });
        }

        Self {
            state,
            busy,
            control_tx: tx,
        }
    }

    pub fn current(&self) -> PipelineState {
        PipelineState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// Starts a recording without the hotkey (Flow Bar click).
    /// Returns false when the pipeline is busy.
    pub fn start_manual(&self) -> bool {
        if self.current() != PipelineState::Idle {
            return false;
        }
        self.control_tx.send(Msg::Hotkey(HotkeyEvent::Down)).is_ok()
    }

    /// Finishes the active recording from the Flow Bar or tray.
    pub fn stop_manual(&self) {
        let _ = self.control_tx.send(Msg::Stop);
    }

    /// Cancels the active session, discarding audio (Esc while recording).
    pub fn cancel(&self) {
        let _ = self.control_tx.send(Msg::Cancel);
    }

    /// Suspends/resumes capture without ending the session (pill button).
    pub fn toggle_pause(&self) {
        let _ = self.control_tx.send(Msg::PauseToggle);
    }

    /// Re-runs a failed transcription through the same serialized worker path
    /// as a normal dictation. The reply is sent by the hotkey handler after it
    /// has checked mode and busy state, so a retry cannot race a new session.
    pub(crate) fn retry(&self, recording: crate::audio::Recording, target_app: String) -> bool {
        if self.current() != PipelineState::Idle || self.busy.load(Ordering::Relaxed) {
            return false;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .control_tx
            .send(Msg::Retry {
                recording,
                target_app,
                reply: reply_tx,
            })
            .is_err()
        {
            return false;
        }
        reply_rx.recv().unwrap_or(false)
    }

    /// Starts or finishes a recording (Flow Bar click).
    pub fn toggle(&self) {
        match self.current() {
            PipelineState::Idle => {
                self.start_manual();
            }
            PipelineState::Recording | PipelineState::Paused => {
                self.stop_manual();
            }
            _ => {}
        }
    }
}

fn set_state(state: &AtomicU8, next: PipelineState) {
    state.store(next.as_u8(), Ordering::Relaxed);
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn emit(app: &AppHandle, event: PipelineEvent) {
    let _ = app.emit("pipeline", event);
}

/// Applies a pipeline state change: updates the FSM atom, drives Flow Bar
/// window visibility natively (so a missed webview event can never leave the
/// pill stuck hidden), plays start/stop chimes on edges, and broadcasts the
/// event to the webviews.
fn transition(
    app: &AppHandle,
    db: &Store,
    state: &Arc<AtomicU8>,
    next: PipelineState,
    error: Option<String>,
    transcript: Option<Transcript>,
) {
    let prev = PipelineState::from_u8(state.load(Ordering::Relaxed));
    set_state(state, next);
    sync_flowbar(app, db, state);
    crate::update_tray(app, next);
    if prev != next && crate::sound::enabled(db) {
        use crate::sound::Chime;
        match (prev, next) {
            (PipelineState::Idle, PipelineState::Recording) => {
                crate::sound::play(Chime::Start);
            }
            (PipelineState::Idle, _) | (_, PipelineState::Idle) => {
                crate::sound::play(Chime::Stop);
            }
            _ => {}
        }
    }
    emit(
        app,
        PipelineEvent {
            state: next,
            error,
            transcript,
        },
    );
}

/// Shows the pill immediately whenever dictation is active; on idle, hides it
/// after the webview's exit animation would have finished. The position is
/// clamped into the current monitor so a saved spot from a disconnected
/// display can't park the pill off-screen.
fn sync_flowbar(app: &AppHandle, db: &Store, state: &Arc<AtomicU8>) {
    let Some(window) = app.get_webview_window("flowbar") else {
        return;
    };
    let next = PipelineState::from_u8(state.load(Ordering::Relaxed));
    if !crate::flowbar_auto_hide(db) || next != PipelineState::Idle {
        clamp_flowbar_position(&window);
        let _ = window.show();
        return;
    }
    let idle_state = Arc::clone(state);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(450));
        // A new session may have started during the grace period.
        if idle_state.load(Ordering::Relaxed) == PipelineState::Idle.as_u8() {
            let _ = window.hide();
        }
    });
}

fn clamp_flowbar_position(window: &tauri::WebviewWindow) {
    use tauri::PhysicalPosition;

    let Ok(outer) = window.outer_position() else {
        return;
    };
    let Some(monitor) = window.current_monitor().ok().flatten() else {
        return;
    };
    let scale = monitor.scale_factor();
    let monitor_position = monitor.position();
    let bounds = monitor.size();
    // Flow Bar now fits its content dynamically. Use the actual physical
    // window size here; the old fixed 240x52 bounds could strand a resized
    // pill partly off-screen and also assumed every monitor began at (0, 0).
    let Ok(window_size) = window.inner_size() else {
        return;
    };
    let bar_w = window_size.width as f64;
    let bar_h = window_size.height as f64;
    // A small margin keeps a sliver of the pill grabbable at the edges.
    let margin = 8.0 * scale;
    let min_x = monitor_position.x as f64 + margin;
    let min_y = monitor_position.y as f64 + margin;
    let max_x = (monitor_position.x as f64 + bounds.width as f64 - bar_w - margin).max(min_x);
    let max_y = (monitor_position.y as f64 + bounds.height as f64 - bar_h - margin).max(min_y);
    let x = (outer.x as f64).clamp(min_x, max_x);
    let y = (outer.y as f64).clamp(min_y, max_y);
    if (x - outer.x as f64).abs() > 1.0 || (y - outer.y as f64).abs() > 1.0 {
        let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    }
}

fn fail(app: &AppHandle, db: &Store, state: &Arc<AtomicU8>, message: String) {
    eprintln!("pipeline error: {message}");
    transition(app, db, state, PipelineState::Idle, Some(message), None);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Ptt,
    HandsFree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TapAction {
    Wait,
    Finish,
    EnterHandsFree,
}

fn classify_tap_up(
    pending_tap: bool,
    second_tap_down: bool,
    first_tap_elapsed_ms: Option<u64>,
) -> TapAction {
    if pending_tap
        && second_tap_down
        && first_tap_elapsed_ms
            .map(|elapsed| elapsed < DOUBLE_TAP_WINDOW_MS)
            .unwrap_or(false)
    {
        TapAction::EnterHandsFree
    } else if pending_tap || second_tap_down {
        TapAction::Finish
    } else {
        TapAction::Wait
    }
}

/// Tail parameters shared with the capture callback plumbing.
struct Metering {
    mic_level: Arc<std::sync::atomic::AtomicU32>,
    mic_voiced: Arc<std::sync::atomic::AtomicU8>,
}

fn handler_loop(
    app: AppHandle,
    db: Arc<Store>,
    rx: mpsc::Receiver<Msg>,
    state: Arc<AtomicU8>,
    worker_flags: WorkerFlags,
    timer_tx: mpsc::Sender<Msg>,
    metering: Metering,
) {
    let mut audio = AudioEngine::new();
    let mut current_app = String::new();
    let mut mode = Mode::Idle;
    let mut pending_tap = false;
    let mut first_tap_at: Option<Instant> = None;
    let mut second_tap_down = false;
    // True while a worker thread is post-processing a finished recording.
    // The hotkey handler stays responsive during that window and queues one
    // start request instead of blocking like the old synchronous flow.
    let mut pending_start = false;
    let mut last_esc_at: Option<Instant> = None;
    // Shared with the level callback (last audible input) and a generation
    // counter that invalidates stale hands-free auto-stop watchdogs.
    let last_voice_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let session_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // Cursor and text for best-effort phrase previews. The final worker still
    // transcribes the complete recording, so a preview can never replace the
    // authoritative result or make cancellation unsafe.
    let mut preview_cursor = 0usize;
    let mut preview_text = String::new();
    let mut preview_busy = false;
    let Metering {
        mic_level,
        mic_voiced,
    } = metering;
    {
        let emitter = app.clone();
        let last_emit = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last_voice_ms = Arc::clone(&last_voice_ms);
        let level_cell = Arc::clone(&mic_level);
        let voiced_cell = Arc::clone(&mic_voiced);
        audio.set_level_callback(move |bar, voiced| {
            voiced_cell.store(voiced as u8, Ordering::Relaxed);
            // Latest display level + voice flag, polled by the pill webview
            // (event push into a throttled overlay window proved unreliable).
            level_cell.store(bar.to_bits(), Ordering::Relaxed);
            if voiced {
                last_voice_ms.store(unix_ms(), Ordering::Relaxed);
            }
            let now = unix_ms();
            let prev = last_emit.swap(now, Ordering::Relaxed);
            if now.saturating_sub(prev) >= 33 {
                let _ = emitter.emit(
                    "audio-level",
                    serde_json::json!({ "bar": bar, "voiced": voiced }),
                );
            }
        });
    }

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Hotkey(HotkeyEvent::Down) => {
                // Double-Esc is only meaningful across consecutive idle Esc
                // presses. Starting/resuming dictation breaks that sequence.
                last_esc_at = None;
                let toggle = hotkey_is_toggle(&db);
                if mode != Mode::Idle
                    && state.load(Ordering::Relaxed) == PipelineState::Paused.as_u8()
                {
                    // Pressing the hotkey while paused resumes capture.
                    audio.resume();
                    last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                    let generation = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                    spawn_max_session_timer(&timer_tx, generation);
                    spawn_preview_scheduler(&timer_tx, Arc::clone(&session_gen), generation);
                    if mode == Mode::HandsFree {
                        spawn_handsfree_watchdog(
                            &timer_tx,
                            Arc::clone(&last_voice_ms),
                            Arc::clone(&session_gen),
                            generation,
                        );
                    }
                    transition(&app, &db, &state, PipelineState::Recording, None, None);
                    continue;
                }
                if toggle && mode == Mode::Ptt {
                    // Toggle activation: second press ends the session,
                    // exactly like releasing in push-to-talk.
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    finish_session(
                        &app,
                        &db,
                        &state,
                        &mut audio,
                        &mut current_app,
                        &worker_flags,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                    continue;
                }
                if mode == Mode::Idle {
                    if worker_flags.busy.load(Ordering::Relaxed) {
                        // Previous dictation still processing. In toggle
                        // mode a press during processing would otherwise
                        // ghost-start a session with no key held later —
                        // ignore it instead of queueing.
                        if toggle {
                            emit_warning(&app, "still finishing the last dictation".to_string());
                            continue;
                        }
                        pending_start = true;
                        continue;
                    }
                    current_app = inject::frontmost_app();
                    audio.set_device(crate::configured_mic(&db));
                    audio.set_processing(noise_suppression_enabled(&db), vad_sensitivity_mult(&db));
                    if let Err(e) = audio.start() {
                        // Never swallow this: a dead mic must look different
                        // from a dead hotkey, or users cannot tell them apart.
                        fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                        continue;
                    }
                    crate::begin_context_capture();
                    reset_live_preview(&mut preview_cursor, &mut preview_text, &mut preview_busy);
                    last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                    mode = Mode::Ptt;
                    pending_tap = false;
                    second_tap_down = false;
                    let generation = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                    spawn_max_session_timer(&timer_tx, generation);
                    spawn_preview_scheduler(&timer_tx, Arc::clone(&session_gen), generation);
                    transition(&app, &db, &state, PipelineState::Recording, None, None);
                }
                // Down while Ptt (second press of entering double-tap) or
                // HandsFree (exit press): keep recording; release decides.
                if pending_tap {
                    // Keep the first tap pending until the second release.
                    // Clearing it on key-down made the subsequent TapUp look
                    // like another first tap, so double-tap could never win.
                    second_tap_down = true;
                    if !first_tap_at
                        .map(|t| t.elapsed() < Duration::from_millis(DOUBLE_TAP_WINDOW_MS))
                        .unwrap_or(false)
                    {
                        // The second press missed the double-tap window. It
                        // is still a real press, so its eventual release
                        // should finish the active session normally.
                        pending_tap = false;
                        first_tap_at = None;
                    }
                }
            }
            Msg::Hotkey(HotkeyEvent::Up) => {
                // Toggle activation ignores releases entirely; the second
                // press does the stopping.
                if hotkey_is_toggle(&db) {
                    continue;
                }
                if mode != Mode::Idle {
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    finish_session(
                        &app,
                        &db,
                        &state,
                        &mut audio,
                        &mut current_app,
                        &worker_flags,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                } else if pending_start {
                    // Released before the queued start fired — the user let
                    // go, so nothing should begin. This kills the ghost
                    // sessions that started "right after a dictation".
                    pending_start = false;
                    transition(&app, &db, &state, PipelineState::Idle, None, None);
                } else {
                    transition(&app, &db, &state, PipelineState::Idle, None, None);
                }
            }
            Msg::Hotkey(HotkeyEvent::TapUp) => match mode {
                // Toggle activation: releases carry no meaning.
                _ if hotkey_is_toggle(&db) => {}
                Mode::Ptt => {
                    match classify_tap_up(
                        pending_tap,
                        second_tap_down,
                        first_tap_at.map(|t| t.elapsed().as_millis() as u64),
                    ) {
                        TapAction::Wait => {
                            // First quick tap: hold judgement briefly in case
                            // a second tap follows, then finish automatically.
                            pending_tap = true;
                            first_tap_at = Some(Instant::now());
                            spawn_tap_judge(&timer_tx, session_gen.load(Ordering::Relaxed));
                        }
                        TapAction::EnterHandsFree => {
                            // Double-tap confirmed: restart capture cleanly and
                            // go hands-free.
                            audio.discard();
                            current_app = inject::frontmost_app();
                            audio.set_device(crate::configured_mic(&db));
                            audio.set_processing(
                                noise_suppression_enabled(&db),
                                vad_sensitivity_mult(&db),
                            );
                            crate::begin_context_capture();
                            if let Err(e) = audio.start() {
                                fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                                mode = Mode::Idle;
                                pending_tap = false;
                                first_tap_at = None;
                                second_tap_down = false;
                            } else {
                                reset_live_preview(
                                    &mut preview_cursor,
                                    &mut preview_text,
                                    &mut preview_busy,
                                );
                                mode = Mode::HandsFree;
                                pending_tap = false;
                                first_tap_at = None;
                                second_tap_down = false;
                                last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                                if crate::sound::enabled(&db) {
                                    crate::sound::play(crate::sound::Chime::Start);
                                }
                                emit_warning(
                                    &app,
                                    "hands-free: speak freely — Esc stops".to_string(),
                                );
                                let gen = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                                spawn_handsfree_watchdog(
                                    &timer_tx,
                                    Arc::clone(&last_voice_ms),
                                    Arc::clone(&session_gen),
                                    gen,
                                );
                                spawn_max_session_timer(&timer_tx, gen);
                                spawn_preview_scheduler(&timer_tx, Arc::clone(&session_gen), gen);
                            }
                        }
                        TapAction::Finish => {
                            // The second tap came too late for double-tap, or
                            // the pending first tap timed out. Finish like a
                            // normal quick release.
                            session_gen.fetch_add(1, Ordering::Relaxed);
                            finish_session(
                                &app,
                                &db,
                                &state,
                                &mut audio,
                                &mut current_app,
                                &worker_flags,
                                &timer_tx,
                            );
                            mode = Mode::Idle;
                            pending_tap = false;
                            first_tap_at = None;
                            second_tap_down = false;
                        }
                    }
                }
                Mode::HandsFree => {
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    finish_session(
                        &app,
                        &db,
                        &state,
                        &mut audio,
                        &mut current_app,
                        &worker_flags,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                }
                Mode::Idle => {}
            },
            Msg::Hotkey(HotkeyEvent::EscapePress) => {
                if mode != Mode::Idle {
                    // Esc cancels the active dictation and discards audio.
                    audio.discard();
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                    pending_start = false;
                    transition(&app, &db, &state, PipelineState::Idle, None, None);
                } else if worker_flags.busy.load(Ordering::Relaxed) {
                    // Processing runs on a worker and cannot always interrupt
                    // local inference immediately, but cancellation must make
                    // its eventual result inert: no command, history row, or
                    // paste after the user pressed Esc.
                    worker_flags.cancelled.store(true, Ordering::Relaxed);
                    pending_start = false;
                    last_esc_at = None;
                    transition(&app, &db, &state, PipelineState::Idle, None, None);
                    emit_warning(&app, "dictation canceled".to_string());
                } else {
                    // Double-Esc while idle removes the last pasted text.
                    let now = Instant::now();
                    let is_double = last_esc_at
                        .map(|t| now.duration_since(t) < Duration::from_millis(600))
                        .unwrap_or(false);
                    if is_double {
                        last_esc_at = None;
                        match crate::scratch_last() {
                            Ok(()) => emit_warning(
                                &app,
                                "last dictation removed from the page".to_string(),
                            ),
                            Err(e) => emit_warning(&app, format!("scratch failed: {e}")),
                        }
                    } else {
                        last_esc_at = Some(now);
                    }
                }
            }
            Msg::Stop => {
                if mode != Mode::Idle {
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    finish_session(
                        &app,
                        &db,
                        &state,
                        &mut audio,
                        &mut current_app,
                        &worker_flags,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                    pending_start = false;
                }
            }
            Msg::PauseToggle => {
                if mode == Mode::Ptt || mode == Mode::HandsFree {
                    let was_paused = state.load(Ordering::Relaxed) == PipelineState::Paused.as_u8();
                    if was_paused {
                        audio.resume();
                        last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                        let generation = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                        spawn_max_session_timer(&timer_tx, generation);
                        spawn_preview_scheduler(&timer_tx, Arc::clone(&session_gen), generation);
                        if mode == Mode::HandsFree {
                            spawn_handsfree_watchdog(
                                &timer_tx,
                                Arc::clone(&last_voice_ms),
                                Arc::clone(&session_gen),
                                generation,
                            );
                        }
                        transition(&app, &db, &state, PipelineState::Recording, None, None);
                    } else {
                        audio.pause();
                        session_gen.fetch_add(1, Ordering::Relaxed);
                        pending_tap = false;
                        first_tap_at = None;
                        second_tap_down = false;
                        transition(&app, &db, &state, PipelineState::Paused, None, None);
                    }
                }
            }
            Msg::Cancel => {
                worker_flags.cancelled.store(true, Ordering::Relaxed);
                audio.discard();
                session_gen.fetch_add(1, Ordering::Relaxed);
                mode = Mode::Idle;
                pending_tap = false;
                first_tap_at = None;
                pending_start = false;
                second_tap_down = false;
                transition(&app, &db, &state, PipelineState::Idle, None, None);
            }
            Msg::Retry {
                recording,
                target_app,
                reply,
            } => {
                last_esc_at = None;
                let accepted = mode == Mode::Idle && !worker_flags.busy.load(Ordering::Relaxed);
                if accepted {
                    spawn_session_worker(
                        &app,
                        &db,
                        &state,
                        recording,
                        target_app,
                        &worker_flags,
                        &timer_tx,
                    );
                }
                let _ = reply.send(accepted);
            }
            Msg::TapJudge { generation } => {
                if generation == session_gen.load(Ordering::Relaxed)
                    && pending_tap
                    && !second_tap_down
                    && mode == Mode::Ptt
                {
                    // No second tap arrived: treat as a normal release.
                    pending_tap = false;
                    first_tap_at = None;
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    finish_session(
                        &app,
                        &db,
                        &state,
                        &mut audio,
                        &mut current_app,
                        &worker_flags,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    second_tap_down = false;
                }
            }
            Msg::AutoStop { generation } => {
                if generation == session_gen.load(Ordering::Relaxed) && mode != Mode::Idle {
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    finish_session(
                        &app,
                        &db,
                        &state,
                        &mut audio,
                        &mut current_app,
                        &worker_flags,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                    pending_start = false;
                }
            }
            Msg::PreviewTick { generation } => {
                if generation != session_gen.load(Ordering::Relaxed)
                    || mode == Mode::Idle
                    || state.load(Ordering::Relaxed) != PipelineState::Recording.as_u8()
                {
                    continue;
                }
                if preview_busy {
                    continue;
                }
                let end_sample = audio.sample_count();
                let sample_rate = audio.sample_rate();
                let new_samples = end_sample.saturating_sub(preview_cursor);
                let segment_ms = if sample_rate == 0 {
                    0
                } else {
                    (new_samples as u128 * 1000 / sample_rate as u128) as u64
                };
                let silent_for = unix_ms().saturating_sub(last_voice_ms.load(Ordering::Relaxed));
                let ready = segment_ms >= LIVE_PREVIEW_MIN_SEGMENT_MS
                    && (silent_for >= LIVE_PREVIEW_SILENCE_MS
                        || segment_ms >= LIVE_PREVIEW_MAX_SEGMENT_MS);
                if !ready {
                    continue;
                }

                let start_sample = preview_cursor;
                match audio.snapshot_since(start_sample) {
                    Ok((Some(recording), copied_end)) => {
                        preview_cursor = copied_end;
                        preview_busy = true;
                        spawn_live_preview_worker(
                            &db,
                            &timer_tx,
                            generation,
                            recording,
                            current_app.clone(),
                        );
                    }
                    Ok((None, copied_end)) => {
                        // Advance over silence or a too-short phrase. The
                        // final full-session path still owns the complete
                        // recording, so this only affects the preview.
                        preview_cursor = copied_end;
                    }
                    Err(_) => {
                        // Preview is deliberately best effort. Do not let a
                        // transient local-model or audio-preparation error
                        // affect the final recording.
                        preview_cursor = audio.sample_count();
                    }
                }
            }
            Msg::PreviewDone { generation, text } => {
                if generation != session_gen.load(Ordering::Relaxed) {
                    continue;
                }
                preview_busy = false;
                let Some(text) = text.filter(|value| !value.trim().is_empty()) else {
                    continue;
                };
                let merged = merge_preview_text(&preview_text, &text);
                if merged == preview_text {
                    continue;
                }
                preview_text = merged.clone();
                let _ = app.emit("stt-partial", serde_json::json!({ "text": merged }));
            }
            Msg::SessionDone => {
                worker_flags.busy.store(false, Ordering::Relaxed);
                if pending_start && mode == Mode::Idle && crate::hotkey::hotkey_held() {
                    // A hotkey press arrived while the worker was busy —
                    // begin that queued dictation now.
                    pending_start = false;
                    current_app = inject::frontmost_app();
                    audio.set_device(crate::configured_mic(&db));
                    audio.set_processing(noise_suppression_enabled(&db), vad_sensitivity_mult(&db));
                    crate::begin_context_capture();
                    if let Err(e) = audio.start() {
                        fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                    } else {
                        mode = Mode::Ptt;
                        pending_tap = false;
                        first_tap_at = None;
                        second_tap_down = false;
                        reset_live_preview(
                            &mut preview_cursor,
                            &mut preview_text,
                            &mut preview_busy,
                        );
                        last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                        let generation = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                        spawn_max_session_timer(&timer_tx, generation);
                        spawn_preview_scheduler(&timer_tx, Arc::clone(&session_gen), generation);
                        transition(&app, &db, &state, PipelineState::Recording, None, None);
                    }
                }
            }
        }
    }
}

fn reset_live_preview(cursor: &mut usize, text: &mut String, busy: &mut bool) {
    *cursor = 0;
    text.clear();
    *busy = false;
}

/// Runs one low-cost scheduler for a recording generation. Changing the
/// generation disarms the old scheduler without leaving a thread behind.
fn spawn_preview_scheduler(
    tx: &mpsc::Sender<Msg>,
    session_gen: Arc<std::sync::atomic::AtomicU64>,
    generation: u64,
) {
    let tx = tx.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(LIVE_PREVIEW_TICK_MS));
        if session_gen.load(Ordering::Relaxed) != generation {
            return;
        }
        if tx.send(Msg::PreviewTick { generation }).is_err() {
            return;
        }
    });
}

/// Transcribes one captured phrase in the background. Preview failures are
/// intentionally silent: missing models, a changing device, or an optional
/// engine error must never turn a successful final dictation into an error.
fn spawn_live_preview_worker(
    db: &Arc<Store>,
    tx: &mpsc::Sender<Msg>,
    generation: u64,
    recording: crate::audio::Recording,
    target_app: String,
) {
    let db = Arc::clone(db);
    let tx = tx.clone();
    let max_frame_rms = recording.max_frame_rms;
    std::thread::spawn(move || {
        let text = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let language = session_language(&db, &target_app);
            let prompt = db_prompt(&db);
            let mut ignore_delta = |_text: &str| {};
            stt::stream_transcribe(
                &db,
                &recording.wav,
                &language,
                Some(&prompt),
                &mut ignore_delta,
            )
            .map(|result| result.text)
        }))
        .ok()
        .and_then(|result| result.ok())
        .and_then(|value| filter_preview_text(value, max_frame_rms));

        let _ = tx.send(Msg::PreviewDone { generation, text });
    });
}

/// Joins phrase previews while removing a small repeated boundary. The final
/// full-session transcription replaces this display text after the user
/// stops, so this helper favors a stable readable preview over punctuation
/// preservation at an unfinished phrase boundary.
fn merge_preview_text(existing: &str, next: &str) -> String {
    let existing_words: Vec<&str> = existing.split_whitespace().collect();
    let next_words: Vec<&str> = next.split_whitespace().collect();
    if existing_words.is_empty() {
        return next_words.join(" ");
    }
    if next_words.is_empty() {
        return existing_words.join(" ");
    }

    let max_overlap = existing_words.len().min(next_words.len()).min(12);
    let overlap = (1..=max_overlap)
        .rev()
        .find(|&count| {
            existing_words[existing_words.len() - count..]
                .iter()
                .zip(&next_words[..count])
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
        .unwrap_or(0);
    let mut merged = existing_words.join(" ");
    if overlap < next_words.len() {
        if !merged.is_empty() {
            merged.push(' ');
        }
        merged.push_str(&next_words[overlap..].join(" "));
    }
    merged
}

fn spawn_handsfree_watchdog(
    tx: &mpsc::Sender<Msg>,
    last_voice_ms: Arc<std::sync::atomic::AtomicU64>,
    session_gen: Arc<std::sync::atomic::AtomicU64>,
    target_gen: u64,
) {
    let tx = tx.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        if session_gen.load(Ordering::Relaxed) != target_gen {
            // Session ended, paused, or was cancelled — disarm.
            return;
        }
        let silent_for = unix_ms().saturating_sub(last_voice_ms.load(Ordering::Relaxed));
        if silent_for > HANDS_FREE_SILENCE_STOP_MS {
            let _ = tx.send(Msg::AutoStop {
                generation: target_gen,
            });
            return;
        }
    });
}

fn spawn_tap_judge(tx: &mpsc::Sender<Msg>, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(TAP_JUDGE_WAIT_MS));
        let _ = tx.send(Msg::TapJudge { generation });
    });
}

fn spawn_max_session_timer(tx: &mpsc::Sender<Msg>, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(MAX_SESSION_SECS));
        let _ = tx.send(Msg::AutoStop { generation });
    });
}

fn finish_session(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    audio: &mut AudioEngine,
    current_app: &mut String,
    worker_flags: &WorkerFlags,
    done_tx: &mpsc::Sender<Msg>,
) {
    match audio.stop() {
        Ok(Some(recording)) => {
            let target_app = std::mem::take(current_app);
            spawn_session_worker(app, db, state, recording, target_app, worker_flags, done_tx);
        }
        Ok(None) => {
            if let Some(note) = audio.take_discard_note() {
                emit_warning(app, note);
            }
            transition(app, db, state, PipelineState::Idle, None, None);
        }
        Err(e) => fail(app, db, state, e.to_string()),
    }
}

fn spawn_session_worker(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    recording: crate::audio::Recording,
    target_app: String,
    worker_flags: &WorkerFlags,
    done_tx: &mpsc::Sender<Msg>,
) {
    // Hand the recording to a worker thread so the hotkey handler can keep
    // reacting while STT/LLM/injection run.
    worker_flags.busy.store(true, Ordering::Relaxed);
    worker_flags.cancelled.store(false, Ordering::Relaxed);
    let worker_app = app.clone();
    let worker_db = Arc::clone(db);
    let worker_state = Arc::clone(state);
    let worker_cancelled = Arc::clone(&worker_flags.cancelled);
    let done_tx = done_tx.clone();
    std::thread::spawn(move || {
        // SessionDone must be sent even if processing panics, otherwise the
        // pipeline would stay busy forever.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_session(
                &worker_app,
                &worker_db,
                &worker_state,
                &worker_cancelled,
                recording,
                target_app,
            );
        }));
        if result.is_err() && !worker_cancelled.load(Ordering::Relaxed) {
            fail(
                &worker_app,
                &worker_db,
                &worker_state,
                "dictation worker panicked".to_string(),
            );
        }
        let _ = done_tx.send(Msg::SessionDone);
    });
}

/// Returns true once processing was cancelled and repairs the visible state
/// if a worker transition raced the handler's immediate Idle transition.
/// No new worker can start until SessionDone, so this repair cannot clobber a
/// later dictation.
fn processing_cancelled(
    app: &AppHandle,
    db: &Store,
    state: &Arc<AtomicU8>,
    worker_cancelled: &std::sync::atomic::AtomicBool,
) -> bool {
    if !worker_cancelled.load(Ordering::Relaxed) {
        return false;
    }
    if state.load(Ordering::Relaxed) != PipelineState::Idle.as_u8() {
        transition(app, db, state, PipelineState::Idle, None, None);
    }
    true
}

/// Accumulates per-stage wall-clock durations for one dictation and reports
/// them once the session ends — to stderr and as a `pipeline-timing` event —
/// so release-to-paste latency stays measurable.
struct StageTimings {
    started: Instant,
    stages: Vec<(&'static str, u128)>,
}

impl StageTimings {
    fn begin() -> Self {
        Self {
            started: Instant::now(),
            stages: Vec::new(),
        }
    }

    /// Records elapsed time since `since` under `name`; returns a fresh
    /// instant for timing the next stage.
    fn mark(&mut self, name: &'static str, since: Instant) -> Instant {
        let now = Instant::now();
        self.stages
            .push((name, now.duration_since(since).as_millis()));
        now
    }

    fn report(self, app: &AppHandle) {
        let total_ms = self.started.elapsed().as_millis();
        let breakdown = self
            .stages
            .iter()
            .map(|(name, ms)| format!("{name} {ms}ms"))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("dictation latency: total {total_ms}ms ({breakdown})");
        let _ = app.emit(
            "pipeline-timing",
            serde_json::json!({
                "totalMs": total_ms,
                "stages": self
                    .stages
                    .iter()
                    .map(|(name, ms)| serde_json::json!({ "name": name, "ms": ms }))
                    .collect::<Vec<_>>(),
            }),
        );
    }
}

pub(crate) fn run_session(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    worker_cancelled: &Arc<std::sync::atomic::AtomicBool>,
    recording: crate::audio::Recording,
    target_app: String,
) {
    let mut timings = StageTimings::begin();
    if processing_cancelled(app, db, state, worker_cancelled) {
        return;
    }
    transition(app, db, state, PipelineState::Transcribing, None, None);
    if processing_cancelled(app, db, state, worker_cancelled) {
        return;
    }

    // Language precedence: a matching per-app style's pinned language wins;
    // otherwise the global setting ("auto" by default).
    let language = session_language(db, &target_app);

    // A failure at this stage keeps the audio around so it can be retried
    // without re-recording; success clears any stale job. Streaming deltas
    // are forwarded to the pill as `stt-partial` (cumulative text) so the
    // user watches words appear instead of staring at bouncing dots.
    let stt_started = timings.mark("session-prep", timings.started);
    let partial_app = app.clone();
    let result = stt::stream_transcribe(
        db,
        &recording.wav,
        &language,
        Some(&db_prompt(db)),
        &mut |cumulative| {
            let _ = partial_app.emit("stt-partial", serde_json::json!({ "text": cumulative }));
        },
    );
    let stt_done = timings.mark("stt", stt_started);
    if processing_cancelled(app, db, state, worker_cancelled) {
        timings.report(app);
        return;
    }

    let raw_text = match result {
        Ok(r) => r.text,
        Err(e) => {
            crate::store_retry_job(recording.wav, target_app);
            timings.report(app);
            return fail(app, db, state, e.to_string());
        }
    };
    if raw_text.trim().is_empty() {
        crate::store_retry_job(recording.wav, target_app);
        timings.report(app);
        return fail(app, db, state, "transcription came back empty".to_string());
    }
    // Any non-empty STT result supersedes an older retry, even when the
    // artifact guard below decides not to paste this particular capture.
    crate::clear_retry_job();
    // Hallucination guard: sound labels and quiet stock phrases are not user
    // speech. Drop them instead of pasting or saving phantom text.
    if should_drop_transcript(&raw_text, recording.max_frame_rms) {
        eprintln!(
            "artifact guard: dropped non-speech text {:?} (rms {:.4})",
            raw_text, recording.max_frame_rms
        );
        timings.report(app);
        emit_warning(
            app,
            "no speech detected — ignored a phantom transcription".to_string(),
        );
        transition(app, db, state, PipelineState::Idle, None, None);
        return;
    }
    let raw_text = crate::emoji::apply(&raw_text);

    let data = SessionData::new(recording.duration_ms, target_app, language);

    // Fast path: whole utterance matches a snippet trigger — no LLM call.
    let snippet = crate::cloud::try_snippet(db, &raw_text).unwrap_or(None);
    if let Some(expanded) = snippet {
        timings.mark("snippet", stt_done);
        return finish(
            app,
            db,
            state,
            worker_cancelled,
            SessionOutput {
                final_text: &expanded,
                raw_text: &raw_text,
                data: &data,
            },
            timings,
        );
    }

    transition(app, db, state, PipelineState::Injecting, None, None);
    if processing_cancelled(app, db, state, worker_cancelled) {
        timings.report(app);
        return;
    }

    // LLM cleanup; fall back to the raw transcription on any failure so a
    // cleanup outage never costs the user their dictation. Skippable for
    // minimum-latency raw paste via the cleanupEnabled setting, and skipped
    // automatically for short utterances (cleanupSkipShort) since raw STT is
    // usually already clean there.
    let short = raw_text.chars().count() < SHORT_UTTERANCE_CHARS;
    let caret_context = crate::take_caret_context();
    let polished = if cleanup_enabled(db) && !(short && cleanup_skip_short(db)) {
        match llm::polish(db, &raw_text, &data.target_app, caret_context.as_deref()) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("cleanup skipped: {e}");
                emit_warning(app, format!("cleanup unavailable — pasted raw text ({e})"));
                raw_text.clone()
            }
        }
    } else {
        raw_text.clone()
    };
    timings.mark("llm-cleanup", stt_done);
    if processing_cancelled(app, db, state, worker_cancelled) {
        timings.report(app);
        return;
    }

    // Vocabulary learning: diff raw vs polished speech and auto-capture
    // recurring names/jargon (gated by the autoLearnVocabulary setting).
    crate::learn::observe(db, &raw_text, &polished);

    // Command mode: recognized spoken commands execute instead of pasting.
    if crate::commands::is_enabled(db) {
        if let Some(command) = crate::commands::parse(&polished) {
            timings.report(app);
            return run_command(
                app,
                db,
                state,
                worker_cancelled,
                SessionOutput {
                    final_text: &polished,
                    raw_text: &raw_text,
                    data: &data,
                },
                &command,
            );
        }
    }

    finish(
        app,
        db,
        state,
        worker_cancelled,
        SessionOutput {
            final_text: &polished,
            raw_text: &raw_text,
            data: &data,
        },
        timings,
    );
}

/// Utterances shorter than this skip LLM cleanup by default: short
/// dictations rarely need rewriting, and skipping removes the single
/// largest latency component from the release-to-paste path.
const SHORT_UTTERANCE_CHARS: usize = 120;

fn cleanup_enabled(db: &Store) -> bool {
    db.get_setting("cleanupEnabled")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true)
}

/// Activation style: "toggle" (press to start, press again to stop — the
/// default) or "push_to_talk" (hold the key, release to stop, double-tap
/// for hands-free).
fn hotkey_is_toggle(db: &Store) -> bool {
    db.get_setting("hotkeyMode")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .as_deref()
        != Some("push_to_talk")
}

fn cleanup_skip_short(db: &Store) -> bool {
    db.get_setting("cleanupSkipShort")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true)
}

#[derive(Clone, Copy)]
struct SessionOutput<'a> {
    final_text: &'a str,
    raw_text: &'a str,
    data: &'a SessionData,
}

fn run_command(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    worker_cancelled: &Arc<std::sync::atomic::AtomicBool>,
    output: SessionOutput<'_>,
    command: &crate::commands::Command,
) {
    if processing_cancelled(app, db, state, worker_cancelled) {
        return;
    }
    match crate::commands::execute(app, command) {
        Ok(()) => {
            eprintln!("command executed: {}", crate::commands::describe(command));
        }
        Err(e) => {
            emit_warning(app, format!("command failed — text pasted instead ({e})"));
            return finish(
                app,
                db,
                state,
                worker_cancelled,
                output,
                StageTimings::begin(),
            );
        }
    }

    if processing_cancelled(app, db, state, worker_cancelled) {
        return;
    }

    match db.insert_transcript(
        output.final_text,
        output.raw_text,
        &output.data.language,
        output.data.duration_ms,
        "command",
    ) {
        Ok(transcript) => transition(app, db, state, PipelineState::Idle, None, Some(transcript)),
        Err(e) => fail(app, db, state, e.to_string()),
    }
}

struct SessionData {
    duration_ms: i64,
    target_app: String,
    language: String,
}

impl SessionData {
    fn new(duration_ms: i64, target_app: String, language: String) -> Self {
        Self {
            duration_ms,
            target_app,
            language,
        }
    }
}

fn finish(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    worker_cancelled: &Arc<std::sync::atomic::AtomicBool>,
    output: SessionOutput<'_>,
    mut timings: StageTimings,
) {
    if processing_cancelled(app, db, state, worker_cancelled) {
        timings.report(app);
        return;
    }
    transition(app, db, state, PipelineState::Injecting, None, None);

    let inject_started = Instant::now();
    if processing_cancelled(app, db, state, worker_cancelled) {
        timings.report(app);
        return;
    }
    if let Err(e) = inject::paste_text(output.final_text) {
        store_anyway(db, output.final_text, output.raw_text, output.data);
        timings.report(app);
        fail(
            app,
            db,
            state,
            format!("paste failed: {e} — text saved to history"),
        );
        return;
    }
    // Only arm scratch/undo after the target app actually accepted the paste.
    // Remembering it before injection made a failed paste undo unrelated text.
    crate::remember_pasted(&output.data.target_app);
    timings.mark("inject", inject_started);

    match db.insert_transcript(
        output.final_text,
        output.raw_text,
        &output.data.language,
        output.data.duration_ms,
        &output.data.target_app,
    ) {
        Ok(transcript) => {
            transition(app, db, state, PipelineState::Idle, None, Some(transcript));
        }
        Err(e) => fail(app, db, state, e.to_string()),
    }
    timings.report(app);
}

fn store_anyway(db: &Store, text: &str, raw_text: &str, data: &SessionData) {
    let _ = db.insert_transcript(
        text,
        raw_text,
        &data.language,
        data.duration_ms,
        &data.target_app,
    );
}

fn emit_warning(app: &AppHandle, message: String) {
    let _ = app.emit(
        "pipeline-warning",
        serde_json::json!({ "message": message }),
    );
}

fn db_prompt(db: &Store) -> String {
    stt::build_prompt(db).unwrap_or_default()
}

fn session_language(db: &Store, target_app: &str) -> String {
    let style_info = llm::resolve_style(db, target_app).ok().flatten();
    style_info
        .as_ref()
        .and_then(|(_, lang)| lang.clone())
        .or_else(|| {
            db.get_setting("language")
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str::<String>(&v).ok())
                .filter(|language| !language.is_empty() && language != "auto")
        })
        .unwrap_or_else(|| "auto".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_matcher_catches_stock_phrases_only() {
        assert!(is_whisper_artifact("Thank you."));
        assert!(is_whisper_artifact("Thanks for watching!"));
        assert!(is_whisper_artifact("Bye"));
        assert!(is_whisper_artifact("  you "));
        assert!(is_whisper_artifact("music"));
        assert!(is_whisper_artifact(""));
        // Real dictation of the same words must NOT be flagged when the
        // energy gate passes it through with more content.
        assert!(!is_whisper_artifact("thank you for the quick review"));
        assert!(!is_whisper_artifact("bye everyone, see you tomorrow"));
        assert!(!is_whisper_artifact("play music"));
        assert!(!is_whisper_artifact("music is playing"));
    }

    #[test]
    fn non_speech_annotations_are_dropped_at_any_volume() {
        for text in [
            "[Music]",
            "(MUSIC)",
            "[Silence]",
            "[Noise]",
            "[Applause]",
            "[Laughter]",
            "[BLANK_AUDIO]",
            "♪♫",
            "♪ [Music] ♫",
        ] {
            assert!(should_drop_transcript(text, 1.0), "did not drop {text:?}");
        }
    }

    #[test]
    fn artifact_guard_keeps_real_music_dictation() {
        assert!(!should_drop_transcript("play music", 0.0));
        assert!(!should_drop_transcript("music is playing", 0.0));
        assert!(!should_drop_transcript(
            "[Music] starts after the title",
            1.0
        ));
        assert!(should_drop_transcript("music", ARTIFACT_RAW_RMS / 2.0));
        assert!(!should_drop_transcript("music", ARTIFACT_RAW_RMS * 2.0));
    }

    #[test]
    fn preview_filter_hides_non_speech_text() {
        assert_eq!(filter_preview_text("[Music]".to_string(), 1.0), None);
        assert_eq!(filter_preview_text("  ".to_string(), 1.0), None);
        assert_eq!(
            filter_preview_text("play music".to_string(), 0.0).as_deref(),
            Some("play music")
        );
    }

    #[test]
    fn double_tap_requires_second_press_before_release() {
        assert_eq!(
            classify_tap_up(true, true, Some(DOUBLE_TAP_WINDOW_MS - 1)),
            TapAction::EnterHandsFree
        );
        assert_eq!(
            classify_tap_up(true, true, Some(DOUBLE_TAP_WINDOW_MS)),
            TapAction::Finish
        );
        assert_eq!(classify_tap_up(false, false, None), TapAction::Wait);
    }

    #[test]
    fn preview_text_appends_new_phrases() {
        assert_eq!(
            merge_preview_text("Please open", "the settings page"),
            "Please open the settings page"
        );
    }

    #[test]
    fn preview_text_removes_repeated_phrase_boundaries() {
        assert_eq!(
            merge_preview_text("Please open the", "open the settings page"),
            "Please open the settings page"
        );
        assert_eq!(merge_preview_text("Hello", "hello"), "Hello");
    }

    #[test]
    fn pipeline_event_uses_state_as_the_frontend_discriminator() {
        let value = serde_json::to_value(PipelineEvent {
            state: PipelineState::Recording,
            error: None,
            transcript: None,
        })
        .unwrap();
        assert_eq!(value.get("type"), Some(&serde_json::json!("recording")));
        assert!(value.get("state").is_none());
    }
}
