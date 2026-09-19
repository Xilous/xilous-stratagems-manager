#![cfg_attr(windows, windows_subsystem = "windows")]

mod active_loadout;
mod app_events;
mod app_state;
mod assets;
mod automation;
mod capture;
mod catalog;
mod color_normalization;
mod config;
mod engine;
mod game_settings;
mod game_window;
mod image_rect;
mod input;
mod item;
mod loadout;
mod permissions;
mod preset;
mod preset_action;
mod stratagem_input;
mod tray;
mod ui;
mod vision;
mod window;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{Level, error, info};
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::prelude::*;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW,
};
use windows::core::PCWSTR;

use crate::app_state::{AppHandle, LogBuffer};
use crate::catalog::Catalog;
use crate::config::{PRESETS_RELATIVE_PATH, load_app_config};
use crate::preset::archive_legacy_preset_file;

const CONFIG_RELATIVE_PATH: &str = "data/config.toml";
const LOG_RELATIVE_PATH: &str = "data/app.log";
#[cfg(feature = "diagnostics")]
const DIAGNOSTIC_SCORES_RELATIVE_PATH: &str = "data/diagnostics/matcher-scores.jsonl";
const ACTIVE_LOADOUT_RELATIVE_PATH: &str = "data/active_loadout.json";
const LAST_FAILURE_DEBUG_PATH: &str = "data/debug/last_failure";

fn main() -> ExitCode {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            show_fatal_error(&error);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let log_path = app_path(LOG_RELATIVE_PATH)?;
    let log_buffer = LogBuffer::default();
    let _log_guard = init_tracing(&log_path, log_buffer.clone())?;
    #[cfg(feature = "diagnostics")]
    vision::init_diagnostics(&app_path(DIAGNOSTIC_SCORES_RELATIVE_PATH)?)?;
    let config_path = app_path(CONFIG_RELATIVE_PATH)?;
    let presets_path = app_path(PRESETS_RELATIVE_PATH)?;
    let active_path = app_path(ACTIVE_LOADOUT_RELATIVE_PATH)?;
    let failure_debug_dir = app_path(LAST_FAILURE_DEBUG_PATH)?;

    let result = (|| -> Result<()> {
        let (config, notify_reset) = load_app_config(&config_path)?;
        let legacy_presets = archive_legacy_preset_file(&presets_path)?;
        if notify_reset {
            show_config_reset(&config_path, &presets_path);
        }
        if let Some(backup_path) = legacy_presets {
            info!(
                path = %presets_path.display(),
                backup = %backup_path.display(),
                "legacy preset data archived"
            );
            show_preset_format_updated(&backup_path);
        }

        let catalog = Arc::new(Catalog::load()?);
        info!(
            stratagems = catalog.entries().len(),
            synced_at = catalog.synced_at(),
            "stratagem catalog loaded"
        );

        let (handle, commands) = AppHandle::new(log_buffer);
        let tray = tray::spawn()?;
        let start_hidden = config.window.start_hidden;
        info!(
            config = %config_path.display(),
            preset_hotkeys = config.hotkey.keys.len(),
            mission_enabled = config.mission.enabled,
            "application starting"
        );
        let engine = engine::spawn(engine::EngineContext {
            config,
            config_path,
            presets_path,
            active_path,
            failure_debug_dir,
            catalog: catalog.clone(),
            handle: handle.clone(),
            commands,
            tray,
        })?;

        let window_result = ui::run(handle.clone(), catalog, ui::WindowOptions { start_hidden });
        handle.request_stop();
        let _ = engine.join();
        window_result
    })();

    if let Err(error) = &result {
        error!(error = %format!("{error:#}"), "application terminated");
    }
    result
}

fn init_tracing(path: &Path, log_buffer: LogBuffer) -> Result<WorkerGuard> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    }
    let file = fs::File::create(path)
        .with_context(|| format!("failed to create log {}", path.display()))?;
    let (writer, guard) = NonBlockingBuilder::default().lossy(false).finish(file);
    let level = if cfg!(feature = "diagnostics") {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_names(true)
        .with_writer(writer)
        .compact()
        .with_filter(
            Targets::new()
                .with_target(module_path!(), level)
                .with_target("xilous_stratagems_manager", level),
        );
    let window_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_timer(tracing_subscriber::fmt::time::uptime())
        .with_writer(log_buffer)
        .compact()
        .with_filter(
            Targets::new()
                .with_target(module_path!(), Level::INFO)
                .with_target("xilous_stratagems_manager", Level::INFO),
        );

    tracing_subscriber::registry()
        .with(file_layer)
        .with(window_layer)
        .init();
    Ok(guard)
}

fn show_fatal_error(error: &anyhow::Error) {
    let error = format!("{error:#}")
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    let log_hint = app_path(LOG_RELATIVE_PATH).map_or_else(
        |_| String::new(),
        |path| {
            format!(
                "\r\n\r\nSee {} for more information if the log was created.",
                path.display()
            )
        },
    );
    let message = format!(
        "Xilous Stratagems Manager could not start or encountered a fatal error.\r\n\r\n{error}{log_hint}"
    );
    let title = wide_null("Xilous Stratagems Manager");
    let message = wide_null(&message);

    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

fn show_config_reset(config_path: &Path, presets_path: &Path) {
    let message = format!(
        "The configuration file was reset for this version.\r\n\r\nSaved presets in\r\n{}\r\nwere not changed.\r\n\r\nIf you previously customized the settings, configure them again in the window or in\r\n{}",
        presets_path.display(),
        config_path.display(),
    );
    let title = wide_null("Xilous Stratagems Manager - Configuration Updated");
    let message = wide_null(&message);

    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
}

fn show_preset_format_updated(backup_path: &Path) {
    let message = format!(
        "The preset format changed in this version.\r\n\r\nPresets created by an earlier version cannot be used and must be recreated in game.\r\n\r\nThe old preset file was backed up to:\r\n{}\r\n\r\nYour configuration was not changed.",
        backup_path.display(),
    );
    let title = wide_null("Xilous Stratagems Manager - Presets Updated");
    let message = wide_null(&message);

    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn app_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let executable = std::env::current_exe().context("failed to locate the executable")?;
    let directory = executable.parent().with_context(|| {
        format!(
            "executable has no parent directory: {}",
            executable.display()
        )
    })?;
    Ok(directory.join(path))
}
