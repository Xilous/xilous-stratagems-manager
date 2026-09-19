use std::sync::mpsc::{Receiver, channel};

use anyhow::Result;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
use windows::WindowsTray as PlatformTray;

pub struct TrayHandle {
    _platform: PlatformTray,
    events: Receiver<TrayEvent>,
}

impl TrayHandle {
    pub fn try_event(&self) -> Option<TrayEvent> {
        self.events.try_recv().ok()
    }
}

pub enum TrayEvent {
    ShowWindow,
    ExitRequested,
}

pub fn spawn() -> Result<TrayHandle> {
    let (event_tx, events) = channel();
    let platform = PlatformTray::spawn(event_tx)?;
    Ok(TrayHandle {
        _platform: platform,
        events,
    })
}
