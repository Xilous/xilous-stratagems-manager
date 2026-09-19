//! Stratagem catalog synced from helldivers.wiki.gg (see `tools/sync-stratagems.ps1`).
//!
//! The catalog JSON and every icon file are embedded at build time, so the
//! application never needs network access or files beside the executable.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::assets::resize_rgba_box;
use crate::item::StratagemCategory;

include!(concat!(env!("OUT_DIR"), "/catalog_icons.rs"));

const CATALOG_JSON: &str = include_str!("../data/stratagems.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub const fn arrow(self) -> char {
        match self {
            Self::Up => '↑',
            Self::Down => '↓',
            Self::Left => '←',
            Self::Right => '→',
        }
    }
}

pub fn code_arrows(code: &[Direction]) -> String {
    code.iter().map(|direction| direction.arrow()).collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct StratagemEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub permit: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub code: Vec<Direction>,
    #[serde(default)]
    pub cooldown: String,
    #[serde(default)]
    pub icon: Option<String>,
}

impl StratagemEntry {
    /// Loadout category, or `None` for mission stratagems that cannot be equipped.
    pub fn category(&self) -> Option<StratagemCategory> {
        let permit = self.permit.trim().to_ascii_lowercase();
        if permit.starts_with("off") {
            Some(StratagemCategory::Offensive)
        } else if permit.starts_with("sup") {
            Some(StratagemCategory::Supply)
        } else if permit.starts_with("def") {
            Some(StratagemCategory::Defensive)
        } else {
            None
        }
    }

    /// Whether the stratagem can be equipped in a loadout slot. The wiki tags a
    /// few objective items with a permit, so the type is checked as well.
    pub fn is_loadout_item(&self) -> bool {
        self.category().is_some()
            && !matches!(
                self.kind.trim().to_ascii_lowercase().as_str(),
                "objective" | "ship" | "other"
            )
    }

    pub fn has_code(&self) -> bool {
        !self.code.is_empty()
    }

    pub fn arrows(&self) -> String {
        code_arrows(&self.code)
    }
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    source: String,
    #[serde(default)]
    synced_at: String,
    stratagems: Vec<StratagemEntry>,
}

pub struct Catalog {
    entries: Vec<StratagemEntry>,
    by_id: HashMap<String, usize>,
    source: String,
    synced_at: String,
}

impl Catalog {
    pub fn load() -> Result<Self> {
        let json = CATALOG_JSON
            .strip_prefix('\u{feff}')
            .unwrap_or(CATALOG_JSON);
        let file: CatalogFile =
            serde_json::from_str(json).context("failed to parse the embedded stratagem catalog")?;
        ensure!(
            !file.stratagems.is_empty(),
            "the embedded stratagem catalog is empty; run tools/sync-stratagems.ps1"
        );

        let mut by_id = HashMap::with_capacity(file.stratagems.len());
        for (index, entry) in file.stratagems.iter().enumerate() {
            ensure!(
                !entry.id.is_empty(),
                "catalog entry {index} has an empty id"
            );
            if by_id.insert(entry.id.clone(), index).is_some() {
                bail!("catalog contains duplicate id {}", entry.id);
            }
        }

        Ok(Self {
            entries: file.stratagems,
            by_id,
            source: file.source,
            synced_at: file.synced_at,
        })
    }

    pub fn entries(&self) -> &[StratagemEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&StratagemEntry> {
        self.by_id.get(id).map(|&index| &self.entries[index])
    }

    pub fn name_of(&self, id: &str) -> String {
        self.get(id)
            .map_or_else(|| id.to_string(), |entry| entry.name.clone())
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn synced_at(&self) -> &str {
        &self.synced_at
    }

    pub fn loadout_entries(&self) -> impl Iterator<Item = &StratagemEntry> {
        self.entries.iter().filter(|entry| entry.is_loadout_item())
    }

    /// Stratagems available in every mission regardless of loadout.
    pub fn mission_entries(&self) -> impl Iterator<Item = &StratagemEntry> {
        self.entries
            .iter()
            .filter(|entry| !entry.is_loadout_item() && entry.has_code())
    }

    #[cfg(test)]
    pub fn icon_bytes(&self, entry: &StratagemEntry) -> Option<&'static [u8]> {
        entry.icon.as_deref().and_then(icon_file_bytes)
    }

    /// Rasterizes the icon of `id` to a `size`x`size` RGBA image.
    pub fn render_icon(&self, id: &str, size: u32) -> Result<RgbaImage> {
        let entry = self
            .get(id)
            .with_context(|| format!("unknown stratagem id {id}"))?;
        let file = entry
            .icon
            .as_deref()
            .with_context(|| format!("stratagem {id} has no icon"))?;
        let bytes = icon_file_bytes(file)
            .with_context(|| format!("icon file {file} is not embedded; rebuild after syncing"))?;
        if file.to_ascii_lowercase().ends_with(".svg") {
            rasterize_svg(bytes, size).with_context(|| format!("failed to rasterize icon {file}"))
        } else {
            let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                .with_context(|| format!("failed to decode icon {file}"))?
                .to_rgba8();
            resize_rgba_box(&image, size, size)
        }
    }
}

fn icon_file_bytes(name: &str) -> Option<&'static [u8]> {
    CATALOG_ICON_FILES
        .binary_search_by(|(file, _)| (*file).cmp(name))
        .ok()
        .map(|index| CATALOG_ICON_FILES[index].1)
}

pub fn rasterize_svg(bytes: &[u8], size: u32) -> Result<RgbaImage> {
    use resvg::{tiny_skia, usvg};

    ensure!(size > 0, "icon size must be positive");
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .map_err(|error| anyhow!("invalid SVG: {error}"))?;
    let svg_size = tree.size();
    ensure!(
        svg_size.width() > 0.0 && svg_size.height() > 0.0,
        "SVG has an empty size"
    );
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("failed to allocate pixmap")?;
    let transform = tiny_skia::Transform::from_scale(
        size as f32 / svg_size.width(),
        size as f32 / svg_size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut image = RgbaImage::new(size, size);
    for (destination, source) in image.pixels_mut().zip(pixmap.pixels()) {
        let color = source.demultiply();
        destination.0 = [color.red(), color.green(), color.blue(), color.alpha()];
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_with_codes_and_icons() {
        let catalog = Catalog::load().expect("catalog");
        assert!(catalog.loadout_entries().count() >= 60);
        for entry in catalog.loadout_entries() {
            assert!(entry.has_code(), "{} has no code", entry.name);
            assert!(
                catalog.icon_bytes(entry).is_some(),
                "{} has no icon",
                entry.name
            );
        }
        assert!(
            catalog
                .get("reinforce")
                .is_some_and(|entry| !entry.is_loadout_item())
        );
    }

    #[test]
    fn icons_rasterize() {
        let catalog = Catalog::load().expect("catalog");
        let entry = catalog.loadout_entries().next().expect("entry");
        let image = catalog.render_icon(&entry.id, 64).expect("render");
        assert_eq!(image.dimensions(), (64, 64));
        assert!(image.pixels().any(|pixel| pixel[3] > 0));
    }
}
