mod net;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "client=info".into()),
        )
        .init();

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
