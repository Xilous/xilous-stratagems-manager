//! `data/config.toml`: loading, validation, and in-place edits that keep the
//! comments of the file intact.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tracing::info;

use crate::input::{self, HotkeyBinding};
use crate::stratagem_input::StratagemInputSettings;

pub const DEFAULT_CONFIG_TOML: &str = include_str!("../data/config.toml");
pub const MAX_PRESET_HOTKEYS: usize = 4;
pub const PRESETS_RELATIVE_PATH: &str = "data/presets.json";

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub presets: PresetsConfig,
    pub hotkey: HotkeyConfig,
    pub mission: MissionConfig,
    pub window: WindowConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PresetsConfig {
    #[serde(rename = "path")]
    pub legacy_path: Option<PathBuf>,
    pub apply_in_saved_order: bool,
    pub auto_ready_up: bool,
    pub save_fallback_when_taken: bool,
    pub labels: BTreeMap<String, String>,
}

impl Default for PresetsConfig {
    fn default() -> Self {
        Self {
            legacy_path: None,
            // Slot order must match the saved preset so slot hotkeys stay predictable.
            apply_in_saved_order: true,
            auto_ready_up: false,
            save_fallback_when_taken: false,
            labels: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeyConfig {
    pub modifiers: Vec<input::HotkeyModifier>,
    pub keys: Vec<input::Key>,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            modifiers: vec![input::HotkeyModifier::Shift],
            keys: vec![input::Key::F1, input::Key::F2, input::Key::F3, input::Key::F4],
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MissionConfig {
    pub enabled: bool,
    pub slot_keys: Vec<HotkeyBinding>,
    pub menu_key: input::Key,
    pub menu_mode: crate::stratagem_input::MenuMode,
    /// Older layout selector; the explicit direction keys below win when set.
    pub direction_keys: Option<crate::stratagem_input::DirectionKeys>,
    pub direction_up: Option<input::Key>,
    pub direction_down: Option<input::Key>,
    pub direction_left: Option<input::Key>,
    pub direction_right: Option<input::Key>,
    pub menu_open_delay_ms: u64,
    pub key_hold_ms: u64,
    pub key_gap_ms: u64,
    pub menu_release_delay_ms: u64,
    /// Catalog id -> hotkey for mission stratagems available in every loadout.
    pub bindings: BTreeMap<String, HotkeyBinding>,
}

impl Default for MissionConfig {
    fn default() -> Self {
        let input = StratagemInputSettings::default();
        Self {
            enabled: true,
            slot_keys: vec![
                HotkeyBinding::bare(input::Key::F1),
                HotkeyBinding::bare(input::Key::F2),
                HotkeyBinding::bare(input::Key::F3),
                HotkeyBinding::bare(input::Key::F4),
            ],
            menu_key: input.menu_key,
            menu_mode: input.menu_mode,
            direction_keys: None,
            direction_up: Some(input.direction_up),
            direction_down: Some(input.direction_down),
            direction_left: Some(input.direction_left),
            direction_right: Some(input.direction_right),
            menu_open_delay_ms: input.menu_open_delay_ms,
            key_hold_ms: input.key_hold_ms,
            key_gap_ms: input.key_gap_ms,
            menu_release_delay_ms: input.menu_release_delay_ms,
            bindings: BTreeMap::new(),
        }
    }
}

impl MissionConfig {
    pub fn stratagem_input(&self) -> StratagemInputSettings {
        let mut settings = StratagemInputSettings {
            menu_key: self.menu_key,
            menu_mode: self.menu_mode,
            menu_open_delay_ms: self.menu_open_delay_ms,
            key_hold_ms: self.key_hold_ms,
            key_gap_ms: self.key_gap_ms,
            menu_release_delay_ms: self.menu_release_delay_ms,
            ..StratagemInputSettings::default()
        };
        if let Some(layout) = self.direction_keys {
            settings.apply_layout(layout);
        }
        for (direction, key) in [
            (crate::catalog::Direction::Up, self.direction_up),
            (crate::catalog::Direction::Down, self.direction_down),
            (crate::catalog::Direction::Left, self.direction_left),
            (crate::catalog::Direction::Right, self.direction_right),
        ] {
            if let Some(key) = key {
                settings.set_direction_key(direction, key);
            }
        }
        settings
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub start_hidden: bool,
}

/// Loads the configuration, creating or resetting the file when needed.
/// Returns the configuration and whether the user should be told about a reset.
pub fn load_app_config(config_path: &Path) -> Result<(AppConfig, bool)> {
    if !config_path.exists() {
        if let Some(parent) = config_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        fs::write(config_path, DEFAULT_CONFIG_TOML)
            .with_context(|| format!("failed to create {}", config_path.display()))?;
        info!(path = %config_path.display(), "default configuration created");
    }

    let text = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut config: AppConfig = match toml::from_str(&text) {
        Ok(config) => config,
        Err(error) => {
            // Configuration files from HD2 Preset Helper (e.g. an [overlay] table)
            // cannot be migrated meaningfully; keep a backup and start fresh.
            let backup = config_path.with_extension("toml.bak");
            fs::copy(config_path, &backup)
                .with_context(|| format!("failed to back up {}", config_path.display()))?;
            fs::write(config_path, DEFAULT_CONFIG_TOML)
                .with_context(|| format!("failed to reset {}", config_path.display()))?;
            info!(
                path = %config_path.display(),
                backup = %backup.display(),
                error = %error,
                "unreadable configuration reset to defaults"
            );
            let config = toml::from_str(DEFAULT_CONFIG_TOML)
                .context("failed to parse the embedded default configuration")?;
            validate(&config)?;
            return Ok((config, true));
        }
    };

    let reset_action = config_reset_action(&config);
    if reset_action != ConfigResetAction::None {
        fs::write(config_path, DEFAULT_CONFIG_TOML)
            .with_context(|| format!("failed to reset {}", config_path.display()))?;
        config = toml::from_str(DEFAULT_CONFIG_TOML)
            .context("failed to parse the embedded default configuration")?;
        info!(
            path = %config_path.display(),
            notify = reset_action == ConfigResetAction::Notify,
            "legacy configuration reset"
        );
    }
    validate(&config)?;
    Ok((config, reset_action == ConfigResetAction::Notify))
}

fn validate(config: &AppConfig) -> Result<()> {
    validate_hotkey_keys(&config.hotkey.keys)?;
    validate_preset_labels(&config.presets.labels)?;
    validate_mission(&config.mission)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigResetAction {
    None,
    Silent,
    Notify,
}

fn config_reset_action(config: &AppConfig) -> ConfigResetAction {
    let Some(legacy_path) = config.presets.legacy_path.as_deref() else {
        return ConfigResetAction::None;
    };

    let mut current_without_legacy_path = config.clone();
    current_without_legacy_path.presets.legacy_path = None;
    if legacy_path == Path::new(PRESETS_RELATIVE_PATH)
        && current_without_legacy_path == AppConfig::default()
    {
        ConfigResetAction::Silent
    } else {
        ConfigResetAction::Notify
    }
}

fn validate_hotkey_keys(keys: &[input::Key]) -> Result<()> {
    if keys.is_empty() || keys.len() > MAX_PRESET_HOTKEYS {
        bail!(
            "hotkey.keys must contain 1 to {} keys, got {}",
            MAX_PRESET_HOTKEYS,
            keys.len(),
        );
    }
    if let Some(key) = keys.iter().find(|key| !key.is_function_key()) {
        bail!("hotkey.keys only supports f1 through f12, got {key:?}");
    }

    Ok(())
}

fn validate_preset_labels(labels: &BTreeMap<String, String>) -> Result<()> {
    for (preset, label) in labels {
        let valid_key = preset
            .strip_prefix("preset_")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|index| (1..=MAX_PRESET_HOTKEYS).contains(index))
            .is_some_and(|index| preset == &format!("preset_{index}"));
        if !valid_key {
            bail!(
                "presets.labels key must be preset_1 through preset_{MAX_PRESET_HOTKEYS}, got {preset}"
            );
        }
        if label.chars().any(char::is_control) {
            bail!("presets.labels.{preset} must not contain control characters");
        }
    }
    Ok(())
}

fn validate_mission(mission: &MissionConfig) -> Result<()> {
    if mission.slot_keys.len() != 4 {
        bail!(
            "mission.slot_keys must contain exactly 4 keys, got {}",
            mission.slot_keys.len()
        );
    }
    mission.stratagem_input().validate()?;
    Ok(())
}

/// Applies `edit` to the configuration document and writes it back, preserving
/// formatting and comments. Missing tables are created from the defaults.
pub fn edit_config(
    config_path: &Path,
    edit: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
) -> Result<()> {
    let text = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    let defaults = DEFAULT_CONFIG_TOML
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse the embedded default configuration for editing")?;
    for section in ["presets", "hotkey", "mission", "window"] {
        if document.get(section).is_none() {
            document[section] = defaults[section].clone();
        }
    }

    edit(&mut document)?;

    fs::write(config_path, document.to_string())
        .with_context(|| format!("failed to update {}", config_path.display()))
}

pub fn set_bool(document: &mut toml_edit::DocumentMut, section: &str, key: &str, value: bool) {
    document[section][key] = toml_edit::value(value);
}

pub fn set_string(document: &mut toml_edit::DocumentMut, section: &str, key: &str, value: &str) {
    document[section][key] = toml_edit::value(value);
}

pub fn set_integer(document: &mut toml_edit::DocumentMut, section: &str, key: &str, value: u64) {
    document[section][key] = toml_edit::value(value as i64);
}

pub fn set_string_array(
    document: &mut toml_edit::DocumentMut,
    section: &str,
    key: &str,
    values: impl IntoIterator<Item = String>,
) {
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value);
    }
    document[section][key] = toml_edit::value(array);
}

/// Sets or removes `key` in a nested table such as `[mission.bindings]`.
pub fn set_nested_string(
    document: &mut toml_edit::DocumentMut,
    section: &str,
    table: &str,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    let section_table = document[section]
        .as_table_like_mut()
        .with_context(|| format!("[{section}] is not a table"))?;
    if section_table.get(table).is_none() {
        section_table.insert(table, toml_edit::table());
    }
    let nested = section_table
        .get_mut(table)
        .and_then(|item| item.as_table_like_mut())
        .with_context(|| format!("[{section}.{table}] is not a table"))?;
    match value {
        Some(value) => {
            nested.insert(key, toml_edit::value(value));
        }
        None => {
            nested.remove(key);
        }
    }
    Ok(())
}
