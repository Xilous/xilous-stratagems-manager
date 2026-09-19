use std::sync::mpsc::{Sender, SyncSender, sync_channel};
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_QUIT,
};

use super::TrayEvent;

pub(super) struct WindowsTray {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Drop for WindowsTray {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl WindowsTray {
    pub(super) fn spawn(event_tx: Sender<TrayEvent>) -> Result<Self> {
        let (ready_tx, ready_rx) = sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("xilous-stratagems-manager-tray".to_string())
            .spawn(move || {
                let tray_thread_id = unsafe { GetCurrentThreadId() };
                let result = run_tray(tray_thread_id, &ready_tx, event_tx);
                if let Err(error) = result {
                    let _ = ready_tx.send(Err(format!("{error:#}")));
                }
            })
            .context("failed to start tray thread")?;
        let thread_id = ready_rx
            .recv()
            .context("tray thread ended before initialization")?
            .map_err(|error| anyhow!(error))?;

        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

fn run_tray(
    tray_thread_id: u32,
    ready_tx: &SyncSender<std::result::Result<u32, String>>,
    event_tx: Sender<TrayEvent>,
) -> Result<()> {
    let show_item = MenuItem::with_id("show", "Show window", true, None);
    let show_id = show_item.id().clone();
    let separator = PredefinedMenuItem::separator();
    let close_item = MenuItem::with_id("close", "Exit", true, None);
    let close_id = close_item.id().clone();
    let menu = Menu::with_items(&[&show_item, &separator, &close_item])
        .context("failed to create tray menu")?;
    let icon = tray_icon()?;
    let _tray = TrayIconBuilder::new()
        .with_tooltip("Xilous Stratagems Manager")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .context("failed to create tray icon")?;

    let menu_tx = event_tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let tray_event = if event.id == show_id {
            TrayEvent::ShowWindow
        } else if event.id == close_id {
            TrayEvent::ExitRequested
        } else {
            return;
        };
        let exiting = matches!(tray_event, TrayEvent::ExitRequested);
        let _ = menu_tx.send(tray_event);
        if exiting {
            let _ = unsafe { PostThreadMessageW(tray_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = event_tx.send(TrayEvent::ShowWindow);
        }
    }));

    ready_tx
        .send(Ok(tray_thread_id))
        .context("failed to signal tray readiness")?;

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn tray_icon() -> Result<Icon> {
    Icon::from_resource(1, Some((32, 32))).context("failed to load embedded tray icon")
}
