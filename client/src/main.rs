#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod crypto;
mod devices;
mod hotkeys;
mod jitter;
mod net;
mod screen_capture;
mod screen_decode;
mod settings;
mod tray;
mod ui;
mod voice;

use iced::{Application, Settings};

fn main() -> iced::Result {
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

    let settings = Settings {
        window: iced::window::Settings {
            size: iced::Size::new(800.0, 600.0),
            min_size: Some(iced::Size::new(400.0, 300.0)),
            icon: load_icon(),
            ..iced::window::Settings::default()
        },
        ..Settings::default()
    };

    ui::App::run(settings)
}

fn load_icon() -> Option<iced::window::Icon> {
    // TODO: Fix icon loading for iced 0.12
    // let bytes = include_bytes!("../assets/icon.png");
    // let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    // let (width, height) = img.dimensions();
    // iced::window::Icon::from_rgba(img.into_raw(), width, height).ok()
    None
}