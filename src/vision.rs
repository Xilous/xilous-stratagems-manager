mod booster;
mod color;
#[cfg(feature = "diagnostics")]
mod diagnostics;
mod geometry;
#[cfg(feature = "diagnostics")]
mod home_tone_diagnostics;
mod matcher;
mod recognizer;
mod semantic_extractor;
mod template_classifier;

use anyhow::{Result, bail};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::image_rect::ImageRect;
use crate::item::ItemKind;

pub(crate) use booster::crop_sample as crop_booster_sample;
pub use color::{icon_likeness, luma601_u8};
#[cfg(feature = "diagnostics")]
pub use diagnostics::init as init_diagnostics;
#[cfg(feature = "diagnostics")]
pub use diagnostics::save_fallback_slot;
#[cfg(feature = "diagnostics")]
pub use diagnostics::save_list_map_image;
#[cfg(feature = "diagnostics")]
pub(crate) use home_tone_diagnostics::log_home_tone;
pub use recognizer::{RecognizerRuntime, RecognizerSession};
pub(crate) use semantic_extractor::slot_core_rect;
pub use semantic_extractor::{TEMPLATE_PHYSICAL_SIZE_LOGICAL, crop_slot_sample};
pub use template_classifier::{TemplateClassifier, TemplateMatchCandidate};

pub const ROI_REFERENCE_H: u32 = 624;

const LIST_COLS: [i32; 4] = [58, 143, 228, 313];
const HOME_COLS: [i32; 4] = [8, 93, 178, 263];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Stratagem,
    StratagemEmpty,
    Booster,
    /// Special "no booster" list cell; occupies the grid but has no item template.
    NoBoosterOption,
    /// Filled booster slot on the loadout home screen.
    HomeBooster,
    /// Empty booster slot on the loadout home screen.
    HomeBoosterEmpty,
}

impl SlotKind {
    pub const fn is_selectable_item_for(self, item_kind: ItemKind) -> bool {
        matches!(
            (item_kind, self),
            (ItemKind::Stratagem, Self::Stratagem) | (ItemKind::Booster, Self::Booster)
        )
    }

    pub const fn is_home_booster(self) -> bool {
        matches!(self, Self::HomeBooster | Self::HomeBoosterEmpty)
    }
}

/// Page-level layout expected by the detector and attached to each observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotLayout {
    Home,
    List(ItemKind),
}

impl SlotLayout {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::List(ItemKind::Stratagem) => "stratagem_list",
            Self::List(ItemKind::Booster) => "booster_list",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Calibration {
    pub reference: ReferenceSize,
    pub roi_ref: ImageRect,
    pub scale_axis: ScaleAxis,
    #[serde(default)]
    pub anchor: RoiAnchor,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ReferenceSize {
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleAxis {
    Width,
    Height,
    Fit,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoiAnchor {
    #[default]
    TopLeft,
    TopCenter,
    Center,
}

#[derive(Debug)]
pub struct RoiObservation {
    pub image: RgbaImage,
    pub layout: SlotLayout,
    pub slots: Vec<Slot>,
}

#[derive(Debug, Clone, Copy)]
pub struct RoiGeometry {
    pub scale: f64,
    pub logical_origin_x: f64,
    pub logical_origin_y: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedRoi {
    pub rect: ImageRect,
    pub geometry: RoiGeometry,
}

#[derive(Debug, Clone)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    center_x: f32,
    center_y: f32,
    pub row: u32,
    pub col: u32,
    pub kind: SlotKind,
    pub classification: Option<Classification>,
}

impl Slot {
    pub fn center(&self) -> (u32, u32) {
        (self.center_x.round() as u32, self.center_y.round() as u32)
    }

    pub fn center_f32(&self) -> (f32, f32) {
        (self.center_x, self.center_y)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct SampleGeometry {
    pub center_x: f32,
    pub center_y: f32,
    pub physical_size: f32,
}

pub struct ImageSample {
    pub image: RgbaImage,
    pub geometry: SampleGeometry,
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub item_id: String,
    /// Raw fused template similarity used by the acceptance gate.
    pub match_score: f32,
    /// Difference between the best and second-best template scores.
    pub match_margin: f32,
    /// Normalized surplus above the weaker acceptance gate; not a probability.
    pub gate_quality: f32,
}

pub fn resolve_calibration_roi_for_size(
    image_w: u32,
    image_h: u32,
    calibration: &Calibration,
) -> Result<ResolvedRoi> {
    let reference = calibration.reference;
    let rect = calibration.roi_ref;
    if reference.w == 0 || reference.h == 0 {
        bail!(
            "reference width and height must be greater than zero, got {}x{}",
            reference.w,
            reference.h
        );
    }
    if rect.w == 0 || rect.h == 0 {
        bail!("roi_ref width and height must be greater than zero");
    }
    if rect.x + rect.w > reference.w || rect.y + rect.h > reference.h {
        bail!(
            "roi_ref ({},{},{},{}) is outside reference {}x{}",
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            reference.w,
            reference.h
        );
    }

    let scale = match calibration.scale_axis {
        ScaleAxis::Width => image_w as f64 / reference.w as f64,
        ScaleAxis::Height => image_h as f64 / reference.h as f64,
        ScaleAxis::Fit => {
            (image_w as f64 / reference.w as f64).min(image_h as f64 / reference.h as f64)
        }
    };

    let scaled_reference_w = reference.w as f64 * scale;
    let scaled_reference_h = reference.h as f64 * scale;
    let (offset_x, offset_y) = match calibration.anchor {
        RoiAnchor::TopLeft => (0.0, 0.0),
        RoiAnchor::TopCenter => ((image_w as f64 - scaled_reference_w) * 0.5, 0.0),
        RoiAnchor::Center => (
            (image_w as f64 - scaled_reference_w) * 0.5,
            (image_h as f64 - scaled_reference_h) * 0.5,
        ),
    };

    let left = offset_x + rect.x as f64 * scale;
    let top = offset_y + rect.y as f64 * scale;
    let right = offset_x + (rect.x + rect.w) as f64 * scale;
    let bottom = offset_y + (rect.y + rect.h) as f64 * scale;
    if left < 0.0 || top < 0.0 || right > image_w as f64 || bottom > image_h as f64 {
        bail!(
            "scaled ROI ({:.1},{:.1},{:.1},{:.1}) is outside image {}x{}",
            left,
            top,
            right - left,
            bottom - top,
            image_w,
            image_h
        );
    }

    // Keep the complete continuous Page ROI inside the integer capture rect.
    // The fractional offset is preserved in RoiGeometry for native sampling.
    let x = left.floor() as u32;
    let y = top.floor() as u32;
    let right = right.ceil() as u32;
    let bottom = bottom.ceil() as u32;

    Ok(ResolvedRoi {
        rect: ImageRect {
            x,
            y,
            w: right.saturating_sub(x).max(1),
            h: bottom.saturating_sub(y).max(1),
        },
        geometry: RoiGeometry {
            scale,
            logical_origin_x: left - x as f64,
            logical_origin_y: top - y as f64,
        },
    })
}

pub fn detect_slot_layout(
    image: RgbaImage,
    geometry: RoiGeometry,
    expected_layout: SlotLayout,
) -> Result<RoiObservation> {
    if image.width() == 0 || image.height() == 0 {
        bail!("cannot detect slots in an empty ROI image");
    }
    let slots = geometry::detect(&image, geometry, expected_layout)?;
    Ok(RoiObservation {
        image,
        layout: expected_layout,
        slots,
    })
}
