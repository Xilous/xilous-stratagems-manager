//! Sends a Stratagem input code to the game: hold the Stratagem menu key, tap
//! the direction keys, release. This is open-loop; the game is not observed.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::catalog::Direction;
use crate::input::{InputSession, Key};

const MAX_DELAY_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuMode {
    /// The Stratagem menu is open while the menu key is held (game default).
    Hold,
    /// Pressing the menu key toggles the Stratagem menu.
    Press,
}

impl MenuMode {
    pub const ALL: [MenuMode; 2] = [MenuMode::Hold, MenuMode::Press];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Hold => "Hold",
            Self::Press => "Press (toggle)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DirectionKeys {
    Arrows,
    Wasd,
}

impl DirectionKeys {
    pub const ALL: [DirectionKeys; 2] = [DirectionKeys::Arrows, DirectionKeys::Wasd];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Arrows => "Arrow keys",
            Self::Wasd => "WASD",
        }
    }

    pub const fn key(self, direction: Direction) -> Key {
        match (self, direction) {
            (Self::Arrows, Direction::Up) => Key::Up,
            (Self::Arrows, Direction::Down) => Key::Down,
            (Self::Arrows, Direction::Left) => Key::Left,
            (Self::Arrows, Direction::Right) => Key::Right,
            (Self::Wasd, Direction::Up) => Key::W,
            (Self::Wasd, Direction::Down) => Key::S,
            (Self::Wasd, Direction::Left) => Key::A,
            (Self::Wasd, Direction::Right) => Key::D,
        }
    }
}

pub const DIRECTIONS: [Direction; 4] = [
    Direction::Up,
    Direction::Down,
    Direction::Left,
    Direction::Right,
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StratagemInputSettings {
    /// Key bound to "Stratagem menu" in Helldivers 2.
    pub menu_key: Key,
    pub menu_mode: MenuMode,
    /// Keys bound to the four Stratagem directions in Helldivers 2.
    pub direction_up: Key,
    pub direction_down: Key,
    pub direction_left: Key,
    pub direction_right: Key,
    /// Wait after opening the menu before the first direction.
    pub menu_open_delay_ms: u64,
    /// How long each direction key is held.
    pub key_hold_ms: u64,
    /// Gap between direction keys.
    pub key_gap_ms: u64,
    /// Wait after the last direction before releasing the menu key (hold mode).
    pub menu_release_delay_ms: u64,
}

impl Default for StratagemInputSettings {
    fn default() -> Self {
        Self {
            menu_key: Key::LCtrl,
            menu_mode: MenuMode::Hold,
            direction_up: Key::Up,
            direction_down: Key::Down,
            direction_left: Key::Left,
            direction_right: Key::Right,
            menu_open_delay_ms: 60,
            key_hold_ms: 40,
            key_gap_ms: 40,
            menu_release_delay_ms: 40,
        }
    }
}

impl StratagemInputSettings {
    pub const fn direction_key(&self, direction: Direction) -> Key {
        match direction {
            Direction::Up => self.direction_up,
            Direction::Down => self.direction_down,
            Direction::Left => self.direction_left,
            Direction::Right => self.direction_right,
        }
    }

    pub fn set_direction_key(&mut self, direction: Direction, key: Key) {
        match direction {
            Direction::Up => self.direction_up = key,
            Direction::Down => self.direction_down = key,
            Direction::Left => self.direction_left = key,
            Direction::Right => self.direction_right = key,
        }
    }

    /// Applies one of the common layouts to all four direction keys.
    pub fn apply_layout(&mut self, layout: DirectionKeys) {
        for direction in DIRECTIONS {
            self.set_direction_key(direction, layout.key(direction));
        }
    }

    pub fn matches_layout(&self, layout: DirectionKeys) -> bool {
        DIRECTIONS
            .iter()
            .all(|direction| self.direction_key(*direction) == layout.key(*direction))
    }

    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("menu_open_delay_ms", self.menu_open_delay_ms),
            ("key_hold_ms", self.key_hold_ms),
            ("key_gap_ms", self.key_gap_ms),
            ("menu_release_delay_ms", self.menu_release_delay_ms),
        ] {
            ensure!(
                value <= MAX_DELAY_MS,
                "mission.{name} must be at most {MAX_DELAY_MS} ms, got {value}"
            );
        }
        ensure!(
            self.key_hold_ms > 0,
            "mission.key_hold_ms must be greater than zero"
        );
        let direction_keys = DIRECTIONS.map(|direction| self.direction_key(direction));
        if direction_keys.contains(&self.menu_key) {
            bail!("the Stratagem menu key cannot also be a direction key");
        }
        for (index, key) in direction_keys.iter().enumerate() {
            if direction_keys[..index].contains(key) {
                bail!("{} is used for more than one direction", key.name());
            }
        }
        Ok(())
    }

    /// Total time the sequence takes, for status display.
    pub fn estimated_duration(&self, code_len: usize) -> Duration {
        let taps = code_len as u64;
        let mut total = self.menu_open_delay_ms + taps * self.key_hold_ms + self.key_hold_ms;
        if taps > 1 {
            total += (taps - 1) * self.key_gap_ms;
        }
        if self.menu_mode == MenuMode::Hold {
            total += self.menu_release_delay_ms;
        }
        Duration::from_millis(total)
    }
}

/// Enters `code` in the game. `session` guards that the game keeps focus and
/// releases any held key if the sequence is interrupted.
pub fn execute(
    session: &mut InputSession,
    settings: &StratagemInputSettings,
    code: &[Direction],
) -> Result<()> {
    ensure!(!code.is_empty(), "stratagem code is empty");
    settings.validate()?;

    match settings.menu_mode {
        MenuMode::Hold => session.press_key(settings.menu_key)?,
        MenuMode::Press => session.tap_key(settings.menu_key, settings.key_hold_ms)?,
    }
    sleep(Duration::from_millis(settings.menu_open_delay_ms));

    for (index, direction) in code.iter().enumerate() {
        session.tap_key(settings.direction_key(*direction), settings.key_hold_ms)?;
        if index + 1 < code.len() {
            sleep(Duration::from_millis(settings.key_gap_ms));
        }
    }

    if settings.menu_mode == MenuMode::Hold {
        sleep(Duration::from_millis(settings.menu_release_delay_ms));
        session.release_key(settings.menu_key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        StratagemInputSettings::default().validate().expect("valid");
    }

    #[test]
    fn menu_key_cannot_be_direction() {
        let mut settings = StratagemInputSettings {
            menu_key: Key::W,
            ..Default::default()
        };
        settings.apply_layout(DirectionKeys::Wasd);
        assert!(settings.validate().is_err());
    }

    #[test]
    fn directions_must_be_distinct() {
        let settings = StratagemInputSettings {
            direction_left: Key::Up,
            ..Default::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn layouts_and_custom_keys() {
        let mut settings = StratagemInputSettings::default();
        assert!(settings.matches_layout(DirectionKeys::Arrows));
        settings.apply_layout(DirectionKeys::Wasd);
        assert_eq!(settings.direction_key(Direction::Left), Key::A);
        settings.set_direction_key(Direction::Up, Key::Numpad8);
        assert!(!settings.matches_layout(DirectionKeys::Wasd));
        settings.menu_key = Key::Home;
        settings.validate().expect("custom layout is valid");
    }
}
