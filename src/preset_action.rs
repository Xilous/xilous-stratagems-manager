use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use tracing::{debug, info, info_span};

use crate::app_events::{AppEvent, AppEventSink, PresetCompletion};
use crate::automation::AutomationSession;
use crate::capture::CaptureSessionManager;
use crate::color_normalization::ColorNormalizer;
use crate::game_settings::read_color_settings;
use crate::game_window::find_game_window;
use crate::input;
use crate::loadout::{
    BoosterApplyOutcome, UiState, apply_booster_from_home, apply_empty_loadout_preset,
    bind_loadout_region, collect_current_preset, detect_ui_state, scan_loadout_home,
};
use crate::permissions;
use crate::preset::{
    CapturedPreset, Preset, invalid_preset_reason, load_preset, save_captured_preset,
};
#[cfg(feature = "diagnostics")]
use crate::vision::log_home_tone;
use crate::vision::{RecognizerRuntime, RecognizerSession, RoiObservation};

const READY_UP_HOLD_MS: u64 = 45;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetActionOutcome {
    Saved,
    Applied,
}

pub struct PresetHotkeyBinding {
    pub hotkey: input::HotkeySpec,
    pub preset: String,
}

pub fn preset_hotkeys(
    modifiers: input::HotkeyModifiers,
    keys: &[input::Key],
) -> Vec<PresetHotkeyBinding> {
    keys.iter()
        .enumerate()
        .map(|(index, key)| PresetHotkeyBinding {
            hotkey: input::HotkeySpec {
                id: 1001 + index as i32,
                modifiers,
                key: *key,
            },
            preset: format!("preset_{}", index + 1),
        })
        .collect()
}

pub struct PresetActionConfig<'a> {
    pub presets: &'a Path,
    pub apply_in_saved_order: bool,
    pub auto_ready_up: bool,
    pub events: &'a AppEventSink,
}

pub fn handle_preset_hotkey(
    runtime: &RecognizerRuntime,
    config: &PresetActionConfig<'_>,
    preset_name: &str,
    capture_session: &mut CaptureSessionManager,
) -> Result<PresetActionOutcome> {
    let span = info_span!("preset_action");
    let _guard = span.enter();

    let action_start = Instant::now();
    info!(preset = %preset_name, "preset action started");
    config.events.emit(AppEvent::PresetStarted {
        preset: preset_name.to_string(),
    });

    let game_window = find_game_window().context("failed to locate Helldivers window")?;
    permissions::ensure_input_access()?;
    let (client_w, client_h) = game_window.client_size();
    debug!(client_w, client_h, "game window ready");

    let capture_start = Instant::now();
    let capture = capture_session
        .get_or_create(&game_window)
        .context("failed to get capture session")?;
    debug!(
        elapsed = ?capture_start.elapsed(),
        "capture session ready"
    );
    let game_color_settings =
        read_color_settings().context("failed to read Helldivers color settings")?;
    let color_normalizer = ColorNormalizer::new(game_color_settings, capture.display_color_info())
        .context("failed to configure UI color normalization")?;
    let bound_region = bind_loadout_region(capture, runtime.calibration())
        .context("failed to bind loadout capture region")?;
    let recognizer = runtime.bind(bound_region.geometry);
    let mut automation = AutomationSession::new(bound_region.region, game_window, color_normalizer)
        .context("failed to start automation session")?;

    let (initial_result, ui_state) = {
        let initial_result = scan_loadout_home(&mut automation, recognizer)
            .context("failed to scan loadout home")?;
        let ui_state = detect_ui_state(&initial_result);
        debug!(ui_state = %ui_state.label(), "detected loadout UI state");
        config.events.emit(AppEvent::UiStateDetected {
            state: ui_state.label(),
        });
        (initial_result, ui_state)
    };

    #[cfg(feature = "diagnostics")]
    if matches!(ui_state, UiState::HomeFilled | UiState::HomeEmpty) {
        log_home_tone(&initial_result);
    }

    let (outcome, ready_up_after_apply, completion) = match ui_state {
        UiState::HomeFilled => {
            let captured = collect_current_preset(&initial_result, recognizer.ui_scale())
                .context("failed to collect current preset")?;
            save_current_preset(config, preset_name, &captured)?;
            (
                PresetActionOutcome::Saved,
                false,
                PresetCompletion::Complete,
            )
        }

        UiState::HomeMixed => {
            bail!(
                "loadout home is partially filled; clear or complete the loadout before saving or applying a preset"
            );
        }

        UiState::HomeEmpty => {
            let preset = load_named_preset(config, preset_name)?;
            debug!(
                stratagem_count = preset.stratagems.len(),
                booster_present = preset.booster.is_some(),
                "applying preset from empty home"
            );
            log_preset_contents(&preset);
            let home = apply_empty_loadout_preset(
                recognizer,
                &mut automation,
                config.events,
                config.presets,
                &preset.stratagems,
                config.apply_in_saved_order,
            )
            .context("failed to apply stratagems from empty home")?;
            let booster =
                apply_booster_if_present(recognizer, &mut automation, config, &preset, home)?;
            let (ready_up_after_apply, completion) = match booster {
                Some(BoosterApplyOutcome::Applied) => (true, PresetCompletion::Complete),
                Some(BoosterApplyOutcome::Unavailable) => {
                    (false, PresetCompletion::BoosterUnavailable)
                }
                None => (false, PresetCompletion::Complete),
            };
            (
                PresetActionOutcome::Applied,
                ready_up_after_apply,
                completion,
            )
        }

        UiState::List(_) | UiState::Unknown => {
            bail!("loadout home not detected; return to the loadout home before using a preset");
        }
    };

    if config.auto_ready_up && ready_up_after_apply {
        debug!("booster preset applied; sending READY UP key");
        automation.tap_key(input::Key::B, READY_UP_HOLD_MS)?;
    }

    config.events.emit(AppEvent::PresetDone {
        preset: preset_name.to_string(),
        completion,
    });
    info!(
        preset = %preset_name,
        elapsed = ?action_start.elapsed(),
        "preset action completed"
    );
    Ok(outcome)
}

fn load_named_preset(config: &PresetActionConfig<'_>, preset_name: &str) -> Result<Preset> {
    let preset = load_preset(config.presets, preset_name)
        .with_context(|| format!("failed to load preset \"{preset_name}\""))?;

    if let Some(reason) = invalid_preset_reason(config.presets, &preset) {
        bail!("preset \"{preset_name}\" is invalid: {reason}");
    }

    Ok(preset)
}

fn save_current_preset(
    config: &PresetActionConfig<'_>,
    preset_name: &str,
    captured: &CapturedPreset,
) -> Result<()> {
    debug!(
        stratagem_count = captured.stratagems.len(),
        booster_present = captured.booster.is_some(),
        "saving current preset"
    );

    let preset = save_captured_preset(config.presets, preset_name, captured)
        .with_context(|| format!("failed to save preset \"{preset_name}\""))?;
    log_preset_contents(&preset);
    config.events.emit(AppEvent::PresetSaved {
        preset: preset_name.to_string(),
        stratagems: preset
            .stratagems
            .iter()
            .map(|template| template.path.clone())
            .collect(),
        booster: preset
            .booster
            .as_ref()
            .map(|template| template.path.clone()),
    });
    info!(
        preset = %preset_name,
        stratagem_count = preset.stratagems.len(),
        booster_present = preset.booster.is_some(),
        presets_path = %config.presets.display(),
        "preset saved"
    );

    Ok(())
}

fn log_preset_contents(preset: &Preset) {
    let stratagems = preset
        .stratagems
        .iter()
        .map(|template| template.path.as_str())
        .collect::<Vec<_>>();
    debug!(
        ?stratagems,
        booster_present = preset.booster.is_some(),
        booster_template = preset
            .booster
            .as_ref()
            .map_or("", |template| template.path.as_str()),
        "preset contents"
    );
}

fn apply_booster_if_present(
    recognizer: RecognizerSession,
    automation: &mut AutomationSession<'_>,
    config: &PresetActionConfig<'_>,
    preset: &Preset,
    home: RoiObservation,
) -> Result<Option<BoosterApplyOutcome>> {
    let Some(booster) = preset.booster.as_ref() else {
        return Ok(None);
    };
    apply_booster_from_home(
        recognizer,
        automation,
        config.events,
        &home,
        config.presets,
        booster,
    )
    .map(Some)
    .context("failed to apply booster from home")
}
