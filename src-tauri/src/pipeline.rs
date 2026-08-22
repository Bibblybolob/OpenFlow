use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::audio::AudioEngine;
use crate::cloud::{llm, stt};
use crate::hotkey::{HotkeyEvent, HotkeyWatcher, PushToTalkWatcher, SharedHotkeyConfig};
use crate::inject;
use crate::store::{Store, Transcript};

const MAX_SESSION_SECS: u64 = 360;
const DOUBLE_TAP_WINDOW_MS: u64 = 700;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PipelineState {
    Idle,
    Recording,
    Transcribing,
    Injecting,
}

impl PipelineState {
    const fn as_u8(self) -> u8 {
        match self {
            PipelineState::Idle => 0,
            PipelineState::Recording => 1,
            PipelineState::Transcribing => 2,
            PipelineState::Injecting => 3,
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
}

pub struct Pipeline {
    state: Arc<AtomicU8>,
    control_tx: mpsc::Sender<Msg>,
}

impl Pipeline {
    pub fn start(app: AppHandle, db: Arc<Store>, hotkey_config: SharedHotkeyConfig) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let state = Arc::new(AtomicU8::new(PipelineState::Idle.as_u8()));

        let (hk_tx, hk_rx) = mpsc::channel::<HotkeyEvent>();
        if inject::is_accessibility_trusted() {
            PushToTalkWatcher {
                config: hotkey_config,
                poll_interval_ms: 20,
            }
            .spawn(hk_tx);
        } else {
            eprintln!("dictation disabled: grant Accessibility permission in System Settings");
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
        match self.state.load(Ordering::Relaxed) {
            1 => PipelineState::Recording,
            2 => PipelineState::Transcribing,
            3 => PipelineState::Injecting,
            _ => PipelineState::Idle,
        }
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

fn emit(app: &AppHandle, event: PipelineEvent) {
    let _ = app.emit("pipeline", event);
}

fn fail(app: &AppHandle, state: &AtomicU8, message: String) {
    eprintln!("pipeline error: {message}");
    set_state(state, PipelineState::Idle);
    emit(
        app,
        PipelineEvent {
            state: PipelineState::Idle,
            error: Some(message),
            transcript: None,
        },
    );
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
    {
        let emitter = app.clone();
        let last_emit = Arc::new(std::sync::atomic::AtomicU64::new(0));
        audio.set_level_callback(move |level| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let prev = last_emit.swap(now, Ordering::Relaxed);
            if now.saturating_sub(prev) >= 33 {
                let _ = emitter.emit("audio-level", level);
            }
        });
    }

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Hotkey(HotkeyEvent::Down) => {
                if mode == Mode::Idle {
                    current_app = inject::frontmost_app();
                    if audio.start().is_err() {
                        continue;
                    }
                    mode = Mode::Ptt;
                    pending_tap = false;
                    spawn_max_session_timer(&timer_tx);
                    set_state(&state, PipelineState::Recording);
                    emit(
                        &app,
                        PipelineEvent {
                            state: PipelineState::Recording,
                            error: None,
                            transcript: None,
                        },
                    );
                }
                // Down while Ptt (second press of entering double-tap) or
                // HandsFree (exit press): keep recording; release decides.
            }
            Msg::Hotkey(HotkeyEvent::Up) => {
                if mode != Mode::Idle {
                    finish_session(&app, &db, &state, &mut audio, &mut current_app);
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                } else {
                    set_state(&state, PipelineState::Idle);
                    emit(
                        &app,
                        PipelineEvent {
                            state: PipelineState::Idle,
                            error: None,
                            transcript: None,
                        },
                    );
                }
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
                        if audio.start().is_ok() {
                            mode = Mode::HandsFree;
                            pending_tap = false;
                            spawn_max_session_timer(&timer_tx);
                        } else {
                            fail(&app, &state, "microphone unavailable".to_string());
                            mode = Mode::Idle;
                        }
                    } else {
                        // Slow second tap — treat as hands-free entry too
                        // but without resetting the buffer.
                        mode = Mode::HandsFree;
                        pending_tap = false;
                    }
                }
                Mode::HandsFree => {
                    finish_session(&app, &db, &state, &mut audio, &mut current_app);
                    mode = Mode::Idle;
                    pending_tap = false;
                    first_tap_at = None;
                }
                Mode::Idle => {}
            },
            Msg::Cancel => {
                audio.discard();
                mode = Mode::Idle;
                pending_tap = false;
                first_tap_at = None;
                set_state(&state, PipelineState::Idle);
                emit(
                    &app,
                    PipelineEvent {
                        state: PipelineState::Idle,
                        error: None,
                        transcript: None,
                    },
                );
            }
        }
    }
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
) {
    let wav_path = temp_wav_path();
    match audio.stop(&wav_path) {
        Ok(Some(duration_ms)) => run_session(
            app,
            db,
            state,
            &wav_path,
            duration_ms,
            std::mem::take(current_app),
        ),
        Ok(None) => {
            let _ = std::fs::remove_file(&wav_path);
            set_state(state, PipelineState::Idle);
            emit(
                app,
                PipelineEvent {
                    state: PipelineState::Idle,
                    error: None,
                    transcript: None,
                },
            );
        }
        Err(e) => fail(app, state, e.to_string()),
    }
}

fn run_session(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    wav_path: &std::path::Path,
    duration_ms: i64,
    target_app: String,
) {
    set_state(state, PipelineState::Transcribing);
    emit(
        app,
        PipelineEvent {
            state: PipelineState::Transcribing,
            error: None,
            transcript: None,
        },
    );

    let language = {
        let raw = db.get_setting("language").ok().flatten();
        match raw.and_then(|v| serde_json::from_str::<String>(&v).ok()) {
            Some(lang) => lang,
            None => "auto".to_string(),
        }
    };

    let result = stt::transcribe(db, wav_path, &language, Some(&db_prompt(db)));
    let _ = std::fs::remove_file(wav_path);

    let raw_text = match result {
        Ok(r) => r.text,
        Err(e) => return fail(app, state, e.to_string()),
    };
    if raw_text.trim().is_empty() {
        return fail(app, state, "transcription came back empty".to_string());
    }

    let data = SessionData::new(duration_ms, target_app, language);

    // Fast path: whole utterance matches a snippet trigger — no LLM call.
    let snippet = crate::cloud::try_snippet(db, &raw_text).unwrap_or(None);
    if let Some(expanded) = snippet {
        return finish(app, db, state, &expanded, &raw_text, &data);
    }

    set_state(state, PipelineState::Injecting);
    emit(
        app,
        PipelineEvent {
            state: PipelineState::Injecting,
            error: None,
            transcript: None,
        },
    );

    // LLM cleanup; fall back to the raw transcription on any failure so a
    // cleanup outage never costs the user their dictation.
    let polished = match llm::polish(db, &raw_text, &data.target_app) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("cleanup skipped: {e}");
            emit_warning(app, format!("cleanup unavailable — pasted raw text ({e})"));
            raw_text.clone()
        }
    };

    finish(app, db, state, &polished, &raw_text, &data);
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
) {
    set_state(state, PipelineState::Injecting);
    emit(
        app,
        PipelineEvent {
            state: PipelineState::Injecting,
            error: None,
            transcript: None,
        },
    );

    if let Err(e) = inject::paste_text(final_text) {
        store_anyway(db, final_text, raw_text, data);
        fail(
            app,
            state,
            format!("paste failed: {e} — text saved to history"),
        );
        return;
    }

    match db.insert_transcript(
        final_text,
        raw_text,
        &data.language,
        data.duration_ms,
        &data.target_app,
    ) {
        Ok(transcript) => {
            set_state(state, PipelineState::Idle);
            emit(
                app,
                PipelineEvent {
                    state: PipelineState::Idle,
                    error: None,
                    transcript: Some(transcript),
                },
            );
        }
        Err(e) => fail(app, state, e.to_string()),
    }
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

fn temp_wav_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("flowclone-{nanos}.wav"))
}
