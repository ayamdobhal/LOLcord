use crate::audio::AudioState;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
    Icon,
};

pub enum TrayCommand {
    Show,
    ToggleMute,
    ToggleDeafen,
    Quit,
}

/// Create a basic tray icon. Returns the TrayIcon (must be kept alive) and a
/// receiver for menu commands. Must be called from the main/UI thread.
pub fn create_tray() -> Option<(TrayIcon, std::sync::mpsc::Receiver<TrayCommand>)> {
    let menu = Menu::new();
    let show_item = MenuItem::new("Show", true, None);
    let mute_item = MenuItem::new("Mute", true, None);
    let deafen_item = MenuItem::new("Deafen", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let show_id = show_item.id().clone();
    let mute_id = mute_item.id().clone();
    let deafen_id = deafen_item.id().clone();
    let quit_id = quit_item.id().clone();

    menu.append(&show_item).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&mute_item).ok()?;
    menu.append(&deafen_item).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit_item).ok()?;

    // Load icon from embedded PNG
    let icon = {
        let bytes = include_bytes!("../assets/icon.png");
        let img = image::load_from_memory(bytes).ok()?.into_rgba8();
        let (w, h) = img.dimensions();
        Icon::from_rgba(img.into_raw(), w, h).ok()?
    };

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("LOLcord")
        .with_icon(icon)
        .build()
        .ok()?;

    let (tx, rx) = std::sync::mpsc::channel();

    // Spawn menu event listener thread
    std::thread::Builder::new()
        .name("tray-events".into())
        .spawn(move || {
            loop {
                if let Ok(event) = MenuEvent::receiver().recv() {
                    let cmd = if event.id == show_id {
                        TrayCommand::Show
                    } else if event.id == mute_id {
                        TrayCommand::ToggleMute
                    } else if event.id == deafen_id {
                        TrayCommand::ToggleDeafen
                    } else if event.id == quit_id {
                        TrayCommand::Quit
                    } else {
                        continue;
                    };
                    if tx.send(cmd).is_err() {
                        break;
                    }
                }
            }
        })
        .ok()?;

    Some((tray, rx))
}
