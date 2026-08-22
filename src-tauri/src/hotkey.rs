use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use device_query::{DeviceQuery, DeviceState, Keycode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Down,
    Up,
}

pub trait HotkeyWatcher: Send {
    fn spawn(self, tx: Sender<HotkeyEvent>) -> thread::JoinHandle<()>;
}

/// Push-to-talk: fires Down when every configured key is held,
/// Up as soon as any of them is released.
#[derive(Debug, Clone)]
pub struct PushToTalkWatcher {
    pub keys: Vec<Keycode>,
    pub poll_interval_ms: u64,
}

impl Default for PushToTalkWatcher {
    fn default() -> Self {
        Self {
            keys: vec![Keycode::F5],
            poll_interval_ms: 20,
        }
    }
}

impl HotkeyWatcher for PushToTalkWatcher {
    fn spawn(self, tx: Sender<HotkeyEvent>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            // device_query panics without Accessibility permission on macOS;
            // degrade to "no dictation" instead of crashing the thread loudly.
            let device =
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(DeviceState::new)) {
                    Ok(d) => d,
                    Err(_) => {
                        eprintln!("hotkey watcher unavailable: missing accessibility permission");
                        return;
                    }
                };
            let mut held = false;
            loop {
                let pressed = device.get_keys();
                let down = self.keys.iter().all(|k| pressed.contains(k));
                match (down, held) {
                    (true, false) => {
                        if tx.send(HotkeyEvent::Down).is_err() {
                            return;
                        }
                        held = true;
                    }
                    (false, true) => {
                        if tx.send(HotkeyEvent::Up).is_err() {
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
