//! State shared between the automation thread and the window, plus the
//! commands the window sends back.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use crate::active_loadout::ActiveLoadout;
use crate::input::HotkeyBinding;
use crate::stratagem_input::StratagemInputSettings;

pub const LOG_CAPACITY: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Working,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct StatusLine {
    pub tone: Tone,
    pub text: String,
}

impl StatusLine {
    pub fn new(tone: Tone, text: impl Into<String>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

impl Default for StatusLine {
    fn default() -> Self {
        Self::new(Tone::Info, "Starting")
    }
}

#[derive(Clone, Debug, Default)]
pub struct PresetSummary {
    /// Internal name, e.g. `preset_1`.
    pub name: String,
    pub index: usize,
    pub hotkey_label: String,
    pub label: String,
    pub saved: bool,
    /// Catalog ids in saved order.
    pub stratagems: [Option<String>; 4],
    pub booster: bool,
    pub fallback_booster: bool,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct HotkeyStatus {
    pub armed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveSettings {
    pub apply_in_saved_order: bool,
    pub auto_ready_up: bool,
    pub save_fallback_when_taken: bool,
    pub labels: BTreeMap<String, String>,
    pub preset_hotkey_labels: Vec<String>,
    pub mission_enabled: bool,
    pub slot_keys: Vec<HotkeyBinding>,
    pub stratagem_input: StratagemInputSettings,
    pub bindings: BTreeMap<String, HotkeyBinding>,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            apply_in_saved_order: false,
            auto_ready_up: false,
            save_fallback_when_taken: false,
            labels: BTreeMap::new(),
            preset_hotkey_labels: Vec::new(),
            mission_enabled: true,
            slot_keys: Vec::new(),
            stratagem_input: StratagemInputSettings::default(),
            bindings: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
pub struct SharedState {
    pub game_foreground: bool,
    pub busy: bool,
    pub status: StatusLine,
    /// Items selected so far during an apply, when known.
    pub progress: Option<(usize, usize)>,
    pub hotkeys: HotkeyStatus,
    pub active_loadout: Option<ActiveLoadout>,
    pub presets: Vec<PresetSummary>,
    pub settings: LiveSettings,
    pub fatal_error: Option<String>,
    pub show_window_requested: bool,
    pub exit_requested: bool,
    pub egui_ctx: Option<egui::Context>,
}

pub enum UiCommand {
    SetPresetFlag {
        key: &'static str,
        value: bool,
    },
    SetLabel {
        preset: String,
        label: String,
    },
    SetPresetStratagem {
        preset: String,
        slot: usize,
        id: Option<String>,
    },
    DeletePreset {
        preset: String,
    },
    /// Re-run stratagem identification on the captured icons of a preset.
    IdentifyPreset {
        preset: String,
    },
    SetActiveFromPreset {
        preset: String,
    },
    SetActiveSlot {
        slot: usize,
        id: Option<String>,
    },
    ClearActive,
    SetMissionEnabled(bool),
    SetSlotKeys(Vec<HotkeyBinding>),
    SetStratagemInput(StratagemInputSettings),
    SetBinding {
        id: String,
        binding: Option<HotkeyBinding>,
    },
    Exit,
}

#[derive(Clone)]
pub struct AppHandle {
    pub state: Arc<Mutex<SharedState>>,
    pub log: LogBuffer,
    commands: Sender<UiCommand>,
    stop: Arc<AtomicBool>,
}

impl AppHandle {
    pub fn new(log: LogBuffer) -> (Self, Receiver<UiCommand>) {
        let (commands, receiver) = channel();
        let handle = Self {
            state: Arc::new(Mutex::new(SharedState::default())),
            log,
            commands,
            stop: Arc::new(AtomicBool::new(false)),
        };
        (handle, receiver)
    }

    pub fn send(&self, command: UiCommand) {
        let _ = self.commands.send(command);
    }

    /// Mutates the shared state and asks the window to repaint.
    pub fn update(&self, edit: impl FnOnce(&mut SharedState)) {
        let mut state = self.state.lock().expect("shared state poisoned");
        edit(&mut state);
        if let Some(ctx) = &state.egui_ctx {
            ctx.request_repaint();
        }
    }

    pub fn snapshot<T>(&self, read: impl FnOnce(&SharedState) -> T) -> T {
        let state = self.state.lock().expect("shared state poisoned");
        read(&state)
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Ring buffer of recent log lines shown in the window.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<VecDeque<String>>>);

impl LogBuffer {
    pub fn push(&self, line: String) {
        let mut lines = self.0.lock().expect("log buffer poisoned");
        if lines.len() >= LOG_CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("log buffer poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

pub struct LogWriter {
    buffer: LogBuffer,
    pending: Vec<u8>,
}

impl LogWriter {
    fn flush_lines(&mut self, final_flush: bool) {
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=position).collect();
            self.buffer
                .push(String::from_utf8_lossy(&line).trim_end().to_string());
        }
        if final_flush && !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.buffer
                .push(String::from_utf8_lossy(&line).trim_end().to_string());
        }
    }
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        self.flush_lines(false);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_lines(true);
        Ok(())
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        self.flush_lines(true);
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter {
            buffer: self.clone(),
            pending: Vec::new(),
        }
    }
}
