//! Native macOS hotkey backend: a listen-only CGEventTap driven by its own
//! CFRunLoop. Replaces device_query on macOS — its polling stack asserted
//! permissions opaquely and failed invisibly (watcher "ready" while keys
//! went unseen). The tap reports exactly why it cannot start, and every
//! keystroke it observes is recorded for the Settings diagnostics panel.
#![allow(clippy::upper_case_acronyms)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use device_query::Keycode;

use crate::hotkey::{HotkeyEvent, SharedHotkeyConfig};

pub type TapCallback = Arc<dyn Fn(&str, Option<String>) + Send + Sync>;

// --- Virtual keycodes (Events.h kVK_*) for keys we support ---
pub const VK_ESCAPE: u64 = 53;
pub const VK_CAPSLOCK: u64 = 57;
const VK_RSHIFT: u64 = 60;
const VK_ROPTION: u64 = 61;
const VK_RCMD: u64 = 54;
const VK_LCMD: u64 = 55;

/// device_query keycode -> macOS virtual keycode, covering everything the
/// platform's key picker offers.
pub fn vk_of(key: Keycode) -> Option<u64> {
    match key {
        Keycode::F1 => Some(122),
        Keycode::F2 => Some(120),
        Keycode::F3 => Some(99),
        Keycode::F4 => Some(118),
        Keycode::F5 => Some(96),
        Keycode::F6 => Some(97),
        Keycode::F7 => Some(98),
        Keycode::F8 => Some(100),
        Keycode::F9 => Some(101),
        Keycode::F10 => Some(109),
        Keycode::F11 => Some(103),
        Keycode::F12 => Some(111),
        Keycode::CapsLock => Some(VK_CAPSLOCK),
        Keycode::RShift => Some(VK_RSHIFT),
        Keycode::ROption => Some(VK_ROPTION),
        Keycode::Command => Some(VK_RCMD), // both Cmd keys share one logical key
        _ => None,
    }
}

/// Friendly names for the diagnostics trail: pickable keys plus common
/// typing keys, so "what does the backend see" reads naturally.
pub fn vk_name(vk: u64) -> String {
    let named: &[(&[u64], &str)] = &[
        (&[VK_ESCAPE], "Esc"),
        (&[VK_CAPSLOCK], "CapsLock"),
        (&[VK_RSHIFT], "Right Shift"),
        (&[59], "Left Shift"),
        (&[VK_ROPTION], "Right Option"),
        (&[58], "Left Option"),
        (&[VK_RCMD, VK_LCMD], "Cmd"),
        (&[49], "Space"),
        (&[36], "Return"),
        (&[48], "Tab"),
        (
            &[122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111],
            "Fn-key",
        ),
    ];
    for (codes, name) in named {
        if codes.contains(&vk) {
            return (*name).to_string();
        }
    }
    let letters: HashMap<u64, char> = [
        (0u64, 'a'),
        (11, 'b'),
        (8, 'c'),
        (2, 'd'),
        (14, 'e'),
        (3, 'f'),
        (5, 'g'),
        (4, 'h'),
        (34, 'i'),
        (38, 'j'),
        (40, 'k'),
        (37, 'l'),
        (46, 'm'),
        (45, 'n'),
        (31, 'o'),
        (35, 'p'),
        (12, 'q'),
        (15, 'r'),
        (1, 's'),
        (17, 't'),
        (32, 'u'),
        (9, 'v'),
        (13, 'w'),
        (7, 'x'),
        (16, 'y'),
        (6, 'z'),
    ]
    .into_iter()
    .collect();
    if let Some(c) = letters.get(&vk) {
        return c.to_ascii_uppercase().to_string();
    }
    format!("key {vk}")
}

// --- Pure state machine (unit-testable without any FFI) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorEvent {
    Down,
    /// Released after a hold shorter than the tap threshold.
    TapUp(u64),
    Up(u64),
    Escape,
}

#[derive(Debug)]
pub struct KeyMonitor {
    pub config_vks: Vec<u64>,
    pub tap_ms: u64,
    pressed: HashSet<u64>,
    caps_prev_alpha: bool,
    down_at: Option<u64>,
    down_active: bool,
}

impl KeyMonitor {
    pub fn new(config_vks: Vec<u64>, tap_ms: u64) -> Self {
        Self {
            config_vks,
            tap_ms,
            pressed: HashSet::new(),
            caps_prev_alpha: false,
            down_at: None,
            down_active: false,
        }
    }

    /// Feeds one raw system event; returns hotkey transitions it caused.
    /// `etype`: 10=keyDown, 11=keyUp, 12=flagsChanged.
    pub fn observe(&mut self, etype: u32, vk: u64, flags: u64, now_ms: u64) -> Vec<MonitorEvent> {
        let mut out = Vec::new();
        match etype {
            10 => {
                if vk == VK_ESCAPE && self.pressed.insert(vk) {
                    out.push(MonitorEvent::Escape);
                }
                if self.pressed.insert(vk) {
                    self.reevaluate(now_ms, &mut out);
                }
            }
            11 => {
                if self.pressed.remove(&vk) {
                    self.reevaluate(now_ms, &mut out);
                }
            }
            12 => {
                if vk == VK_CAPSLOCK {
                    let alpha = (flags >> 16) & 1 == 1;
                    if alpha != self.caps_prev_alpha {
                        self.caps_prev_alpha = alpha;
                        let changed = if alpha {
                            self.pressed.insert(vk)
                        } else {
                            self.pressed.remove(&vk)
                        };
                        if changed {
                            self.reevaluate(now_ms, &mut out);
                        }
                    }
                } else if let Some(is_down) = modifier_pressed(vk, flags) {
                    let changed = if is_down {
                        self.pressed.insert(vk)
                    } else {
                        self.pressed.remove(&vk)
                    };
                    if changed {
                        self.reevaluate(now_ms, &mut out);
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn reevaluate(&mut self, now_ms: u64, out: &mut Vec<MonitorEvent>) {
        let all_down =
            !self.config_vks.is_empty() && self.config_vks.iter().all(|k| self.pressed.contains(k));
        match (all_down, self.down_active) {
            (true, false) => {
                self.down_active = true;
                self.down_at = Some(now_ms);
                out.push(MonitorEvent::Down);
            }
            (false, true) => {
                self.down_active = false;
                let hold = now_ms - self.down_at.take().unwrap_or(now_ms);
                out.push(if hold < self.tap_ms {
                    MonitorEvent::TapUp(hold)
                } else {
                    MonitorEvent::Up(hold)
                });
            }
            _ => {}
        }
    }
}

/// Side disambiguation for modifier flagsChanged events: the low 16 flag
/// bits carry device-specific masks (Carbon Events.h); CapsLock is handled
/// separately via the alpha-shift bit.
fn modifier_pressed(vk: u64, flags: u64) -> Option<bool> {
    let device = flags & 0xFFFF;
    match vk {
        VK_RSHIFT => Some(device & 0x04 != 0),
        59 => Some(device & 0x02 != 0),
        VK_ROPTION => Some(device & 0x40 != 0),
        58 => Some(device & 0x20 != 0),
        VK_RCMD | VK_LCMD => Some(device & 0x18 != 0),
        _ => None,
    }
}

// --- Diagnostics ring buffer ---
// Stays in process memory only; rendered on demand in Settings.

#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyEventRecord {
    pub name: String,
    pub down: bool,
    pub ago_ms: u64,
}

static EVENT_LOG: OnceLock<Mutex<VecDeque<(u64, String, bool)>>> = OnceLock::new();

fn event_log() -> &'static Mutex<VecDeque<(u64, String, bool)>> {
    EVENT_LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(8)))
}

fn record_event(vk: u64, down: bool) {
    let mut log = event_log().lock().unwrap();
    log.push_back((unix_ms(), vk_name(vk), down));
    while log.len() > 8 {
        log.pop_front();
    }
}

/// Recent keystrokes the backend saw (oldest first), for Settings.
pub fn recent_events() -> Vec<KeyEventRecord> {
    let now = unix_ms();
    event_log()
        .lock()
        .unwrap()
        .iter()
        .map(|(t, name, down)| KeyEventRecord {
            name: name.clone(),
            down: *down,
            ago_ms: now.saturating_sub(*t),
        })
        .collect()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// --- FFI ---

type CGEventRef = *mut std::ffi::c_void;
type CFMachPortRef = *mut std::ffi::c_void;
type CFRunLoopSourceRef = *mut std::ffi::c_void;
type CFRunLoopRef = *mut std::ffi::c_void;
type CGEventTapCallBack = unsafe extern "C" fn(
    *mut std::ffi::c_void,
    u32,
    CGEventRef,
    *mut std::ffi::c_void,
) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        eventsOfInterest: u64,
        callback: CGEventTapCallBack,
        userInfo: *mut std::ffi::c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: i64) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *mut std::ffi::c_void,
        port: CFMachPortRef,
        order: i64,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopAddSource(
        rl: CFRunLoopRef,
        source: CFRunLoopSourceRef,
        mode: *const std::ffi::c_void,
    );
    fn CFRunLoopRun();
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFDictionaryCreate(
        allocator: *mut std::ffi::c_void,
        keys: *const *const std::ffi::c_void,
        values: *const *const std::ffi::c_void,
        numKeys: isize,
        keyCallbacks: *const std::ffi::c_void,
        valueCallbacks: *const std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    static kCFBooleanTrue: *const std::ffi::c_void;
    static kCFRunLoopDefaultMode: *const std::ffi::c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// Returns CoreFoundation `Boolean` (u8).
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> u8;
    static kAXTrustedCheckOptionPrompt: *const std::ffi::c_void;
}

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 1;
const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;
const MASK_KEY_DOWN: u64 = 1 << 10;
const MASK_KEY_UP: u64 = 1 << 11;
const MASK_FLAGS_CHANGED: u64 = 1 << 12;
const FIELD_KEYCODE: i64 = 9;
const ETAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const ETAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

static ACTIVE_TAP: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn tap_callback(
    _proxy: *mut std::ffi::c_void,
    etype: u32,
    event: CGEventRef,
    user_info: *mut std::ffi::c_void,
) -> CGEventRef {
    if etype == ETAP_DISABLED_BY_TIMEOUT || etype == ETAP_DISABLED_BY_USER_INPUT {
        let tap = ACTIVE_TAP.load(Ordering::Relaxed) as CFMachPortRef;
        if !tap.is_null() {
            CGEventTapEnable(tap, true);
        }
        return std::ptr::null_mut();
    }
    let inner = &*(user_info as *const TapInner);
    let vk = CGEventGetIntegerValueField(event, FIELD_KEYCODE) as u64;
    let flags = CGEventGetFlags(event);
    record_event(vk, etype != 11);
    let events = inner
        .monitor
        .lock()
        .unwrap()
        .observe(etype, vk, flags, unix_ms());
    for ev in events {
        let mapped = match ev {
            MonitorEvent::Down => {
                crate::hotkey::set_held(true);
                HotkeyEvent::Down
            }
            MonitorEvent::TapUp(_) => {
                crate::hotkey::set_held(false);
                HotkeyEvent::TapUp
            }
            MonitorEvent::Up(_) => {
                crate::hotkey::set_held(false);
                HotkeyEvent::Up
            }
            MonitorEvent::Escape => HotkeyEvent::EscapePress,
        };
        if inner.tx.send(mapped).is_err() {
            break;
        }
    }
    // Listen-only tap: pass the event through untouched.
    event
}

struct TapInner {
    monitor: Mutex<KeyMonitor>,
    tx: std::sync::mpsc::Sender<HotkeyEvent>,
}

/// Brings up the tap — reporting lifecycle through `status` ("ready",
/// "waiting-accessibility", "waiting-input-monitoring") — and runs the
/// receive loop forever. Blocks its calling thread; designed to be spawned.
pub fn run(
    config: SharedHotkeyConfig,
    tx: std::sync::mpsc::Sender<HotkeyEvent>,
    status: TapCallback,
) {
    loop {
        if !crate::inject::is_accessibility_trusted() {
            status("waiting-accessibility", None);
            prompt_for_accessibility();
            while !crate::inject::is_accessibility_trusted() {
                std::thread::sleep(Duration::from_secs(2));
            }
        }

        let initial = {
            let cfg = config.read().unwrap();
            let vks: Vec<u64> = cfg.keys.iter().filter_map(|k| vk_of(*k)).collect();
            Arc::new(TapInner {
                monitor: Mutex::new(KeyMonitor::new(vks, cfg.tap_ms)),
                tx: tx.clone(),
            })
        };

        let tap = unsafe {
            CGEventTapCreate(
                K_CG_SESSION_EVENT_TAP,
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_LISTEN_ONLY,
                MASK_KEY_DOWN | MASK_KEY_UP | MASK_FLAGS_CHANGED,
                tap_callback,
                Arc::as_ptr(&initial) as *mut std::ffi::c_void,
            )
        };
        if tap.is_null() {
            eprintln!("hotkey tap: creation failed — Input Monitoring missing?");
            status("waiting-input-monitoring", None);
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        ACTIVE_TAP.store(tap as usize, Ordering::Relaxed);

        let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0) };
        if source.is_null() {
            status("unavailable:could not create run-loop source", None);
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        unsafe { CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode) };

        // Apply hotkey changes live: a side thread mirrors the shared config
        // into the monitor the tap callback reads.
        {
            let inner = Arc::clone(&initial);
            let cfg = Arc::clone(&config);
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(500));
                let (vks, tap_ms) = {
                    let c = cfg.read().unwrap();
                    (
                        c.keys
                            .iter()
                            .filter_map(|k| vk_of(*k))
                            .collect::<Vec<u64>>(),
                        c.tap_ms,
                    )
                };
                let mut m = inner.monitor.lock().unwrap();
                if m.config_vks != vks {
                    m.config_vks = vks;
                }
                m.tap_ms = tap_ms;
            });
        }

        eprintln!("hotkey tap: active");
        status("ready", None);
        unsafe { CFRunLoopRun() };
        eprintln!("hotkey tap: run loop exited — restarting");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Fires the macOS Accessibility prompt once per process (the system dialog
/// explains itself; further nudging is the UI's job via the Hub banner).
fn prompt_for_accessibility() {
    use std::sync::atomic::AtomicBool;
    static PROMPTED: AtomicBool = AtomicBool::new(false);
    if PROMPTED.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        let key: *const std::ffi::c_void = kAXTrustedCheckOptionPrompt;
        let dict = CFDictionaryCreate(
            std::ptr::null_mut(),
            &key,
            &kCFBooleanTrue,
            1,
            std::ptr::null(),
            std::ptr::null(),
        );
        if !dict.is_null() {
            AXIsProcessTrustedWithOptions(dict);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_table_covers_pickable_keys() {
        for (name, _) in crate::hotkey::KEY_TABLE {
            if *name == "Right Ctrl" || *name == "Right Alt" || *name == "Right Win" {
                continue; // not offered on macOS
            }
            let code = crate::hotkey::parse_key(name).expect(name);
            assert!(vk_of(code).is_some(), "{name} unmapped");
        }
    }

    #[test]
    fn hold_produces_down_then_up() {
        let mut m = KeyMonitor::new(vec![VK_RSHIFT], 250);
        assert_eq!(
            m.observe(12, VK_RSHIFT, 0x04, 1000),
            vec![MonitorEvent::Down]
        );
        assert_eq!(
            m.observe(12, VK_RSHIFT, 0x00, 1500),
            vec![MonitorEvent::Up(500)]
        );
    }

    #[test]
    fn quick_release_is_a_tap() {
        let mut m = KeyMonitor::new(vec![VK_RSHIFT], 250);
        assert_eq!(
            m.observe(12, VK_RSHIFT, 0x04, 1000),
            vec![MonitorEvent::Down]
        );
        assert_eq!(
            m.observe(12, VK_RSHIFT, 0x00, 1100),
            vec![MonitorEvent::TapUp(100)]
        );
    }

    #[test]
    fn left_shift_does_not_trigger_right_shift_hotkey() {
        let mut m = KeyMonitor::new(vec![VK_RSHIFT], 250);
        assert!(m.observe(12, 59, 0x02, 1000).is_empty());
        assert!(m.observe(12, 59, 0x00, 1100).is_empty());
    }

    #[test]
    fn multi_key_combo_requires_all() {
        let mut m = KeyMonitor::new(vec![VK_RSHIFT, 96 /* F5 */], 250);
        assert!(m.observe(12, VK_RSHIFT, 0x04, 1000).is_empty());
        assert_eq!(m.observe(10, 96, 0x04, 1050), vec![MonitorEvent::Down]);
        assert_eq!(m.observe(11, 96, 0x04, 1400), vec![MonitorEvent::Up(350)]);
    }

    #[test]
    fn escape_yields_escape_event_once_per_press() {
        let mut m = KeyMonitor::new(vec![VK_RSHIFT], 250);
        assert_eq!(
            m.observe(10, VK_ESCAPE, 0, 1000),
            vec![MonitorEvent::Escape]
        );
        // Auto-repeat keyDowns must not re-fire.
        assert!(m.observe(10, VK_ESCAPE, 0, 1050).is_empty());
        assert!(m.observe(11, VK_ESCAPE, 0, 1100).is_empty());
        assert_eq!(
            m.observe(10, VK_ESCAPE, 0, 1200),
            vec![MonitorEvent::Escape]
        );
    }

    #[test]
    fn capslock_toggles_via_alpha_bit_only_on_change() {
        let mut m = KeyMonitor::new(vec![VK_CAPSLOCK], 250);
        assert_eq!(
            m.observe(12, VK_CAPSLOCK, 1 << 16, 1000),
            vec![MonitorEvent::Down]
        );
        assert!(m.observe(12, VK_CAPSLOCK, 1 << 16, 1100).is_empty());
        assert_eq!(
            m.observe(12, VK_CAPSLOCK, 0, 1200),
            vec![MonitorEvent::TapUp(200)]
        );
    }

    #[test]
    fn recent_events_ring_is_capped_and_ordered() {
        EVENT_LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(8)));
        let mut log = event_log().lock().unwrap();
        log.clear();
        for i in 0..12u64 {
            log.push_back((1_000 + i, vk_name(VK_RSHIFT), i % 2 == 1));
            while log.len() > 8 {
                log.pop_front();
            }
        }
        assert_eq!(log.len(), 8);
        assert_eq!(log.front().unwrap().0, 1_004, "oldest evicted");
        assert!(log.back().unwrap().2, "newest kept last");
    }
}
