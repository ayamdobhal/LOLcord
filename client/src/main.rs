mod audio;
mod crypto;
mod devices;
mod hotkeys;
mod jitter;
mod net;
mod tray;
mod ui;
mod voice;

use eframe::egui;

fn main() -> eframe::Result {
    // Log to file on Windows so we can debug
    let log_file = std::fs::File::create("voicechat.log").ok();
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([700.0, 450.0])
            .with_min_inner_size([400.0, 300.0])
            .with_title("voicechat"),
        ..Default::default()
    };

    eframe::run_native(
        "voicechat",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
}
