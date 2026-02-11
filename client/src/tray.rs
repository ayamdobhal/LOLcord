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

    // Create a simple 16x16 RGBA icon (green circle)
    let size = 16u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = ((y * size + x) * 4) as usize;
            if dist < center - 1.0 {
                rgba[idx] = 80;     // R
                rgba[idx + 1] = 200; // G
                rgba[idx + 2] = 80;  // B
                rgba[idx + 3] = 255; // A
            }
        }
    }

    let icon = Icon::from_rgba(rgba, size, size).ok()?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("voicechat")
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
