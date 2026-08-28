use std::thread;
use std::time::Duration;

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, TextPatternRangeEndpoint_Start,
    TextUnit_Character, UIA_TextPatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL,
};

const VK_Z: VIRTUAL_KEY = VIRTUAL_KEY(0x5A);
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

pub fn is_accessibility_trusted() -> bool {
    true
}

/// Pastes text at the cursor: stage on the clipboard, synthesize Ctrl+V via
/// SendInput, then restore the previous clipboard contents. Note: injection
/// cannot reach apps running elevated (as administrator) from a non-elevated
/// process.
pub fn paste_text(text: &str) -> Result<(), String> {
    let restore_generation = super::stage_clipboard_text(text)?;
    // Give Windows' clipboard owner a moment to publish the new data before
    // the target app handles Ctrl+V, matching the macOS injection path.
    thread::sleep(Duration::from_millis(40));
    let paste_result = send_ctrl_v();
    if paste_result.is_ok() {
        super::restore_clipboard_later(restore_generation);
    } else {
        super::keep_staged_clipboard(restore_generation);
    }
    paste_result
}

fn key_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_ctrl_v() -> Result<(), String> {
    const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
    let inputs = [
        key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(
            "keystroke failed — paste manually with Ctrl+V (text stays on your clipboard)"
                .to_string(),
        );
    }
    Ok(())
}

/// Best-effort removal of the last pasted text via synthesized Ctrl+Z.
pub fn undo_paste() -> Result<(), String> {
    let inputs = [
        key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_Z, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_Z, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err("keystroke failed — undo manually with Ctrl+Z".to_string());
    }
    Ok(())
}

/// Executable name (without extension) of the foreground window's process —
/// the Windows analogue of a macOS bundle identifier for style matching.
pub fn frontmost_app() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        process_name_for_window(hwnd).unwrap_or_default()
    }
}

fn process_name_for_window(hwnd: HWND) -> Option<String> {
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;

    Some(
        String::from_utf16_lossy(&buf[..len as usize])
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or("")
            .trim_end_matches(".exe")
            .to_lowercase(),
    )
}

struct WindowSearch {
    target: String,
    found: HWND,
}

unsafe extern "system" fn find_target_window(hwnd: HWND, param: LPARAM) -> BOOL {
    let search = unsafe { &mut *(param.0 as *mut WindowSearch) };
    if unsafe { IsWindowVisible(hwnd) }.as_bool()
        && process_name_for_window(hwnd).as_deref() == Some(search.target.as_str())
    {
        search.found = hwnd;
        return false.into();
    }
    true.into()
}

/// Focuses a visible top-level window owned by the recorded target process.
/// Because this runs directly from a user click in FlowClone's foreground
/// window, Windows permits the foreground transfer in normal applications.
pub fn focus_app(identifier: &str) -> Result<(), String> {
    let target = identifier.trim().trim_end_matches(".exe").to_lowercase();
    if target.is_empty() || target == "flowclone" {
        return Err("the original target app is unknown".to_string());
    }

    let mut search = WindowSearch {
        target: target.clone(),
        found: HWND::default(),
    };
    // EnumWindows reports a false return as an error when our callback stops
    // early. The populated HWND is the source of truth, so that result is
    // intentionally ignored.
    let _ = unsafe {
        EnumWindows(
            Some(find_target_window),
            LPARAM((&mut search as *mut WindowSearch) as isize),
        )
    };
    if search.found.is_invalid() {
        return Err("the original target app is no longer running".to_string());
    }
    if unsafe { IsIconic(search.found) }.as_bool() {
        let _ = unsafe { ShowWindow(search.found, SW_RESTORE) };
    }
    if !unsafe { SetForegroundWindow(search.found) }.as_bool() {
        return Err("the original target app did not accept focus".to_string());
    }
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(25));
        if frontmost_app() == target {
            return Ok(());
        }
    }
    Err("the original target app did not accept focus".to_string())
}

/// Reads up to 400 characters immediately before the caret from the focused
/// UI Automation text element. Many Windows text controls expose this through
/// TextPattern; unsupported controls simply return no context and cleanup
/// continues without a hint.
pub fn preceding_context() -> String {
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return String::new();
        }
        let _com = ComGuard;

        let automation: IUIAutomation =
            match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                Ok(value) => value,
                Err(_) => return String::new(),
            };
        let focused = match automation.GetFocusedElement() {
            Ok(value) => value,
            Err(_) => return String::new(),
        };
        let text_pattern: IUIAutomationTextPattern =
            match focused.GetCurrentPatternAs(UIA_TextPatternId) {
                Ok(value) => value,
                Err(_) => return String::new(),
            };
        let selection = match text_pattern.GetSelection() {
            Ok(value) => value,
            Err(_) => return String::new(),
        };
        if selection
            .Length()
            .ok()
            .filter(|length| *length > 0)
            .is_none()
        {
            return String::new();
        }
        let range = match selection.GetElement(0) {
            Ok(value) => value,
            Err(_) => return String::new(),
        };

        // A selection's start is the insertion point for a collapsed caret.
        // Moving only the start endpoint also gives sensible context when a
        // user has selected text and is about to replace it.
        if range
            .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -400)
            .is_err()
        {
            return String::new();
        }
        range
            .GetText(400)
            .map(|text| text.to_string().trim().to_string())
            .unwrap_or_default()
    }
}

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
