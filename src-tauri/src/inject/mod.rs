#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{is_accessibility_trusted, paste_text};

#[cfg(not(target_os = "macos"))]
pub fn is_accessibility_trusted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn paste_text(_text: &str) -> Result<(), String> {
    Err("text injection is not implemented on this platform yet".to_string())
}
