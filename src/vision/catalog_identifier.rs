//! Identifies which catalog Stratagem a captured loadout slot shows.
//!
//! Reference icons come from the wiki SVGs, which use the same white/tint
//! palette as the in-game icons, so both sides go through the same semantic
//! extraction before the alignment-tolerant matcher compares them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use rayon::prelude::*;
use tracing::{debug, warn};

use crate::catalog::Catalog;
use crate::item::StratagemCategory;

use super::matcher::{
    MatchDomain, PreparedTemplate, SemanticImage, compare, prepare_template, render_to_raster,
};
use super::semantic_extractor::SemanticSource;
use super::{ImageSample, SampleGeometry};

/// Pixel size the reference SVGs are rasterized at before extraction.
pub const REFERENCE_RENDER_SIZE: u32 = 96;
const ENV_PHASES: [f32; 5] = [-0.45, -0.225, 0.0, 0.225, 0.45];
/// Zoom of the reference relative to the on-screen tile. Measured on real
/// captures: with the border masked, the wiki artwork matches the in-game
/// tile 1:1, and a single global zoom stops wrong candidates from picking a
/// zoom that happens to suit them.
const SCALE_CANDIDATES: [f32; 1] = [1.0];
/// Width of the border in the wiki artwork (14 of 256 px), plus anti-aliasing.
const FRAME_FRACTION: f32 = 0.065;
/// Loadout tile background in game; matches the extractor's fixed background.
const TILE_BACKGROUND: u8 = 44;
/// Best score at or below this is a plausible match (lower is better).
/// Real captures score 0.06 (sentries) to 0.66 (Guard Dog variants).
pub const ACCEPT_SCORE: f64 = 0.75;
/// The runner-up must trail the best by at least this fraction of the best
/// score. Families that share artwork (the Guard Dogs) separate by about 11%;
/// everything else by far more.
pub const ACCEPT_MARGIN_RELATIVE: f64 = 0.10;
/// ...and by at least this much in absolute terms.
pub const ACCEPT_MARGIN: f64 = 0.05;

struct Reference {
    id: String,
    category: StratagemCategory,
    semantic: SemanticImage,
    physical_size: f32,
}

/// Candidate geometry the references were prepared for; slots of one screen share it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GeometryKey {
    width: u32,
    height: u32,
    center_x: i32,
    center_y: i32,
    physical_size: i32,
    category: StratagemCategory,
}

impl GeometryKey {
    fn new(sample: &ImageSample, category: StratagemCategory) -> Self {
        Self {
            width: sample.image.width(),
            height: sample.image.height(),
            center_x: (sample.geometry.center_x * 100.0).round() as i32,
            center_y: (sample.geometry.center_y * 100.0).round() as i32,
            physical_size: (sample.geometry.physical_size * 100.0).round() as i32,
            category,
        }
    }
}

struct PreparedReference {
    index: usize,
    /// One prepared template per entry of `SCALE_CANDIDATES`.
    templates: Vec<PreparedTemplate>,
}

pub struct CatalogIdentifier {
    references: Vec<Reference>,
    cache: Mutex<HashMap<GeometryKey, Arc<Vec<PreparedReference>>>>,
}

#[derive(Debug, Clone)]
pub struct IdentificationCandidate {
    pub id: String,
    /// Raw matching error; lower is better.
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct Identification {
    pub category: StratagemCategory,
    /// Catalog id when the best candidate passed the acceptance gates.
    pub accepted: Option<String>,
    /// All compared candidates, best first.
    pub ranked: Vec<IdentificationCandidate>,
    /// Score gap between the best and second-best candidate.
    pub margin: f64,
}

impl Identification {
    pub fn best(&self) -> Option<&IdentificationCandidate> {
        self.ranked.first()
    }
}

impl CatalogIdentifier {
    pub fn from_catalog(catalog: &Catalog) -> Result<Self> {
        let started = Instant::now();
        let mut references = Vec::new();
        for entry in catalog
            .loadout_entries()
            .filter(|entry| entry.icon.is_some())
        {
            let category = entry
                .category()
                .expect("loadout entries always have a category");
            let prepared = catalog
                .render_icon(&entry.id, REFERENCE_RENDER_SIZE)
                .and_then(|image| Self::reference(entry.id.clone(), category, image));
            match prepared {
                Ok(reference) => references.push(reference),
                // One bad icon must not disable identification for everything else.
                Err(error) => warn!(
                    stratagem = %entry.name,
                    error = %format!("{error:#}"),
                    "reference icon skipped"
                ),
            }
        }
        ensure!(
            !references.is_empty(),
            "the catalog has no loadout stratagems with icons"
        );
        debug!(
            references = references.len(),
            elapsed = ?started.elapsed(),
            "catalog identifier ready"
        );
        Ok(Self {
            references,
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn reference(
        id: String,
        category: StratagemCategory,
        mut image: RgbaImage,
    ) -> Result<Reference> {
        mask_frame(&mut image);
        let sample = ImageSample {
            geometry: SampleGeometry {
                center_x: image.width() as f32 * 0.5,
                center_y: image.height() as f32 * 0.5,
                physical_size: image.width() as f32,
            },
            image,
        };
        let source = SemanticSource::prepare(&sample)?;
        let semantic = source.extract(category)?.image;
        Ok(Reference {
            id,
            category,
            semantic,
            physical_size: sample.geometry.physical_size,
        })
    }

    pub fn len(&self) -> usize {
        self.references.len()
    }

    /// Identifies the stratagem shown in `sample`. When `allowed` is given,
    /// only those catalog ids are considered (e.g. the four of a known preset).
    pub fn identify(
        &self,
        sample: &ImageSample,
        allowed: Option<&[String]>,
    ) -> Result<Identification> {
        let started = Instant::now();
        let source = SemanticSource::prepare(sample)?;
        let category = source.infer_category()?;
        let extraction = source.extract(category)?;
        let candidate_physical_size = sample.geometry.physical_size;
        let prepared = self.prepared_for(sample, category)?;

        let mut ranked = prepared
            .par_iter()
            .filter(|reference| {
                allowed.is_none_or(|ids| {
                    ids.iter()
                        .any(|id| *id == self.references[reference.index].id)
                })
            })
            .map(|reference| {
                let mut best = f64::INFINITY;
                for template in &reference.templates {
                    let result = compare(template, &extraction.image, candidate_physical_size)?;
                    best = best.min(result.score);
                }
                Ok(IdentificationCandidate {
                    id: self.references[reference.index].id.clone(),
                    score: best,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ranked.sort_by(|left, right| left.score.total_cmp(&right.score));

        let margin = match (ranked.first(), ranked.get(1)) {
            (Some(best), Some(second)) => second.score - best.score,
            (Some(_), None) => f64::INFINITY,
            (None, _) => 0.0,
        };
        let accepted = ranked
            .first()
            .filter(|best| {
                best.score <= ACCEPT_SCORE
                    && margin >= ACCEPT_MARGIN
                    && margin >= ACCEPT_MARGIN_RELATIVE * best.score
            })
            .map(|best| best.id.clone());

        debug!(
            category = category.label(),
            candidates = ranked.len(),
            best = ranked.first().map(|candidate| candidate.id.as_str()),
            best_score = ranked.first().map(|candidate| candidate.score),
            margin,
            accepted = accepted.is_some(),
            elapsed = ?started.elapsed(),
            "catalog identification"
        );
        Ok(Identification {
            category,
            accepted,
            ranked,
            margin,
        })
    }

    fn prepared_for(
        &self,
        sample: &ImageSample,
        category: StratagemCategory,
    ) -> Result<Arc<Vec<PreparedReference>>> {
        let key = GeometryKey::new(sample, category);
        if let Some(hit) = self
            .cache
            .lock()
            .expect("identifier cache poisoned")
            .get(&key)
        {
            return Ok(hit.clone());
        }

        let started = Instant::now();
        let target_size = (
            sample.image.width() as usize,
            sample.image.height() as usize,
        );
        let target_center = (sample.geometry.center_x, sample.geometry.center_y);
        let candidate_physical_size = sample.geometry.physical_size;
        let prepared = self
            .references
            .par_iter()
            .enumerate()
            .filter(|(_, reference)| reference.category == category)
            .map(|(index, reference)| {
                let templates = SCALE_CANDIDATES
                    .iter()
                    .map(|scale| {
                        let physical_size = reference.physical_size / scale;
                        let mut env_candidates = Vec::with_capacity(25);
                        for phase_y in ENV_PHASES {
                            for phase_x in ENV_PHASES {
                                env_candidates.push(render_to_raster(
                                    &reference.semantic,
                                    physical_size,
                                    target_size,
                                    target_center,
                                    candidate_physical_size,
                                    (phase_x, phase_y),
                                ));
                            }
                        }
                        prepare_template(
                            &reference.semantic,
                            physical_size,
                            &env_candidates,
                            candidate_physical_size,
                            MatchDomain::Full,
                            true,
                        )
                    })
                    .collect::<Result<Vec<_>>>()
                    .with_context(|| format!("failed to prepare reference {}", reference.id))?;
                Ok(PreparedReference { index, templates })
            })
            .collect::<Result<Vec<_>>>()?;
        debug!(
            target: "xilous_stratagems_manager::perf",
            category = category.label(),
            references = prepared.len(),
            candidate_physical_size,
            elapsed = ?started.elapsed(),
            "catalog references prepared"
        );

        let prepared = Arc::new(prepared);
        self.cache
            .lock()
            .expect("identifier cache poisoned")
            .insert(key, prepared.clone());
        Ok(prepared)
    }
}

/// The wiki artwork draws a colored border around the icon that the in-game
/// loadout tile does not have; paint it with the tile background so it cannot
/// contribute to the match.
fn mask_frame(image: &mut RgbaImage) {
    let width = image.width();
    let height = image.height();
    let band_x = (width as f32 * FRAME_FRACTION).ceil() as u32;
    let band_y = (height as f32 * FRAME_FRACTION).ceil() as u32;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        if x < band_x || y < band_y || x >= width - band_x || y >= height - band_y {
            pixel.0 = [TILE_BACKGROUND, TILE_BACKGROUND, TILE_BACKGROUND, 255];
        }
    }
}

#[cfg(test)]
impl CatalogIdentifier {
    /// Scores `sample` against every reference of its category at one zoom
    /// level. Returns the score of `expected_id` and the best other candidate.
    fn sweep_scores(
        &self,
        sample: &ImageSample,
        expected_id: &str,
        scale: f32,
    ) -> Result<(f64, String, f64)> {
        let source = SemanticSource::prepare(sample)?;
        let category = source.infer_category()?;
        let extraction = source.extract(category)?;
        let target_size = (
            sample.image.width() as usize,
            sample.image.height() as usize,
        );
        let target_center = (sample.geometry.center_x, sample.geometry.center_y);
        let candidate_physical_size = sample.geometry.physical_size;
        let mut expected = f64::INFINITY;
        let mut best_other = ("".to_string(), f64::INFINITY);
        for reference in self.references.iter().filter(|r| r.category == category) {
            let physical_size = reference.physical_size / scale;
            let mut env = Vec::with_capacity(25);
            for phase_y in ENV_PHASES {
                for phase_x in ENV_PHASES {
                    env.push(render_to_raster(
                        &reference.semantic,
                        physical_size,
                        target_size,
                        target_center,
                        candidate_physical_size,
                        (phase_x, phase_y),
                    ));
                }
            }
            let template = prepare_template(
                &reference.semantic,
                physical_size,
                &env,
                candidate_physical_size,
                MatchDomain::Full,
                true,
            )?;
            let score = compare(&template, &extraction.image, candidate_physical_size)?.score;
            if reference.id == expected_id {
                expected = score;
            } else if score < best_other.1 {
                best_other = (reference.id.clone(), score);
            }
        }
        Ok((expected, best_other.0, best_other.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loadout-screen crops captured at 1080p by the tool itself, committed as
    /// fixtures. Override with XSM_TEMPLATE_DIR / XSM_TEMPLATE_IDS to test
    /// other captures (e.g. `data/local_templates/preset_2`).
    fn real_capture_samples() -> Vec<(String, ImageSample)> {
        let dir = std::env::var("XSM_TEMPLATE_DIR")
            .unwrap_or_else(|_| "tests/fixtures/loadout-1080p".to_string());
        let ids = std::env::var("XSM_TEMPLATE_IDS").unwrap_or_else(|_| {
            "a-mg-43-machine-gun-sentry,a-g-16-gatling-sentry,las-99-quasar-cannon,ax-tx-13-dog-breath"
                .to_string()
        });
        let mut samples = Vec::new();
        for (index, id) in ids.split(',').enumerate() {
            let path = format!("{dir}/stratagem-{}.png", index + 1);
            let Ok(bytes) = std::fs::read(&path) else {
                eprintln!("skipping {path}: not found");
                continue;
            };
            let image = crate::assets::decode_rgba8(&bytes).expect("template png");
            samples.push((
                id.to_string(),
                ImageSample {
                    geometry: SampleGeometry {
                        center_x: image.width() as f32 * 0.5,
                        center_y: image.height() as f32 * 0.5,
                        physical_size: 69.975,
                    },
                    image,
                },
            ));
        }
        samples
    }

    /// Real loadout-screen captures must be accepted through the public path.
    #[test]
    fn real_captures_are_identified() {
        let samples = real_capture_samples();
        if samples.is_empty() {
            return;
        }
        let catalog = Catalog::load().expect("catalog");
        let identifier = CatalogIdentifier::from_catalog(&catalog).expect("identifier");
        for (id, sample) in &samples {
            let identification = identifier.identify(sample, None).expect("identify");
            let top = identification
                .ranked
                .iter()
                .take(4)
                .map(|candidate| format!("{}={:.3}", candidate.id, candidate.score))
                .collect::<Vec<_>>()
                .join("  ");
            eprintln!(
                "{id}: accepted={:?} margin={:.3}  top: {top}",
                identification.accepted, identification.margin
            );
            assert_eq!(identification.accepted.as_deref(), Some(id.as_str()));
        }
    }

    /// Writes the border-masked reference render of XSM_DUMP_ID next to the
    /// fixtures, to compare wiki artwork with a capture by eye.
    #[test]
    #[ignore = "diagnostic; set XSM_DUMP_ID"]
    fn dump_reference_icon() {
        let Ok(id) = std::env::var("XSM_DUMP_ID") else {
            return;
        };
        let catalog = Catalog::load().expect("catalog");
        let mut image = catalog.render_icon(&id, 70).expect("render");
        mask_frame(&mut image);
        let path = format!("target/reference-{id}.png");
        image.save(&path).expect("save");
        eprintln!("wrote {path}");
    }

    /// Prints how real loadout-screen captures score against their catalog
    /// entries across zoom levels. Set XSM_TEMPLATE_DIR to a preset template
    /// directory and XSM_TEMPLATE_IDS to the four expected ids (comma-separated).
    #[test]
    #[ignore = "diagnostic; run with --nocapture"]
    fn real_capture_scale_sweep() {
        let catalog = Catalog::load().expect("catalog");
        let identifier = CatalogIdentifier::from_catalog(&catalog).expect("identifier");
        let samples = real_capture_samples();
        let scales = std::env::var("XSM_SCALES").unwrap_or_else(|_| {
            "0.95,1.00,1.025,1.05,1.075,1.10,1.125,1.15,1.175,1.20,1.25".to_string()
        });
        for scale in scales
            .split(',')
            .map(|s| s.trim().parse::<f32>().expect("scale"))
        {
            let mut total = 0.0;
            let mut min_margin = f64::INFINITY;
            let mut lines = Vec::new();
            for (id, sample) in &samples {
                let (expected, other_id, other) =
                    identifier.sweep_scores(sample, id, scale).expect("sweep");
                total += expected;
                min_margin = min_margin.min(other - expected);
                lines.push(format!(
                    "    {id:<28} true={expected:.3}  other={other:.3} ({other_id})  margin={:+.3}",
                    other - expected
                ));
            }
            eprintln!(
                "scale {scale:.3}: mean true={:.3}  min margin={min_margin:+.3}",
                total / samples.len().max(1) as f64
            );
            for line in lines {
                eprintln!("{line}");
            }
        }
    }

    /// Simulates an on-screen slot: the reference rendered smaller, dimmed, and
    /// slightly off-center, then identified against the whole catalog.
    #[test]
    #[ignore = "slow in debug builds; run with: cargo test --release -- --ignored"]
    fn references_identify_themselves() {
        let catalog = Catalog::load().expect("catalog");
        let identifier = CatalogIdentifier::from_catalog(&catalog).expect("identifier");
        let mut failures = Vec::new();
        let mut checked = 0;
        for entry in catalog.loadout_entries().step_by(4) {
            let mut image = catalog.render_icon(&entry.id, 70).expect("render");
            // In-game tiles have no border; simulate that like the references do.
            mask_frame(&mut image);
            for pixel in image.pixels_mut() {
                for channel in &mut pixel.0[..3] {
                    *channel = (*channel as f32 * 0.88) as u8;
                }
            }
            let sample = ImageSample {
                geometry: SampleGeometry {
                    center_x: 35.4,
                    center_y: 34.7,
                    physical_size: 70.0,
                },
                image,
            };
            checked += 1;
            let identification = identifier.identify(&sample, None).expect("identify");
            if identification.accepted.as_deref() != Some(entry.id.as_str()) {
                failures.push(format!(
                    "{} -> {:?} (score {:?}, margin {:.3})",
                    entry.id,
                    identification.accepted,
                    identification.best().map(|best| best.score),
                    identification.margin
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {checked} references misidentified:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
