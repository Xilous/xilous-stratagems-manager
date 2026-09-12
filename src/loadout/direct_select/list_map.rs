use std::collections::{HashMap, HashSet};

use tracing::debug;

use crate::item::ItemKind;
use crate::vision::{ItemAvailability, RoiObservation, Slot, TemplateMatchCandidate};

use super::ScrollDirection;
use super::page_navigation::{PageSnapshot, PageTurnInput, SlotLuma};

#[cfg(feature = "diagnostics")]
#[path = "list_map_diagnostics.rs"]
mod diagnostics;

const LANDMARK_MIN_ZNCC: f32 = 0.70;
const PAGE_ALIGNMENT_MIN_MARGIN: f32 = 0.04;
const POSITION_TOLERANCE_PX: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SlotId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalPosition {
    row: u32,
    col: u32,
}

impl LocalPosition {
    const fn of(slot: &Slot) -> Self {
        Self {
            row: slot.row,
            col: slot.col,
        }
    }

    const fn of_luma(slot: &SlotLuma) -> Self {
        Self {
            row: slot.row,
            col: slot.col,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MappedCandidate {
    slot_id: SlotId,
    score: f64,
    match_margin: f32,
    gate_quality: f32,
    availability: ItemAvailability,
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

struct MapSlot {
    col: u32,
    content_y: f32,
    sample: SlotLuma,
    edge_clearance: f32,
}

#[derive(Clone, Copy, Debug)]
struct PlacedSlot {
    local: LocalPosition,
    id: SlotId,
}

#[derive(Clone, Debug)]
struct PagePlacement {
    offset_y: f32,
    mean_zncc: f32,
    support: usize,
    slots: Vec<PlacedSlot>,
}

#[derive(Clone, Copy, Debug)]
struct PairMatch {
    page_index: usize,
    offset_y: f32,
    zncc: f32,
}

#[derive(Clone, Copy, Debug)]
struct AlignmentCandidate {
    offset_y: f32,
    mean_zncc: f32,
    support: usize,
}

#[derive(Clone, Debug)]
pub(super) struct PostClickPlacement {
    page: PagePlacement,
    pub(super) vertical_shift: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum NavigationHint {
    Scroll(ScrollDirection),
    ExpectedVisible,
}

pub(super) struct ListMap {
    item_kind: ItemKind,
    slots: Vec<MapSlot>,
    current: PagePlacement,
    items: HashMap<String, SlotId>,
    candidate_evidence: HashMap<String, HashMap<SlotId, CandidateEvidence>>,
    active_fallbacks: HashMap<String, MappedCandidate>,
    selected: HashSet<SlotId>,
}

impl ListMap {
    pub(super) fn new(page: &PageSnapshot, item_kind: ItemKind) -> Self {
        let mut map = Self {
            item_kind,
            slots: Vec::new(),
            current: PagePlacement {
                offset_y: 0.0,
                mean_zncc: 1.0,
                support: page.slot_luma.len(),
                slots: Vec::new(),
            },
            items: HashMap::new(),
            candidate_evidence: HashMap::new(),
            active_fallbacks: HashMap::new(),
            selected: HashSet::new(),
        };
        let initial = map.placement_at_offset(page, 0.0, 1.0, page.slot_luma.len());
        map.commit_page(page, initial, "initial");
        map
    }

    pub(super) fn advance(&mut self, page: &PageSnapshot, input: PageTurnInput) -> bool {
        let Some(placement) = self.find_placement(page, Some(input.direction()), None, "page-turn")
        else {
            return false;
        };
        self.commit_page(page, placement, "page-turn");
        true
    }

    pub(super) fn locate_after_click(
        &self,
        page: &PageSnapshot,
        clicked_slot: &Slot,
    ) -> Option<PostClickPlacement> {
        let clicked_id = self.current_slot_id(clicked_slot)?;
        let placement = self.find_placement(page, None, Some(clicked_id), "post-click")?;
        let vertical_shift = self.current.offset_y - placement.offset_y;
        debug!(
            offset_y = placement.offset_y,
            vertical_shift,
            support = placement.support,
            mean_zncc = placement.mean_zncc,
            "post-click page located in temporary list map"
        );
        Some(PostClickPlacement {
            page: placement,
            vertical_shift,
        })
    }

    pub(super) fn slot_after_placement(
        &self,
        placement: &PostClickPlacement,
        page: &RoiObservation,
        previous_slot: &Slot,
    ) -> Option<Slot> {
        let id = self.current_slot_id(previous_slot)?;
        let local = placement
            .page
            .slots
            .iter()
            .find(|placed| placed.id == id)?
            .local;
        page.slots
            .iter()
            .find(|slot| LocalPosition::of(slot) == local)
            .cloned()
    }

    pub(super) fn commit_after_click(
        &mut self,
        page: &PageSnapshot,
        placement: PostClickPlacement,
    ) {
        self.commit_page(page, placement.page, "post-click");
    }

    fn find_placement(
        &self,
        page: &PageSnapshot,
        direction: Option<ScrollDirection>,
        excluded: Option<SlotId>,
        context: &'static str,
    ) -> Option<PagePlacement> {
        let pairs = page
            .slot_luma
            .iter()
            .enumerate()
            .filter_map(|(page_index, current)| {
                self.slots
                    .iter()
                    .enumerate()
                    .filter(move |(index, mapped)| {
                        let id = SlotId(*index);
                        mapped.col == current.col
                            && !self.selected.contains(&id)
                            && excluded != Some(id)
                    })
                    .filter_map(move |(_, mapped)| {
                        let zncc = best_shifted_zncc(&mapped.sample, current)?;
                        Some(PairMatch {
                            page_index,
                            offset_y: mapped.content_y - current.page_y,
                            zncc,
                        })
                    })
                    .max_by(|left, right| left.zncc.total_cmp(&right.zncc))
                    .filter(|pair| pair.zncc >= LANDMARK_MIN_ZNCC)
            })
            .collect::<Vec<_>>();

        let mut candidates = offset_hypotheses(&pairs)
            .into_iter()
            .filter(|&offset_y| self.direction_allows(offset_y, direction))
            .filter_map(|offset_y| evaluate_alignment(&pairs, offset_y))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .support
                .cmp(&left.support)
                .then_with(|| right.mean_zncc.total_cmp(&left.mean_zncc))
        });

        let best = *candidates.first()?;
        let runner_up = candidates.get(1).copied();
        let margin = runner_up.map_or(1.0, |runner| best.mean_zncc - runner.mean_zncc);
        let accepted = runner_up.is_none_or(|runner| {
            best.support > runner.support || margin >= PAGE_ALIGNMENT_MIN_MARGIN
        });
        debug!(
            context,
            current_offset_y = self.current.offset_y,
            best_offset_y = best.offset_y,
            vertical_shift = self.current.offset_y - best.offset_y,
            best_mean_zncc = best.mean_zncc,
            best_support = best.support,
            runner_up_mean_zncc = runner_up.map(|candidate| candidate.mean_zncc),
            runner_up_support = runner_up.map(|candidate| candidate.support),
            margin,
            accepted,
            candidates = ?candidates.iter().take(8).collect::<Vec<_>>(),
            "continuous slot-luma map alignment evaluated"
        );
        accepted
            .then(|| self.placement_at_offset(page, best.offset_y, best.mean_zncc, best.support))
    }

    fn direction_allows(&self, offset_y: f32, direction: Option<ScrollDirection>) -> bool {
        let movement = offset_y - self.current.offset_y;
        match direction {
            Some(ScrollDirection::Down) => movement >= -POSITION_TOLERANCE_PX,
            Some(ScrollDirection::Up) => movement <= POSITION_TOLERANCE_PX,
            None => true,
        }
    }

    fn placement_at_offset(
        &self,
        page: &PageSnapshot,
        offset_y: f32,
        mean_zncc: f32,
        support: usize,
    ) -> PagePlacement {
        let slots = page
            .slot_luma
            .iter()
            .filter_map(|current| {
                let content_y = current.page_y + offset_y;
                let (index, _) = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(_, mapped)| mapped.col == current.col)
                    .map(|(index, mapped)| (index, (mapped.content_y - content_y).abs()))
                    .filter(|(_, distance)| *distance <= POSITION_TOLERANCE_PX)
                    .min_by(|left, right| left.1.total_cmp(&right.1))?;
                Some(PlacedSlot {
                    local: LocalPosition::of_luma(current),
                    id: SlotId(index),
                })
            })
            .collect();
        PagePlacement {
            offset_y,
            mean_zncc,
            support,
            slots,
        }
    }

    fn commit_page(
        &mut self,
        page: &PageSnapshot,
        mut placement: PagePlacement,
        context: &'static str,
    ) {
        let min_y = page
            .slot_luma
            .iter()
            .map(|slot| slot.page_y)
            .min_by(f32::total_cmp)
            .unwrap_or(0.0);
        let max_y = page
            .slot_luma
            .iter()
            .map(|slot| slot.page_y)
            .max_by(f32::total_cmp)
            .unwrap_or(0.0);

        for current in &page.slot_luma {
            let local = LocalPosition::of_luma(current);
            let edge_clearance = (current.page_y - min_y).min(max_y - current.page_y);
            if let Some(placed) = placement.slots.iter().find(|placed| placed.local == local) {
                let mapped = &mut self.slots[placed.id.0];
                if !self.selected.contains(&placed.id) && edge_clearance > mapped.edge_clearance {
                    mapped.sample = current.clone();
                    mapped.edge_clearance = edge_clearance;
                }
                continue;
            }

            let id = SlotId(self.slots.len());
            self.slots.push(MapSlot {
                col: current.col,
                content_y: current.page_y + placement.offset_y,
                sample: current.clone(),
                edge_clearance,
            });
            placement.slots.push(PlacedSlot { local, id });
        }

        self.current = placement;
        self.record_current(&page.roi);
        self.record_candidates(&page.match_candidates);
        debug!(
            context,
            offset_y = self.current.offset_y,
            mean_zncc = self.current.mean_zncc,
            support = self.current.support,
            mapped_slots = self.slots.len(),
            "temporary list map placed in continuous list coordinates"
        );
    }

    pub(super) fn record_page(&mut self, page: &PageSnapshot) {
        let placement = self.placement_at_offset(
            page,
            self.current.offset_y,
            self.current.mean_zncc,
            self.current.support,
        );
        self.commit_page(page, placement, "same-viewport");
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
            let Some(id) = self.current_slot_id(slot) else {
                continue;
            };
            if !self.selected.contains(&id) {
                self.items.insert(classification.item_id.clone(), id);
            }
        }
    }

    pub(super) fn mark_selected(&mut self, slot: &Slot) {
        let Some(id) = self.current_slot_id(slot) else {
            return;
        };
        self.selected.insert(id);
        self.items.retain(|_, mapped| *mapped != id);
        self.candidate_evidence.retain(|_, candidates| {
            candidates.remove(&id);
            !candidates.is_empty()
        });
        self.active_fallbacks
            .retain(|_, candidate| candidate.slot_id != id);
    }

    pub(super) fn is_selected_slot(&self, slot: &Slot) -> bool {
        self.current_slot_id(slot)
            .is_some_and(|id| self.selected.contains(&id))
    }

    pub(super) fn contains_item(&self, item_id: &str) -> bool {
        self.items.contains_key(item_id)
    }

    fn record_candidates(&mut self, candidates: &[TemplateMatchCandidate]) {
        for candidate in candidates {
            let Some(slot_id) = self.current_slot_id(&candidate.slot) else {
                continue;
            };
            let mapped = MappedCandidate {
                slot_id,
                score: candidate.score,
                match_margin: candidate.match_margin,
                gate_quality: candidate.gate_quality,
                availability: candidate.availability,
            };
            if self.selected.contains(&slot_id) {
                continue;
            }
            self.candidate_evidence
                .entry(candidate.item_id.clone())
                .or_default()
                .entry(slot_id)
                .and_modify(|evidence| evidence.record(mapped))
                .or_insert_with(|| CandidateEvidence::new(mapped));
        }
    }

    pub(super) fn activate_best_candidate(&mut self, item_id: &str) -> Option<f64> {
        let candidates = self.candidate_evidence.get(item_id)?;
        let (candidate, observations) = candidates
            .values()
            .filter(|evidence| !self.selected.contains(&evidence.best.slot_id))
            .map(|evidence| (evidence.candidate(), evidence.observations))
            .min_by(|(left, _), (right, _)| left.score.total_cmp(&right.score))?;
        self.items.insert(item_id.to_string(), candidate.slot_id);
        self.active_fallbacks.insert(item_id.to_string(), candidate);
        let mapped = &self.slots[candidate.slot_id.0];
        debug!(
            item_id,
            content_y = mapped.content_y,
            col = mapped.col,
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
        let id = *self.items.get(item_id)?;
        if self.selected.contains(&id) {
            return None;
        }
        let local = self
            .current
            .slots
            .iter()
            .find(|placed| placed.id == id)?
            .local;
        let slot = page.slots.iter().find(|slot| {
            slot.kind.is_selectable_item_for(self.item_kind) && LocalPosition::of(slot) == local
        })?;
        let candidate = self.active_fallbacks.get(item_id).copied().or_else(|| {
            self.candidate_evidence
                .get(item_id)?
                .get(&id)
                .copied()
                .map(CandidateEvidence::candidate)
        });
        Some(TemplateMatchCandidate {
            item_id: item_id.to_string(),
            slot: slot.clone(),
            score: candidate.map_or(1.0, |candidate| candidate.score),
            match_margin: candidate.map_or(0.0, |candidate| candidate.match_margin),
            gate_quality: candidate.map_or(0.0, |candidate| candidate.gate_quality),
            availability: candidate.map_or(ItemAvailability::Available, |candidate| {
                candidate.availability
            }),
        })
    }

    pub(super) fn navigation_hint(&self, item_id: &str) -> NavigationHint {
        let Some(&target_id) = self.items.get(item_id) else {
            return NavigationHint::Scroll(ScrollDirection::Down);
        };
        if self
            .current
            .slots
            .iter()
            .any(|placed| placed.id == target_id)
        {
            return NavigationHint::ExpectedVisible;
        }

        let target_y = self.slots[target_id.0].content_y;
        let mut visible = self
            .current
            .slots
            .iter()
            .map(|placed| self.slots[placed.id.0].content_y);
        let Some(first) = visible.next() else {
            return NavigationHint::ExpectedVisible;
        };
        let (visible_min, visible_max) =
            visible.fold((first, first), |(min, max), y| (min.min(y), max.max(y)));

        if target_y < visible_min {
            NavigationHint::Scroll(ScrollDirection::Up)
        } else if target_y > visible_max {
            NavigationHint::Scroll(ScrollDirection::Down)
        } else {
            NavigationHint::ExpectedVisible
        }
    }

    fn current_slot_id(&self, slot: &Slot) -> Option<SlotId> {
        let local = LocalPosition::of(slot);
        self.current
            .slots
            .iter()
            .find(|placed| placed.local == local)
            .map(|placed| placed.id)
    }
}

fn offset_hypotheses(pairs: &[PairMatch]) -> Vec<f32> {
    let mut offsets = pairs.iter().map(|pair| pair.offset_y).collect::<Vec<_>>();
    offsets.sort_unstable_by(f32::total_cmp);
    let mut hypotheses = Vec::new();
    let mut start = 0;
    while start < offsets.len() {
        let mut end = start + 1;
        while end < offsets.len() && offsets[end] - offsets[end - 1] <= POSITION_TOLERANCE_PX {
            end += 1;
        }
        hypotheses.push(interpolated_median(&offsets[start..end]));
        start = end;
    }
    hypotheses
}

fn evaluate_alignment(pairs: &[PairMatch], offset_y: f32) -> Option<AlignmentCandidate> {
    let mut best_by_page = HashMap::<usize, PairMatch>::new();
    for &pair in pairs
        .iter()
        .filter(|pair| (pair.offset_y - offset_y).abs() <= POSITION_TOLERANCE_PX)
    {
        best_by_page
            .entry(pair.page_index)
            .and_modify(|best| {
                if pair.zncc > best.zncc {
                    *best = pair;
                }
            })
            .or_insert(pair);
    }
    let support = best_by_page.len();
    if support == 0 {
        return None;
    }
    let mean_zncc = best_by_page.values().map(|pair| pair.zncc).sum::<f32>() / support as f32;
    let mut refined_offsets = best_by_page
        .values()
        .map(|pair| pair.offset_y)
        .collect::<Vec<_>>();
    refined_offsets.sort_unstable_by(f32::total_cmp);
    Some(AlignmentCandidate {
        offset_y: interpolated_median(&refined_offsets),
        mean_zncc,
        support,
    })
}

fn interpolated_median(values: &[f32]) -> f32 {
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        0.5 * (values[middle - 1] + values[middle])
    } else {
        values[middle]
    }
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
