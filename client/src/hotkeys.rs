use crate::audio::AudioState;
use device_query::{DeviceQuery, DeviceState, Keycode};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Maps our key names to device_query Keycodes.
pub fn keycode_from_name(name: &str) -> Option<Keycode> {
    match name.to_uppercase().as_str() {
        "V" => Some(Keycode::V),
        "B" => Some(Keycode::B),
        "N" => Some(Keycode::N),
        "M" => Some(Keycode::M),
        "G" => Some(Keycode::G),
        "T" => Some(Keycode::T),
        "CAPSLOCK" | "CAPS" => Some(Keycode::CapsLock),
        "LALT" | "LEFT ALT" => Some(Keycode::LAlt),
        "RALT" | "RIGHT ALT" => Some(Keycode::RAlt),
        "LCTRL" | "LEFT CTRL" => Some(Keycode::LControl),
        "RCTRL" | "RIGHT CTRL" => Some(Keycode::RControl),
        "LSHIFT" | "LEFT SHIFT" => Some(Keycode::LShift),
        "RSHIFT" | "RIGHT SHIFT" => Some(Keycode::RShift),
        "SPACE" => Some(Keycode::Space),
        "TAB" => Some(Keycode::Tab),
        "F1" => Some(Keycode::F1),
        "F2" => Some(Keycode::F2),
        "F3" => Some(Keycode::F3),
        "F4" => Some(Keycode::F4),
        "F5" => Some(Keycode::F5),
        "F6" => Some(Keycode::F6),
        "F7" => Some(Keycode::F7),
        "F8" => Some(Keycode::F8),
        "F9" => Some(Keycode::F9),
        "F10" => Some(Keycode::F10),
        "F11" => Some(Keycode::F11),
        "F12" => Some(Keycode::F12),
        "MOUSE4" | "MOUSE5" => None, // not supported by device_query
        _ => {
            // Try single char
            if name.len() == 1 {
                let c = name.chars().next()?;
                match c {
                    'A'..='Z' => keycode_from_name(name),
                    '0' => Some(Keycode::Key0),
                    '1' => Some(Keycode::Key1),
                    '2' => Some(Keycode::Key2),
                    '3' => Some(Keycode::Key3),
                    '4' => Some(Keycode::Key4),
                    '5' => Some(Keycode::Key5),
                    '6' => Some(Keycode::Key6),
                    '7' => Some(Keycode::Key7),
                    '8' => Some(Keycode::Key8),
                    '9' => Some(Keycode::Key9),
                    _ => None,
                }
            } else {
                None
            }
        }
    }
}

pub fn key_names() -> &'static [&'static str] {
    &[
        "V", "B", "N", "M", "G", "T",
        "CapsLock", "LAlt", "RAlt", "LCtrl", "RCtrl", "LShift", "RShift",
        "Space", "Tab",
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    ]
}

/// Polls global keyboard state in a background thread.
/// Updates `audio.ptt_active` based on the configured PTT key.
pub fn spawn_global_hotkey_thread(
    audio: Arc<AudioState>,
    ptt_key: Arc<std::sync::Mutex<Keycode>>,
    running: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || {
            let device_state = DeviceState::new();
            while running.load(Ordering::Relaxed) {
                let keys = device_state.get_keys();
                let key = *ptt_key.lock().unwrap_or_else(|e| e.into_inner());

                let pressed = keys.contains(&key);
                audio.ptt_active.store(pressed, Ordering::Relaxed);

                // Poll at ~100Hz
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
        .ok();
}
