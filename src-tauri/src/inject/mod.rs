#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{frontmost_app, is_accessibility_trusted, paste_text};

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{frontmost_app, is_accessibility_trusted, paste_text};

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
