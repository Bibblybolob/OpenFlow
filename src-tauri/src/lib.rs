mod audio;
mod cloud;
mod commands;
mod emoji;
mod hotkey;
#[cfg(target_os = "macos")]
mod hotkey_tap;
mod inject;
mod learn;
mod pipeline;
mod sound;
mod store;

use std::fs;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};

use hotkey::{key_name, key_options, parse_key, HotkeyConfig, SharedHotkeyConfig};
use pipeline::Pipeline;
use store::Store;

pub struct AppState {
    db: Arc<Store>,
    pipeline: Pipeline,
    hotkey: SharedHotkeyConfig,
    watcher_status: Arc<RwLock<String>>,
    mic_level: Arc<AtomicU32>,
    mic_voiced: Arc<std::sync::atomic::AtomicU8>,
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Store) -> T) -> T {
    f(&state.db)
}

/// Preferred input device name from settings; `None` means system default.
pub(crate) fn configured_mic(db: &Store) -> Option<String> {
    db.get_setting("micDevice")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<Option<String>>(&value).ok())
        .flatten()
}

pub(crate) const FLOWBAR_SIZE: (f64, f64) = (240.0, 52.0);

/// Location and time of the most recent injection. Scratch is only safe while
/// that same app is still focused and the paste is recent; otherwise Ctrl/Cmd
/// +Z could undo an unrelated action in a different application.
struct LastPaste {
    target_app: String,
    pasted_at: std::time::Instant,
}

static LAST_PASTED: Mutex<Option<LastPaste>> = Mutex::new(None);
const SCRATCH_MAX_AGE_SECS: u64 = 30;

/// A transcription whose STT stage failed, kept around so it can be retried
/// without re-recording.
#[derive(Debug, Clone)]
struct RetryJob {
    wav: Vec<u8>,
    target_app: String,
}
pub(crate) static PENDING_RETRY: Mutex<Option<RetryJob>> = Mutex::new(None);

pub(crate) fn store_retry_job(wav: Vec<u8>, target_app: String) {
    if let Ok(mut slot) = PENDING_RETRY.lock() {
        *slot = Some(RetryJob { wav, target_app });
    }
}

pub(crate) fn clear_retry_job() {
    if let Ok(mut slot) = PENDING_RETRY.lock() {
        *slot = None;
    }
}

/// Text captured from before the caret while a session records; taken by
/// the processing worker when the recording ends.
pub(crate) static CARET_CONTEXT: Mutex<Option<String>> = Mutex::new(None);
static CARET_CONTEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn begin_context_capture() {
    let generation = CARET_CONTEXT_GENERATION.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    if let Ok(mut slot) = CARET_CONTEXT.lock() {
        *slot = None;
    }
    // Runs concurrently with recording: reading AX can take 50-150ms and
    // must not sit on the hotkey path. Focus cannot change meanwhile —
    // the pill window is not focusable.
    std::thread::spawn(move || {
        let ctx = inject::preceding_context();
        if !ctx.is_empty() {
            if let Ok(mut slot) = CARET_CONTEXT.lock() {
                // A rapid cancel/restart can leave an older AX/UIA read in
                // flight. Never let that old result overwrite the context for
                // the newer dictation.
                if CARET_CONTEXT_GENERATION.load(AtomicOrdering::Relaxed) == generation {
                    *slot = Some(ctx);
                }
            }
        }
    });
}

pub(crate) fn take_caret_context() -> Option<String> {
    CARET_CONTEXT.lock().ok().and_then(|mut slot| slot.take())
}

pub(crate) fn remember_pasted(target_app: &str) {
    if let Ok(mut slot) = LAST_PASTED.lock() {
        *slot = Some(LastPaste {
            target_app: target_app.to_string(),
            pasted_at: std::time::Instant::now(),
        });
    }
}

/// Removes the last pasted text via synthesized undo ("scratch that").
pub(crate) fn scratch_last() -> Result<(), String> {
    let mut slot = LAST_PASTED
        .lock()
        .map_err(|_| "lock poisoned".to_string())?;
    let Some(last) = slot.as_ref() else {
        return Err("nothing recent to remove".to_string());
    };
    if last.pasted_at.elapsed() > std::time::Duration::from_secs(SCRATCH_MAX_AGE_SECS) {
        *slot = None;
        return Err("the last dictation is too old to remove safely".to_string());
    }
    let current_app = inject::frontmost_app();
    if last.target_app.is_empty() || !current_app.eq_ignore_ascii_case(&last.target_app) {
        return Err(format!(
            "switch back to {} before removing the last dictation",
            if last.target_app.is_empty() {
                "the original app"
            } else {
                &last.target_app
            }
        ));
    }
    let result = inject::undo_paste();
    if result.is_ok() {
        *slot = None;
    }
    result
}

/// Handle to the tray's Start/Stop item so the pipeline can relabel it as
/// the dictation state changes.
pub struct TrayMenu {
    toggle: MenuItem<tauri::Wry>,
}

/// Reflects the pipeline state into the tray: the first menu item toggles
/// between "Start dictation" / "Stop dictation" and the tooltip names the
/// current state.
pub(crate) fn update_tray(app: &tauri::AppHandle, next: pipeline::PipelineState) {
    let Some(tray_menu) = app.try_state::<TrayMenu>() else {
        return;
    };
    let label = match next {
        pipeline::PipelineState::Idle => "Start dictation",
        pipeline::PipelineState::Recording | pipeline::PipelineState::Paused => "Stop dictation",
        _ => "Working…",
    };
    let _ = tray_menu.toggle.set_text(label);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(format!("FlowClone — {next}")));
    }
}

/// Shows the existing Hub window for tray, reopen, and second-launch actions.
fn show_hub(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("hub") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub(crate) fn flowbar_auto_hide(db: &Store) -> bool {
    db.get_setting("flowBarStyle")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<Value>(&v).ok())
        .and_then(|v| v.get("autoHide").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

fn create_flowbar(app: &tauri::AppHandle, db: &Store) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::{PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

    let saved: Option<(f64, f64)> = db
        .get_setting("flowBarPos")?
        .and_then(|v| serde_json::from_str(&v).ok());
    let start_visible = !flowbar_auto_hide(db);

    let window = WebviewWindowBuilder::new(app, "flowbar", WebviewUrl::App("/#/flowbar".into()))
        .title("FlowBar")
        .inner_size(FLOWBAR_SIZE.0, FLOWBAR_SIZE.1)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .visible(start_visible)
        .build()?;

    if let Some((x, y)) = saved {
        window.set_position(PhysicalPosition::new(x, y))?;
    } else if let Some(monitor) = app.primary_monitor()? {
        let scale = monitor.scale_factor();
        let size = monitor.size();
        let bar_w = FLOWBAR_SIZE.0 * scale;
        let bar_h = FLOWBAR_SIZE.1 * scale;
        let x = ((size.width as f64 - bar_w) / 2.0).max(0.0);
        let y = (size.height as f64 - bar_h - 48.0 * scale).max(0.0);
        window.set_position(PhysicalPosition::new(x, y))?;
    }
    Ok(())
}

#[tauri::command]
fn insert_transcript(
    state: tauri::State<AppState>,
    text: String,
    raw_text: String,
    language: String,
    duration_ms: i64,
    target_app: String,
) -> store::Result<store::Transcript> {
    with_db(&state, |db| {
        db.insert_transcript(&text, &raw_text, &language, duration_ms, &target_app)
    })
}

#[tauri::command]
fn list_transcripts(
    state: tauri::State<AppState>,
    limit: i64,
    offset: i64,
) -> store::Result<Vec<store::Transcript>> {
    with_db(&state, |db| db.list_transcripts(limit, offset))
}

#[tauri::command]
fn search_transcripts(
    state: tauri::State<AppState>,
    query: String,
) -> store::Result<Vec<store::Transcript>> {
    with_db(&state, |db| db.search_transcripts(&query))
}

#[tauri::command]
fn delete_transcript(state: tauri::State<AppState>, id: i64) -> store::Result<()> {
    with_db(&state, |db| db.delete_transcript(id))
}

#[tauri::command]
fn set_flagged(state: tauri::State<AppState>, id: i64, flagged: bool) -> store::Result<()> {
    with_db(&state, |db| db.set_flagged(id, flagged))
}

#[tauri::command]
fn transcript_stats(state: tauri::State<AppState>) -> store::Result<store::Stats> {
    with_db(&state, |db| db.stats())
}

#[tauri::command]
fn add_dictionary_term(
    state: tauri::State<AppState>,
    term: String,
    replacement: Option<String>,
) -> store::Result<store::DictionaryEntry> {
    with_db(&state, |db| {
        db.add_dictionary_term(&term, replacement.as_deref())
    })
}

#[tauri::command]
fn list_dictionary(state: tauri::State<AppState>) -> store::Result<Vec<store::DictionaryEntry>> {
    with_db(&state, |db| db.list_dictionary())
}

#[tauri::command]
fn set_dictionary_starred(
    state: tauri::State<AppState>,
    id: i64,
    starred: bool,
) -> store::Result<()> {
    with_db(&state, |db| db.set_dictionary_starred(id, starred))
}

#[tauri::command]
fn delete_dictionary_term(state: tauri::State<AppState>, id: i64) -> store::Result<()> {
    with_db(&state, |db| db.delete_dictionary_term(id))
}

#[tauri::command]
fn list_vocab_suggestions(
    state: tauri::State<AppState>,
) -> store::Result<Vec<store::VocabSuggestion>> {
    with_db(&state, |db| db.list_vocab_suggestions())
}

#[tauri::command]
fn accept_vocab_suggestion(state: tauri::State<AppState>, id: i64) -> store::Result<()> {
    with_db(&state, |db| db.accept_vocab_suggestion(id))
}

#[tauri::command]
fn dismiss_vocab_suggestion(state: tauri::State<AppState>, id: i64) -> store::Result<()> {
    with_db(&state, |db| db.dismiss_vocab_suggestion(id))
}

#[tauri::command]
fn add_snippet(
    state: tauri::State<AppState>,
    trigger: String,
    body: String,
) -> store::Result<store::Snippet> {
    with_db(&state, |db| db.add_snippet(&trigger, &body))
}

#[tauri::command]
fn list_snippets(state: tauri::State<AppState>) -> store::Result<Vec<store::Snippet>> {
    with_db(&state, |db| db.list_snippets())
}

#[tauri::command]
fn delete_snippet(state: tauri::State<AppState>, id: i64) -> store::Result<()> {
    with_db(&state, |db| db.delete_snippet(id))
}

#[tauri::command]
fn upsert_style(
    state: tauri::State<AppState>,
    app_pattern: String,
    label: String,
    instructions: String,
    language: Option<String>,
) -> store::Result<store::Style> {
    with_db(&state, |db| {
        db.upsert_style(
            &app_pattern,
            &label,
            &instructions,
            language.as_deref().filter(|l| !l.is_empty()),
        )
    })
}

#[tauri::command]
fn list_styles(state: tauri::State<AppState>) -> store::Result<Vec<store::Style>> {
    with_db(&state, |db| db.list_styles())
}

#[tauri::command]
fn set_style_enabled(state: tauri::State<AppState>, id: i64, enabled: bool) -> store::Result<()> {
    with_db(&state, |db| db.set_style_enabled(id, enabled))
}

#[tauri::command]
fn delete_style(state: tauri::State<AppState>, id: i64) -> store::Result<()> {
    with_db(&state, |db| db.delete_style(id))
}

#[tauri::command]
fn resolve_style(
    state: tauri::State<AppState>,
    app_identifier: String,
) -> store::Result<Option<String>> {
    with_db(&state, |db| db.resolve_style_for_app(&app_identifier))
}

#[tauri::command]
fn get_setting(state: tauri::State<AppState>, key: String) -> store::Result<Option<Value>> {
    let raw = with_db(&state, |db| db.get_setting(&key))?;
    raw.map(|s| serde_json::from_str(&s).map_err(store::StoreError::Json))
        .transpose()
}

#[tauri::command]
fn set_setting(state: tauri::State<AppState>, key: String, value: Value) -> store::Result<()> {
    with_db(&state, |db| db.set_setting(&key, &value))
}

#[tauri::command]
fn pipeline_status(state: tauri::State<AppState>) -> pipeline::PipelineState {
    state.pipeline.current()
}

/// Last reported hotkey-watcher lifecycle state: "waiting-permissions",
/// "ready", or "unavailable:<reason>". Queryable so the Hub shows the truth
/// even if it opened after the events fired.
/// Latest microphone RMS (0.0..1.0), pulled by the pill webview on a timer
/// instead of pushed via events — WebKit throttles event/rAF delivery in
/// hidden overlay windows, but invoke+setInterval keep working.
#[derive(serde::Serialize)]
struct MicLevel {
    bar: f32,
    voiced: bool,
}

#[tauri::command]
fn mic_level(state: tauri::State<AppState>) -> MicLevel {
    MicLevel {
        bar: f32::from_bits(state.mic_level.load(AtomicOrdering::Relaxed)),
        voiced: state.mic_voiced.load(AtomicOrdering::Relaxed) != 0,
    }
}

#[tauri::command]
fn hotkey_watcher_status(state: tauri::State<AppState>) -> String {
    state.watcher_status.read().unwrap().clone()
}

#[tauri::command]
fn toggle_recording(state: tauri::State<AppState>) -> pipeline::PipelineState {
    state.pipeline.toggle();
    state.pipeline.current()
}

#[tauri::command]
fn cancel_recording(state: tauri::State<AppState>) {
    state.pipeline.cancel();
}

/// Re-pastes a history row into the app it originally came from. The Hub is
/// necessarily focused when its button is clicked, so transfer focus before
/// injecting or the text would be pasted back into FlowClone itself.
#[tauri::command]
fn paste_text_at_cursor(
    _state: tauri::State<AppState>,
    text: String,
    target_app: String,
) -> Result<(), String> {
    inject::focus_app(&target_app)?;
    inject::paste_text(&text)?;
    remember_pasted(&target_app);
    Ok(())
}

#[tauri::command]
fn retry_last(state: tauri::State<AppState>) -> Result<bool, String> {
    let Some(job) = PENDING_RETRY
        .lock()
        .map_err(|_| "lock poisoned".to_string())?
        .clone()
    else {
        return Ok(false);
    };
    Ok(state.pipeline.retry(
        crate::audio::Recording {
            duration_ms: 0,
            wav: job.wav,
            // Retried jobs predate per-session metering; treat as
            // clearly-voiced so the artifact guard never eats a retry.
            max_frame_rms: 1.0,
        },
        job.target_app,
    ))
}

#[tauri::command]
fn toggle_pause(state: tauri::State<AppState>) -> pipeline::PipelineState {
    state.pipeline.toggle_pause();
    state.pipeline.current()
}

#[tauri::command]
fn list_mics() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn mic_device_status(state: tauri::State<AppState>) -> store::Result<audio::MicDeviceStatus> {
    let configured = with_db(&state, configured_mic);
    audio::input_device_status(configured)
        .map_err(|error| store::StoreError::Other(error.to_string()))
}

#[tauri::command]
fn set_mic_device(state: tauri::State<AppState>, name: Option<String>) -> store::Result<()> {
    with_db(&state, |db| {
        db.set_setting("micDevice", &serde_json::json!(name))
    })
}

#[tauri::command]
fn get_hotkey(state: tauri::State<AppState>) -> Vec<String> {
    let cfg = state.hotkey.read().unwrap();
    cfg.keys
        .iter()
        .map(|k| key_name(*k).unwrap_or("Unknown").to_string())
        .collect()
}

#[tauri::command]
fn hotkey_options() -> Vec<String> {
    key_options()
}

#[tauri::command]
fn set_hotkey(state: tauri::State<AppState>, names: Vec<String>) -> store::Result<Vec<String>> {
    if names.is_empty() {
        return Err(store::StoreError::Other(
            "hotkey needs at least one key".to_string(),
        ));
    }
    let mut keys = Vec::with_capacity(names.len());
    for n in &names {
        match parse_key(n) {
            Some(k) => keys.push(k),
            None => return Err(store::StoreError::Other(format!("unsupported key: {n}"))),
        }
    }
    with_db(&state, |db| {
        db.set_setting("hotkeyKeys", &serde_json::json!(names))
    })?;
    *state.hotkey.write().unwrap() = HotkeyConfig {
        keys,
        ..HotkeyConfig::default()
    };
    Ok(names)
}

#[tauri::command]
fn autostart_status(app: tauri::AppHandle) -> store::Result<bool> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| store::StoreError::Other(e.to_string()))
}

#[tauri::command]
fn autostart_set(app: tauri::AppHandle, enable: bool) -> store::Result<()> {
    let autolaunch = app.autolaunch();
    let result = if enable {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    result.map_err(|e| store::StoreError::Other(e.to_string()))
}

#[tauri::command]
fn check_mic_permission(state: tauri::State<AppState>) -> store::Result<bool> {
    // A live recording is stronger evidence than opening a competing probe
    // stream (which can fail on exclusive-mode Windows devices).
    if state.pipeline.current() != pipeline::PipelineState::Idle {
        return Ok(true);
    }
    let mut probe = audio::AudioEngine::new();
    probe.set_device(with_db(&state, configured_mic));
    match probe.probe() {
        Ok(()) => Ok(true),
        Err(e) => Err(store::StoreError::Other(format!(
            "microphone unavailable: {e} — check permission in System Settings"
        ))),
    }
}

/// Catalog of on-device transcription models with download status.
#[tauri::command]
fn local_model_status() -> Vec<serde_json::Value> {
    use cloud::local_stt::{is_downloaded, LOCAL_MODELS};
    LOCAL_MODELS
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "label": m.label,
                "approxMb": m.approx_mb,
                "downloaded": is_downloaded(m.id),
            })
        })
        .collect()
}

/// Reports whether the optional Parakeet model bundle is available. The
/// command remains registered in standard builds so the frontend can probe
/// capability without knowing which Cargo features produced the binary.
#[tauri::command]
fn local_parakeet_status() -> serde_json::Value {
    #[cfg(feature = "parakeet")]
    {
        serde_json::json!({
            "id": cloud::local_parakeet::MODEL_ID,
            "available": true,
            "downloaded": cloud::local_parakeet::is_downloaded(),
        })
    }
    #[cfg(not(feature = "parakeet"))]
    {
        serde_json::json!({
            "id": "parakeet-tdt-0.6b-v3",
            "available": false,
            "downloaded": false,
        })
    }
}

#[tauri::command]
fn download_local_parakeet(app: tauri::AppHandle) -> store::Result<String> {
    #[cfg(feature = "parakeet")]
    {
        cloud::local_parakeet::download_model(&app).map(|path| path.to_string_lossy().into_owned())
    }
    #[cfg(not(feature = "parakeet"))]
    {
        let _ = app;
        Err(store::StoreError::Other(
            "Parakeet support is not enabled in this build".to_string(),
        ))
    }
}

/// Downloads an on-device model, streaming progress to the Hub. Runs on its
/// own thread so the UI stays responsive.
#[tauri::command]
fn download_local_model(app: tauri::AppHandle, model: String) -> Result<(), String> {
    std::thread::spawn(move || {
        if let Err(e) = cloud::local_stt::download_model(&app, &model) {
            eprintln!("model download failed: {e}");
            let _ = app.emit(
                "local-model-progress",
                serde_json::json!({
                    "type": "error",
                    "model": model,
                    "message": e.to_string(),
                }),
            );
        }
    });
    Ok(())
}

#[tauri::command]
fn set_local_model(state: tauri::State<AppState>, model: String) -> store::Result<()> {
    if !cloud::local_stt::LOCAL_MODELS
        .iter()
        .any(|entry| entry.id == model.as_str())
    {
        return Err(store::StoreError::Other(format!(
            "unknown local model: {model}"
        )));
    }
    with_db(&state, |db| {
        db.set_setting("sttLocalModel", &serde_json::json!(model))
    })
}

/// Catalog of on-device cleanup LLMs with download status.
#[tauri::command]
fn local_llm_status() -> Vec<serde_json::Value> {
    use cloud::local_llm::{is_downloaded, LOCAL_LLMS};
    LOCAL_LLMS
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "label": m.label,
                "approxMb": m.approx_mb,
                "downloaded": is_downloaded(m.id),
            })
        })
        .collect()
}

/// Downloads an on-device cleanup model, streaming progress to the Hub.
#[tauri::command]
fn download_local_llm(app: tauri::AppHandle, model: String) -> Result<(), String> {
    std::thread::spawn(move || {
        if let Err(e) = cloud::local_llm::download_model(&app, &model) {
            eprintln!("local llm download failed: {e}");
            let _ = app.emit(
                "local-llm-progress",
                serde_json::json!({
                    "type": "error",
                    "model": model,
                    "message": e.to_string(),
                }),
            );
        }
    });
    Ok(())
}

#[tauri::command]
fn set_local_llm(state: tauri::State<AppState>, model: String) -> store::Result<()> {
    if cloud::local_llm::catalog_llm(&model).is_none() {
        return Err(store::StoreError::Other(format!(
            "unknown local LLM: {model}"
        )));
    }
    with_db(&state, |db| {
        db.set_setting("llmLocalModel", &serde_json::json!(model))
    })
}

#[tauri::command]
fn set_flowbar_visible(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("flowbar")
        .ok_or("flowbar window not found")?;
    if visible {
        window.show().map_err(|e| e.to_string())
    } else {
        window.hide().map_err(|e| e.to_string())
    }
}

/// Padding (logical px) the fit command adds around the reported content
/// size — glow ring breathing room. Mirrors GLOW_PAD in FlowBar.tsx.
const FLOWBAR_FIT_PAD: f64 = 18.0;

/// Resizes the flowbar window to wrap its pill exactly: `width`/`height`
/// are the measured logical content size; the window grows by FLOWBAR_FIT_PAD
/// on every side and is repositioned so the pill's CENTER stays put. This
/// keeps the OS window footprint equal to the visible capsule — no invisible
/// slab to mis-click.
#[tauri::command]
fn flowbar_fit(app: tauri::AppHandle, width: f64, height: f64) -> Result<(), String> {
    use tauri::LogicalPosition;
    let window = app
        .get_webview_window("flowbar")
        .ok_or_else(|| "flowbar window not found".to_string())?;
    let sf = window.scale_factor().map_err(|e| e.to_string())?;
    // Current inner rect in logical coordinates; the pill fills it with a
    // GLOW_PAD inset, so its center is the window's center.
    let cur_inner = window.inner_size().map_err(|e| e.to_string())?;
    let cur_outer = window.outer_position().map_err(|e| e.to_string())?;
    let cur_w = cur_inner.width as f64 / sf;
    let cur_h = cur_inner.height as f64 / sf;
    let cx = cur_outer.x as f64 / sf + cur_w / 2.0;
    let cy = cur_outer.y as f64 / sf + cur_h / 2.0;

    let w = width.clamp(120.0, 4096.0) + FLOWBAR_FIT_PAD * 2.0;
    let h = height.clamp(44.0, 2048.0) + FLOWBAR_FIT_PAD * 2.0;
    let mut x = cx - w / 2.0;
    let mut y = cy - h / 2.0;

    // Clamp into the current monitor so growth (long partials, open menu)
    // can never push the pill off-screen.
    if let Ok(Some(mon)) = window.current_monitor() {
        let mpos = mon.position();
        let msize = mon.size();
        let mx = mpos.x as f64 / sf;
        let my = mpos.y as f64 / sf;
        let mw = msize.width as f64 / sf;
        let mh = msize.height as f64 / sf;
        x = x.clamp(mx, (mx + mw - w).max(mx));
        y = y.clamp(my, (my + mh - h).max(my));
    }

    window
        .set_size(tauri::LogicalSize::new(w, h))
        .map_err(|e| e.to_string())?;
    window
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_flowbar_preset(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    preset: String,
) -> store::Result<()> {
    use tauri::PhysicalPosition;
    let window = app
        .get_webview_window("flowbar")
        .ok_or_else(|| store::StoreError::Other("flowbar window not found".to_string()))?;
    let monitor = app
        .primary_monitor()
        .map_err(|e| store::StoreError::Other(e.to_string()))?
        .ok_or_else(|| store::StoreError::Other("no primary monitor".to_string()))?;
    let scale = monitor.scale_factor();
    let monitor_position = monitor.position();
    let screen = monitor.size();
    let window_size = window
        .inner_size()
        .map_err(|e| store::StoreError::Other(e.to_string()))?;
    let bar_w = window_size.width as f64;
    let bar_h = window_size.height as f64;
    let margin = 24.0 * scale;
    let origin_x = monitor_position.x as f64;
    let origin_y = monitor_position.y as f64;

    let (x, y) = match preset.as_str() {
        "top_left" => (origin_x + margin, origin_y + margin),
        "top_center" => (
            origin_x + (screen.width as f64 - bar_w) / 2.0,
            origin_y + margin,
        ),
        "top_right" => (
            origin_x + screen.width as f64 - bar_w - margin,
            origin_y + margin,
        ),
        "bottom_left" => (
            origin_x + margin,
            origin_y + screen.height as f64 - bar_h - margin,
        ),
        "bottom_center" => (
            origin_x + (screen.width as f64 - bar_w) / 2.0,
            origin_y + screen.height as f64 - bar_h - margin,
        ),
        "bottom_right" => (
            origin_x + screen.width as f64 - bar_w - margin,
            origin_y + screen.height as f64 - bar_h - margin,
        ),
        other => {
            return Err(store::StoreError::Other(format!(
                "unknown position preset: {other}"
            )))
        }
    };
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|e| store::StoreError::Other(e.to_string()))?;
    with_db(&state, |db| {
        db.set_setting("flowBarPos", &serde_json::json!([x, y]))?;
        db.set_setting("flowBarPreset", &serde_json::json!(preset))
    })
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| format!("updater unavailable: {e}"))?
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    Ok(update.map(|u| u.version))
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app
        .updater()
        .map_err(|e| format!("updater unavailable: {e}"))?;
    if let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?
    {
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| format!("update install failed: {e}"))?;
        app.restart();
    }
    Ok(())
}

fn load_hotkey_config(db: &Store) -> SharedHotkeyConfig {
    let mut config = HotkeyConfig::default();
    if let Some(names) = db
        .get_setting("hotkeyKeys")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
    {
        let migrated: Vec<String> = names
            .iter()
            .filter_map(|n| canonicalize_key_name(n))
            .collect();
        let keys: Vec<_> = migrated.iter().filter_map(|n| parse_key(n)).collect();
        if !keys.is_empty() {
            config.keys = keys;
            if migrated != names {
                // Persist the rewritten names so Settings shows real options.
                let _ = db.set_setting("hotkeyKeys", &serde_json::json!(migrated));
            }
        }
    }
    migrate_stale_f5_default(db, &mut config);
    Arc::new(RwLock::new(config))
}

/// One-time upgrade: installs built before the Right Shift default persisted
/// `["F5"]`, and on modern MacBooks F5 is the mic/dictation key whose HID
/// usage never surfaces as keyboard-F5 — the watcher reports ready while the
/// key stays invisible. Migrate those to the current default once; users who
/// deliberately re-pick F5 afterwards keep it.
fn migrate_stale_f5_default(db: &Store, config: &mut HotkeyConfig) {
    let already = db
        .get_setting("hotkeyMigratedRightShift")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(false);
    if already {
        return;
    }
    let _ = db.set_setting("hotkeyMigratedRightShift", &serde_json::json!(true));
    let is_f5_only = config.keys.iter().all(|k| *k == device_query::Keycode::F5);
    if !config.keys.is_empty() && is_f5_only && parse_key("F5").is_some() {
        eprintln!("hotkey: migrating stale default F5 -> Right Shift");
        config.keys = vec![device_query::Keycode::RShift];
        let _ = db.set_setting("hotkeyKeys", &serde_json::json!(["Right Shift"]));
    }
}

/// Maps a persisted hotkey name onto this platform's KEY_TABLE. Handles
/// names written by older releases ("Right Cmd/Win") and drops names the
/// running platform's keyboard backend cannot detect (e.g. Right Ctrl on
/// macOS, which reports no such key).
fn canonicalize_key_name(name: &str) -> Option<String> {
    if parse_key(name).is_some() {
        return Some(name.to_string());
    }
    match name {
        "Right Cmd/Win" | "Right Cmd" => {
            if parse_key("Cmd").is_some() {
                Some("Cmd".to_string())
            } else {
                Some("Right Win".to_string())
            }
        }
        "Right Alt" if parse_key("Right Option").is_some() => Some("Right Option".to_string()),
        _ => None,
    }
}

#[tauri::command]
fn accessibility_status() -> bool {
    inject::is_accessibility_trusted()
}

#[tauri::command]
fn input_monitoring_status() -> bool {
    inject::is_listen_event_trusted()
}

/// Recent raw keystrokes seen by the hotkey backend, for the Settings
/// diagnostics row ("press your hotkey and watch it appear").
#[tauri::command]
fn hotkey_last_seen() -> Vec<serde_json::Value> {
    #[cfg(target_os = "macos")]
    {
        crate::hotkey_tap::recent_events()
            .into_iter()
            .map(|e| serde_json::json!({ "name": e.name, "down": e.down, "agoMs": e.ago_ms }))
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

#[tauri::command]
fn open_accessibility_settings(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_input_monitoring_settings(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        show_hub(app);
    }));

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir)?;
            cloud::local_stt::init_models_dir(dir.join("models"));
            let db = Arc::new(Store::open(&dir.join("flowclone.db"))?);
            let hotkey = load_hotkey_config(&db);
            create_flowbar(app.handle(), &db)?;
            let watcher_status = Arc::new(RwLock::new("waiting-permissions".to_string()));
            let mic_level = Arc::new(AtomicU32::new(0.0f32.to_bits()));
            let mic_voiced = Arc::new(std::sync::atomic::AtomicU8::new(0));
            let pipeline = Pipeline::start(
                app.handle().clone(),
                Arc::clone(&db),
                Arc::clone(&hotkey),
                Arc::clone(&watcher_status),
                Arc::clone(&mic_level),
                Arc::clone(&mic_voiced),
            );
            app.manage(AppState {
                db,
                pipeline,
                hotkey,
                watcher_status,
                mic_level,
                mic_voiced,
            });

            // Menu-bar tray: status tooltip, start/stop, cancel, Hub access
            // and quit — a control surface that survives a hidden pill.
            if let Some(icon) = app.default_window_icon().cloned() {
                let toggle =
                    MenuItem::with_id(app, "toggle", "Start dictation", true, None::<&str>)?;
                let cancel =
                    MenuItem::with_id(app, "cancel", "Cancel dictation", true, None::<&str>)?;
                let hub = MenuItem::with_id(app, "hub", "Open Hub", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "Quit FlowClone", true, None::<&str>)?;
                let sep = PredefinedMenuItem::separator(app)?;
                let menu = Menu::with_items(
                    app,
                    &[
                        &toggle,
                        &cancel,
                        &sep,
                        &hub,
                        &PredefinedMenuItem::separator(app)?,
                        &quit,
                    ],
                )?;
                TrayIconBuilder::with_id("main-tray")
                    .icon(icon)
                    .tooltip("FlowClone — idle")
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "toggle" => {
                            app.state::<AppState>().pipeline.toggle();
                        }
                        "cancel" => {
                            app.state::<AppState>().pipeline.cancel();
                        }
                        "hub" => show_hub(app),
                        "quit" => app.exit(0),
                        _ => {}
                    })
                    .build(app)?;
                app.manage(TrayMenu { toggle });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            insert_transcript,
            list_transcripts,
            search_transcripts,
            delete_transcript,
            set_flagged,
            transcript_stats,
            add_dictionary_term,
            list_dictionary,
            set_dictionary_starred,
            delete_dictionary_term,
            list_vocab_suggestions,
            accept_vocab_suggestion,
            dismiss_vocab_suggestion,
            add_snippet,
            list_snippets,
            delete_snippet,
            upsert_style,
            list_styles,
            set_style_enabled,
            delete_style,
            resolve_style,
            get_setting,
            set_setting,
            pipeline_status,
            mic_level,
            hotkey_watcher_status,
            toggle_recording,
            cancel_recording,
            paste_text_at_cursor,
            retry_last,
            toggle_pause,
            list_mics,
            mic_device_status,
            set_mic_device,
            get_hotkey,
            hotkey_options,
            set_hotkey,
            autostart_status,
            autostart_set,
            check_mic_permission,
            set_flowbar_visible,
            flowbar_fit,
            set_flowbar_preset,
            check_for_update,
            install_update,
            accessibility_status,
            input_monitoring_status,
            hotkey_last_seen,
            open_accessibility_settings,
            open_input_monitoring_settings,
            local_model_status,
            local_parakeet_status,
            download_local_model,
            download_local_parakeet,
            set_local_model,
            local_llm_status,
            app_version,
            download_local_llm,
            set_local_llm
        ])
        .on_window_event(|window, event| {
            // Closing (or minimizing) the Hub parks FlowClone in the tray —
            // dictation, hotkeys and the flowbar stay alive in the
            // background. Reopen via the tray menu (or the Dock on macOS);
            // only the tray's Quit ends the app.
            if window.label() == "hub" {
                match event {
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    // No cancelable minimize event exists; catch the
                    // minimized state on resize and park it in the tray
                    // instead of leaving a taskbar/dock button behind.
                    tauri::WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => {
                        let _ = window.unminimize();
                        let _ = window.hide();
                    }
                    _ => {}
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            // Clicking the Dock icon on macOS reopens the Hub even when
            // every window is hidden in the menu bar / tray.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_hub(_app);
            }
        });
}
