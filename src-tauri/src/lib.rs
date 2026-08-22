mod audio;
mod cloud;
mod commands;
mod hotkey;
mod inject;
mod pipeline;
mod store;

use std::fs;
use std::sync::{Arc, RwLock};

use serde_json::Value;
use tauri::Manager;
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};

use hotkey::{key_name, parse_key, HotkeyConfig, SharedHotkeyConfig};
use pipeline::Pipeline;
use store::Store;

pub struct AppState {
    db: Arc<Store>,
    pipeline: Pipeline,
    hotkey: SharedHotkeyConfig,
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Store) -> T) -> T {
    f(&state.db)
}

const FLOWBAR_SIZE: (f64, f64) = (300.0, 72.0);

fn create_flowbar(app: &tauri::AppHandle, db: &Store) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::{PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

    let saved: Option<(f64, f64)> = db
        .get_setting("flowBarPos")?
        .and_then(|v| serde_json::from_str(&v).ok());

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
        .visible(true)
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
fn get_hotkey(state: tauri::State<AppState>) -> Vec<String> {
    let cfg = state.hotkey.read().unwrap();
    cfg.keys
        .iter()
        .map(|k| key_name(*k).unwrap_or("Unknown").to_string())
        .collect()
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

fn load_hotkey_config(db: &Store) -> SharedHotkeyConfig {
    let mut config = HotkeyConfig::default();
    if let Some(names) = db
        .get_setting("hotkeyKeys")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
    {
        let keys: Vec<_> = names.iter().filter_map(|n| parse_key(n)).collect();
        if !keys.is_empty() {
            config.keys = keys;
        }
    }
    Arc::new(RwLock::new(config))
}

#[tauri::command]
fn accessibility_status() -> bool {
    inject::is_accessibility_trusted()
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir)?;
            let db = Arc::new(Store::open(&dir.join("flowclone.db"))?);
            let hotkey = load_hotkey_config(&db);
            create_flowbar(app.handle(), &db)?;
            let pipeline =
                Pipeline::start(app.handle().clone(), Arc::clone(&db), Arc::clone(&hotkey));
            app.manage(AppState {
                db,
                pipeline,
                hotkey,
            });
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
            toggle_recording,
            cancel_recording,
            get_hotkey,
            set_hotkey,
            autostart_status,
            autostart_set,
            accessibility_status,
            open_accessibility_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
