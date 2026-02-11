#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod crypto;
mod devices;
mod hotkeys;
mod jitter;
mod net;
mod settings;
mod tray;
mod ui;
mod voice;

use eframe::egui;

fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn main() -> eframe::Result {
    // Log to file in config dir (overwrites each launch — no unbounded growth)
    let log_path = settings::config_dir().join("lolcord.log");
    let log_file = std::fs::File::create(log_path).ok();
    if let Some(file) = log_file {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "client=debug".into()),
            )
            .with_writer(std::sync::Mutex::new(file))
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "client=debug".into()),
            )
            .init();
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([700.0, 450.0])
        .with_min_inner_size([400.0, 300.0])
        .with_title("LOLcord");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "LOLcord",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
}
