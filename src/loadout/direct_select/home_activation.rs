use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, debug_span};

use crate::automation::AutomationSession;
use crate::item::ItemKind;
use crate::vision::{RecognizerSession, RoiObservation};

use super::super::{UiState, home_booster_slot};

use super::CLICK_HOLD_MS;

const LIST_OPEN_TIMEOUT: Duration = Duration::from_millis(1500);
const LIST_OPEN_INITIAL_DELAY: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug)]
pub(super) struct HomeOpenTarget {
    pub(super) item_kind: ItemKind,
    pub(super) point: (u32, u32),
}

pub(super) fn open_slot_list(
    automation: &mut AutomationSession<'_>,
    recognizer: RecognizerSession,
    target: HomeOpenTarget,
) -> Result<RoiObservation> {
    let span = debug_span!("open_slot_list", item_kind = %target.item_kind.label());
    let _guard = span.enter();
    let target_state = UiState::List(target.item_kind);

    debug!(
        x = target.point.0,
        y = target.point.1,
        hold_ms = CLICK_HOLD_MS,
        "opening home list with a direct mouse click"
    );
    automation.click(target.point, CLICK_HOLD_MS)?;
    std::thread::sleep(LIST_OPEN_INITIAL_DELAY);

    let observation = super::super::wait_for_stable_ui_state(
        automation,
        recognizer,
        target_state,
        LIST_OPEN_TIMEOUT,
    )?
    .with_context(|| format!("timed out waiting for {} UI state", target_state.label()))?;

    Ok(observation)
}

pub(super) fn home_booster_target(observation: &RoiObservation) -> Result<HomeOpenTarget> {
    let span = debug_span!("locate_home_booster_slot");
    let _guard = span.enter();

    let slot =
        home_booster_slot(observation).context("home layout did not contain a booster slot")?;
    let point = slot.center();

    debug!(
        click_x = point.0,
        click_y = point.1,
        booster_x = slot.x,
        booster_y = slot.y,
        booster_w = slot.w,
        booster_h = slot.h,
        "home booster slot ready for direct mouse click"
    );
    Ok(HomeOpenTarget {
        item_kind: ItemKind::Booster,
        point,
    })
}
