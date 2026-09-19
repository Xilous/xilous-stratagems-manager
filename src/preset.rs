use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::assets::{decode_rgba8, parse_json_file};
use crate::vision::{ImageSample, SampleGeometry};

const PRESET_SCHEMA_VERSION: u32 = 1;
const LOCAL_TEMPLATES_DIR: &str = "local_templates";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocalTemplate {
    /// Local template path relative to the directory containing presets.json.
    pub path: String,
    #[serde(flatten)]
    pub geometry: SampleGeometry,
    /// Catalog id of the stratagem this template shows, once identified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stratagem: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Preset {
    pub stratagems: Vec<LocalTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booster: Option<LocalTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_booster: Option<LocalTemplate>,
}

impl Preset {
    /// Catalog ids of the four stratagems in saved order.
    pub fn stratagem_ids(&self) -> [Option<String>; 4] {
        std::array::from_fn(|index| {
            self.stratagems
                .get(index)
                .and_then(|template| template.stratagem.clone())
        })
    }
}

pub struct CapturedPreset {
    pub stratagems: Vec<ImageSample>,
    pub booster: Option<ImageSample>,
}

pub fn invalid_preset_reason(path: &Path, preset: &Preset) -> Option<String> {
    for template in preset
        .stratagems
        .iter()
        .chain(preset.booster.iter())
        .chain(preset.fallback_booster.iter())
    {
        if !valid_sample_geometry(template.geometry) {
            return Some(format!("invalid sample geometry for {}", template.path));
        }
        let resolved = match resolve_template_path(path, &template.path) {
            Ok(path) => path,
            Err(error) => {
                return Some(format!(
                    "invalid template path {:?}: {error}",
                    template.path
                ));
            }
        };
        if !resolved.is_file() {
            return Some(format!("missing local template {}", resolved.display()));
        }
    }

    None
}

#[derive(Debug, Deserialize, Serialize)]
struct PresetFile {
    #[serde(default)]
    schema_version: u32,
    presets: BTreeMap<String, Preset>,
}

impl Default for PresetFile {
    fn default() -> Self {
        Self {
            schema_version: PRESET_SCHEMA_VERSION,
            presets: BTreeMap::new(),
        }
    }
}

pub fn load_preset(path: &Path, name: &str) -> Result<Preset> {
    let presets = load_preset_file(path)?;
    let preset = presets
        .presets
        .get(name)
        .with_context(|| format!("preset not found: {name}"))?;

    validate_preset(name, preset)?;
    Ok(preset.clone())
}

pub(crate) fn load_presets(path: &Path) -> Result<BTreeMap<String, Preset>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    Ok(load_preset_file(path)?.presets)
}

pub(crate) fn archive_legacy_preset_file(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let document: serde_json::Value = parse_json_file(path)?;
    let schema_version = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);

    match schema_version {
        Some(version) if version == u64::from(PRESET_SCHEMA_VERSION) => return Ok(None),
        Some(version) if version > u64::from(PRESET_SCHEMA_VERSION) => {
            bail!(
                "preset data uses schema version {version}, but this version only supports schema version {PRESET_SCHEMA_VERSION}"
            );
        }
        _ => {}
    }

    let backup = path.with_file_name("presets.backup.json");
    if backup.exists() {
        fs::remove_file(&backup)
            .with_context(|| format!("failed to replace {}", backup.display()))?;
    }

    fs::rename(path, &backup).with_context(|| {
        format!(
            "failed to archive legacy preset data from {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(Some(backup))
}

pub fn save_captured_preset(path: &Path, name: &str, captured: &CapturedPreset) -> Result<Preset> {
    validate_preset_name(name)?;
    ensure!(
        captured.stratagems.len() == 4,
        "preset {name} must contain exactly 4 captured stratagems, got {}",
        captured.stratagems.len()
    );
    ensure!(
        captured
            .stratagems
            .iter()
            .all(|sample| valid_sample_geometry(sample.geometry)),
        "preset {name} has invalid local-template geometry"
    );

    let mut presets = if path.exists() {
        load_preset_file(path)?
    } else {
        PresetFile::default()
    };

    let mut preset = Preset {
        stratagems: Vec::with_capacity(4),
        booster: None,
        fallback_booster: None,
    };
    for (index, sample) in captured.stratagems.iter().enumerate() {
        let relative = format!("{LOCAL_TEMPLATES_DIR}/{name}/stratagem-{}.png", index + 1);
        save_template_image(path, &relative, &sample.image)?;
        preset.stratagems.push(LocalTemplate {
            path: relative,
            geometry: sample.geometry,
            stratagem: None,
        });
    }
    if let Some(sample) = &captured.booster {
        ensure!(
            valid_sample_geometry(sample.geometry),
            "preset {name} has invalid booster-template geometry"
        );
        let relative = format!("{LOCAL_TEMPLATES_DIR}/{name}/booster.png");
        save_template_image(path, &relative, &sample.image)?;
        preset.booster = Some(LocalTemplate {
            path: relative,
            geometry: sample.geometry,
            stratagem: None,
        });
    }

    validate_preset(name, &preset)?;
    presets.schema_version = PRESET_SCHEMA_VERSION;
    presets.presets.insert(name.to_string(), preset.clone());
    write_preset_file(path, &presets)?;
    remove_template_image_if_exists(
        path,
        &format!("{LOCAL_TEMPLATES_DIR}/{name}/fallback-booster.png"),
    )?;
    if captured.booster.is_none() {
        remove_template_image_if_exists(
            path,
            &format!("{LOCAL_TEMPLATES_DIR}/{name}/booster.png"),
        )?;
    }
    Ok(preset)
}

/// Records the identified catalog ids for all four stratagem slots of a preset.
pub fn set_preset_stratagems(
    path: &Path,
    name: &str,
    ids: &[Option<String>; 4],
) -> Result<Preset> {
    let mut presets = load_preset_file(path)?;
    let preset = presets
        .presets
        .get_mut(name)
        .with_context(|| format!("preset not found: {name}"))?;
    for (template, id) in preset.stratagems.iter_mut().zip(ids) {
        template.stratagem = id.clone();
    }
    let saved = preset.clone();
    write_preset_file(path, &presets)?;
    Ok(saved)
}

/// Overrides the catalog id of one stratagem slot (0-based).
pub fn set_preset_stratagem(
    path: &Path,
    name: &str,
    slot: usize,
    id: Option<String>,
) -> Result<Preset> {
    let mut presets = load_preset_file(path)?;
    let preset = presets
        .presets
        .get_mut(name)
        .with_context(|| format!("preset not found: {name}"))?;
    let template = preset
        .stratagems
        .get_mut(slot)
        .with_context(|| format!("preset {name} has no stratagem slot {}", slot + 1))?;
    template.stratagem = id;
    let saved = preset.clone();
    write_preset_file(path, &presets)?;
    Ok(saved)
}

/// Removes a preset and its captured templates.
pub fn delete_preset(path: &Path, name: &str) -> Result<()> {
    validate_preset_name(name)?;
    if path.exists() {
        let mut presets = load_preset_file(path)?;
        if presets.presets.remove(name).is_some() {
            write_preset_file(path, &presets)?;
        }
    }
    let directory = resolve_template_path(path, &format!("{LOCAL_TEMPLATES_DIR}/{name}"))?;
    match fs::remove_dir_all(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to remove {}", directory.display()))
        }
    }
}

pub fn save_fallback_booster(path: &Path, name: &str, sample: &ImageSample) -> Result<Preset> {
    validate_preset_name(name)?;
    ensure!(
        valid_sample_geometry(sample.geometry),
        "preset {name} has invalid fallback booster-template geometry"
    );

    let mut presets = load_preset_file(path)?;
    let preset = presets
        .presets
        .get_mut(name)
        .with_context(|| format!("preset not found: {name}"))?;
    let relative = format!("{LOCAL_TEMPLATES_DIR}/{name}/fallback-booster.png");
    save_template_image(path, &relative, &sample.image)?;
    preset.fallback_booster = Some(LocalTemplate {
        path: relative,
        geometry: sample.geometry,
        stratagem: None,
    });
    validate_preset(name, preset)?;
    let saved = preset.clone();
    write_preset_file(path, &presets)?;
    Ok(saved)
}

pub fn resolve_template_path(presets_path: &Path, relative: &str) -> Result<PathBuf> {
    let relative_path = Path::new(relative);
    ensure!(
        !relative_path.is_absolute(),
        "local template path must be relative"
    );
    ensure!(
        relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "local template path contains an invalid component"
    );
    ensure!(
        relative_path.starts_with(LOCAL_TEMPLATES_DIR),
        "local template path must be inside {LOCAL_TEMPLATES_DIR}"
    );
    let base = presets_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(base.join(relative_path))
}

pub fn load_template_image(presets_path: &Path, relative: &str) -> Result<RgbaImage> {
    let path = resolve_template_path(presets_path, relative)?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("failed to read local template {}", path.display()))?;
    decode_rgba8(&bytes)
        .with_context(|| format!("failed to decode local template {}", path.display()))
}

pub fn load_template_sample(presets_path: &Path, template: &LocalTemplate) -> Result<ImageSample> {
    let image = load_template_image(presets_path, &template.path)?;
    ensure!(
        valid_sample_geometry(template.geometry)
            && template.geometry.center_x <= image.width() as f32
            && template.geometry.center_y <= image.height() as f32,
        "local template {} has invalid sample geometry",
        template.path
    );
    Ok(ImageSample {
        image,
        geometry: template.geometry,
    })
}

pub(crate) fn validate_preset(name: &str, preset: &Preset) -> Result<()> {
    if preset.stratagems.len() != 4 {
        bail!(
            "preset {name} must contain exactly 4 stratagems, got {}",
            preset.stratagems.len()
        );
    }

    for (index, template) in preset.stratagems.iter().enumerate() {
        if preset.stratagems[..index]
            .iter()
            .any(|previous| previous.path == template.path)
        {
            bail!(
                "preset {name} contains duplicate template {}",
                template.path
            );
        }
    }
    Ok(())
}

fn valid_sample_geometry(geometry: SampleGeometry) -> bool {
    geometry.center_x.is_finite()
        && geometry.center_x >= 0.0
        && geometry.center_y.is_finite()
        && geometry.center_y >= 0.0
        && geometry.physical_size.is_finite()
        && geometry.physical_size > 0.0
}

fn load_preset_file(path: &Path) -> Result<PresetFile> {
    let presets: PresetFile = parse_json_file(path)?;
    ensure!(
        presets.schema_version == PRESET_SCHEMA_VERSION,
        "unsupported preset schema version {}; expected {}. Delete the old preset data before continuing",
        presets.schema_version,
        PRESET_SCHEMA_VERSION
    );
    Ok(presets)
}

fn write_preset_file(path: &Path, presets: &PresetFile) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(presets)?;
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

fn validate_preset_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "preset name must not be empty");
    ensure!(
        name.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "preset name contains characters that cannot be used in a template directory: {name:?}"
    );
    Ok(())
}

fn save_template_image(presets_path: &Path, relative: &str, image: &RgbaImage) -> Result<()> {
    ensure!(
        image.width() > 0 && image.height() > 0,
        "cannot save an empty local template"
    );
    let destination = resolve_template_path(presets_path, relative)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    image
        .save(&destination)
        .with_context(|| format!("failed to save local template {}", destination.display()))
}

fn remove_template_image_if_exists(presets_path: &Path, relative: &str) -> Result<()> {
    let destination = resolve_template_path(presets_path, relative)?;
    match fs::remove_file(&destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to remove local template {}", destination.display())),
    }
}
