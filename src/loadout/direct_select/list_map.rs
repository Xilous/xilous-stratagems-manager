use std::collections::{BTreeSet, HashMap, HashSet};

use tracing::debug;

use crate::item::ItemKind;
use crate::vision::{RoiObservation, Slot, TemplateMatchCandidate};

use super::ScrollDirection;
use super::page_navigation::{PageSnapshot, PageTurnInput, SlotLuma};

#[cfg(feature = "diagnostics")]
#[path = "list_map_diagnostics.rs"]
mod diagnostics;

const PAGE_ALIGNMENT_MIN_MEAN_ZNCC: f32 = 0.80;
const PAGE_ALIGNMENT_MIN_MARGIN: f32 = 0.04;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GridPosition {
    row: i32,
    col: u32,
}

#[derive(Clone, Copy, Debug)]
struct MappedCandidate {
    position: GridPosition,
    score: f64,
    match_margin: f32,
    gate_quality: f32,
}

#[derive(Clone, Copy, Debug)]
struct CandidateEvidence {
    best: MappedCandidate,
    score_sum: f64,
    observations: u32,
}

impl CandidateEvidence {
    fn new(candidate: MappedCandidate) -> Self {
        Self {
            best: candidate,
            score_sum: candidate.score,
            observations: 1,
        }
    }

    fn record(&mut self, candidate: MappedCandidate) {
        self.score_sum += candidate.score;
        self.observations += 1;
        if candidate.score < self.best.score {
            self.best = candidate;
        }
    }

    fn candidate(self) -> MappedCandidate {
        MappedCandidate {
            score: self.score_sum / f64::from(self.observations),
            ..self.best
        }
    }
}

struct MappedSlotLuma {
    sample: SlotLuma,
    interior_depth: u32,
}

#[derive(Clone, Copy, Debug)]
struct PageAlignment {
    row_delta: i32,
    mean_zncc: f32,
    support: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PostClickPlacement {
    alignment: PageAlignment,
    pub(super) vertical_shift: f32,
}

impl PostClickPlacement {
    pub(super) const fn row_delta(self) -> i32 {
        self.alignment.row_delta
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum NavigationHint {
    Scroll(ScrollDirection),
    ExpectedVisible,
}

pub(super) struct ListMap {
    item_kind: ItemKind,
    row_base: i32,
    items: HashMap<String, GridPosition>,
    slot_luma: HashMap<GridPosition, MappedSlotLuma>,
    candidate_evidence: HashMap<String, HashMap<GridPosition, CandidateEvidence>>,
    active_fallbacks: HashMap<String, MappedCandidate>,
    selected: HashSet<GridPosition>,
}

impl ListMap {
    pub(super) fn new(page: &PageSnapshot, item_kind: ItemKind) -> Self {
        let mut map = Self {
            item_kind,
            row_base: 0,
            items: HashMap::new(),
            slot_luma: HashMap::new(),
            candidate_evidence: HashMap::new(),
            active_fallbacks: HashMap::new(),
            selected: HashSet::new(),
        };
        map.record_page(page);
        map
    }

    pub(super) fn advance(&mut self, current: &PageSnapshot, input: PageTurnInput) -> bool {
        let predicted = match input {
            PageTurnInput::Full(ScrollDirection::Down) => 0..=4,
            PageTurnInput::Full(ScrollDirection::Up) => -4..=0,
            PageTurnInput::Nudge(ScrollDirection::Down) => 0..=2,
            PageTurnInput::Nudge(ScrollDirection::Up) => -2..=0,
        };
        self.place_page(current, predicted, "predicted")
    }

    pub(super) fn locate_after_click(
        &self,
        before_slots: &[Slot],
        page: &PageSnapshot,
        clicked_slot: &Slot,
    ) -> Option<PostClickPlacement> {
        let clicked_position = GridPosition {
            row: self.row_base + clicked_slot.row as i32,
            col: clicked_slot.col,
        };
        let alignment = self.find_alignment(page, -2..=2, "post-click", Some(clicked_position))?;
        let vertical_shift = median_vertical_shift(
            before_slots,
            &page.roi.slots,
            alignment.row_delta,
            self.item_kind,
        )?;
        debug!(
            row_delta = alignment.row_delta,
            vertical_shift,
            support = alignment.support,
            "post-click page located in temporary list map"
        );
        Some(PostClickPlacement {
            alignment,
            vertical_shift,
        })
    }

    pub(super) fn commit_after_click(
        &mut self,
        page: &PageSnapshot,
        placement: PostClickPlacement,
    ) {
        self.commit_page(page, placement.alignment, "post-click");
    }

    fn place_page(
        &mut self,
        page: &PageSnapshot,
        predicted_deltas: impl IntoIterator<Item = i32>,
        context: &'static str,
    ) -> bool {
        let Some(alignment) = self.find_alignment(page, predicted_deltas, context, None) else {
            return false;
        };
        self.commit_page(page, alignment, context);
        true
    }

    fn find_alignment(
        &self,
        page: &PageSnapshot,
        predicted_deltas: impl IntoIterator<Item = i32>,
        context: &'static str,
        excluded: Option<GridPosition>,
    ) -> Option<PageAlignment> {
        let predicted = predicted_deltas.into_iter().collect::<Vec<_>>();
        self.evaluate_alignments(&page.slot_luma, excluded, predicted, context, "local")
            .or_else(|| {
                self.evaluate_alignments(
                    &page.slot_luma,
                    excluded,
                    alignment_deltas(&self.slot_luma, &page.slot_luma, self.row_base),
                    context,
                    "global",
                )
            })
    }

    fn evaluate_alignments(
        &self,
        current: &[SlotLuma],
        excluded: Option<GridPosition>,
        deltas: impl IntoIterator<Item = i32>,
        context: &'static str,
        search: &'static str,
    ) -> Option<PageAlignment> {
        let mut alignments = deltas
            .into_iter()
            .filter_map(|row_delta| {
                let similarities = current
                    .iter()
                    .filter_map(|slot| {
                        let position = GridPosition {
                            row: self.row_base + row_delta + slot.row as i32,
                            col: slot.col,
                        };
                        if self.selected.contains(&position) || excluded == Some(position) {
                            return None;
                        }
                        best_shifted_zncc(&self.slot_luma.get(&position)?.sample, slot)
                    })
                    .collect::<Vec<_>>();
                let support = similarities.len();
                (support > 0).then(|| PageAlignment {
                    row_delta,
                    mean_zncc: similarities.iter().sum::<f32>() / support as f32,
                    support,
                })
            })
            .collect::<Vec<_>>();
        alignments.sort_by(|left, right| {
            right
                .mean_zncc
                .total_cmp(&left.mean_zncc)
                .then_with(|| right.support.cmp(&left.support))
        });

        let best = *alignments.first()?;
        let runner_up = alignments
            .get(1)
            .map_or(-1.0, |candidate| candidate.mean_zncc);
        let margin = best.mean_zncc - runner_up;
        let accepted =
            best.mean_zncc >= PAGE_ALIGNMENT_MIN_MEAN_ZNCC && margin >= PAGE_ALIGNMENT_MIN_MARGIN;
        debug!(
            context,
            search,
            best_delta = best.row_delta,
            best_mean_zncc = best.mean_zncc,
            best_support = best.support,
            runner_up_mean_zncc = runner_up,
            margin,
            accepted,
            candidates = ?alignments,
            "slot-luma map alignment evaluated"
        );
        accepted.then_some(best)
    }

    fn commit_page(
        &mut self,
        page: &PageSnapshot,
        alignment: PageAlignment,
        context: &'static str,
    ) {
        self.row_base += alignment.row_delta;
        self.record_page(page);
        debug!(
            row_base = self.row_base,
            row_delta = alignment.row_delta,
            mean_zncc = alignment.mean_zncc,
            support = alignment.support,
            mapped_slots = self.slot_luma.len(),
            context,
            "temporary list map placed from global slot luma"
        );
    }

    pub(super) fn record_page(&mut self, page: &PageSnapshot) {
        self.record_current(&page.roi);
        self.record_slot_luma(&page.slot_luma);
        self.record_candidates(&page.match_candidates);
    }

    fn record_current(&mut self, page: &RoiObservation) {
        for slot in page
            .slots
            .iter()
            .filter(|slot| slot.kind.is_selectable_item_for(self.item_kind))
        {
            let Some(classification) = &slot.classification else {
                continue;
            };
            let position = GridPosition {
                row: self.row_base + slot.row as i32,
                col: slot.col,
            };
            if self.selected.contains(&position) {
                continue;
            }
            self.items.insert(classification.item_id.clone(), position);
        }
    }

    fn record_slot_luma(&mut self, slots: &[SlotLuma]) {
        let Some(min_row) = slots.iter().map(|slot| slot.row).min() else {
            return;
        };
        let max_row = slots.iter().map(|slot| slot.row).max().unwrap_or(min_row);

        for slot in slots {
            let position = GridPosition {
                row: self.row_base + slot.row as i32,
                col: slot.col,
            };
            if self.selected.contains(&position) {
                continue;
            }
            let interior_depth = (slot.row - min_row).min(max_row - slot.row);
            let entry = self
                .slot_luma
                .entry(position)
                .or_insert_with(|| MappedSlotLuma {
                    sample: slot.clone(),
                    interior_depth,
                });
            if interior_depth > entry.interior_depth {
                *entry = MappedSlotLuma {
                    sample: slot.clone(),
                    interior_depth,
                };
            }
        }
    }

    pub(super) fn mark_selected(&mut self, slot: &Slot) {
        let position = GridPosition {
            row: self.row_base + slot.row as i32,
            col: slot.col,
        };
        self.selected.insert(position);
        self.items.retain(|_, mapped| *mapped != position);
        self.candidate_evidence.retain(|_, candidates| {
            candidates.remove(&position);
            !candidates.is_empty()
        });
        self.active_fallbacks
            .retain(|_, candidate| candidate.position != position);
    }

    pub(super) fn is_selected_slot(&self, slot: &Slot) -> bool {
        self.selected.contains(&GridPosition {
            row: self.row_base + slot.row as i32,
            col: slot.col,
        })
    }

    pub(super) fn contains_item(&self, item_id: &str) -> bool {
        self.items.contains_key(item_id)
    }

    fn record_candidates(&mut self, candidates: &[TemplateMatchCandidate]) {
        for candidate in candidates {
            let mapped = MappedCandidate {
                position: GridPosition {
                    row: self.row_base + candidate.slot.row as i32,
                    col: candidate.slot.col,
                },
                score: candidate.score,
                match_margin: candidate.match_margin,
                gate_quality: candidate.gate_quality,
            };
            if self.selected.contains(&mapped.position) {
                continue;
            }
            self.candidate_evidence
                .entry(candidate.item_id.clone())
                .or_default()
                .entry(mapped.position)
                .and_modify(|evidence| evidence.record(mapped))
                .or_insert_with(|| CandidateEvidence::new(mapped));
        }
    }

    pub(super) fn activate_best_candidate(&mut self, item_id: &str) -> Option<f64> {
        let candidates = self.candidate_evidence.get(item_id)?;
        let (candidate, observations) = candidates
            .values()
            .filter(|evidence| !self.selected.contains(&evidence.best.position))
            .map(|evidence| (evidence.candidate(), evidence.observations))
            .min_by(|(left, _), (right, _)| left.score.total_cmp(&right.score))?;
        self.items.insert(item_id.to_string(), candidate.position);
        self.active_fallbacks.insert(item_id.to_string(), candidate);
        debug!(
            item_id,
            row = candidate.position.row,
            col = candidate.position.col,
            score = candidate.score,
            observations,
            candidate_positions = candidates.len(),
            "Top-1 fallback selected from accumulated slot evidence"
        );
        Some(candidate.score)
    }

    pub(super) fn visible_mapped_target(
        &self,
        item_id: &str,
        page: &RoiObservation,
    ) -> Option<TemplateMatchCandidate> {
        let position = *self.items.get(item_id)?;
        if self.selected.contains(&position) {
            return None;
        }
        let local_row = position.row - self.row_base;
        let slot = page.slots.iter().find(|slot| {
            slot.kind.is_selectable_item_for(self.item_kind)
                && slot.row as i32 == local_row
                && slot.col == position.col
        })?;
        let candidate = self.active_fallbacks.get(item_id).copied().or_else(|| {
            self.candidate_evidence
                .get(item_id)?
                .get(&position)
                .copied()
                .map(CandidateEvidence::candidate)
        });
        Some(TemplateMatchCandidate {
            item_id: item_id.to_string(),
            slot: slot.clone(),
            score: candidate.map_or(1.0, |candidate| candidate.score),
            match_margin: candidate.map_or(0.0, |candidate| candidate.match_margin),
            gate_quality: candidate.map_or(0.0, |candidate| candidate.gate_quality),
        })
    }

    pub(super) fn navigation_hint(&self, item_id: &str, page: &RoiObservation) -> NavigationHint {
        let Some(target) = self.items.get(item_id) else {
            return NavigationHint::Scroll(ScrollDirection::Down);
        };
        let Some((local_min, local_max)) = local_row_range(page, self.item_kind) else {
            return NavigationHint::ExpectedVisible;
        };
        let visible_min = self.row_base + local_min;
        let visible_max = self.row_base + local_max;

        if target.row < visible_min {
            NavigationHint::Scroll(ScrollDirection::Up)
        } else if target.row > visible_max {
            NavigationHint::Scroll(ScrollDirection::Down)
        } else {
            NavigationHint::ExpectedVisible
        }
    }
}

fn alignment_deltas(
    mapped: &HashMap<GridPosition, MappedSlotLuma>,
    current: &[SlotLuma],
    row_base: i32,
) -> Vec<i32> {
    mapped
        .keys()
        .flat_map(|position| {
            current
                .iter()
                .filter(move |slot| position.col == slot.col)
                .map(move |slot| position.row - row_base - slot.row as i32)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn median_vertical_shift(
    previous: &[Slot],
    current: &[Slot],
    row_delta: i32,
    item_kind: ItemKind,
) -> Option<f32> {
    let mut shifts = current
        .iter()
        .filter(|slot| slot.kind.is_selectable_item_for(item_kind))
        .filter_map(|current_slot| {
            let previous_row = current_slot.row as i32 + row_delta;
            let previous_slot = previous.iter().find(|previous_slot| {
                previous_slot.kind.is_selectable_item_for(item_kind)
                    && previous_slot.row as i32 == previous_row
                    && previous_slot.col == current_slot.col
            })?;
            Some(current_slot.center_f32().1 - previous_slot.center_f32().1)
        })
        .collect::<Vec<_>>();
    if shifts.is_empty() {
        return None;
    }
    shifts.sort_unstable_by(f32::total_cmp);
    let middle = shifts.len() / 2;
    Some(if shifts.len().is_multiple_of(2) {
        0.5 * (shifts[middle - 1] + shifts[middle])
    } else {
        shifts[middle]
    })
}

fn best_shifted_zncc(left: &SlotLuma, right: &SlotLuma) -> Option<f32> {
    (-1..=1)
        .filter_map(|dy| shifted_zncc(left, right, dy))
        .max_by(f32::total_cmp)
}

fn shifted_zncc(left: &SlotLuma, right: &SlotLuma, dy: i32) -> Option<f32> {
    let offset_x = (right.center_x - left.center_x).round() as i32;
    let offset_y = (right.center_y - left.center_y).round() as i32 + dy;
    let mut count = 0.0f64;
    let mut sum_left = 0.0f64;
    let mut sum_right = 0.0f64;
    let mut sum_left_sq = 0.0f64;
    let mut sum_right_sq = 0.0f64;
    let mut sum_product = 0.0f64;

    for left_y in 0..left.height as i32 {
        let right_y = left_y + offset_y;
        if !(0..right.height as i32).contains(&right_y) {
            continue;
        }
        for left_x in 0..left.width as i32 {
            let right_x = left_x + offset_x;
            if !(0..right.width as i32).contains(&right_x) {
                continue;
            }
            let left_value =
                left.pixels[left_y as usize * left.width as usize + left_x as usize] as f64;
            let right_value =
                right.pixels[right_y as usize * right.width as usize + right_x as usize] as f64;
            count += 1.0;
            sum_left += left_value;
            sum_right += right_value;
            sum_left_sq += left_value * left_value;
            sum_right_sq += right_value * right_value;
            sum_product += left_value * right_value;
        }
    }

    if count < 64.0 {
        return None;
    }
    let covariance = sum_product - sum_left * sum_right / count;
    let variance_left = sum_left_sq - sum_left * sum_left / count;
    let variance_right = sum_right_sq - sum_right * sum_right / count;
    let denominator = (variance_left * variance_right).sqrt();
    (denominator > 1e-8).then(|| (covariance / denominator).clamp(-1.0, 1.0) as f32)
}

fn local_row_range(page: &RoiObservation, item_kind: ItemKind) -> Option<(i32, i32)> {
    let mut rows = page
        .slots
        .iter()
        .filter(|slot| slot.kind.is_selectable_item_for(item_kind))
        .map(|slot| slot.row as i32);
    let first = rows.next()?;
    Some(rows.fold((first, first), |(min, max), row| {
        (min.min(row), max.max(row))
    }))
}
