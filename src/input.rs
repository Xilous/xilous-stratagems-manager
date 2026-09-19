#[cfg(target_os = "windows")]
mod windows;

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    B,
    W,
    A,
    S,
    D,

    Up,
    Down,
    Left,
    Right,

    #[serde(rename = "0")]
    Digit0,
    #[serde(rename = "1")]
    Digit1,
    #[serde(rename = "2")]
    Digit2,
    #[serde(rename = "3")]
    Digit3,
    #[serde(rename = "4")]
    Digit4,
    #[serde(rename = "5")]
    Digit5,
    #[serde(rename = "6")]
    Digit6,
    #[serde(rename = "7")]
    Digit7,
    #[serde(rename = "8")]
    Digit8,
    #[serde(rename = "9")]
    Digit9,

    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,

    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,

    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    LCtrl,
    RCtrl,
    LShift,
    RShift,
    LAlt,
    RAlt,
    LWin,
    RWin,
}

impl Key {
    pub const ALL: [Key; 55] = [
        Key::B,
        Key::W,
        Key::A,
        Key::S,
        Key::D,
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Digit0,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
        Key::Numpad0,
        Key::Numpad1,
        Key::Numpad2,
        Key::Numpad3,
        Key::Numpad4,
        Key::Numpad5,
        Key::Numpad6,
        Key::Numpad7,
        Key::Numpad8,
        Key::Numpad9,
        Key::Home,
        Key::End,
        Key::Insert,
        Key::Delete,
        Key::PageUp,
        Key::PageDown,
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::LCtrl,
        Key::RCtrl,
        Key::LShift,
        Key::RShift,
        Key::LAlt,
        Key::RAlt,
        Key::LWin,
        Key::RWin,
    ];

    pub fn is_function_key(self) -> bool {
        matches!(
            self,
            Self::F1
                | Self::F2
                | Self::F3
                | Self::F4
                | Self::F5
                | Self::F6
                | Self::F7
                | Self::F8
                | Self::F9
                | Self::F10
                | Self::F11
                | Self::F12
        )
    }

    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            Self::LCtrl
                | Self::RCtrl
                | Self::LShift
                | Self::RShift
                | Self::LAlt
                | Self::RAlt
                | Self::LWin
                | Self::RWin
        )
    }

    /// Keys that may be registered as global hotkeys. Movement, action, and
    /// modifier keys are excluded because a global hotkey swallows the key.
    pub fn is_bindable(self) -> bool {
        !matches!(self, Self::B | Self::W | Self::A | Self::S | Self::D)
            && !matches!(self, Self::Up | Self::Down | Self::Left | Self::Right)
            && !self.is_modifier()
    }

    pub fn bindable() -> impl Iterator<Item = Key> {
        Self::ALL.into_iter().filter(|key| key.is_bindable())
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Key::B => "B",
            Key::W => "W",
            Key::A => "A",
            Key::S => "S",
            Key::D => "D",
            Key::Up => "Up",
            Key::Down => "Down",
            Key::Left => "Left",
            Key::Right => "Right",
            Key::Digit0 => "0",
            Key::Digit1 => "1",
            Key::Digit2 => "2",
            Key::Digit3 => "3",
            Key::Digit4 => "4",
            Key::Digit5 => "5",
            Key::Digit6 => "6",
            Key::Digit7 => "7",
            Key::Digit8 => "8",
            Key::Digit9 => "9",
            Key::Numpad0 => "Num 0",
            Key::Numpad1 => "Num 1",
            Key::Numpad2 => "Num 2",
            Key::Numpad3 => "Num 3",
            Key::Numpad4 => "Num 4",
            Key::Numpad5 => "Num 5",
            Key::Numpad6 => "Num 6",
            Key::Numpad7 => "Num 7",
            Key::Numpad8 => "Num 8",
            Key::Numpad9 => "Num 9",
            Key::Home => "Home",
            Key::End => "End",
            Key::Insert => "Insert",
            Key::Delete => "Delete",
            Key::PageUp => "Page Up",
            Key::PageDown => "Page Down",
            Key::F1 => "F1",
            Key::F2 => "F2",
            Key::F3 => "F3",
            Key::F4 => "F4",
            Key::F5 => "F5",
            Key::F6 => "F6",
            Key::F7 => "F7",
            Key::F8 => "F8",
            Key::F9 => "F9",
            Key::F10 => "F10",
            Key::F11 => "F11",
            Key::F12 => "F12",
            Key::LCtrl => "LCtrl",
            Key::RCtrl => "RCtrl",
            Key::LShift => "LShift",
            Key::RShift => "RShift",
            Key::LAlt => "LAlt",
            Key::RAlt => "RAlt",
            Key::LWin => "LWin",
            Key::RWin => "RWin",
        }
    }

    /// Lowercase identifier matching the configuration file spelling.
    pub fn config_name(self) -> &'static str {
        match self {
            Key::Numpad0 => "numpad0",
            Key::Numpad1 => "numpad1",
            Key::Numpad2 => "numpad2",
            Key::Numpad3 => "numpad3",
            Key::Numpad4 => "numpad4",
            Key::Numpad5 => "numpad5",
            Key::Numpad6 => "numpad6",
            Key::Numpad7 => "numpad7",
            Key::Numpad8 => "numpad8",
            Key::Numpad9 => "numpad9",
            Key::PageUp => "pageup",
            Key::PageDown => "pagedown",
            Key::LCtrl => "lctrl",
            Key::RCtrl => "rctrl",
            Key::LShift => "lshift",
            Key::RShift => "rshift",
            Key::LAlt => "lalt",
            Key::RAlt => "ralt",
            Key::LWin => "lwin",
            Key::RWin => "rwin",
            Key::B => "b",
            Key::W => "w",
            Key::A => "a",
            Key::S => "s",
            Key::D => "d",
            Key::Up => "up",
            Key::Down => "down",
            Key::Left => "left",
            Key::Right => "right",
            Key::Home => "home",
            Key::End => "end",
            Key::Insert => "insert",
            Key::Delete => "delete",
            Key::Digit0 => "0",
            Key::Digit1 => "1",
            Key::Digit2 => "2",
            Key::Digit3 => "3",
            Key::Digit4 => "4",
            Key::Digit5 => "5",
            Key::Digit6 => "6",
            Key::Digit7 => "7",
            Key::Digit8 => "8",
            Key::Digit9 => "9",
            Key::F1 => "f1",
            Key::F2 => "f2",
            Key::F3 => "f3",
            Key::F4 => "f4",
            Key::F5 => "f5",
            Key::F6 => "f6",
            Key::F7 => "f7",
            Key::F8 => "f8",
            Key::F9 => "f9",
            Key::F10 => "f10",
            Key::F11 => "f11",
            Key::F12 => "f12",
        }
    }

    pub fn from_config_name(name: &str) -> Option<Key> {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|key| key.config_name() == name)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A key with optional modifiers, written as `ctrl+shift+f5` in configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub modifiers: HotkeyModifiers,
    pub key: Key,
}

impl HotkeyBinding {
    pub fn new(modifiers: HotkeyModifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    pub fn bare(key: Key) -> Self {
        Self {
            modifiers: HotkeyModifiers::none(),
            key,
        }
    }

    pub fn label(self) -> String {
        self.modifiers.label_with_key(self.key)
    }

    pub fn config_string(self) -> String {
        let mut parts = self
            .modifiers
            .iter()
            .map(|modifier| modifier.config_name().to_string())
            .collect::<Vec<_>>();
        parts.push(self.key.config_name().to_string());
        parts.join("+")
    }
}

impl FromStr for HotkeyBinding {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut modifiers = Vec::new();
        let mut key = None;
        for part in value
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            if let Some(modifier) = HotkeyModifier::from_config_name(part) {
                modifiers.push(modifier);
            } else if let Some(parsed) = Key::from_config_name(part) {
                if key.replace(parsed).is_some() {
                    bail!("hotkey {value:?} names more than one key");
                }
            } else {
                bail!("hotkey {value:?} contains unknown key {part:?}");
            }
        }
        let Some(key) = key else {
            bail!("hotkey {value:?} does not name a key");
        };
        if !key.is_bindable() {
            bail!(
                "hotkey {value:?}: {} cannot be used as a hotkey",
                key.name()
            );
        }
        Ok(Self {
            modifiers: HotkeyModifiers::new(modifiers)?,
            key,
        })
    }
}

impl fmt::Display for HotkeyBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label())
    }
}

impl serde::Serialize for HotkeyBinding {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.config_string())
    }
}

impl<'de> serde::Deserialize<'de> for HotkeyBinding {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(target_os = "windows")]
pub use windows::{
    HotkeyModifier, HotkeyModifiers, HotkeyPoll, HotkeySpec, InputSession, RegisteredHotkeys,
    discard_pending_hotkeys, poll_hotkey, wait_hotkey_released,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_round_trips() {
        let binding: HotkeyBinding = "ctrl+shift+f5".parse().expect("parse");
        assert_eq!(binding.key, Key::F5);
        assert_eq!(binding.config_string(), "ctrl+shift+f5");
        assert_eq!(binding.label(), "Ctrl + Shift + F5");

        let bare: HotkeyBinding = "numpad7".parse().expect("parse");
        assert!(bare.modifiers.is_empty());
        assert_eq!(bare.label(), "Num 7");
    }

    #[test]
    fn gameplay_keys_are_not_bindable() {
        assert!("w".parse::<HotkeyBinding>().is_err());
        assert!("ctrl".parse::<HotkeyBinding>().is_err());
        assert!("f1".parse::<HotkeyBinding>().is_ok());
    }
}
