mod audio;
mod cloud;
mod commands;
mod hotkey;
mod inject;
mod learn;
mod pipeline;
mod sound;
mod store;

use std::fs;
use std::sync::{Arc, RwLock};

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
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Store) -> T) -> T {
    f(&state.db)
}

pub(crate) const FLOWBAR_SIZE: (f64, f64) = (300.0, 72.0);

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
        pipeline::PipelineState::Recording => "Stop dictation",
        _ => "Working…",
    };
    let _ = tray_menu.toggle.set_text(label);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(format!("FlowClone — {next}")));
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
) -> store::Result<store::Style> {
    with_db(&state, |db| {
        db.upsert_style(&app_pattern, &label, &instructions)
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
fn check_mic_permission() -> store::Result<bool> {
    let mut probe = audio::AudioEngine::new();
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
    with_db(&state, |db| {
        db.set_setting("sttLocalModel", &serde_json::json!(model))
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
    let screen = monitor.size();
    let bar_w = FLOWBAR_SIZE.0 * scale;
    let bar_h = FLOWBAR_SIZE.1 * scale;
    let margin = 24.0 * scale;

    let (x, y) = match preset.as_str() {
        "top_left" => (margin, margin),
        "top_center" => ((screen.width as f64 - bar_w) / 2.0, margin),
        "top_right" => (screen.width as f64 - bar_w - margin, margin),
        "bottom_left" => (margin, screen.height as f64 - bar_h - margin),
        "bottom_center" => (
            (screen.width as f64 - bar_w) / 2.0,
            screen.height as f64 - bar_h - margin,
        ),
        "bottom_right" => (
            screen.width as f64 - bar_w - margin,
            screen.height as f64 - bar_h - margin,
        ),
        other => {
            return Err(store::StoreError::Other(format!(
                "unknown position preset: {other}"
            )))
        }
    };
    let (x, y) = (x.max(0.0), y.max(0.0));
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|e| store::StoreError::Other(e.to_string()))?;
    with_db(&state, |db| {
        db.set_setting("flowBarPos", &serde_json::json!([x, y]))
    })
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
    Arc::new(RwLock::new(config))
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
    tauri::Builder::default()
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
            let pipeline = Pipeline::start(
                app.handle().clone(),
                Arc::clone(&db),
                Arc::clone(&hotkey),
                Arc::clone(&watcher_status),
            );
            app.manage(AppState {
                db,
                pipeline,
                hotkey,
                watcher_status,
            });
            // Pay the DNS+TCP+TLS handshake for the transcription API now,
            // in the background, so the first dictation doesn't. The pooled
            // client keeps the connection warm afterwards.
            std::thread::spawn(move || {
                if let Ok(client) = cloud::http_client() {
                    let _ = client
                        .get("https://api.openai.com/v1/models")
                        .timeout(std::time::Duration::from_secs(10))
                        .send();
                }
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
                        "hub" => {
                            if let Some(window) = app.get_webview_window("hub") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
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
            hotkey_watcher_status,
            toggle_recording,
            cancel_recording,
            toggle_pause,
            list_mics,
            set_mic_device,
            get_hotkey,
            hotkey_options,
            set_hotkey,
            autostart_status,
            autostart_set,
            check_mic_permission,
            set_flowbar_visible,
            set_flowbar_preset,
            check_for_update,
            install_update,
            accessibility_status,
            input_monitoring_status,
            open_accessibility_settings,
            open_input_monitoring_settings,
            local_model_status,
            download_local_model,
            set_local_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
