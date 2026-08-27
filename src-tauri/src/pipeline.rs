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
            | "you"
    )
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
#[serde(rename_all = "camelCase", tag = "type")]
pub struct PipelineEvent {
    state: PipelineState,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<Transcript>,
}

enum Msg {
    Hotkey(HotkeyEvent),
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
    TapJudge,
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
            let busy = Arc::clone(&busy);
            let timer_tx = tx.clone();
            let metering = Metering {
                mic_level,
                mic_voiced,
            };
            std::thread::spawn(move || handler_loop(app, db, rx, state, busy, timer_tx, metering));
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

    /// Finishes the active recording (Flow Bar click or Esc).
    pub fn stop_manual(&self) {
        let _ = self.control_tx.send(Msg::Hotkey(HotkeyEvent::Up));
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
            PipelineState::Recording => {
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
    busy: Arc<std::sync::atomic::AtomicBool>,
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
                let toggle = hotkey_is_toggle(&db);
                if mode != Mode::Idle
                    && state.load(Ordering::Relaxed) == PipelineState::Paused.as_u8()
                {
                    // Pressing the hotkey while paused resumes capture.
                    audio.resume();
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
                        &busy,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                    second_tap_down = false;
                    continue;
                }
                if mode == Mode::Idle {
                    if busy.load(Ordering::Relaxed) {
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
                    audio.set_device(mic_preference(&db));
                    audio.set_processing(noise_suppression_enabled(&db), vad_sensitivity_mult(&db));
                    if let Err(e) = audio.start() {
                        // Never swallow this: a dead mic must look different
                        // from a dead hotkey, or users cannot tell them apart.
                        fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                        continue;
                    }
                    crate::begin_context_capture();
                    mode = Mode::Ptt;
                    pending_tap = false;
                    second_tap_down = false;
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    spawn_max_session_timer(&timer_tx);
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
                        &busy,
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
                            spawn_tap_judge(&timer_tx);
                        }
                        TapAction::EnterHandsFree => {
                            // Double-tap confirmed: restart capture cleanly and
                            // go hands-free.
                            audio.discard();
                            current_app = inject::frontmost_app();
                            audio.set_device(mic_preference(&db));
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
                                spawn_max_session_timer(&timer_tx);
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
                                &busy,
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
                        &busy,
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
            Msg::PauseToggle => {
                if mode == Mode::Ptt || mode == Mode::HandsFree {
                    let was_paused = state.load(Ordering::Relaxed) == PipelineState::Paused.as_u8();
                    if was_paused {
                        audio.resume();
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
                let accepted = mode == Mode::Idle && !busy.load(Ordering::Relaxed);
                if accepted {
                    spawn_session_worker(
                        &app, &db, &state, recording, target_app, &busy, &timer_tx,
                    );
                }
                let _ = reply.send(accepted);
            }
            Msg::TapJudge => {
                if pending_tap && !second_tap_down && mode == Mode::Ptt {
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
                        &busy,
                        &timer_tx,
                    );
                    mode = Mode::Idle;
                    second_tap_down = false;
                }
            }
            Msg::SessionDone => {
                busy.store(false, Ordering::Relaxed);
                if pending_start && mode == Mode::Idle && crate::hotkey::hotkey_held() {
                    // A hotkey press arrived while the worker was busy —
                    // begin that queued dictation now.
                    pending_start = false;
                    current_app = inject::frontmost_app();
                    audio.set_device(mic_preference(&db));
                    audio.set_processing(noise_suppression_enabled(&db), vad_sensitivity_mult(&db));
                    crate::begin_context_capture();
                    if let Err(e) = audio.start() {
                        fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                    } else {
                        mode = Mode::Ptt;
                        pending_tap = false;
                        first_tap_at = None;
                        second_tap_down = false;
                        spawn_max_session_timer(&timer_tx);
                        transition(&app, &db, &state, PipelineState::Recording, None, None);
                    }
                }
            }
        }
    }
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
            let _ = tx.send(Msg::Hotkey(HotkeyEvent::Up));
            return;
        }
    });
}

fn spawn_tap_judge(tx: &mpsc::Sender<Msg>) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(TAP_JUDGE_WAIT_MS));
        let _ = tx.send(Msg::TapJudge);
    });
}

fn spawn_max_session_timer(tx: &mpsc::Sender<Msg>) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(MAX_SESSION_SECS));
        let _ = tx.send(Msg::Hotkey(HotkeyEvent::Up));
    });
}

fn finish_session(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    audio: &mut AudioEngine,
    current_app: &mut String,
    busy: &std::sync::atomic::AtomicBool,
    done_tx: &mpsc::Sender<Msg>,
) {
    match audio.stop() {
        Ok(Some(recording)) => {
            let target_app = std::mem::take(current_app);
            spawn_session_worker(app, db, state, recording, target_app, busy, done_tx);
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
    busy: &std::sync::atomic::AtomicBool,
    done_tx: &mpsc::Sender<Msg>,
) {
    // Hand the recording to a worker thread so the hotkey handler can keep
    // reacting while STT/LLM/injection run.
    busy.store(true, Ordering::Relaxed);
    let worker_app = app.clone();
    let worker_db = Arc::clone(db);
    let worker_state = Arc::clone(state);
    let done_tx = done_tx.clone();
    std::thread::spawn(move || {
        // SessionDone must be sent even if processing panics, otherwise the
        // pipeline would stay busy forever.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_session(
                &worker_app,
                &worker_db,
                &worker_state,
                recording,
                target_app,
            );
        }));
        if result.is_err() {
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
    recording: crate::audio::Recording,
    target_app: String,
) {
    let mut timings = StageTimings::begin();
    transition(app, db, state, PipelineState::Transcribing, None, None);

    // Language precedence: a matching per-app style's pinned language wins;
    // otherwise the global setting ("auto" by default).
    let style_info = db.resolve_style_full(&target_app).ok().flatten();
    let language = style_info
        .as_ref()
        .and_then(|(_, lang)| lang.clone())
        .or_else(|| {
            db.get_setting("language")
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str::<String>(&v).ok())
                .filter(|l| !l.is_empty() && l != "auto")
        })
        .unwrap_or_else(|| "auto".to_string());

    // A failure at this stage keeps the audio around so it can be retried
    // without re-recording; success clears any stale job. Streaming deltas
    // are forwarded to the pill as `stt-partial` (cumulative text) so the
    // user watches words appear instead of staring at bouncing dots.
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
    let stt_started = timings.mark("wav-encode+prep", timings.started);

    let raw_text = match result {
        Ok(r) => r.text,
        Err(e) => {
            crate::store_retry_job(recording.wav, target_app);
            timings.report(app);
            return fail(app, db, state, e.to_string());
        }
    };
    let stt_done = timings.mark("stt", stt_started);
    if raw_text.trim().is_empty() {
        crate::store_retry_job(recording.wav, target_app);
        timings.report(app);
        return fail(app, db, state, "transcription came back empty".to_string());
    }
    // Hallucination guard: a near-silent capture that still produced text is
    // model confabulation — drop it instead of pasting phantom words.
    if recording.max_frame_rms < ARTIFACT_RAW_RMS && is_whisper_artifact(&raw_text) {
        eprintln!(
            "artifact guard: dropped quiet-session text {:?} (rms {:.4})",
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
    crate::clear_retry_job();
    let raw_text = crate::emoji::apply(&raw_text);

    let data = SessionData::new(recording.duration_ms, target_app, language);

    // Fast path: whole utterance matches a snippet trigger — no LLM call.
    let snippet = crate::cloud::try_snippet(db, &raw_text).unwrap_or(None);
    if let Some(expanded) = snippet {
        timings.mark("snippet", stt_done);
        return finish(app, db, state, &expanded, &raw_text, &data, timings);
    }

    transition(app, db, state, PipelineState::Injecting, None, None);

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

    // Vocabulary learning: diff raw vs polished speech and auto-capture
    // recurring names/jargon (gated by the autoLearnVocabulary setting).
    crate::learn::observe(db, &raw_text, &polished);

    // Command mode: recognized spoken commands execute instead of pasting.
    if crate::commands::is_enabled(db) {
        if let Some(command) = crate::commands::parse(&polished) {
            timings.report(app);
            return run_command(app, db, state, &polished, &raw_text, &data, &command);
        }
    }

    finish(app, db, state, &polished, &raw_text, &data, timings);
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

/// Preferred input device name from settings; None = system default.
fn mic_preference(db: &Store) -> Option<String> {
    db.get_setting("micDevice")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<Option<String>>(&v).ok())
        .flatten()
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

fn run_command(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    polished: &str,
    raw_text: &str,
    data: &SessionData,
    command: &crate::commands::Command,
) {
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
                polished,
                raw_text,
                data,
                StageTimings::begin(),
            );
        }
    }

    match db.insert_transcript(
        polished,
        raw_text,
        &data.language,
        data.duration_ms,
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
    final_text: &str,
    raw_text: &str,
    data: &SessionData,
    mut timings: StageTimings,
) {
    transition(app, db, state, PipelineState::Injecting, None, None);

    let inject_started = Instant::now();
    crate::remember_pasted(final_text);
    if let Err(e) = inject::paste_text(final_text) {
        store_anyway(db, final_text, raw_text, data);
        timings.report(app);
        fail(
            app,
            db,
            state,
            format!("paste failed: {e} — text saved to history"),
        );
        return;
    }
    timings.mark("inject", inject_started);

    match db.insert_transcript(
        final_text,
        raw_text,
        &data.language,
        data.duration_ms,
        &data.target_app,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_matcher_catches_stock_phrases_only() {
        assert!(is_whisper_artifact("Thank you."));
        assert!(is_whisper_artifact("Thanks for watching!"));
        assert!(is_whisper_artifact("Bye"));
        assert!(is_whisper_artifact("  you "));
        assert!(is_whisper_artifact(""));
        // Real dictation of the same words must NOT be flagged when the
        // energy gate passes it through with more content.
        assert!(!is_whisper_artifact("thank you for the quick review"));
        assert!(!is_whisper_artifact("bye everyone, see you tomorrow"));
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
}
