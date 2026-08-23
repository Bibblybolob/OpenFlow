#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{
    frontmost_app, is_accessibility_trusted, is_listen_event_trusted, paste_text,
    preceding_context, undo_paste,
};

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{frontmost_app, is_accessibility_trusted, paste_text, undo_paste};

/// Windows reads no caret context yet (UI Automation integration is future
/// work); cleanup simply runs without a continuation hint.
#[cfg(target_os = "windows")]
pub fn preceding_context() -> String {
    String::new()
}

/// Windows has no per-app gate on reading global key state.
#[cfg(target_os = "windows")]
pub fn is_listen_event_trusted() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn is_accessibility_trusted() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn paste_text(_text: &str) -> Result<(), String> {
    Err("text injection is not implemented on this platform yet".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn frontmost_app() -> String {
    String::new()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn preceding_context() -> String {
    String::new()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn undo_paste() -> Result<(), String> {
    Err("undo is not implemented on this platform yet".to_string())
}
