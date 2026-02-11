use egui;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

pub enum TrayCommand {
    Show,
    ToggleMute,
    ToggleDeafen,
    Quit,
}

pub struct TrayState {
    pub tray: TrayIcon,
    pub rx: std::sync::mpsc::Receiver<TrayCommand>,
    pub mute_item: CheckMenuItem,
    pub deafen_item: CheckMenuItem,
    /// Set this to the egui Context so tray events can wake the UI loop
    pub egui_ctx: std::sync::Arc<std::sync::Mutex<Option<egui::Context>>>,
}

/// Create a tray icon with menu. Returns TrayState with handles to update menu items.
pub fn create_tray() -> Option<TrayState> {
    let menu = Menu::new();
    let show_item = MenuItem::new("Show", true, None);
    let mute_item = CheckMenuItem::new("Mute", true, false, None);
    let deafen_item = CheckMenuItem::new("Deafen", true, false, None);
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
    let egui_ctx: std::sync::Arc<std::sync::Mutex<Option<egui::Context>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let ctx_ref = egui_ctx.clone();

    // Tray icon double-click listener
    let click_tx = tx.clone();
    let click_ctx = egui_ctx.clone();
    std::thread::Builder::new()
        .name("tray-click".into())
        .spawn(move || {
            loop {
                if let Ok(event) = TrayIconEvent::receiver().recv() {
                    if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                        let _ = click_tx.send(TrayCommand::Show);
                        if let Ok(guard) = click_ctx.lock() {
                            if let Some(ref ctx) = *guard {
                                ctx.request_repaint();
                            }
                        }
                    }
                }
            }
        })
        .ok()?;

    // Menu event listener
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
                    // Wake the UI event loop immediately
                    if let Ok(guard) = ctx_ref.lock() {
                        if let Some(ref ctx) = *guard {
                            ctx.request_repaint();
                        }
                    }
                }
            }
        })
        .ok()?;

    Some(TrayState {
        tray,
        rx,
        mute_item,
        deafen_item,
        egui_ctx,
    })
}
