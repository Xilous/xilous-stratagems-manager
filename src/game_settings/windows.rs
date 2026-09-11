use std::path::PathBuf;

use anyhow::{Context, Result};

pub(super) fn settings_path() -> Result<PathBuf> {
    Ok(std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .context("APPDATA is unavailable")?
        .join("Arrowhead/Helldivers2/user_settings.config"))
}
