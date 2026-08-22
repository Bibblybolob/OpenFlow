mod store;

use std::fs;
use std::sync::Mutex;

use serde_json::Value;
use tauri::Manager;

use store::{DictionaryEntry, Result, Snippet, Stats, Store, Style, Transcript};

pub struct AppState {
    db: Mutex<Store>,
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
    let guard = state.db.lock().unwrap();
    f(&guard)
}

#[tauri::command]
fn insert_transcript(
    state: tauri::State<AppState>,
    text: String,
    raw_text: String,
    language: String,
    duration_ms: i64,
    target_app: String,
) -> Result<Transcript> {
    with_db(&state, |db| {
        db.insert_transcript(&text, &raw_text, &language, duration_ms, &target_app)
    })
}

#[tauri::command]
fn list_transcripts(state: tauri::State<AppState>, limit: i64, offset: i64) -> Result<Vec<Transcript>> {
    with_db(&state, |db| db.list_transcripts(limit, offset))
}

#[tauri::command]
fn search_transcripts(state: tauri::State<AppState>, query: String) -> Result<Vec<Transcript>> {
    with_db(&state, |db| db.search_transcripts(&query))
}

#[tauri::command]
fn delete_transcript(state: tauri::State<AppState>, id: i64) -> Result<()> {
    with_db(&state, |db| db.delete_transcript(id))
}

#[tauri::command]
fn set_flagged(state: tauri::State<AppState>, id: i64, flagged: bool) -> Result<()> {
    with_db(&state, |db| db.set_flagged(id, flagged))
}

#[tauri::command]
fn transcript_stats(state: tauri::State<AppState>) -> Result<Stats> {
    with_db(&state, |db| db.stats())
}

#[tauri::command]
fn add_dictionary_term(
    state: tauri::State<AppState>,
    term: String,
    replacement: Option<String>,
) -> Result<DictionaryEntry> {
    with_db(&state, |db| db.add_dictionary_term(&term, replacement.as_deref()))
}

#[tauri::command]
fn list_dictionary(state: tauri::State<AppState>) -> Result<Vec<DictionaryEntry>> {
    with_db(&state, |db| db.list_dictionary())
}

#[tauri::command]
fn set_dictionary_starred(state: tauri::State<AppState>, id: i64, starred: bool) -> Result<()> {
    with_db(&state, |db| db.set_dictionary_starred(id, starred))
}

#[tauri::command]
fn delete_dictionary_term(state: tauri::State<AppState>, id: i64) -> Result<()> {
    with_db(&state, |db| db.delete_dictionary_term(id))
}

#[tauri::command]
fn add_snippet(state: tauri::State<AppState>, trigger: String, body: String) -> Result<Snippet> {
    with_db(&state, |db| db.add_snippet(&trigger, &body))
}

#[tauri::command]
fn list_snippets(state: tauri::State<AppState>) -> Result<Vec<Snippet>> {
    with_db(&state, |db| db.list_snippets())
}

#[tauri::command]
fn delete_snippet(state: tauri::State<AppState>, id: i64) -> Result<()> {
    with_db(&state, |db| db.delete_snippet(id))
}

#[tauri::command]
fn upsert_style(
    state: tauri::State<AppState>,
    app_pattern: String,
    label: String,
    instructions: String,
) -> Result<Style> {
    with_db(&state, |db| db.upsert_style(&app_pattern, &label, &instructions))
}

#[tauri::command]
fn list_styles(state: tauri::State<AppState>) -> Result<Vec<Style>> {
    with_db(&state, |db| db.list_styles())
}

#[tauri::command]
fn set_style_enabled(state: tauri::State<AppState>, id: i64, enabled: bool) -> Result<()> {
    with_db(&state, |db| db.set_style_enabled(id, enabled))
}

#[tauri::command]
fn delete_style(state: tauri::State<AppState>, id: i64) -> Result<()> {
    with_db(&state, |db| db.delete_style(id))
}

#[tauri::command]
fn resolve_style(state: tauri::State<AppState>, app_identifier: String) -> Result<Option<String>> {
    with_db(&state, |db| db.resolve_style_for_app(&app_identifier))
}

#[tauri::command]
fn get_setting(state: tauri::State<AppState>, key: String) -> Result<Option<Value>> {
    let raw = with_db(&state, |db| db.get_setting(&key))?;
    raw.map(|s| serde_json::from_str(&s).map_err(store::StoreError::Json))
        .transpose()
}

#[tauri::command]
fn set_setting(state: tauri::State<AppState>, key: String, value: Value) -> Result<()> {
    with_db(&state, |db| db.set_setting(&key, &value))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir)?;
            let db = Store::open(&dir.join("flowclone.db"))?;
            app.manage(AppState { db: Mutex::new(db) });
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
            set_setting
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
