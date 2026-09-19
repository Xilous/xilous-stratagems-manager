//! Automation thread: owns the global hotkeys, screen capture, recognition,
//! and input injection. The window only reads shared state and sends commands.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle, sleep};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{debug, error, info, warn};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION;

use crate::active_loadout::{self, ActiveLoadout, ActiveSource};
use crate::app_events::{AppEvent, AppEventSink, PresetCompletion};
use crate::app_state::{
    AppHandle, HotkeyStatus, LiveSettings, PresetSummary, StatusLine, Tone, UiCommand,
};
use crate::capture::CaptureSessionManager;
use crate::catalog::Catalog;
use crate::color_normalization::ColorNormalizer;
use crate::config::{self, AppConfig};
use crate::game_settings::read_color_settings;
use crate::game_window::find_game_window_once;
use crate::input::{
    self, HotkeyModifiers, HotkeyPoll, HotkeySpec, InputSession, Key, RegisteredHotkeys,
};
use crate::loadout::{UiState, bind_loadout_region, collect_current_preset};
use crate::permissions;
use crate::preset::{self, invalid_preset_reason, load_presets, validate_preset};
use crate::preset_action::{
    PresetActionConfig, PresetActionOutcome, handle_preset_hotkey, quick_ui_state,
};
use crate::stratagem_input;
use crate::tray::{TrayEvent, TrayHandle};
use crate::vision::{CatalogIdentifier, ImageSample, RecognizerRuntime, RoiObservation};

const TICK: Duration = Duration::from_millis(50);
const HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const HOTKEY_RETRY_DELAY: Duration = Duration::from_secs(5);
const PRESET_HOTKEY_ID_BASE: i32 = 1001;
const SLOT_HOTKEY_ID_BASE: i32 = 2001;
const BINDING_HOTKEY_ID_BASE: i32 = 3001;

pub struct EngineContext {
    pub config: AppConfig,
    pub config_path: PathBuf,
    pub presets_path: PathBuf,
    pub active_path: PathBuf,
    pub failure_debug_dir: PathBuf,
    pub catalog: Arc<Catalog>,
    pub handle: AppHandle,
    pub commands: Receiver<UiCommand>,
    pub tray: TrayHandle,
}

pub fn spawn(context: EngineContext) -> Result<JoinHandle<()>> {
    let handle = context.handle.clone();
    thread::Builder::new()
        .name("xilous-engine".to_string())
        .spawn(move || {
            if let Err(error) = run(context) {
                let message = format!("{error:#}");
                error!(error = %message, "automation stopped");
                handle.update(|state| {
                    state.fatal_error = Some(message.clone());
                    state.status = StatusLine::new(Tone::Error, "Automation stopped");
                    state.hotkeys = HotkeyStatus {
                        armed: false,
                        detail: "stopped".to_string(),
                    };
                });
            }
        })
        .context("failed to start the automation thread")
}

fn run(context: EngineContext) -> Result<()> {
    let mut engine = Engine::new(context)?;
    engine.publish_all();
    info!("automation ready");
    engine.set_status(Tone::Info, "Ready");

    loop {
        if engine.handle.stop_requested() {
            return Ok(());
        }
        if engine.process_commands() {
            engine.handle.update(|state| state.exit_requested = true);
            return Ok(());
        }
        if engine.process_tray() {
            engine.handle.update(|state| state.exit_requested = true);
            return Ok(());
        }
        engine.refresh_foreground();
        engine.sync_hotkeys();

        match input::poll_hotkey(TICK) {
            HotkeyPoll::Triggered(hotkey_id) => {
                engine.on_hotkey(hotkey_id);
                input::discard_pending_hotkeys();
            }
            HotkeyPoll::Timeout => engine.on_idle_tick(),
        }
    }
}

#[derive(Clone)]
enum HotkeyAction {
    Preset(usize),
    Slot(usize),
    Stratagem(String),
}

struct Engine {
    handle: AppHandle,
    catalog: Arc<Catalog>,
    runtime: RecognizerRuntime,
    identifier: CatalogIdentifier,
    config_path: PathBuf,
    presets_path: PathBuf,
    active_path: PathBuf,
    failure_debug_dir: PathBuf,
    preset_modifiers: HotkeyModifiers,
    preset_keys: Vec<Key>,
    settings: LiveSettings,
    active: Option<ActiveLoadout>,
    game_foreground: bool,
    registered: Option<RegisteredHotkeys>,
    registered_signature: Vec<(i32, String)>,
    actions: HashMap<i32, HotkeyAction>,
    next_registration_attempt: Instant,
    capture_session: CaptureSessionManager,
    events: AppEventSink,
    modifiers_were_down: bool,
    prewarm_suppressed: bool,
    commands: Receiver<UiCommand>,
    tray: TrayHandle,
}

impl Engine {
    fn new(context: EngineContext) -> Result<Self> {
        let EngineContext {
            config,
            config_path,
            presets_path,
            active_path,
            failure_debug_dir,
            catalog,
            handle,
            commands,
            tray,
        } = context;

        let runtime = RecognizerRuntime::load()?;
        let identifier_start = Instant::now();
        let identifier = CatalogIdentifier::from_catalog(&catalog)?;
        info!(
            references = identifier.len(),
            elapsed = ?identifier_start.elapsed(),
            "stratagem identifier ready"
        );

        let preset_modifiers = HotkeyModifiers::new(config.hotkey.modifiers.clone())?;
        let preset_keys = config.hotkey.keys.clone();
        let settings = LiveSettings {
            apply_in_saved_order: config.presets.apply_in_saved_order,
            auto_ready_up: config.presets.auto_ready_up,
            save_fallback_when_taken: config.presets.save_fallback_when_taken,
            labels: config.presets.labels.clone(),
            preset_hotkey_labels: preset_keys
                .iter()
                .map(|key| preset_modifiers.label_with_key(*key))
                .collect(),
            mission_enabled: config.mission.enabled,
            slot_keys: config.mission.slot_keys.clone(),
            stratagem_input: config.mission.stratagem_input(),
            bindings: config.mission.bindings.clone(),
        };

        let active = match active_loadout::load(&active_path) {
            Ok(active) => active,
            Err(error) => {
                warn!(error = %format!("{error:#}"), "active loadout could not be read; ignoring it");
                None
            }
        };

        let events = {
            let handle = handle.clone();
            AppEventSink::new(move |event| apply_event(&handle, event))
        };

        Ok(Self {
            handle,
            catalog,
            runtime,
            identifier,
            config_path,
            presets_path,
            active_path,
            failure_debug_dir,
            preset_modifiers,
            preset_keys,
            settings,
            active,
            game_foreground: false,
            registered: None,
            registered_signature: Vec::new(),
            actions: HashMap::new(),
            next_registration_attempt: Instant::now(),
            capture_session: CaptureSessionManager::new(),
            events,
            modifiers_were_down: false,
            prewarm_suppressed: false,
            commands,
            tray,
        })
    }

    // ----- publishing -------------------------------------------------------

    fn publish_all(&self) {
        self.publish_settings();
        self.publish_active();
        self.refresh_presets();
    }

    fn publish_settings(&self) {
        let settings = self.settings.clone();
        self.handle.update(|state| state.settings = settings);
    }

    fn publish_active(&self) {
        let active = self.active.clone();
        self.handle.update(|state| state.active_loadout = active);
    }

    fn set_status(&self, tone: Tone, text: impl Into<String>) {
        let status = StatusLine::new(tone, text);
        self.handle.update(|state| {
            state.status = status;
            state.progress = None;
        });
    }

    fn set_busy(&self, busy: bool) {
        self.handle.update(|state| state.busy = busy);
    }

    fn refresh_presets(&self) {
        let presets = self.preset_summaries();
        self.handle.update(|state| state.presets = presets);
    }

    fn preset_summaries(&self) -> Vec<PresetSummary> {
        let loaded = match load_presets(&self.presets_path) {
            Ok(presets) => Ok(presets),
            Err(error) => Err(format!("{error:#}")),
        };

        (0..self.preset_keys.len())
            .map(|index| {
                let name = format!("preset_{}", index + 1);
                let mut summary = PresetSummary {
                    hotkey_label: self.preset_modifiers.label_with_key(self.preset_keys[index]),
                    label: self.settings.labels.get(&name).cloned().unwrap_or_default(),
                    index,
                    name: name.clone(),
                    ..PresetSummary::default()
                };
                match &loaded {
                    Err(error) => summary.problem = Some(error.clone()),
                    Ok(presets) => {
                        if let Some(preset) = presets.get(&name) {
                            summary.saved = true;
                            summary.stratagems = preset.stratagem_ids();
                            summary.booster = preset.booster.is_some();
                            summary.fallback_booster = preset.fallback_booster.is_some();
                            summary.problem = validate_preset(&name, preset)
                                .err()
                                .map(|error| format!("{error:#}"))
                                .or_else(|| invalid_preset_reason(&self.presets_path, preset));
                        }
                    }
                }
                summary
            })
            .collect()
    }

    fn preset_display_name(&self, name: &str) -> String {
        match self.settings.labels.get(name).map(|label| label.trim()) {
            Some(label) if !label.is_empty() => format!("{name} ({label})"),
            _ => name.to_string(),
        }
    }

    // ----- commands from the window ------------------------------------------

    /// Returns `true` when the application should exit.
    fn process_commands(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            match self.handle_command(command) {
                Ok(true) => return true,
                Ok(false) => {}
                Err(error) => {
                    let message = format!("{error:#}");
                    warn!(error = %message, "window command failed");
                    self.set_status(Tone::Error, message);
                }
            }
            self.publish_settings();
        }
        false
    }

    fn handle_command(&mut self, command: UiCommand) -> Result<bool> {
        match command {
            UiCommand::SetPresetFlag { key, value } => {
                match key {
                    "apply_in_saved_order" => self.settings.apply_in_saved_order = value,
                    "auto_ready_up" => self.settings.auto_ready_up = value,
                    "save_fallback_when_taken" => self.settings.save_fallback_when_taken = value,
                    _ => bail!("unknown preset setting {key}"),
                }
                self.persist(|document| {
                    config::set_bool(document, "presets", key, value);
                    Ok(())
                })?;
                info!(setting = key, value, "preset setting changed");
            }
            UiCommand::SetLabel { preset, label } => {
                let label = label.trim().to_string();
                if label.chars().any(char::is_control) {
                    bail!("labels cannot contain control characters");
                }
                if label.is_empty() {
                    self.settings.labels.remove(&preset);
                } else {
                    self.settings.labels.insert(preset.clone(), label.clone());
                }
                let value = (!label.is_empty()).then_some(label);
                self.persist(|document| {
                    config::set_nested_string(
                        document,
                        "presets",
                        "labels",
                        &preset,
                        value.as_deref(),
                    )
                })?;
                self.refresh_presets();
            }
            UiCommand::SetPresetStratagem { preset, slot, id } => {
                preset::set_preset_stratagem(&self.presets_path, &preset, slot, id.clone())?;
                if let Some(active) = &mut self.active
                    && active.preset == preset
                    && slot < active.slots.len()
                {
                    active.slots[slot] = id.clone();
                    self.save_active();
                    self.publish_active();
                }
                let stratagem_name = id
                    .as_deref()
                    .map_or_else(|| "unknown".to_string(), |id| self.catalog.name_of(id));
                info!(preset = %preset, slot = slot + 1, stratagem = %stratagem_name, "preset slot set");
                self.set_status(
                    Tone::Info,
                    format!(
                        "{}: slot {} set to {stratagem_name}",
                        self.preset_display_name(&preset),
                        slot + 1
                    ),
                );
                self.refresh_presets();
            }
            UiCommand::DeletePreset { preset } => {
                preset::delete_preset(&self.presets_path, &preset)?;
                if self.active.as_ref().is_some_and(|active| active.preset == preset) {
                    self.clear_active()?;
                }
                info!(preset = %preset, "preset deleted");
                self.set_status(Tone::Info, format!("Deleted {}", self.preset_display_name(&preset)));
                self.refresh_presets();
            }
            UiCommand::SetActiveFromPreset { preset } => {
                let presets = load_presets(&self.presets_path)?;
                let loaded = presets
                    .get(&preset)
                    .with_context(|| format!("{preset} is not saved"))?;
                self.set_active(ActiveLoadout::new(
                    &preset,
                    loaded.stratagem_ids(),
                    ActiveSource::Manual,
                ));
                self.set_status(
                    Tone::Info,
                    format!("Active loadout set to {}", self.preset_display_name(&preset)),
                );
            }
            UiCommand::SetActiveSlot { slot, id } => {
                let Some(active) = &mut self.active else {
                    bail!("there is no active loadout");
                };
                if slot >= active.slots.len() {
                    bail!("slot {} does not exist", slot + 1);
                }
                active.slots[slot] = id.clone();
                active.source = ActiveSource::Manual;
                self.save_active();
                self.publish_active();
                let stratagem_name = id
                    .as_deref()
                    .map_or_else(|| "unknown".to_string(), |id| self.catalog.name_of(id));
                self.set_status(
                    Tone::Info,
                    format!("Active slot {} set to {stratagem_name}", slot + 1),
                );
            }
            UiCommand::ClearActive => {
                self.clear_active()?;
                self.set_status(Tone::Info, "Active loadout cleared");
            }
            UiCommand::SetMissionEnabled(enabled) => {
                self.settings.mission_enabled = enabled;
                self.persist(|document| {
                    config::set_bool(document, "mission", "enabled", enabled);
                    Ok(())
                })?;
                info!(enabled, "mission hotkeys toggled");
            }
            UiCommand::SetSlotKeys(keys) => {
                if keys.len() != 4 {
                    bail!("exactly 4 slot keys are required");
                }
                self.settings.slot_keys = keys.clone();
                self.persist(|document| {
                    config::set_string_array(
                        document,
                        "mission",
                        "slot_keys",
                        keys.iter().map(|binding| binding.config_string()),
                    );
                    Ok(())
                })?;
                info!(
                    keys = ?keys.iter().map(|binding| binding.label()).collect::<Vec<_>>(),
                    "mission slot keys changed"
                );
            }
            UiCommand::SetStratagemInput(input_settings) => {
                input_settings.validate()?;
                self.settings.stratagem_input = input_settings.clone();
                self.persist(|document| {
                    config::set_string(
                        document,
                        "mission",
                        "menu_key",
                        input_settings.menu_key.config_name(),
                    );
                    config::set_string(
                        document,
                        "mission",
                        "menu_mode",
                        &enum_config_name(&input_settings.menu_mode),
                    );
                    config::set_string(
                        document,
                        "mission",
                        "direction_keys",
                        &enum_config_name(&input_settings.direction_keys),
                    );
                    config::set_integer(
                        document,
                        "mission",
                        "menu_open_delay_ms",
                        input_settings.menu_open_delay_ms,
                    );
                    config::set_integer(document, "mission", "key_hold_ms", input_settings.key_hold_ms);
                    config::set_integer(document, "mission", "key_gap_ms", input_settings.key_gap_ms);
                    config::set_integer(
                        document,
                        "mission",
                        "menu_release_delay_ms",
                        input_settings.menu_release_delay_ms,
                    );
                    Ok(())
                })?;
                info!(?input_settings, "stratagem input settings changed");
                self.set_status(Tone::Info, "Stratagem input settings saved");
            }
            UiCommand::SetBinding { id, binding } => {
                if self.catalog.get(&id).is_none() {
                    bail!("unknown stratagem {id}");
                }
                match binding {
                    Some(binding) => {
                        self.settings.bindings.insert(id.clone(), binding);
                    }
                    None => {
                        self.settings.bindings.remove(&id);
                    }
                }
                let value = binding.map(|binding| binding.config_string());
                self.persist(|document| {
                    config::set_nested_string(document, "mission", "bindings", &id, value.as_deref())
                })?;
                info!(
                    stratagem = %self.catalog.name_of(&id),
                    hotkey = binding.map(|binding| binding.label()),
                    "mission stratagem binding changed"
                );
            }
            UiCommand::Exit => {
                info!("exit requested from the window");
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn persist(
        &self,
        edit: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
    ) -> Result<()> {
        config::edit_config(&self.config_path, edit)
            .with_context(|| format!("failed to update {}", self.config_path.display()))
    }

    /// Returns `true` when the tray asked to exit.
    fn process_tray(&mut self) -> bool {
        while let Some(event) = self.tray.try_event() {
            match event {
                TrayEvent::ShowWindow => {
                    self.handle.update(|state| state.show_window_requested = true);
                }
                TrayEvent::ExitRequested => {
                    info!("tray exit requested");
                    return true;
                }
            }
        }
        false
    }

    // ----- active loadout -----------------------------------------------------

    fn set_active(&mut self, loadout: ActiveLoadout) {
        info!(
            preset = %loadout.preset,
            source = loadout.source.label(),
            slots = ?loadout.slots,
            "active loadout set"
        );
        self.active = Some(loadout);
        self.save_active();
        self.publish_active();
    }

    fn save_active(&self) {
        if let Some(active) = &self.active
            && let Err(error) = active_loadout::save(&self.active_path, active)
        {
            warn!(error = %format!("{error:#}"), "failed to persist the active loadout");
        }
    }

    fn clear_active(&mut self) -> Result<()> {
        self.active = None;
        active_loadout::clear(&self.active_path)?;
        self.publish_active();
        info!("active loadout cleared");
        Ok(())
    }

    // ----- hotkeys ------------------------------------------------------------

    fn refresh_foreground(&mut self) {
        let foreground = find_game_window_once().is_ok();
        if foreground != self.game_foreground {
            self.game_foreground = foreground;
            debug!(foreground, "game window focus changed");
            self.handle.update(|state| state.game_foreground = foreground);
        }
    }

    fn desired_hotkeys(&self) -> (Vec<HotkeySpec>, HashMap<i32, HotkeyAction>, Vec<String>) {
        let mut specs = Vec::new();
        let mut actions = HashMap::new();
        let mut conflicts = Vec::new();
        if !self.game_foreground {
            return (specs, actions, conflicts);
        }

        let mut push = |spec: HotkeySpec, action: HotkeyAction, owner: String| {
            let duplicate = specs
                .iter()
                .any(|existing: &HotkeySpec| existing.modifiers == spec.modifiers && existing.key == spec.key);
            if duplicate {
                conflicts.push(format!("{} ({owner})", spec.label()));
            } else {
                actions.insert(spec.id, action);
                specs.push(spec);
            }
        };

        for (index, key) in self.preset_keys.iter().enumerate() {
            push(
                HotkeySpec {
                    id: PRESET_HOTKEY_ID_BASE + index as i32,
                    modifiers: self.preset_modifiers,
                    key: *key,
                },
                HotkeyAction::Preset(index),
                format!("preset {}", index + 1),
            );
        }

        if self.settings.mission_enabled {
            if self.active.is_some() {
                for (index, binding) in self.settings.slot_keys.iter().enumerate() {
                    push(
                        HotkeySpec {
                            id: SLOT_HOTKEY_ID_BASE + index as i32,
                            modifiers: binding.modifiers,
                            key: binding.key,
                        },
                        HotkeyAction::Slot(index),
                        format!("slot {}", index + 1),
                    );
                }
            }
            for (index, (id, binding)) in self.settings.bindings.iter().enumerate() {
                let Some(entry) = self.catalog.get(id) else {
                    continue;
                };
                if !entry.has_code() {
                    continue;
                }
                push(
                    HotkeySpec {
                        id: BINDING_HOTKEY_ID_BASE + index as i32,
                        modifiers: binding.modifiers,
                        key: binding.key,
                    },
                    HotkeyAction::Stratagem(id.clone()),
                    entry.name.clone(),
                );
            }
        }

        (specs, actions, conflicts)
    }

    fn sync_hotkeys(&mut self) {
        let (specs, actions, conflicts) = self.desired_hotkeys();
        let signature = specs
            .iter()
            .map(|spec| (spec.id, spec.label()))
            .collect::<Vec<_>>();
        if signature == self.registered_signature {
            return;
        }
        if Instant::now() < self.next_registration_attempt {
            return;
        }

        self.registered = None;
        self.actions.clear();

        if specs.is_empty() {
            self.registered_signature = signature;
            let detail = if self.game_foreground {
                "nothing to register".to_string()
            } else {
                "waiting for Helldivers 2 to be focused".to_string()
            };
            self.handle.update(|state| {
                state.hotkeys = HotkeyStatus {
                    armed: false,
                    detail,
                };
            });
            return;
        }

        match RegisteredHotkeys::register(&specs) {
            Ok(registered) => {
                let slot_count = actions
                    .values()
                    .filter(|action| matches!(action, HotkeyAction::Slot(_)))
                    .count();
                let stratagem_count = actions
                    .values()
                    .filter(|action| matches!(action, HotkeyAction::Stratagem(_)))
                    .count();
                let mut detail = format!(
                    "{} preset, {slot_count} slot, {stratagem_count} mission",
                    self.preset_keys.len()
                );
                if !conflicts.is_empty() {
                    detail.push_str(&format!("; skipped duplicates: {}", conflicts.join(", ")));
                }
                debug!(count = specs.len(), %detail, "hotkeys registered");
                self.registered = Some(registered);
                self.actions = actions;
                self.registered_signature = signature;
                self.handle.update(|state| {
                    state.hotkeys = HotkeyStatus {
                        armed: true,
                        detail,
                    };
                });
            }
            Err(error) => {
                let message = format!("{error:#}");
                warn!(error = %message, "hotkey registration failed; retrying");
                self.registered_signature = Vec::new();
                self.next_registration_attempt = Instant::now() + HOTKEY_RETRY_DELAY;
                self.handle.update(|state| {
                    state.hotkeys = HotkeyStatus {
                        armed: false,
                        detail: message,
                    };
                });
            }
        }
    }

    fn on_hotkey(&mut self, hotkey_id: i32) {
        let Some(action) = self.actions.get(&hotkey_id).cloned() else {
            debug!(hotkey_id, "ignoring hotkey without an action");
            return;
        };
        match action {
            HotkeyAction::Preset(index) => self.on_preset_hotkey(index),
            HotkeyAction::Slot(index) => self.call_slot(index),
            HotkeyAction::Stratagem(id) => self.call_stratagem(&id),
        }
    }

    fn on_idle_tick(&mut self) {
        let modifiers_down = self.preset_modifiers.is_down();
        if modifiers_down && !self.modifiers_were_down && !self.prewarm_suppressed {
            self.prewarm();
        } else if !modifiers_down && self.modifiers_were_down {
            self.capture_session.discard();
            self.prewarm_suppressed = false;
        }
        self.modifiers_were_down = modifiers_down;
    }

    fn prewarm(&mut self) {
        let result = find_game_window_once().and_then(|target| {
            self.capture_session.get_or_create(&target)?;
            Ok(())
        });
        if let Err(error) = result {
            debug!(error = %format!("{error:#}"), "capture preparation skipped");
        }
    }

    // ----- preset hotkeys -----------------------------------------------------

    fn on_preset_hotkey(&mut self, index: usize) {
        let preset_name = format!("preset_{}", index + 1);
        let spec = HotkeySpec {
            id: PRESET_HOTKEY_ID_BASE + index as i32,
            modifiers: self.preset_modifiers,
            key: self.preset_keys[index],
        };

        // Sprinting holds Shift, which turns a slot key into a preset hotkey.
        // If the loadout screen is not showing, treat it as the slot call.
        if self.settings.mission_enabled && self.active.is_some() {
            self.prewarm();
            match quick_ui_state(&self.runtime, &mut self.capture_session) {
                Ok(UiState::List(_) | UiState::Unknown) => {
                    debug!(preset = %preset_name, "loadout screen not visible; calling slot instead");
                    self.call_slot(index);
                    return;
                }
                Ok(state) => debug!(state = state.label(), "loadout screen visible"),
                Err(error) => debug!(
                    error = %format!("{error:#}"),
                    "quick UI check failed; continuing with the preset action"
                ),
            }
        }

        self.events.emit(AppEvent::HotkeyReleaseRequested {
            preset: preset_name.clone(),
        });
        if !input::wait_hotkey_released(&spec, HOTKEY_RELEASE_TIMEOUT) {
            self.capture_session.discard();
            self.modifiers_were_down = self.preset_modifiers.is_down();
            self.prewarm_suppressed = self.modifiers_were_down;
            warn!(
                preset = %preset_name,
                timeout = ?HOTKEY_RELEASE_TIMEOUT,
                "preset action cancelled because the hotkey was not released"
            );
            self.events.emit(AppEvent::PresetCancelled {
                preset: preset_name,
                reason: "hotkey was not released in time".to_string(),
            });
            sleep(Duration::from_millis(200));
            return;
        }

        self.set_busy(true);
        let action_start = Instant::now();
        let outcome = {
            let action_config = PresetActionConfig {
                presets: &self.presets_path,
                apply_in_saved_order: self.settings.apply_in_saved_order,
                auto_ready_up: self.settings.auto_ready_up,
                save_fallback_when_taken: self.settings.save_fallback_when_taken,
                events: &self.events,
            };
            handle_preset_hotkey(
                &self.runtime,
                &action_config,
                &preset_name,
                &mut self.capture_session,
            )
        };

        match outcome {
            Ok(PresetActionOutcome::Saved { captured }) => {
                unsafe {
                    MessageBeep(MB_ICONINFORMATION).ok();
                }
                self.finish_saved(&preset_name, &captured.stratagems);
            }
            Ok(PresetActionOutcome::Applied { home, ui_scale }) => {
                self.finish_applied(&preset_name, &home, ui_scale);
            }
            Err(error) => {
                self.save_last_failure(&error);
                let message = format!("{error:#}");
                error!(
                    preset = %preset_name,
                    elapsed = ?action_start.elapsed(),
                    error = %message,
                    "preset action failed"
                );
                self.events.emit(AppEvent::PresetFailed {
                    preset: preset_name.clone(),
                    error: message,
                });
            }
        }

        self.capture_session.discard();
        self.modifiers_were_down = self.preset_modifiers.is_down();
        self.prewarm_suppressed = self.modifiers_were_down;
        self.set_busy(false);
        self.refresh_presets();
        sleep(Duration::from_millis(200));
    }

    fn finish_saved(&mut self, preset_name: &str, samples: &[ImageSample]) {
        self.set_status(Tone::Working, "Identifying stratagems");
        let ids = self.identify_samples(samples, None);
        if let Err(error) = preset::set_preset_stratagems(&self.presets_path, preset_name, &ids) {
            warn!(error = %format!("{error:#}"), "failed to record identified stratagems");
        }
        let identified = ids.iter().flatten().count();
        self.set_active(ActiveLoadout::new(preset_name, ids, ActiveSource::Saved));

        let display = self.preset_display_name(preset_name);
        if identified == 4 {
            self.set_status(Tone::Success, format!("Saved {display}; all 4 stratagems identified"));
        } else {
            self.set_status(
                Tone::Warning,
                format!(
                    "Saved {display}; {identified}/4 stratagems identified. Set the rest in the window."
                ),
            );
        }
    }

    fn finish_applied(&mut self, preset_name: &str, home: &RoiObservation, ui_scale: f32) {
        self.set_status(Tone::Working, "Reading the applied loadout");
        let saved_ids = match preset::load_preset(&self.presets_path, preset_name) {
            Ok(preset) => preset.stratagem_ids(),
            Err(error) => {
                warn!(error = %format!("{error:#}"), "failed to reload the applied preset");
                [None, None, None, None]
            }
        };
        let allowed = saved_ids.iter().flatten().cloned().collect::<Vec<_>>();

        let mut ids = match collect_current_preset(home, ui_scale) {
            Ok(captured) if captured.stratagems.len() == 4 => self.identify_samples(
                &captured.stratagems,
                (!allowed.is_empty()).then_some(allowed.as_slice()),
            ),
            Ok(_) => [None, None, None, None],
            Err(error) => {
                warn!(error = %format!("{error:#}"), "failed to crop the applied loadout slots");
                [None, None, None, None]
            }
        };

        // A single unresolved slot must hold the one saved id that was not seen.
        let unused = saved_ids
            .iter()
            .flatten()
            .filter(|id| !ids.iter().flatten().any(|seen| seen == *id))
            .cloned()
            .collect::<Vec<_>>();
        let missing = ids.iter().filter(|id| id.is_none()).count();
        if missing == 1 && unused.len() == 1 {
            if let Some(slot) = ids.iter_mut().find(|id| id.is_none()) {
                *slot = unused.into_iter().next();
            }
        } else if ids.iter().all(Option::is_none) && self.settings.apply_in_saved_order {
            ids = saved_ids;
        }

        let identified = ids.iter().flatten().count();
        self.set_active(ActiveLoadout::new(preset_name, ids, ActiveSource::Applied));
        let display = self.preset_display_name(preset_name);
        if identified == 4 {
            self.set_status(Tone::Success, format!("Applied {display}; slot hotkeys armed"));
        } else {
            self.set_status(
                Tone::Warning,
                format!("Applied {display}; {identified}/4 slots identified. Set the rest in the window."),
            );
        }
    }

    fn identify_samples(
        &self,
        samples: &[ImageSample],
        allowed: Option<&[String]>,
    ) -> [Option<String>; 4] {
        std::array::from_fn(|index| {
            let sample = samples.get(index)?;
            match self.identifier.identify(sample, allowed) {
                Ok(identification) => {
                    if identification.accepted.is_none() {
                        info!(
                            slot = index + 1,
                            category = identification.category.label(),
                            best = identification.best().map(|best| best.id.as_str()),
                            score = identification.best().map(|best| best.score),
                            margin = identification.margin,
                            "stratagem in slot could not be identified with confidence"
                        );
                    } else {
                        debug!(
                            slot = index + 1,
                            stratagem = identification.accepted.as_deref(),
                            score = identification.best().map(|best| best.score),
                            margin = identification.margin,
                            "stratagem identified"
                        );
                    }
                    identification.accepted
                }
                Err(error) => {
                    warn!(
                        slot = index + 1,
                        error = %format!("{error:#}"),
                        "stratagem identification failed"
                    );
                    None
                }
            }
        })
    }

    fn save_last_failure(&mut self, action_error: &anyhow::Error) {
        let result = (|| -> Result<Option<PathBuf>> {
            let Some(capture) = self.capture_session.active_capture() else {
                return Ok(None);
            };
            let normalizer =
                ColorNormalizer::new(read_color_settings()?, capture.display_color_info())?;
            let mut region = bind_loadout_region(capture, self.runtime.calibration())?.region;
            let image = region.capture(&normalizer)?;
            let directory = self.failure_debug_dir.clone();
            fs::create_dir_all(&directory).with_context(|| {
                format!(
                    "failed to create failure debug directory {}",
                    directory.display()
                )
            })?;
            image
                .save(directory.join("frame.png"))
                .context("failed to save failure debug frame")?;
            fs::write(directory.join("error.txt"), format!("{action_error:#}"))
                .context("failed to save failure debug error")?;
            Ok(Some(directory))
        })();

        match result {
            Ok(Some(directory)) => warn!(
                path = %directory.display(),
                "saved last preset failure diagnostics"
            ),
            Ok(None) => debug!("preset failure has no active capture frame to save"),
            Err(error) => warn!(
                error = %format!("{error:#}"),
                "failed to save preset failure diagnostics"
            ),
        }
    }

    // ----- mission call-ins ---------------------------------------------------

    fn call_slot(&mut self, index: usize) {
        let Some(active) = &self.active else {
            self.set_status(
                Tone::Warning,
                "No active loadout; save or apply a preset on the loadout screen first",
            );
            return;
        };
        let Some(id) = active.slot(index).map(str::to_string) else {
            self.set_status(
                Tone::Warning,
                format!(
                    "Slot {} of the active loadout is not identified; set it in the window",
                    index + 1
                ),
            );
            return;
        };
        self.call_stratagem(&id);
    }

    fn call_stratagem(&mut self, id: &str) {
        let Some(entry) = self.catalog.get(id) else {
            warn!(id, "unknown stratagem id");
            self.set_status(Tone::Error, format!("Unknown stratagem {id}"));
            return;
        };
        if entry.code.is_empty() {
            self.set_status(Tone::Warning, format!("{} has no input code", entry.name));
            return;
        }

        let result = (|| -> Result<Duration> {
            let game_window = find_game_window_once().context("Helldivers 2 is not focused")?;
            permissions::ensure_input_access()?;
            let mut session = InputSession::new(game_window)?;
            let started = Instant::now();
            stratagem_input::execute(&mut session, &self.settings.stratagem_input, &entry.code)?;
            Ok(started.elapsed())
        })();

        match result {
            Ok(elapsed) => {
                info!(
                    stratagem = %entry.name,
                    code = %entry.arrows(),
                    ?elapsed,
                    "stratagem code sent"
                );
                self.set_status(Tone::Success, format!("{}  {}", entry.name, entry.arrows()));
            }
            Err(error) => {
                let message = format!("{error:#}");
                warn!(stratagem = %entry.name, error = %message, "stratagem call failed");
                self.set_status(
                    Tone::Error,
                    format!("{}: {}", entry.name, first_line(&message)),
                );
            }
        }
    }
}

fn apply_event(handle: &AppHandle, event: AppEvent) {
    handle.update(|state| match event {
        AppEvent::PresetStarted { preset } => {
            state.status = StatusLine::new(Tone::Working, format!("Starting {preset}"));
            state.progress = None;
        }
        AppEvent::HotkeyReleaseRequested { preset } => {
            state.status = StatusLine::new(
                Tone::Working,
                format!("{preset}: release the keys to continue"),
            );
            state.progress = None;
        }
        AppEvent::PresetCancelled { preset, reason } => {
            state.status = StatusLine::new(Tone::Warning, format!("{preset} cancelled: {reason}"));
            state.progress = None;
        }
        AppEvent::PresetSaved {
            preset,
            stratagems,
            booster,
        } => {
            state.status = StatusLine::new(
                Tone::Working,
                format!(
                    "Saved {preset}: {} stratagems{}",
                    stratagems.len(),
                    if booster.is_some() { " + booster" } else { "" }
                ),
            );
        }
        AppEvent::UiStateDetected { state: ui_state } => {
            state.status = StatusLine::new(Tone::Working, format!("Detected UI: {ui_state}"));
        }
        AppEvent::ListSelectionStarted {
            item_kind,
            requested_items,
        } => {
            state.progress = Some((0, requested_items));
            state.status = StatusLine::new(
                Tone::Working,
                format!("Selecting {}", item_kind.label()),
            );
        }
        AppEvent::FallbackBoosterRequested { preset } => {
            state.status = StatusLine::new(
                Tone::Warning,
                format!(
                    "{preset}: the saved Booster is already in use. Select another to save it as the fallback, or return to cancel."
                ),
            );
        }
        AppEvent::ItemSelected => {
            if let Some((done, total)) = &mut state.progress {
                *done += 1;
                state.status = StatusLine::new(Tone::Working, format!("Selected {done}/{total}"));
            } else {
                state.status = StatusLine::new(Tone::Working, "Selected item");
            }
        }
        AppEvent::PresetDone { preset, completion } => {
            state.progress = None;
            state.status = match completion {
                PresetCompletion::Complete => StatusLine::new(Tone::Success, format!("{preset} done")),
                PresetCompletion::BoosterUnavailable => {
                    StatusLine::new(Tone::Warning, format!("{preset}: Booster already in use"))
                }
                PresetCompletion::FallbackBoosterSaved { path } => StatusLine::new(
                    Tone::Success,
                    format!("{preset}: fallback Booster saved ({path})"),
                ),
                PresetCompletion::FallbackBoosterNotSaved => {
                    StatusLine::new(Tone::Warning, format!("{preset}: fallback Booster not saved"))
                }
            };
        }
        AppEvent::PresetFailed { preset, error } => {
            state.progress = None;
            state.status = StatusLine::new(
                Tone::Error,
                format!("{preset} failed: {}", first_line(&error)),
            );
        }
    });
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

/// Configuration spelling of a lowercase serde enum such as `MenuMode`.
fn enum_config_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}
