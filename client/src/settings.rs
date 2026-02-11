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

/// Return the `.lolcord` config directory (%APPDATA%/.lolcord on Windows, ~/.lolcord otherwise).
/// Creates the directory if it doesn't exist.
pub fn config_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
    } else {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    };
    let dir = base.join(".lolcord");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
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
