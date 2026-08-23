use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use device_query::{DeviceQuery, DeviceState, Keycode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Down,
    Up,
    /// Released after a hold shorter than the configured tap threshold.
    TapUp,
}

/// Supported hotkey keys with stable string names for persistence.
/// Only lists keys the running platform's keyboard backend can actually
/// detect: macOS has no Right Ctrl (and its right Alt is reported as
/// Option, with both Cmd keys collapsing into one `Command` code), while
/// Windows/Linux distinguish Right Ctrl, Right Alt, and Right Win.
pub const KEY_TABLE: &[(&str, Keycode)] = &[
    ("F1", Keycode::F1),
    ("F2", Keycode::F2),
    ("F3", Keycode::F3),
    ("F4", Keycode::F4),
    ("F5", Keycode::F5),
    ("F6", Keycode::F6),
    ("F7", Keycode::F7),
    ("F8", Keycode::F8),
    ("F9", Keycode::F9),
    ("F10", Keycode::F10),
    ("F11", Keycode::F11),
    ("F12", Keycode::F12),
    ("CapsLock", Keycode::CapsLock),
    ("Right Shift", Keycode::RShift),
    #[cfg(target_os = "macos")]
    ("Right Option", Keycode::ROption),
    #[cfg(target_os = "macos")]
    ("Cmd", Keycode::Command),
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    ("Right Ctrl", Keycode::RControl),
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    ("Right Alt", Keycode::RAlt),
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    ("Right Win", Keycode::RMeta),
];

/// Names of the hotkey keys offered on this platform, in menu order.
pub fn key_options() -> Vec<String> {
    KEY_TABLE.iter().map(|(n, _)| (*n).to_string()).collect()
}

pub fn parse_key(name: &str) -> Option<Keycode> {
    KEY_TABLE
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, k)| *k)
}

pub fn key_name(code: Keycode) -> Option<&'static str> {
    KEY_TABLE.iter().find(|(_, k)| *k == code).map(|(n, _)| *n)
}

#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    pub keys: Vec<Keycode>,
    pub tap_ms: u64,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        // Right Shift sits under both palms on every keyboard, never types a
        // character when tapped alone, and — unlike F5 on recent MacBooks —
        // is never intercepted by a macOS system feature.
        Self {
            keys: vec![Keycode::RShift],
            tap_ms: 250,
        }
    }
}

pub type SharedHotkeyConfig = Arc<RwLock<HotkeyConfig>>;

/// Lifecycle of the keyboard backend, reported to the UI so a dead hotkey
/// can never be silent again.
#[derive(Debug, Clone)]
pub enum WatcherStatus {
    /// Keyboard backend opened; keystrokes are being watched.
    Ready,
    /// Backend creation failed (typically permissions revoked mid-flight).
    /// The payload explains why; the watcher keeps retrying.
    Unavailable(String),
}

pub type StatusCallback = Arc<dyn Fn(WatcherStatus) + Send + Sync>;

pub trait HotkeyWatcher: Send {
    fn spawn(self, tx: Sender<HotkeyEvent>) -> thread::JoinHandle<()>;
}

/// Push-to-talk watcher. Reads the shared config every tick so hotkey
/// changes apply without a restart. Fires Down when all configured keys are
/// held; on release distinguishes quick taps (TapUp) from real holds (Up).
///
/// If the keyboard backend cannot be created (macOS revokes Input Monitoring
/// whenever the app bundle is replaced), the watcher reports
/// [`WatcherStatus::Unavailable`] and retries every few seconds instead of
/// dying permanently — granting the permission back is enough to recover
/// without a relaunch.
#[derive(Clone)]
pub struct PushToTalkWatcher {
    pub config: SharedHotkeyConfig,
    pub poll_interval_ms: u64,
    pub on_status: Option<StatusCallback>,
}

impl std::fmt::Debug for PushToTalkWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushToTalkWatcher")
            .field("poll_interval_ms", &self.poll_interval_ms)
            .field("on_status", &self.on_status.is_some())
            .finish()
    }
}

impl Default for PushToTalkWatcher {
    fn default() -> Self {
        Self {
            config: Arc::new(RwLock::new(HotkeyConfig::default())),
            poll_interval_ms: 20,
            on_status: None,
        }
    }
}

const WATCHER_RETRY_MS: u64 = 3000;

impl HotkeyWatcher for PushToTalkWatcher {
    fn spawn(self, tx: Sender<HotkeyEvent>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let device = loop {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(DeviceState::new)) {
                    Ok(d) => break d,
                    Err(_) => {
                        let reason =
                            "keyboard backend unavailable — re-grant Input Monitoring".to_string();
                        eprintln!("hotkey watcher unavailable: {reason}; retrying in {WATCHER_RETRY_MS}ms");
                        if let Some(cb) = &self.on_status {
                            cb(WatcherStatus::Unavailable(reason));
                        }
                        thread::sleep(Duration::from_millis(WATCHER_RETRY_MS));
                    }
                };
            };
            if let Some(cb) = &self.on_status {
                cb(WatcherStatus::Ready);
            }
            let mut held = false;
            let mut held_since = Instant::now();
            loop {
                let keys = self.config.read().unwrap().keys.clone();
                let pressed = device.get_keys();
                let down = !keys.is_empty() && keys.iter().all(|k| pressed.contains(k));
                match (down, held) {
                    (true, false) => {
                        if tx.send(HotkeyEvent::Down).is_err() {
                            return;
                        }
                        held = true;
                        held_since = Instant::now();
                    }
                    (false, true) => {
                        let hold_ms = held_since.elapsed().as_millis() as u64;
                        let tap_threshold = self.config.read().unwrap().tap_ms;
                        let event = if hold_ms < tap_threshold {
                            HotkeyEvent::TapUp
                        } else {
                            HotkeyEvent::Up
                        };
                        if tx.send(event).is_err() {
                            return;
                        }
                        held = false;
                    }
                    _ => {}
                }
                thread::sleep(Duration::from_millis(self.poll_interval_ms));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_table_roundtrip() {
        for (name, code) in KEY_TABLE {
            assert_eq!(parse_key(name), Some(*code));
            assert_eq!(key_name(*code), Some(*name));
        }
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(parse_key("Nonsense"), None);
        assert_eq!(parse_key(""), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_table_matches_hardware() {
        // macOS exposes no right-hand Ctrl (device-query reports only one
        // Control code), labels the right Alt as Option, and reports both
        // Cmd keys through a single Command code.
        assert!(parse_key("Right Ctrl").is_none());
        assert!(parse_key("Right Alt").is_none());
        assert_eq!(parse_key("Right Option"), Some(Keycode::ROption));
        assert_eq!(parse_key("Cmd"), Some(Keycode::Command));
        let names: Vec<_> = KEY_TABLE.iter().map(|(n, _)| *n).collect();
        assert!(!names.contains(&"Right Cmd/Win"));
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn pc_table_matches_hardware() {
        assert_eq!(parse_key("Right Ctrl"), Some(Keycode::RControl));
        assert_eq!(parse_key("Right Alt"), Some(Keycode::RAlt));
        assert_eq!(parse_key("Right Win"), Some(Keycode::RMeta));
    }
}
