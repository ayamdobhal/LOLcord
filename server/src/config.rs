use serde::Deserialize;
use std::path::Path;
use tracing::info;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub max_users_per_room: usize,
    pub max_rooms: usize,
    pub server_name: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0".into(),
            tcp_port: shared::DEFAULT_PORT,
            udp_port: shared::DEFAULT_VOICE_PORT,
            max_users_per_room: shared::MAX_ROOM_USERS,
            max_rooms: 50,
            server_name: "voicechat-server".into(),
        }
    }
}

impl ServerConfig {
    /// Load config from `config.toml` (if it exists), then override with env vars.
    pub fn load() -> Self {
        let mut cfg = if Path::new("config.toml").exists() {
            match std::fs::read_to_string("config.toml") {
                Ok(contents) => match toml::from_str::<ServerConfig>(&contents) {
                    Ok(c) => {
                        info!("loaded config from config.toml");
                        c
                    }
                    Err(e) => {
                        tracing::warn!("failed to parse config.toml: {e}, using defaults");
                        ServerConfig::default()
                    }
                },
                Err(e) => {
                    tracing::warn!("failed to read config.toml: {e}, using defaults");
                    ServerConfig::default()
                }
            }
        } else {
            info!("no config.toml found, using defaults");
            ServerConfig::default()
        };

        // Env vars override config file
        if let Ok(v) = std::env::var("BIND_ADDR") {
            cfg.bind_addr = v;
        }
        if let Ok(v) = std::env::var("PORT") {
            if let Ok(p) = v.parse() {
                cfg.tcp_port = p;
            }
        }
        if let Ok(v) = std::env::var("VOICE_PORT") {
            if let Ok(p) = v.parse() {
                cfg.udp_port = p;
            }
        }
        if let Ok(v) = std::env::var("MAX_USERS") {
            if let Ok(n) = v.parse() {
                cfg.max_users_per_room = n;
            }
        }
        if let Ok(v) = std::env::var("MAX_ROOMS") {
            if let Ok(n) = v.parse() {
                cfg.max_rooms = n;
            }
        }
        if let Ok(v) = std::env::var("SERVER_NAME") {
            cfg.server_name = v;
        }

        cfg
    }

    pub fn log(&self) {
        info!("=== Server Configuration ===");
        info!("  server_name: {}", self.server_name);
        info!("  bind_addr: {}", self.bind_addr);
        info!("  tcp_port: {}", self.tcp_port);
        info!("  udp_port: {}", self.udp_port);
        info!("  max_users_per_room: {}", self.max_users_per_room);
        info!("  max_rooms: {}", self.max_rooms);
        info!("============================");
    }
}
