use std::process::Command;
use std::thread;
use std::time::Duration;

use arboard::Clipboard;

extern "C" {
    fn AXIsProcessTrusted() -> i32;
}

pub fn is_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Bundle identifier (e.g. "com.apple.Mail") of the frontmost application,
/// used for per-app style resolution. Best-effort: empty string on failure.
pub fn frontmost_app() -> String {
    let script = r#"
        tell application "System Events"
            set frontApp to first process whose frontmost is true
            return bundle identifier of frontApp
        end tell
    "#;
    Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Pastes text at the current cursor location by placing it on the clipboard,
/// synthesizing Cmd+V via System Events, then restoring the previous
/// clipboard contents once the target app has read it.
pub fn paste_text(text: &str) -> Result<(), String> {
    if !is_accessibility_trusted() {
        return Err(
            "FlowClone needs Accessibility permission to type for you — grant it in System Settings → Privacy & Security → Accessibility"
                .to_string(),
        );
    }

    let mut clipboard = Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    let previous = clipboard.get_text().ok();

    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("failed to stage clipboard: {e}"))?;

    let script = r#"tell application "System Events" to keystroke "v" using command down"#;
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("failed to run osascript: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "keystroke failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    // Restore the user's clipboard after the pasted app has consumed it.
    if let Some(previous) = previous {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(800));
            if let Ok(mut cb) = Clipboard::new() {
                let _ = cb.set_text(previous);
            }
        });
    }
    Ok(())
}
