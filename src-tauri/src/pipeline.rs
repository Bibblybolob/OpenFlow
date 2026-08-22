use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::audio::AudioEngine;
use crate::cloud::stt;
use crate::hotkey::{HotkeyEvent, HotkeyWatcher, PushToTalkWatcher};
use crate::inject;
use crate::store::{Store, Transcript};

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
}

pub struct Pipeline {
    state: Arc<AtomicU8>,
    _tx: mpsc::Sender<Msg>,
}

impl Pipeline {
    pub fn start(app: AppHandle, db: Arc<Store>) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let state = Arc::new(AtomicU8::new(PipelineState::Idle.as_u8()));

        let (hk_tx, hk_rx) = mpsc::channel::<HotkeyEvent>();
        if inject::is_accessibility_trusted() {
            PushToTalkWatcher::default().spawn(hk_tx);
        } else {
            eprintln!("dictation disabled: grant Accessibility permission in System Settings");
        }
        let keepalive_tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = hk_rx.recv() {
                if tx.send(Msg::Hotkey(event)).is_err() {
                    return;
                }
            }
        });

        {
            let state = Arc::clone(&state);
            std::thread::spawn(move || handler_loop(app, db, rx, state));
        }

        Self {
            state,
            _tx: keepalive_tx,
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

fn handler_loop(app: AppHandle, db: Arc<Store>, rx: mpsc::Receiver<Msg>, state: Arc<AtomicU8>) {
    let mut audio = AudioEngine::new();

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Hotkey(HotkeyEvent::Down) => {
                if audio.start().is_err() {
                    continue;
                }
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
            Msg::Hotkey(HotkeyEvent::Up) => {
                let wav_path = temp_wav_path();
                match audio.stop(&wav_path) {
                    Ok(Some(duration_ms)) => run_session(&app, &db, &state, &wav_path, duration_ms),
                    Ok(None) => {
                        let _ = std::fs::remove_file(&wav_path);
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
                    Err(e) => fail(&app, &state, e.to_string()),
                }
            }
        }
    }
}

fn run_session(
    app: &AppHandle,
    db: &Arc<Store>,
    state: &Arc<AtomicU8>,
    wav_path: &std::path::Path,
    duration_ms: i64,
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

    let text = match result {
        Ok(r) => r.text,
        Err(e) => return fail(app, state, e.to_string()),
    };
    if text.trim().is_empty() {
        return fail(app, state, "transcription came back empty".to_string());
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

    if let Err(e) = inject::paste_text(&text) {
        store_anyway(db, &text, duration_ms, &language);
        return fail(
            app,
            state,
            format!("paste failed: {e} — text saved to history"),
        );
    }

    match db.insert_transcript(&text, &text, &language, duration_ms, "") {
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

fn db_prompt(db: &Store) -> String {
    stt::build_prompt(db).unwrap_or_default()
}

fn store_anyway(db: &Store, text: &str, duration_ms: i64, language: &str) {
    let _ = db.insert_transcript(text, text, language, duration_ms, "");
}

fn temp_wav_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("flowclone-{nanos}.wav"))
}
