use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioEngine;
use crate::cloud::{llm, stt};
use crate::hotkey::{
    HotkeyEvent, HotkeyWatcher, PushToTalkWatcher, SharedHotkeyConfig, WatcherStatus,
};
use crate::inject;
use crate::store::{Store, Transcript};

const MAX_SESSION_SECS: u64 = 360;
const DOUBLE_TAP_WINDOW_MS: u64 = 700;
/// Hands-free sessions end themselves after this much silence, so walking
/// away from the mic doesn't leave a giant accidental transcript behind.
const HANDS_FREE_SILENCE_STOP_MS: u64 = 5000;
const VOICE_LEVEL_THRESHOLD: f32 = 0.03;

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
    /// Sent by the worker thread once post-processing (STT/LLM/inject)
    /// completes, so the hotkey handler can accept new dictations again.
    SessionDone,
}

pub struct Pipeline {
    state: Arc<AtomicU8>,
    control_tx: mpsc::Sender<Msg>,
}

impl Pipeline {
    pub fn start(
        app: AppHandle,
        db: Arc<Store>,
        hotkey_config: SharedHotkeyConfig,
        watcher_status: std::sync::Arc<std::sync::RwLock<String>>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let state = Arc::new(AtomicU8::new(PipelineState::Idle.as_u8()));

        let (hk_tx, hk_rx) = mpsc::channel::<HotkeyEvent>();
        {
            // Input Monitoring can be granted at any time (and is silently
            // revoked when the app bundle is replaced), so poll until the
            // gate opens. Accessibility is deliberately NOT required here:
            // recording and transcription work without it, and a missing
            // Accessibility grant surfaces as a clear error at paste time.
            let config = Arc::clone(&hotkey_config);
            let status_app = app.clone();
            let status_cell = Arc::clone(&watcher_status);
            std::thread::spawn(move || {
                let mut announced = false;
                loop {
                    if inject::is_listen_event_trusted() {
                        break;
                    }
                    if !announced {
                        *status_cell.write().unwrap() = "waiting-permissions".to_string();
                        emit_hotkey_status(&status_app, "waiting-permissions", None);
                        announced = true;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
                eprintln!("Input Monitoring granted — hotkey watcher starting");
                let status_cell = Arc::clone(&status_cell);
                PushToTalkWatcher {
                    config,
                    poll_interval_ms: 20,
                    on_status: Some(Arc::new(move |status| match status {
                        WatcherStatus::Ready => {
                            *status_cell.write().unwrap() = "ready".to_string();
                            emit_hotkey_status(&status_app, "ready", None);
                        }
                        WatcherStatus::Unavailable(reason) => {
                            *status_cell.write().unwrap() = format!("unavailable:{reason}");
                            emit_hotkey_status(&status_app, "unavailable", Some(reason));
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
            let timer_tx = tx.clone();
            std::thread::spawn(move || handler_loop(app, db, rx, state, timer_tx));
        }

        Self {
            state,
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
    let bounds = monitor.size();
    let bar_w = crate::FLOWBAR_SIZE.0 * scale;
    let bar_h = crate::FLOWBAR_SIZE.1 * scale;
    // A small margin keeps a sliver of the pill grabbable at the edges.
    let margin = 8.0 * scale;
    let max_x = (bounds.width as f64 - bar_w - margin).max(0.0);
    let max_y = (bounds.height as f64 - bar_h - margin).max(0.0);
    let x = (outer.x as f64).clamp(margin, max_x.max(margin));
    let y = (outer.y as f64).clamp(margin, max_y.max(margin));
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

fn handler_loop(
    app: AppHandle,
    db: Arc<Store>,
    rx: mpsc::Receiver<Msg>,
    state: Arc<AtomicU8>,
    timer_tx: mpsc::Sender<Msg>,
) {
    let mut audio = AudioEngine::new();
    let mut current_app = String::new();
    let mut mode = Mode::Idle;
    let mut pending_tap = false;
    let mut first_tap_at: Option<Instant> = None;
    // True while a worker thread is post-processing a finished recording.
    // The hotkey handler stays responsive during that window and queues one
    // start request instead of blocking like the old synchronous flow.
    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut pending_start = false;
    // Shared with the level callback (last audible input) and a generation
    // counter that invalidates stale hands-free auto-stop watchdogs.
    let last_voice_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let session_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
    {
        let emitter = app.clone();
        let last_emit = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last_voice_ms = Arc::clone(&last_voice_ms);
        audio.set_level_callback(move |level| {
            let now = unix_ms();
            if level >= VOICE_LEVEL_THRESHOLD {
                last_voice_ms.store(now, Ordering::Relaxed);
            }
            let prev = last_emit.swap(now, Ordering::Relaxed);
            if now.saturating_sub(prev) >= 33 {
                let _ = emitter.emit("audio-level", level);
            }
        });
    }

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Hotkey(HotkeyEvent::Down) => {
                if mode != Mode::Idle
                    && state.load(Ordering::Relaxed) == PipelineState::Paused.as_u8()
                {
                    // Pressing the hotkey while paused resumes capture.
                    audio.resume();
                    transition(&app, &db, &state, PipelineState::Recording, None, None);
                    continue;
                }
                if mode == Mode::Idle {
                    if busy.load(Ordering::Relaxed) {
                        // Previous dictation still processing — queue a
                        // start for when the worker finishes.
                        pending_start = true;
                        continue;
                    }
                    current_app = inject::frontmost_app();
                    audio.set_device(mic_preference(&db));
                    if let Err(e) = audio.start() {
                        // Never swallow this: a dead mic must look different
                        // from a dead hotkey, or users cannot tell them apart.
                        fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                        continue;
                    }
                    mode = Mode::Ptt;
                    pending_tap = false;
                    session_gen.fetch_add(1, Ordering::Relaxed);
                    spawn_max_session_timer(&timer_tx);
                    transition(&app, &db, &state, PipelineState::Recording, None, None);
                }
                // Down while Ptt (second press of entering double-tap) or
                // HandsFree (exit press): keep recording; release decides.
            }
            Msg::Hotkey(HotkeyEvent::Up) => {
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
                } else if !pending_start {
                    transition(&app, &db, &state, PipelineState::Idle, None, None);
                }
                // Up while a queued start is pending: the eventual session
                // will be a sub-250ms tap and gets discarded on its own.
            }
            Msg::Hotkey(HotkeyEvent::TapUp) => match mode {
                Mode::Ptt => {
                    let confirmed_double = pending_tap
                        && first_tap_at
                            .map(|t| t.elapsed() < Duration::from_millis(DOUBLE_TAP_WINDOW_MS))
                            .unwrap_or(false);
                    if !pending_tap {
                        // First quick tap: hold judgement, keep recording.
                        pending_tap = true;
                        first_tap_at = Some(Instant::now());
                    } else if confirmed_double {
                        // Double-tap confirmed: restart capture cleanly and
                        // go hands-free.
                        audio.discard();
                        current_app = inject::frontmost_app();
                        audio.set_device(mic_preference(&db));
                        if let Err(e) = audio.start() {
                            fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                            mode = Mode::Idle;
                        } else {
                            mode = Mode::HandsFree;
                            pending_tap = false;
                            last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                            let gen = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                            spawn_handsfree_watchdog(
                                &timer_tx,
                                Arc::clone(&last_voice_ms),
                                Arc::clone(&session_gen),
                                gen,
                            );
                            spawn_max_session_timer(&timer_tx);
                        }
                    } else {
                        // Slow second tap — treat as hands-free entry too
                        // but without resetting the buffer.
                        mode = Mode::HandsFree;
                        pending_tap = false;
                        last_voice_ms.store(unix_ms(), Ordering::Relaxed);
                        let gen = session_gen.fetch_add(1, Ordering::Relaxed) + 1;
                        spawn_handsfree_watchdog(
                            &timer_tx,
                            Arc::clone(&last_voice_ms),
                            Arc::clone(&session_gen),
                            gen,
                        );
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
                }
                Mode::Idle => {}
            },
            Msg::PauseToggle => {
                if mode == Mode::Ptt || mode == Mode::HandsFree {
                    let was_paused = state.load(Ordering::Relaxed) == PipelineState::Paused.as_u8();
                    if was_paused {
                        audio.resume();
                        transition(&app, &db, &state, PipelineState::Recording, None, None);
                    } else {
                        audio.pause();
                        session_gen.fetch_add(1, Ordering::Relaxed);
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
                transition(&app, &db, &state, PipelineState::Idle, None, None);
            }
            Msg::SessionDone => {
                busy.store(false, Ordering::Relaxed);
                if pending_start && mode == Mode::Idle {
                    // A hotkey press arrived while the worker was busy —
                    // begin that queued dictation now.
                    pending_start = false;
                    current_app = inject::frontmost_app();
                    audio.set_device(mic_preference(&db));
                    if let Err(e) = audio.start() {
                        fail(&app, &db, &state, format!("microphone unavailable: {e}"));
                    } else {
                        mode = Mode::Ptt;
                        pending_tap = false;
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
            // Hand the recording to a worker thread so the hotkey handler
            // can keep reacting while STT/LLM/injection run.
            busy.store(true, Ordering::Relaxed);
            let worker_app = app.clone();
            let worker_db = Arc::clone(db);
            let worker_state = Arc::clone(state);
            let done_tx = done_tx.clone();
            let target_app = std::mem::take(current_app);
            std::thread::spawn(move || {
                // SessionDone must be sent even if processing panics,
                // otherwise the pipeline would stay busy forever.
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
                    eprintln!("dictation worker panicked");
                    set_state(&worker_state, PipelineState::Idle);
                }
                let _ = done_tx.send(Msg::SessionDone);
            });
        }
        Ok(None) => transition(app, db, state, PipelineState::Idle, None, None),
        Err(e) => fail(app, db, state, e.to_string()),
    }
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

fn run_session(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    recording: crate::audio::Recording,
    target_app: String,
) {
    let mut timings = StageTimings::begin();
    transition(app, db, state, PipelineState::Transcribing, None, None);

    let language = {
        let raw = db.get_setting("language").ok().flatten();
        match raw.and_then(|v| serde_json::from_str::<String>(&v).ok()) {
            Some(lang) => lang,
            None => "auto".to_string(),
        }
    };

    let result = stt::transcribe(db, &recording.wav, &language, Some(&db_prompt(db)));
    let stt_started = timings.mark("wav-encode+prep", timings.started);

    let raw_text = match result {
        Ok(r) => r.text,
        Err(e) => {
            timings.report(app);
            return fail(app, db, state, e.to_string());
        }
    };
    let stt_done = timings.mark("stt", stt_started);
    if raw_text.trim().is_empty() {
        timings.report(app);
        return fail(app, db, state, "transcription came back empty".to_string());
    }

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
    let polished = if cleanup_enabled(db) && !(short && cleanup_skip_short(db)) {
        match llm::polish(db, &raw_text, &data.target_app) {
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
