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
    ("Right Ctrl", Keycode::RControl),
    ("Right Alt", Keycode::RAlt),
    ("Right Cmd", Keycode::Command),
];

pub fn parse_key(name: &str) -> Option<Keycode> {
    KEY_TABLE
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, k)| *k)
}

pub fn key_name(code: Keycode) -> Option<&'static str> {
    KEY_TABLE
        .iter()
        .find(|(_, k)| *k == code)
        .map(|(n, _)| *n)
}

#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    pub keys: Vec<Keycode>,
    pub tap_ms: u64,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            keys: vec![Keycode::F5],
            tap_ms: 250,
        }
    }
}

pub type SharedHotkeyConfig = Arc<RwLock<HotkeyConfig>>;

pub trait HotkeyWatcher: Send {
    fn spawn(self, tx: Sender<HotkeyEvent>) -> thread::JoinHandle<()>;
}

/// Push-to-talk watcher. Reads the shared config every tick so hotkey
/// changes apply without a restart. Fires Down when all configured keys are
/// held; on release distinguishes quick taps (TapUp) from real holds (Up).
#[derive(Debug, Clone)]
pub struct PushToTalkWatcher {
    pub config: SharedHotkeyConfig,
    pub poll_interval_ms: u64,
}

impl Default for PushToTalkWatcher {
    fn default() -> Self {
        Self {
            config: Arc::new(RwLock::new(HotkeyConfig::default())),
            poll_interval_ms: 20,
        }
    }
}

impl HotkeyWatcher for PushToTalkWatcher {
    fn spawn(self, tx: Sender<HotkeyEvent>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let device = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                DeviceState::new,
            )) {
                Ok(d) => d,
                Err(_) => {
                    eprintln!("hotkey watcher unavailable: missing accessibility permission");
                    return;
                }
            };
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
                        let event = if held_since.elapsed().as_millis() as u64
                            < self.config.read().unwrap().tap_ms
                        {
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
