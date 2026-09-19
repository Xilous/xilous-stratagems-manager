//! The loadout the tool believes is currently equipped in game. Mission slot
//! hotkeys map to its four slots.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::assets::parse_json_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActiveSource {
    /// Set when a preset was saved from the loadout screen.
    Saved,
    /// Set when a preset was applied to the loadout screen.
    Applied,
    /// Chosen by hand in the window.
    #[default]
    Manual,
}

impl ActiveSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Saved => "saved from game",
            Self::Applied => "applied to game",
            Self::Manual => "set manually",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveLoadout {
    /// Preset name the loadout came from, e.g. `preset_2`.
    pub preset: String,
    /// Catalog ids in on-screen slot order; `None` when a slot could not be identified.
    pub slots: Vec<Option<String>>,
    #[serde(default)]
    pub source: ActiveSource,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub set_at_unix: u64,
}

impl ActiveLoadout {
    pub fn new(preset: &str, slots: [Option<String>; 4], source: ActiveSource) -> Self {
        Self {
            preset: preset.to_string(),
            slots: slots.to_vec(),
            source,
            set_at_unix: now_unix(),
        }
    }

    pub fn slot(&self, index: usize) -> Option<&str> {
        self.slots.get(index).and_then(|slot| slot.as_deref())
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub fn load(path: &Path) -> Result<Option<ActiveLoadout>> {
    if !path.exists() {
        return Ok(None);
    }
    let loadout: ActiveLoadout = parse_json_file(path)?;
    ensure!(
        loadout.slots.len() == 4,
        "active loadout in {} must have 4 slots, got {}",
        path.display(),
        loadout.slots.len()
    );
    Ok(Some(loadout))
}

pub fn save(path: &Path, loadout: &ActiveLoadout) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(loadout)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

pub fn clear(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}
