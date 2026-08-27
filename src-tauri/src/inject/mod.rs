#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{
    focus_app, frontmost_app, is_accessibility_trusted, is_listen_event_trusted, paste_text,
    preceding_context, undo_paste,
};

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{
    focus_app, frontmost_app, is_accessibility_trusted, paste_text, preceding_context, undo_paste,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone)]
enum ClipboardOriginal {
    Text(String),
    Html {
        html: String,
        alt_text: Option<String>,
    },
    Image(arboard::ImageData<'static>),
    Files(Vec<std::path::PathBuf>),
    Empty,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct ClipboardRestore {
    generation: u64,
    original: ClipboardOriginal,
    staged: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
static CLIPBOARD_RESTORE: std::sync::Mutex<Option<ClipboardRestore>> = std::sync::Mutex::new(None);
#[cfg(any(target_os = "macos", target_os = "windows"))]
static CLIPBOARD_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Stages text while preserving the oldest supported clipboard value across
/// a burst of rapid dictations (plain/rich text, images, files, or empty).
/// The matching restore checks both generation and current contents, so it
/// never overwrites something the user copied in the meantime.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn stage_clipboard_text(text: &str) -> Result<u64, String> {
    let mut state = CLIPBOARD_RESTORE
        .lock()
        .map_err(|_| "clipboard restore lock poisoned".to_string())?;
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    let current_text = clipboard.get_text().ok();
    // Prefer richer formats over their plain-text alternatives. That keeps
    // copied files, screenshots, and formatted web content intact instead of
    // restoring only a filename or stripped text.
    let current = clipboard
        .get()
        .file_list()
        .ok()
        .filter(|files| !files.is_empty())
        .map(ClipboardOriginal::Files)
        .or_else(|| clipboard.get_image().ok().map(ClipboardOriginal::Image))
        .or_else(|| {
            clipboard
                .get()
                .html()
                .ok()
                .map(|html| ClipboardOriginal::Html {
                    html,
                    alt_text: current_text.clone(),
                })
        })
        .or_else(|| current_text.clone().map(ClipboardOriginal::Text))
        .unwrap_or(ClipboardOriginal::Empty);
    let generation = CLIPBOARD_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let original = match state.as_ref() {
        Some(previous)
            if matches!(
                &current,
                ClipboardOriginal::Text(text) if text == &previous.staged
            ) =>
        {
            previous.original.clone()
        }
        Some(_) | None => current,
    };
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("failed to stage clipboard: {e}"))?;
    *state = Some(ClipboardRestore {
        generation,
        original,
        staged: text.to_string(),
    });
    Ok(generation)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn restore_clipboard_later(generation: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        let Ok(mut state) = CLIPBOARD_RESTORE.lock() else {
            return;
        };
        let Some(snapshot) = state.as_ref() else {
            return;
        };
        if snapshot.generation != generation {
            return;
        }
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let staged_text_matches = clipboard
                .get_text()
                .ok()
                .as_deref()
                .is_some_and(|current| current == snapshot.staged);
            // A user may deliberately copy rich content whose plain-text
            // fallback happens to equal the staged dictation. Treat any rich
            // flavor as a clipboard change and leave it untouched.
            let has_rich_content = clipboard
                .get()
                .file_list()
                .ok()
                .is_some_and(|files| !files.is_empty())
                || clipboard.get_image().is_ok()
                || clipboard.get().html().is_ok();
            let unchanged = staged_text_matches && !has_rich_content;
            if unchanged {
                match snapshot.original.clone() {
                    ClipboardOriginal::Text(original) => {
                        let _ = clipboard.set_text(original);
                    }
                    ClipboardOriginal::Image(original) => {
                        let _ = clipboard.set_image(original);
                    }
                    ClipboardOriginal::Html { html, alt_text } => {
                        let _ = clipboard.set_html(html, alt_text);
                    }
                    ClipboardOriginal::Files(original) => {
                        let _ = clipboard.set().file_list(&original);
                    }
                    ClipboardOriginal::Empty => {
                        let _ = clipboard.clear();
                    }
                }
            }
        }
        *state = None;
    });
}

/// A failed keystroke leaves the staged text on the clipboard for manual
/// paste. Clear only our bookkeeping so a later dictation does not treat the
/// abandoned restore as part of the same clipboard burst.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keep_staged_clipboard(generation: u64) {
    if let Ok(mut state) = CLIPBOARD_RESTORE.lock() {
        if state
            .as_ref()
            .is_some_and(|snapshot| snapshot.generation == generation)
        {
            *state = None;
        }
    }
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
pub fn focus_app(_identifier: &str) -> Result<(), String> {
    Err("app focus is not implemented on this platform yet".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn preceding_context() -> String {
    String::new()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn undo_paste() -> Result<(), String> {
    Err("undo is not implemented on this platform yet".to_string())
}
