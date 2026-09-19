//! The window: live state, presets, the active loadout, mission hotkeys, and settings.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use egui::{Color32, ColorImage, RichText, TextureHandle, TextureOptions, Vec2};

use crate::active_loadout::{ActiveLoadout, now_unix};
use crate::app_state::{AppHandle, HotkeyStatus, LiveSettings, PresetSummary, StatusLine, Tone, UiCommand};
use crate::catalog::{Catalog, StratagemEntry};
use crate::input::{HotkeyBinding, HotkeyModifier, HotkeyModifiers, Key};
use crate::item::StratagemCategory;
use crate::stratagem_input::{DIRECTIONS, DirectionKeys, MenuMode, StratagemInputSettings};

const APP_ICON_ICO: &[u8] = include_bytes!("../assets/app-icon.ico");
const ICON_ROW: u32 = 26;
const ICON_PRESET: u32 = 52;
const ICON_ACTIVE: u32 = 80;
const REPAINT_INTERVAL: Duration = Duration::from_millis(500);

pub struct WindowOptions {
    pub start_hidden: bool,
}

pub fn run(handle: AppHandle, catalog: Arc<Catalog>, options: WindowOptions) -> Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Xilous Stratagems Manager")
        .with_app_id("xilous-stratagems-manager")
        .with_inner_size([1240.0, 820.0])
        .with_min_inner_size([960.0, 620.0]);
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Xilous Stratagems Manager",
        native_options,
        Box::new(move |creation| Ok(Box::new(App::new(creation, handle, catalog, options)))),
    )
    .map_err(|error| anyhow!("the window could not be created: {error}"))
}

fn load_window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory_with_format(APP_ICON_ICO, image::ImageFormat::Ico).ok()?;
    let rgba = image.to_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

struct Snapshot {
    game_foreground: bool,
    busy: bool,
    status: StatusLine,
    progress: Option<(usize, usize)>,
    hotkeys: HotkeyStatus,
    active_loadout: Option<ActiveLoadout>,
    presets: Vec<PresetSummary>,
    settings: LiveSettings,
    fatal_error: Option<String>,
    exit_requested: bool,
}

struct App {
    handle: AppHandle,
    catalog: Arc<Catalog>,
    options: WindowOptions,
    first_frame: bool,
    icons: HashMap<(String, u32), Option<TextureHandle>>,
    label_edits: HashMap<String, String>,
    pending_delete: Option<String>,
    input_draft: Option<StratagemInputSettings>,
    show_log: bool,
}

impl App {
    fn new(
        creation: &eframe::CreationContext<'_>,
        handle: AppHandle,
        catalog: Arc<Catalog>,
        options: WindowOptions,
    ) -> Self {
        creation.egui_ctx.set_visuals(egui::Visuals::dark());
        creation.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = Vec2::new(8.0, 6.0);
            style.spacing.button_padding = Vec2::new(8.0, 4.0);
        });
        handle.update(|state| state.egui_ctx = Some(creation.egui_ctx.clone()));
        Self {
            handle,
            catalog,
            options,
            first_frame: true,
            icons: HashMap::new(),
            label_edits: HashMap::new(),
            pending_delete: None,
            input_draft: None,
            show_log: true,
        }
    }

    fn snapshot(&self) -> Snapshot {
        self.handle.snapshot(|state| Snapshot {
            game_foreground: state.game_foreground,
            busy: state.busy,
            status: state.status.clone(),
            progress: state.progress,
            hotkeys: state.hotkeys.clone(),
            active_loadout: state.active_loadout.clone(),
            presets: state.presets.clone(),
            settings: state.settings.clone(),
            fatal_error: state.fatal_error.clone(),
            exit_requested: state.exit_requested,
        })
    }

    fn icon(&mut self, ctx: &egui::Context, id: &str, size: u32) -> Option<TextureHandle> {
        let key = (id.to_string(), size);
        if let Some(texture) = self.icons.get(&key) {
            return texture.clone();
        }
        let texture = self
            .catalog
            .render_icon(id, size)
            .ok()
            .map(|image| {
                let color_image = ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                );
                ctx.load_texture(format!("{id}@{size}"), color_image, TextureOptions::LINEAR)
            });
        self.icons.insert(key, texture.clone());
        texture
    }

    fn show_icon(&mut self, ui: &mut egui::Ui, id: Option<&str>, size: u32) {
        let side = size as f32;
        match id.and_then(|id| self.icon(ui.ctx(), id, size)) {
            Some(texture) => {
                ui.add(egui::Image::from_texture(&texture).fit_to_exact_size(Vec2::splat(side)));
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 4.0, Color32::from_gray(45));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "?",
                    egui::FontId::proportional(side * 0.5),
                    Color32::from_gray(120),
                );
            }
        }
    }

    /// Dropdown over the loadout stratagems. Returns `Some(new value)` on change.
    fn stratagem_picker(
        &self,
        ui: &mut egui::Ui,
        salt: &str,
        current: Option<&str>,
        width: f32,
    ) -> Option<Option<String>> {
        let selected_text = current.map_or_else(|| "Unknown".to_string(), |id| self.catalog.name_of(id));
        let mut changed = None;
        egui::ComboBox::from_id_salt(salt)
            .selected_text(selected_text)
            .width(width)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "Unknown").clicked() {
                    changed = Some(None);
                }
                for category in [
                    StratagemCategory::Offensive,
                    StratagemCategory::Supply,
                    StratagemCategory::Defensive,
                ] {
                    ui.separator();
                    ui.label(RichText::new(category_title(category)).small().strong());
                    for entry in self
                        .catalog
                        .loadout_entries()
                        .filter(|entry| entry.category() == Some(category))
                    {
                        let selected = current == Some(entry.id.as_str());
                        if ui
                            .selectable_label(selected, &entry.name)
                            .on_hover_text(entry_details(entry))
                            .clicked()
                        {
                            changed = Some(Some(entry.id.clone()));
                        }
                    }
                }
            });
        changed
    }

    /// Key dropdown plus modifier toggles. Returns `Some(new binding)` on change.
    fn binding_editor(
        &self,
        ui: &mut egui::Ui,
        salt: &str,
        current: Option<HotkeyBinding>,
        allow_unassigned: bool,
    ) -> Option<Option<HotkeyBinding>> {
        let mut changed = None;
        let modifiers = current.map_or(HotkeyModifiers::none(), |binding| binding.modifiers);
        let selected_text = current.map_or_else(|| "Unassigned".to_string(), |binding| binding.key.name().to_string());
        egui::ComboBox::from_id_salt(salt)
            .selected_text(selected_text)
            .width(110.0)
            .show_ui(ui, |ui| {
                if allow_unassigned && ui.selectable_label(current.is_none(), "Unassigned").clicked() {
                    changed = Some(None);
                }
                for key in Key::bindable() {
                    let selected = current.is_some_and(|binding| binding.key == key);
                    if ui.selectable_label(selected, key.name()).clicked() {
                        changed = Some(Some(HotkeyBinding::new(modifiers, key)));
                    }
                }
            });
        if let Some(binding) = current {
            for modifier in HotkeyModifier::ALL {
                let enabled = binding.modifiers.contains(modifier);
                if ui
                    .selectable_label(enabled, modifier.display_name())
                    .on_hover_text("Toggle this modifier")
                    .clicked()
                {
                    changed = Some(Some(HotkeyBinding::new(
                        binding.modifiers.toggled(modifier, !enabled),
                        binding.key,
                    )));
                }
            }
        }
        changed
    }

    // ----- panels -------------------------------------------------------------

    fn top_panel(&mut self, root: &mut egui::Ui, snapshot: &Snapshot) {
        egui::Panel::top("top").show(root, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.heading("Xilous Stratagems Manager");
                ui.separator();
                chip(
                    ui,
                    "Helldivers 2",
                    if snapshot.game_foreground { "focused" } else { "not focused" },
                    if snapshot.game_foreground { Color32::from_rgb(90, 200, 120) } else { Color32::from_gray(150) },
                );
                chip(
                    ui,
                    "Hotkeys",
                    if snapshot.hotkeys.armed { "armed" } else { "off" },
                    if snapshot.hotkeys.armed { Color32::from_rgb(90, 200, 120) } else { Color32::from_gray(150) },
                )
                .on_hover_text(&snapshot.hotkeys.detail);
                ui.separator();
                if snapshot.busy {
                    ui.spinner();
                }
                let mut status = snapshot.status.text.clone();
                if let Some((done, total)) = snapshot.progress {
                    status.push_str(&format!("  {done}/{total}"));
                }
                ui.label(RichText::new(status).color(tone_color(snapshot.status.tone)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Exit")
                        .on_hover_text("Stop the tool completely (closing the window only minimizes it)")
                        .clicked()
                    {
                        self.handle.send(UiCommand::Exit);
                    }
                });
            });
            if let Some(error) = &snapshot.fatal_error {
                ui.colored_label(
                    tone_color(Tone::Error),
                    format!("Automation stopped: {error}. Fix the problem and restart the application."),
                );
            }
            ui.add_space(4.0);
        });
    }

    fn presets_panel(&mut self, root: &mut egui::Ui, snapshot: &Snapshot) {
        egui::Panel::left("presets")
            .resizable(true)
            .default_size(560.0)
            .size_range(420.0..=900.0)
            .show(root, |ui| {
                ui.add_space(6.0);
                ui.heading("Loadout presets");
                ui.label(
                    RichText::new(
                        "On the loadout screen: full loadout + hotkey saves, empty loadout + hotkey applies.",
                    )
                    .small()
                    .color(Color32::from_gray(170)),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for preset in &snapshot.presets {
                            self.preset_card(ui, preset, snapshot);
                            ui.add_space(6.0);
                        }
                    });
            });
    }

    fn preset_card(&mut self, ui: &mut egui::Ui, preset: &PresetSummary, snapshot: &Snapshot) {
        let is_active = snapshot
            .active_loadout
            .as_ref()
            .is_some_and(|active| active.preset == preset.name);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Preset {}", preset.index + 1)).strong().size(16.0));
                ui.label(RichText::new(&preset.hotkey_label).monospace().color(Color32::from_gray(190)));
                let text = self
                    .label_edits
                    .entry(preset.name.clone())
                    .or_insert_with(|| preset.label.clone());
                let response = ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text("Label")
                        .desired_width(150.0),
                );
                if response.lost_focus() && text.trim() != preset.label.trim() {
                    self.handle.send(UiCommand::SetLabel {
                        preset: preset.name.clone(),
                        label: text.clone(),
                    });
                }
                if is_active {
                    ui.label(RichText::new("ACTIVE").color(Color32::from_rgb(90, 200, 120)).strong());
                }
            });

            if !preset.saved {
                ui.label(
                    RichText::new(format!(
                        "Not saved yet. Fill the loadout in game and press {}.",
                        preset.hotkey_label
                    ))
                    .color(Color32::from_gray(160)),
                );
                if let Some(problem) = &preset.problem {
                    ui.colored_label(tone_color(Tone::Error), problem);
                }
                return;
            }

            ui.horizontal(|ui| {
                for (slot, id) in preset.stratagems.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.set_width(118.0);
                        ui.horizontal(|ui| {
                            ui.add_space((118.0 - ICON_PRESET as f32) / 2.0);
                            self.show_icon(ui, id.as_deref(), ICON_PRESET);
                        });
                        if let Some(entry) = id.as_deref().and_then(|id| self.catalog.get(id)) {
                            ui.label(RichText::new(entry.arrows()).monospace().small());
                        } else {
                            ui.label(RichText::new("not identified").small().color(tone_color(Tone::Warning)));
                        }
                        if let Some(change) = self.stratagem_picker(
                            ui,
                            &format!("{}-slot-{slot}", preset.name),
                            id.as_deref(),
                            112.0,
                        ) {
                            self.handle.send(UiCommand::SetPresetStratagem {
                                preset: preset.name.clone(),
                                slot,
                                id: change,
                            });
                        }
                    });
                }
            });

            ui.horizontal(|ui| {
                let booster = match (preset.booster, preset.fallback_booster) {
                    (true, true) => "Booster + fallback",
                    (true, false) => "Booster",
                    _ => "No booster",
                };
                ui.label(RichText::new(booster).small().color(Color32::from_gray(170)));
                if let Some(problem) = &preset.problem {
                    ui.colored_label(tone_color(Tone::Error), problem);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.pending_delete.as_deref() == Some(preset.name.as_str()) {
                        if ui.button(RichText::new("Confirm delete").color(tone_color(Tone::Error))).clicked() {
                            self.handle.send(UiCommand::DeletePreset {
                                preset: preset.name.clone(),
                            });
                            self.pending_delete = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.pending_delete = None;
                        }
                    } else if ui.button("Delete").clicked() {
                        self.pending_delete = Some(preset.name.clone());
                    }
                    if !is_active && ui.button("Set as active").on_hover_text("Use this preset for the slot hotkeys without touching the game").clicked() {
                        self.handle.send(UiCommand::SetActiveFromPreset {
                            preset: preset.name.clone(),
                        });
                    }
                    if ui.button("Identify").on_hover_text("Re-run stratagem identification on the captured icons").clicked() {
                        self.handle.send(UiCommand::IdentifyPreset {
                            preset: preset.name.clone(),
                        });
                    }
                });
            });
        });
    }

    fn active_section(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.heading("Active loadout");
        let Some(active) = &snapshot.active_loadout else {
            ui.label(
                RichText::new(
                    "None. Save or apply a preset on the loadout screen, or use “Set as active” on a preset. Slot hotkeys stay off until then.",
                )
                .color(Color32::from_gray(170)),
            );
            return;
        };

        let label = snapshot
            .settings
            .labels
            .get(&active.preset)
            .filter(|label| !label.trim().is_empty())
            .map_or_else(|| active.preset.clone(), |label| format!("{} ({label})", active.preset));
        ui.horizontal(|ui| {
            ui.label(format!(
                "{label} · {} · {}",
                active.source.label(),
                time_ago(active.set_at_unix)
            ));
            if ui.button("Clear").clicked() {
                self.handle.send(UiCommand::ClearActive);
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for slot in 0..4 {
                let id = active.slot(slot).map(str::to_string);
                let key_label = snapshot
                    .settings
                    .slot_keys
                    .get(slot)
                    .map_or_else(|| "?".to_string(), |binding| binding.label());
                ui.group(|ui| {
                    ui.set_width(150.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(key_label).strong().size(16.0));
                        self.show_icon(ui, id.as_deref(), ICON_ACTIVE);
                        match id.as_deref().and_then(|id| self.catalog.get(id)) {
                            Some(entry) => {
                                ui.label(RichText::new(&entry.name).strong());
                                ui.label(RichText::new(entry.arrows()).monospace().size(18.0));
                            }
                            None => {
                                ui.label(RichText::new("Not identified").color(tone_color(Tone::Warning)));
                                ui.label(RichText::new("pick below").small());
                            }
                        }
                        if let Some(change) =
                            self.stratagem_picker(ui, &format!("active-slot-{slot}"), id.as_deref(), 140.0)
                        {
                            self.handle.send(UiCommand::SetActiveSlot { slot, id: change });
                        }
                    });
                });
            }
        });
        if !snapshot.settings.mission_enabled {
            ui.colored_label(tone_color(Tone::Warning), "Mission hotkeys are disabled in settings.");
        } else if !snapshot.game_foreground {
            ui.label(
                RichText::new("Slot hotkeys arm automatically while Helldivers 2 is focused.")
                    .small()
                    .color(Color32::from_gray(170)),
            );
        }
    }

    /// The keys the game expects for the Stratagem menu and directions, plus
    /// input timing. Edits are staged in a draft until Apply.
    fn keybinds_section(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        egui::CollapsingHeader::new(RichText::new("Game keybinds").heading())
            .default_open(true)
            .show(ui, |ui| {
                let settings = &snapshot.settings;
                ui.label(
                    RichText::new(
                        "Set these to match Options → Mouse & Keyboard in Helldivers 2. Arrow keys for directions are strongly recommended so movement keys never corrupt a code.",
                    )
                    .small()
                    .color(Color32::from_gray(170)),
                );
                let mut draft = self
                    .input_draft
                    .clone()
                    .unwrap_or_else(|| settings.stratagem_input.clone());
                let mut revert = false;
                let draft = &mut draft;

                egui::Grid::new("keybinds")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Stratagem menu");
                        ui.horizontal(|ui| {
                            key_picker(ui, "menu-key", &mut draft.menu_key);
                            egui::ComboBox::from_id_salt("menu-mode")
                                .selected_text(draft.menu_mode.label())
                                .show_ui(ui, |ui| {
                                    for mode in MenuMode::ALL {
                                        ui.selectable_value(&mut draft.menu_mode, mode, mode.label());
                                    }
                                });
                            ui.label(
                                RichText::new("hold = menu open while held; press = key toggles it")
                                    .small()
                                    .color(Color32::from_gray(150)),
                            );
                        });
                        ui.end_row();

                        for direction in DIRECTIONS {
                            ui.label(format!("Direction {}", direction.arrow()));
                            let mut key = draft.direction_key(direction);
                            if key_picker(ui, &format!("direction-{}", direction.arrow()), &mut key) {
                                draft.set_direction_key(direction, key);
                            }
                            ui.end_row();
                        }

                        ui.label("Layout");
                        ui.horizontal(|ui| {
                            for layout in DirectionKeys::ALL {
                                if ui
                                    .selectable_label(draft.matches_layout(layout), layout.label())
                                    .clicked()
                                {
                                    draft.apply_layout(layout);
                                }
                            }
                        });
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.add(egui::Slider::new(&mut draft.menu_open_delay_ms, 0..=400).suffix(" ms").text("Wait after opening the menu"));
                ui.add(egui::Slider::new(&mut draft.key_hold_ms, 10..=200).suffix(" ms").text("Hold each direction key"));
                ui.add(egui::Slider::new(&mut draft.key_gap_ms, 0..=300).suffix(" ms").text("Gap between directions"));
                ui.add(egui::Slider::new(&mut draft.menu_release_delay_ms, 0..=400).suffix(" ms").text("Wait before releasing the menu key"));

                let dirty = *draft != settings.stratagem_input;
                let problem = draft.validate().err().map(|error| format!("{error:#}"));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(dirty && problem.is_none(), egui::Button::new("Apply"))
                        .clicked()
                    {
                        self.handle.send(UiCommand::SetStratagemInput(draft.clone()));
                    }
                    if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                        revert = true;
                    }
                    if let Some(problem) = &problem {
                        ui.colored_label(tone_color(Tone::Error), problem);
                    } else if dirty {
                        ui.label(
                            RichText::new("Unsaved changes")
                                .small()
                                .color(tone_color(Tone::Warning)),
                        );
                    } else {
                        ui.label(
                            RichText::new(format!(
                                "A 5-arrow code takes about {} ms.",
                                settings.stratagem_input.estimated_duration(5).as_millis()
                            ))
                            .small()
                            .color(Color32::from_gray(160)),
                        );
                    }
                });
                self.input_draft = if revert || !dirty {
                    None
                } else {
                    Some(draft.clone())
                };
            });
    }

    fn mission_section(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        egui::CollapsingHeader::new(RichText::new("Mission stratagems").heading())
            .default_open(true)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Available in every mission regardless of loadout. Assign a key to use one; unassigned ones are inactive.",
                    )
                    .small()
                    .color(Color32::from_gray(170)),
                );
                let entries = self.catalog.mission_entries().cloned().collect::<Vec<StratagemEntry>>();
                let conflicts = binding_conflicts(&snapshot.settings);
                egui::Grid::new("mission-bindings")
                    .num_columns(4)
                    .spacing([10.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for entry in &entries {
                            let binding = snapshot.settings.bindings.get(&entry.id).copied();
                            ui.horizontal(|ui| {
                                self.show_icon(ui, Some(&entry.id), ICON_ROW);
                                ui.label(&entry.name)
                                    .on_hover_text(entry_details(entry));
                            });
                            ui.label(RichText::new(entry.arrows()).monospace());
                            ui.horizontal(|ui| {
                                if let Some(change) =
                                    self.binding_editor(ui, &format!("binding-{}", entry.id), binding, true)
                                {
                                    self.handle.send(UiCommand::SetBinding {
                                        id: entry.id.clone(),
                                        binding: change,
                                    });
                                }
                            });
                            if let Some(binding) = binding
                                && conflicts.contains(&binding)
                            {
                                ui.colored_label(tone_color(Tone::Error), "conflict");
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn settings_section(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        egui::CollapsingHeader::new(RichText::new("Settings").heading())
            .default_open(false)
            .show(ui, |ui| {
                let settings = &snapshot.settings;

                ui.label(RichText::new("Preset hotkeys").strong());
                ui.horizontal_wrapped(|ui| {
                    for (index, label) in settings.preset_hotkey_labels.iter().enumerate() {
                        ui.label(format!("Preset {}: ", index + 1));
                        ui.label(RichText::new(label).monospace());
                    }
                });
                ui.label(
                    RichText::new("Change these under [hotkey] in data/config.toml and restart.")
                        .small()
                        .color(Color32::from_gray(160)),
                );
                let mut apply_in_saved_order = settings.apply_in_saved_order;
                if ui.checkbox(&mut apply_in_saved_order, "Apply stratagems in saved order").changed() {
                    self.handle.send(UiCommand::SetPresetFlag {
                        key: "apply_in_saved_order",
                        value: apply_in_saved_order,
                    });
                }
                let mut auto_ready_up = settings.auto_ready_up;
                if ui.checkbox(&mut auto_ready_up, "Press B (ready up) after applying a preset with a booster").changed() {
                    self.handle.send(UiCommand::SetPresetFlag {
                        key: "auto_ready_up",
                        value: auto_ready_up,
                    });
                }
                let mut save_fallback = settings.save_fallback_when_taken;
                if ui.checkbox(&mut save_fallback, "If the saved booster is taken, save a manually selected fallback").changed() {
                    self.handle.send(UiCommand::SetPresetFlag {
                        key: "save_fallback_when_taken",
                        value: save_fallback,
                    });
                }

                ui.add_space(8.0);
                ui.label(RichText::new("Mission hotkeys").strong());
                let mut mission_enabled = settings.mission_enabled;
                if ui.checkbox(&mut mission_enabled, "Enable mission hotkeys (slot keys and mission stratagems)").changed() {
                    self.handle.send(UiCommand::SetMissionEnabled(mission_enabled));
                }
                ui.label("Slot keys (active loadout slots 1-4):");
                let conflicts = binding_conflicts(settings);
                for slot in 0..4 {
                    let current = settings.slot_keys.get(slot).copied();
                    ui.horizontal(|ui| {
                        ui.label(format!("Slot {}", slot + 1));
                        if let Some(Some(binding)) =
                            self.binding_editor(ui, &format!("slot-key-{slot}"), current, false)
                        {
                            let mut keys = settings.slot_keys.clone();
                            while keys.len() < 4 {
                                keys.push(HotkeyBinding::bare(Key::F1));
                            }
                            keys[slot] = binding;
                            self.handle.send(UiCommand::SetSlotKeys(keys));
                        }
                        if current.is_some_and(|binding| conflicts.contains(&binding)) {
                            ui.colored_label(tone_color(Tone::Error), "conflict");
                        }
                    });
                }

                ui.add_space(8.0);
                ui.label(RichText::new("Catalog").strong());
                ui.label(
                    RichText::new(format!(
                        "{} stratagems, synced {} from {}. Re-run tools/sync-stratagems.ps1 and rebuild after a warbond adds stratagems.",
                        self.catalog.entries().len(),
                        self.catalog.synced_at(),
                        self.catalog.source()
                    ))
                    .small()
                    .color(Color32::from_gray(160)),
                );
            });
    }

    fn log_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(170.0)
            .size_range(60.0..=500.0)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.show_log, "Activity log");
                    ui.label(
                        RichText::new("Closing the window keeps the tool running; exit from the tray icon.")
                            .small()
                            .color(Color32::from_gray(150)),
                    );
                });
                if !self.show_log {
                    return;
                }
                let lines = self.handle.log.lines();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &lines {
                            let color = if line.contains(" WARN ") {
                                tone_color(Tone::Warning)
                            } else if line.contains(" ERROR ") {
                                tone_color(Tone::Error)
                            } else {
                                Color32::from_gray(200)
                            };
                            ui.label(RichText::new(line).monospace().small().color(color));
                        }
                    });
            });
    }
}

impl eframe::App for App {
    /// Runs before every frame and also while the window is minimized or hidden,
    /// so tray requests and exit are handled even when nothing is painted.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            if self.options.start_hidden {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }

        let (exit_requested, show_window_requested) = self
            .handle
            .snapshot(|state| (state.exit_requested, state.show_window_requested));
        if exit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if show_window_requested {
            self.handle.update(|state| state.show_window_requested = false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            // Keep running in the tray; the tray menu exits.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        ctx.request_repaint_after(REPAINT_INTERVAL);
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snapshot = self.snapshot();
        if snapshot.exit_requested {
            return;
        }

        self.top_panel(root, &snapshot);
        self.log_panel(root);
        self.presets_panel(root, &snapshot);
        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    self.active_section(ui, &snapshot);
                    ui.add_space(12.0);
                    self.keybinds_section(ui, &snapshot);
                    ui.add_space(12.0);
                    self.mission_section(ui, &snapshot);
                    ui.add_space(12.0);
                    self.settings_section(ui, &snapshot);
                    ui.add_space(12.0);
                });
        });
    }
}

/// Dropdown over every key the tool can send. Returns whether it changed.
fn key_picker(ui: &mut egui::Ui, salt: &str, key: &mut Key) -> bool {
    let before = *key;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(key.name())
        .width(110.0)
        .show_ui(ui, |ui| {
            for candidate in Key::ALL {
                ui.selectable_value(key, candidate, candidate.name());
            }
        });
    *key != before
}

fn chip(ui: &mut egui::Ui, name: &str, value: &str, color: Color32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{name}:")).color(Color32::from_gray(170)));
        ui.label(RichText::new(value).color(color).strong());
    })
    .response
}

fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Info => Color32::from_gray(210),
        Tone::Working => Color32::from_rgb(240, 200, 90),
        Tone::Success => Color32::from_rgb(90, 200, 120),
        Tone::Warning => Color32::from_rgb(240, 160, 70),
        Tone::Error => Color32::from_rgb(235, 90, 90),
    }
}

fn entry_details(entry: &StratagemEntry) -> String {
    let mut details = entry.arrows();
    if !entry.kind.is_empty() {
        details.push_str(&format!("\n{}", entry.kind));
    }
    if !entry.cooldown.is_empty() {
        details.push_str(&format!("\nCooldown: {}", entry.cooldown));
    }
    details
}

fn category_title(category: StratagemCategory) -> &'static str {
    match category {
        StratagemCategory::Offensive => "Offensive",
        StratagemCategory::Supply => "Supply",
        StratagemCategory::Defensive => "Defensive",
    }
}

/// Bindings used more than once across slot keys and mission stratagems.
fn binding_conflicts(settings: &LiveSettings) -> Vec<HotkeyBinding> {
    let mut seen: Vec<HotkeyBinding> = Vec::new();
    let mut conflicts = Vec::new();
    let all = settings
        .slot_keys
        .iter()
        .copied()
        .chain(settings.bindings.values().copied());
    for binding in all {
        if seen.contains(&binding) {
            if !conflicts.contains(&binding) {
                conflicts.push(binding);
            }
        } else {
            seen.push(binding);
        }
    }
    conflicts
}

fn time_ago(set_at_unix: u64) -> String {
    let elapsed = now_unix().saturating_sub(set_at_unix);
    if elapsed < 60 {
        "just now".to_string()
    } else if elapsed < 3_600 {
        format!("{} min ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{} h ago", elapsed / 3_600)
    } else {
        format!("{} d ago", elapsed / 86_400)
    }
}
