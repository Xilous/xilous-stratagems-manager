use std::{
    mem::size_of,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tracing::{trace, warn};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    RegisterHotKey, SendInput, UnregisterHotKey, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MSG, PM_REMOVE, PeekMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WM_HOTKEY,
};

use crate::window::{ClientPoint, WindowTarget};

use super::Key;

const CLICK_MOVE_SETTLE_DELAY: Duration = Duration::from_millis(30);

#[derive(Default)]
struct InjectedInputState {
    keys: Vec<Key>,
    left_mouse_down: bool,
}

pub struct InputSession {
    target: WindowTarget,
    injected: InjectedInputState,
}

impl Key {
    fn scan_code(self) -> u16 {
        match self {
            Key::B => 0x30,
            Key::W => 0x11,
            Key::A => 0x1E,
            Key::S => 0x1F,
            Key::D => 0x20,
            Key::Up => 0x48,
            Key::Down => 0x50,
            Key::Left => 0x4B,
            Key::Right => 0x4D,
            Key::Digit1 => 0x02,
            Key::Digit2 => 0x03,
            Key::Digit3 => 0x04,
            Key::Digit4 => 0x05,
            Key::Digit5 => 0x06,
            Key::Digit6 => 0x07,
            Key::Digit7 => 0x08,
            Key::Digit8 => 0x09,
            Key::Digit9 => 0x0A,
            Key::Digit0 => 0x0B,
            Key::Numpad0 => 0x52,
            Key::Numpad1 => 0x4F,
            Key::Numpad2 => 0x50,
            Key::Numpad3 => 0x51,
            Key::Numpad4 => 0x4B,
            Key::Numpad5 => 0x4C,
            Key::Numpad6 => 0x4D,
            Key::Numpad7 => 0x47,
            Key::Numpad8 => 0x48,
            Key::Numpad9 => 0x49,
            Key::Home => 0x47,
            Key::End => 0x4F,
            Key::Insert => 0x52,
            Key::Delete => 0x53,
            Key::PageUp => 0x49,
            Key::PageDown => 0x51,
            Key::F1 => 0x3B,
            Key::F2 => 0x3C,
            Key::F3 => 0x3D,
            Key::F4 => 0x3E,
            Key::F5 => 0x3F,
            Key::F6 => 0x40,
            Key::F7 => 0x41,
            Key::F8 => 0x42,
            Key::F9 => 0x43,
            Key::F10 => 0x44,
            Key::F11 => 0x57,
            Key::F12 => 0x58,
            Key::LCtrl | Key::RCtrl => 0x1D,
            Key::LShift => 0x2A,
            Key::RShift => 0x36,
            Key::LAlt | Key::RAlt => 0x38,
            Key::LWin => 0x5B,
            Key::RWin => 0x5C,
        }
    }

    fn virtual_key(self) -> u32 {
        match self {
            Key::B => 0x42,
            Key::W => 0x57,
            Key::A => 0x41,
            Key::S => 0x53,
            Key::D => 0x44,
            Key::Up => 0x26,
            Key::Down => 0x28,
            Key::Left => 0x25,
            Key::Right => 0x27,
            Key::Digit0 => 0x30,
            Key::Digit1 => 0x31,
            Key::Digit2 => 0x32,
            Key::Digit3 => 0x33,
            Key::Digit4 => 0x34,
            Key::Digit5 => 0x35,
            Key::Digit6 => 0x36,
            Key::Digit7 => 0x37,
            Key::Digit8 => 0x38,
            Key::Digit9 => 0x39,
            Key::Numpad0 => 0x60,
            Key::Numpad1 => 0x61,
            Key::Numpad2 => 0x62,
            Key::Numpad3 => 0x63,
            Key::Numpad4 => 0x64,
            Key::Numpad5 => 0x65,
            Key::Numpad6 => 0x66,
            Key::Numpad7 => 0x67,
            Key::Numpad8 => 0x68,
            Key::Numpad9 => 0x69,
            Key::Home => 0x24,
            Key::End => 0x23,
            Key::Insert => 0x2D,
            Key::Delete => 0x2E,
            Key::PageUp => 0x21,
            Key::PageDown => 0x22,
            Key::F1 => 0x70,
            Key::F2 => 0x71,
            Key::F3 => 0x72,
            Key::F4 => 0x73,
            Key::F5 => 0x74,
            Key::F6 => 0x75,
            Key::F7 => 0x76,
            Key::F8 => 0x77,
            Key::F9 => 0x78,
            Key::F10 => 0x79,
            Key::F11 => 0x7A,
            Key::F12 => 0x7B,
            Key::LCtrl => 0xA2,
            Key::RCtrl => 0xA3,
            Key::LShift => 0xA0,
            Key::RShift => 0xA1,
            Key::LAlt => 0xA4,
            Key::RAlt => 0xA5,
            Key::LWin => 0x5B,
            Key::RWin => 0x5C,
        }
    }

    fn is_extended(self) -> bool {
        matches!(
            self,
            Key::RCtrl
                | Key::RAlt
                | Key::LWin
                | Key::RWin
                | Key::Up
                | Key::Down
                | Key::Left
                | Key::Right
                | Key::Home
                | Key::End
                | Key::Insert
                | Key::Delete
                | Key::PageUp
                | Key::PageDown
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyModifier {
    Shift,
    Ctrl,
    Alt,
    Win,
}

impl HotkeyModifier {
    pub const ALL: [HotkeyModifier; 4] = [
        HotkeyModifier::Ctrl,
        HotkeyModifier::Shift,
        HotkeyModifier::Alt,
        HotkeyModifier::Win,
    ];

    pub fn config_name(self) -> &'static str {
        match self {
            Self::Shift => "shift",
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Win => "win",
        }
    }

    pub fn from_config_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "shift" => Some(Self::Shift),
            "ctrl" | "control" => Some(Self::Ctrl),
            "alt" => Some(Self::Alt),
            "win" | "super" => Some(Self::Win),
            _ => None,
        }
    }

    fn hotkey_modifiers(self) -> HOT_KEY_MODIFIERS {
        match self {
            Self::Shift => MOD_SHIFT,
            Self::Ctrl => MOD_CONTROL,
            Self::Alt => MOD_ALT,
            Self::Win => MOD_WIN,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Shift => "Shift",
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Win => "Win",
        }
    }

    fn release_keys(self) -> &'static [Key] {
        match self {
            Self::Shift => &[Key::LShift, Key::RShift],
            Self::Ctrl => &[Key::LCtrl, Key::RCtrl],
            Self::Alt => &[Key::LAlt, Key::RAlt],
            Self::Win => &[Key::LWin, Key::RWin],
        }
    }

    fn is_down(self) -> bool {
        match self {
            Self::Shift => is_pressed(Key::LShift) || is_pressed(Key::RShift),
            Self::Ctrl => is_pressed(Key::LCtrl) || is_pressed(Key::RCtrl),
            Self::Alt => is_pressed(Key::LAlt) || is_pressed(Key::RAlt),
            Self::Win => is_pressed(Key::LWin) || is_pressed(Key::RWin),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyModifiers {
    values: [Option<HotkeyModifier>; 4],
}

impl HotkeyModifiers {
    /// A hotkey without modifiers (the bare key).
    pub const fn none() -> Self {
        Self {
            values: [None, None, None, None],
        }
    }

    pub fn new(values: Vec<HotkeyModifier>) -> Result<Self> {
        if values.len() > 4 {
            bail!(
                "hotkey modifiers support at most 4 keys, got {}",
                values.len()
            );
        }

        let mut registration_bits = 0u32;
        let mut modifiers = Self::none();

        for (index, modifier) in values.into_iter().enumerate() {
            let bit = modifier.hotkey_modifiers().0;
            if registration_bits & bit != 0 {
                bail!(
                    "hotkey modifiers cannot contain duplicate {}",
                    modifier.config_name()
                );
            }
            registration_bits |= bit;
            modifiers.values[index] = Some(modifier);
        }

        Ok(modifiers)
    }

    #[cfg(test)]
    pub fn is_empty(self) -> bool {
        self.values.iter().all(Option::is_none)
    }

    pub fn contains(self, modifier: HotkeyModifier) -> bool {
        self.iter().any(|value| value == modifier)
    }

    /// Returns a copy with `modifier` added or removed.
    pub fn toggled(self, modifier: HotkeyModifier, enabled: bool) -> Self {
        let mut values = self
            .iter()
            .filter(|value| *value != modifier)
            .collect::<Vec<_>>();
        if enabled {
            values.push(modifier);
        }
        // Order the modifiers canonically so equal sets compare equal.
        values.sort_by_key(|value| {
            HotkeyModifier::ALL
                .iter()
                .position(|candidate| candidate == value)
        });
        Self::new(values).expect("a set of at most four distinct modifiers is valid")
    }

    fn hotkey_modifiers(self) -> HOT_KEY_MODIFIERS {
        self.iter().fold(MOD_NOREPEAT, |modifiers, modifier| {
            modifiers | modifier.hotkey_modifiers()
        })
    }

    fn release_keys(self) -> Vec<Key> {
        let mut keys = Vec::new();
        for modifier in self.iter() {
            keys.extend_from_slice(modifier.release_keys());
        }
        keys
    }

    pub(crate) fn is_down(self) -> bool {
        self.iter().all(HotkeyModifier::is_down)
    }

    pub fn iter(self) -> impl Iterator<Item = HotkeyModifier> {
        self.values.into_iter().flatten()
    }

    pub fn label_with_key(self, key: Key) -> String {
        let mut parts = self
            .iter()
            .map(HotkeyModifier::display_name)
            .collect::<Vec<_>>();
        parts.push(key.name());
        parts.join(" + ")
    }
}

fn keyboard_input(key: Key, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if key.is_extended() {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: key.scan_code(),
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_single_input(input: INPUT, description: &str) -> Result<()> {
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent != 1 {
        bail!(
            "Windows did not accept {description} (sent={sent}). If Helldivers is running as administrator, run Xilous Stratagems Manager as administrator too."
        );
    }
    Ok(())
}

impl InputSession {
    pub fn new(target: WindowTarget) -> Result<Self> {
        target.ensure_input_target()?;
        Ok(Self {
            target,
            injected: InjectedInputState::default(),
        })
    }

    fn ensure_target(&self) -> Result<()> {
        self.target.ensure_input_target()
    }

    fn key_down(&mut self, key: Key) -> Result<()> {
        send_single_input(keyboard_input(key, false), "key press input")?;
        if !self.injected.keys.contains(&key) {
            self.injected.keys.push(key);
        }
        Ok(())
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        send_single_input(keyboard_input(key, true), "key release input")?;
        self.injected.keys.retain(|pressed| *pressed != key);
        Ok(())
    }

    /// Presses and holds `key`; it is released by `release_key` or when the session drops.
    pub fn press_key(&mut self, key: Key) -> Result<()> {
        self.ensure_target()?;
        trace!(key = key.name(), "key press input");
        self.key_down(key)
    }

    pub fn release_key(&mut self, key: Key) -> Result<()> {
        self.ensure_target()?;
        trace!(key = key.name(), "key release input");
        self.key_up(key)
    }

    fn left_button_down(&mut self) -> Result<()> {
        send_single_input(mouse_input(MOUSEEVENTF_LEFTDOWN), "left mouse press input")?;
        self.injected.left_mouse_down = true;
        Ok(())
    }

    fn left_button_up(&mut self) -> Result<()> {
        send_single_input(mouse_input(MOUSEEVENTF_LEFTUP), "left mouse release input")?;
        self.injected.left_mouse_down = false;
        Ok(())
    }

    pub fn scroll(&mut self, delta: i32) -> Result<()> {
        self.ensure_target()?;
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        send_single_input(input, "mouse wheel input")
            .with_context(|| format!("failed to send mouse wheel delta {delta}"))
    }

    pub fn click(&mut self, point: ClientPoint, hold_ms: u64) -> Result<()> {
        trace!(
            x = point.x,
            y = point.y,
            hold_ms,
            settle = ?CLICK_MOVE_SETTLE_DELAY,
            "mouse click input"
        );
        self.move_cursor(point)?;
        sleep(CLICK_MOVE_SETTLE_DELAY);
        self.click_current(hold_ms)
    }

    /// Click at the current cursor position without sending another mouse-move event.
    ///
    /// The button events do not carry cursor coordinates, so a caller can verify
    /// hover state between moving the cursor and issuing the click.
    pub fn click_current(&mut self, hold_ms: u64) -> Result<()> {
        self.ensure_target()?;
        self.left_button_down()?;
        sleep(Duration::from_millis(hold_ms));
        self.left_button_up()?;
        Ok(())
    }

    pub fn move_cursor(&mut self, point: ClientPoint) -> Result<()> {
        self.ensure_target()?;
        let (x, y) = self.target.client_point_to_screen(point);
        move_cursor_absolute(x, y)
    }

    pub fn tap_key(&mut self, key: Key, hold_ms: u64) -> Result<()> {
        self.ensure_target()?;
        trace!(key = key.name(), hold_ms, "key tap input");
        self.key_down(key)?;
        sleep(Duration::from_millis(hold_ms));
        self.key_up(key)?;
        Ok(())
    }

    fn release_tracked_inputs_best_effort(&mut self) {
        let state = std::mem::take(&mut self.injected);
        if state.keys.is_empty() && !state.left_mouse_down {
            return;
        }

        let key_names = state.keys.iter().map(|key| key.name()).collect::<Vec<_>>();
        let mut inputs = Vec::with_capacity(state.keys.len() + state.left_mouse_down as usize);
        if state.left_mouse_down {
            inputs.push(mouse_input(MOUSEEVENTF_LEFTUP));
        }
        inputs.extend(
            state
                .keys
                .iter()
                .rev()
                .map(|key| keyboard_input(*key, true)),
        );

        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize;
        if sent != inputs.len() {
            warn!(
                expected = inputs.len(),
                sent,
                keys = ?key_names,
                left_mouse_down = state.left_mouse_down,
                "failed to fully release tracked injected inputs"
            );
        } else {
            warn!(
                keys = ?key_names,
                left_mouse_down = state.left_mouse_down,
                "released tracked injected inputs after interrupted automation"
            );
        }
    }
}

impl Drop for InputSession {
    fn drop(&mut self) {
        self.release_tracked_inputs_best_effort();
    }
}

fn move_cursor_absolute(x: i32, y: i32) -> Result<()> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    let x = x.clamp(left, left + width - 1);
    let y = y.clamp(top, top + height - 1);

    let dx = normalize_absolute_mouse_coord(x - left, width);
    let dy = normalize_absolute_mouse_coord(y - top, height);

    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    send_single_input(input, "absolute mouse move input")
        .context("failed to move cursor with SendInput")
}

fn normalize_absolute_mouse_coord(value: i32, size: i32) -> i32 {
    if size <= 1 {
        return 0;
    }
    ((value as i64 * 65_535) / (size as i64 - 1)) as i32
}

#[derive(Debug, Clone, Copy)]
pub struct HotkeySpec {
    pub id: i32,
    pub modifiers: HotkeyModifiers,
    pub key: Key,
}

impl HotkeySpec {
    pub fn label(self) -> String {
        self.modifiers.label_with_key(self.key)
    }
}

pub enum HotkeyPoll {
    Triggered(i32),
    Timeout,
}

/// A set of registered global hotkeys; they are unregistered on drop.
///
/// Hotkeys must be registered and polled on the same thread, because Windows
/// posts `WM_HOTKEY` to the message queue of the registering thread.
pub struct RegisteredHotkeys {
    hotkeys: Vec<HotkeySpec>,
}

impl RegisteredHotkeys {
    pub fn register(hotkeys: &[HotkeySpec]) -> Result<Self> {
        if hotkeys.is_empty() {
            bail!("no hotkeys to register");
        }

        let mut registered = Vec::with_capacity(hotkeys.len());
        let mut failures = Vec::new();
        let mut seen = Vec::with_capacity(hotkeys.len());
        unsafe {
            for hotkey in hotkeys {
                let registration_key = (
                    hotkey.modifiers.hotkey_modifiers().0,
                    hotkey.key.virtual_key(),
                );
                if seen.contains(&registration_key) {
                    failures.push(format!("{}: configured more than once", hotkey.label()));
                    continue;
                }
                seen.push(registration_key);

                match RegisterHotKey(
                    None,
                    hotkey.id,
                    hotkey.modifiers.hotkey_modifiers(),
                    hotkey.key.virtual_key(),
                ) {
                    Ok(()) => registered.push(*hotkey),
                    Err(error) => failures.push(format!("{}: {error}", hotkey.label())),
                }
            }

            if !failures.is_empty() {
                for hotkey in &registered {
                    let _ = UnregisterHotKey(None, hotkey.id);
                }
            }
        }

        if !failures.is_empty() {
            bail!(
                "the following hotkeys could not be registered:\n- {}",
                failures.join("\n- ")
            );
        }

        Ok(Self {
            hotkeys: registered,
        })
    }
}

impl Drop for RegisteredHotkeys {
    fn drop(&mut self) {
        unsafe {
            for hotkey in &self.hotkeys {
                let _ = UnregisterHotKey(None, hotkey.id);
            }
        }
    }
}

/// Waits up to `timeout` for any `WM_HOTKEY` posted to the current thread.
pub fn poll_hotkey(timeout: Duration) -> HotkeyPoll {
    let start = Instant::now();

    loop {
        if let Some(hotkey_id) = take_pending_hotkey() {
            return HotkeyPoll::Triggered(hotkey_id);
        }

        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return HotkeyPoll::Timeout;
        }

        let remaining = timeout.saturating_sub(elapsed);
        sleep(remaining.min(Duration::from_millis(20)));
    }
}

pub fn discard_pending_hotkeys() {
    while take_pending_hotkey().is_some() {}
}

pub fn wait_hotkey_released(hotkey: &HotkeySpec, timeout: Duration) -> bool {
    let mut release_keys = hotkey.modifiers.release_keys();
    release_keys.push(hotkey.key);
    wait_keys_released(&release_keys, timeout)
}

fn take_pending_hotkey() -> Option<i32> {
    let mut message = MSG::default();
    unsafe {
        PeekMessageW(&mut message, None, WM_HOTKEY, WM_HOTKEY, PM_REMOVE)
            .as_bool()
            .then_some(message.wParam.0 as i32)
    }
}

fn wait_keys_released(keys: &[Key], timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if keys.iter().all(|key| !is_pressed(*key)) {
            return true;
        }
        sleep(Duration::from_millis(20));
    }
    keys.iter().all(|key| !is_pressed(*key))
}

pub fn is_pressed(key: Key) -> bool {
    unsafe { GetAsyncKeyState(key.virtual_key() as i32) < 0 }
}
