use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub server_addr: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    #[serde(default)]
    pub ptt_bind: Option<String>,
    #[serde(default)]
    pub use_open_mic: Option<bool>,
    #[serde(default)]
    pub selected_input_device: Option<String>,
    #[serde(default)]
    pub selected_output_device: Option<String>,
    #[serde(default)]
    pub noise_suppression: Option<bool>,
    #[serde(default)]
    pub noise_gate_threshold: Option<f32>,
    #[serde(default)]
    pub vad_sensitivity: Option<f32>,
}

fn settings_path() -> PathBuf {
    // Same directory as the executable, next to lolcord.log
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lolcord-settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = settings_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}
