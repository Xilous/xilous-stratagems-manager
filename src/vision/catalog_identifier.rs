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
/// The wiki artwork and the on-screen icon do not share an exact scale, so each
/// reference is compared at a few zoom levels and the best one counts.
const SCALE_CANDIDATES: [f32; 3] = [0.92, 1.0, 1.08];
/// Best score at or below this is a plausible match (lower is better).
pub const ACCEPT_SCORE: f64 = 0.60;
/// The runner-up must trail by at least this much for an unambiguous match.
pub const ACCEPT_MARGIN: f64 = 0.03;

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
        for entry in catalog.loadout_entries().filter(|entry| entry.icon.is_some()) {
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

    fn reference(id: String, category: StratagemCategory, image: RgbaImage) -> Result<Reference> {
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
            .filter(|best| best.score <= ACCEPT_SCORE && margin >= ACCEPT_MARGIN)
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
        if let Some(hit) = self.cache.lock().expect("identifier cache poisoned").get(&key) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            for pixel in image.pixels_mut() {
                for channel in &mut pixel.0[..3] {
                    *channel = (*channel as f32 * 0.88) as u8;
                }
            }
            let sample = ImageSample {
                geometry: SampleGeometry {
                    center_x: 35.6,
                    center_y: 34.5,
                    physical_size: 66.0,
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
