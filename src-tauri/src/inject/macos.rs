use std::process::Command;
use std::thread;
use std::time::Duration;

use arboard::Clipboard;

extern "C" {
    fn AXIsProcessTrusted() -> i32;

    // IOKit HID permission check. Request types: 0 = post events,
    // 1 = listen to events (Input Monitoring). Access results:
    // 0 = denied, 1 = unknown/not-determined, 2 = granted.
    #[link_name = "IOHIDCheckAccess"]
    fn io_hid_check_access(request_type: i64) -> i64;

    // CoreGraphics event synthesis — posts Cmd+V without spawning a process.
    // CGEventCreateKeyboardEvent(allocator, virtual_keycode, key_down) -> CGEventRef
    fn CGEventCreateKeyboardEvent(
        allocator: *mut std::ffi::c_void,
        key_code: u16,
        key_down: bool,
    ) -> *mut std::ffi::c_void;
    fn CGEventSetFlags(event: *mut std::ffi::c_void, flags: u64);
    fn CGEventPost(tap_location: u32, event: *const std::ffi::c_void);
    fn CFRelease(cf: *mut std::ffi::c_void);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {}

/// kCGSessionEventTap — routes synthesized keys into the login session.
const K_CG_SESSION_EVENT_TAP: u32 = 1;
/// kCGEventFlagMaskCommand
const K_CG_EVENT_FLAG_COMMAND: u64 = 1 << 20;
/// kVK_ANSI_V
const KEY_V: u16 = 0x09;
/// kVK_ANSI_Z — undo, for "scratch that".
const KEY_Z: u16 = 0x06;

const K_IO_HID_REQUEST_TYPE_LISTEN_EVENT: i64 = 1;
const K_IO_HID_ACCESS_GRANTED: i64 = 2;
const K_IO_HID_ACCESS_UNKNOWN: i64 = 1;

pub fn is_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// True when the app may observe keyboard events from any device — the
/// permission behind global hotkey detection (System Settings → Privacy &
/// Security → Input Monitoring). Distinct from Accessibility, which covers
/// synthesizing keystrokes for paste injection.
pub fn is_listen_event_trusted() -> bool {
    let result = unsafe { io_hid_check_access(K_IO_HID_REQUEST_TYPE_LISTEN_EVENT) };
    result == K_IO_HID_ACCESS_GRANTED || result == K_IO_HID_ACCESS_UNKNOWN
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

/// Reads up to ~400 characters immediately before the caret in the focused
/// element, so cleanup can make dictation continue the surrounding sentence
/// coherently. Best-effort: empty string whenever the frontmost app doesn't
/// expose the Accessibility value (most non-text apps, browsers without
/// AX enabled), or on any AppleScript hiccup.
pub fn preceding_context() -> String {
    let script = r#"
        tell application "System Events"
            tell (first process whose frontmost is true)
                set fe to focused element
                set docText to value of fe
                set selRange to value of attribute "AXSelectedTextRange" of fe
            end tell
        end tell
        set caretLoc to (first item of selRange) as integer
        if caretLoc is 0 or length of docText is 0 then return ""
        set maxStart to caretLoc - 400
        if maxStart < 1 then set maxStart to 1
        return text maxStart thru (caretLoc - 1) of docText
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
/// synthesizing Cmd+V, then restoring the previous clipboard contents once
/// the target app has read it.
///
/// The keystroke is synthesized natively via CGEvent (no process spawn, no
/// AppleScript compile — saves ~150-300ms versus osascript), falling back to
/// System Events if event creation fails.
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

    // Give the pasteboard a beat to settle before the target app reads it.
    thread::sleep(Duration::from_millis(40));

    if !post_cmd_v() {
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

/// Posts Cmd+V key-down/key-up via CoreGraphics. Returns false when event
/// creation fails so the caller can fall back to osascript.
fn post_cmd_v() -> bool {
    post_cmd_key(KEY_V)
}

/// Synthesizes Cmd+Z in the login session — best-effort "scratch that" that
/// removes the last paste in apps with a working undo stack. Returns false
/// when event synthesis fails.
fn post_cmd_z() -> bool {
    post_cmd_key(KEY_Z)
}

fn post_cmd_key(key: u16) -> bool {
    unsafe {
        let down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, true);
        if down.is_null() {
            return false;
        }
        CGEventSetFlags(down, K_CG_EVENT_FLAG_COMMAND);
        CGEventPost(K_CG_SESSION_EVENT_TAP, down);
        CFRelease(down);

        thread::sleep(Duration::from_millis(10));

        let up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, false);
        if up.is_null() {
            return false;
        }
        CGEventSetFlags(up, K_CG_EVENT_FLAG_COMMAND);
        CGEventPost(K_CG_SESSION_EVENT_TAP, up);
        CFRelease(up);
    }
    true
}

/// Best-effort removal of the last pasted text via synthesized undo.
pub fn undo_paste() -> Result<(), String> {
    if !is_accessibility_trusted() {
        return Err(
            "FlowClone needs Accessibility permission to undo — grant it in System Settings → Privacy & Security → Accessibility"
                .to_string(),
        );
    }
    if !post_cmd_z() {
        return Err("could not synthesize Cmd+Z".to_string());
    }
    Ok(())
}
