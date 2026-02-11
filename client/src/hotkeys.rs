use crate::audio::AudioState;
use device_query::{DeviceQuery, DeviceState, Keycode};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A PTT binding — either a keyboard key or a mouse button.
#[derive(Clone, Debug, PartialEq)]
pub enum PttBind {
    Key(Keycode),
    Mouse(u8), // button number: 2=right, 3=middle, 4=mouse4, 5=mouse5
}

impl PttBind {
    /// Display name for the binding.
    pub fn display_name(&self) -> String {
        match self {
            PttBind::Key(kc) => keycode_to_name(kc),
            PttBind::Mouse(btn) => mouse_button_name(*btn),
        }
    }

    /// Serialize to a string for settings persistence.
    pub fn to_setting_string(&self) -> String {
        match self {
            PttBind::Key(kc) => keycode_to_name(kc),
            PttBind::Mouse(btn) => mouse_button_name(*btn),
        }
    }

    /// Deserialize from a settings string.
    pub fn from_setting_string(s: &str) -> Option<PttBind> {
        match s {
            "Mouse2" | "MouseRight" => Some(PttBind::Mouse(2)),
            "Mouse3" | "MouseMiddle" => Some(PttBind::Mouse(3)),
            "Mouse4" => Some(PttBind::Mouse(4)),
            "Mouse5" => Some(PttBind::Mouse(5)),
            other => keycode_from_name(other).map(PttBind::Key),
        }
    }
}

fn mouse_button_name(btn: u8) -> String {
    match btn {
        2 => "Mouse2 (Right)".into(),
        3 => "Mouse3 (Middle)".into(),
        4 => "Mouse4".into(),
        5 => "Mouse5".into(),
        n => format!("Mouse{n}"),
    }
}

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
        _ => {
            // Try single char
            let upper = name.to_uppercase();
            if upper.len() == 1 {
                let c = upper.chars().next()?;
                match c {
                    'A' => Some(Keycode::A), 'B' => Some(Keycode::B), 'C' => Some(Keycode::C),
                    'D' => Some(Keycode::D), 'E' => Some(Keycode::E), 'F' => Some(Keycode::F),
                    'G' => Some(Keycode::G), 'H' => Some(Keycode::H), 'I' => Some(Keycode::I),
                    'J' => Some(Keycode::J), 'K' => Some(Keycode::K), 'L' => Some(Keycode::L),
                    'M' => Some(Keycode::M), 'N' => Some(Keycode::N), 'O' => Some(Keycode::O),
                    'P' => Some(Keycode::P), 'Q' => Some(Keycode::Q), 'R' => Some(Keycode::R),
                    'S' => Some(Keycode::S), 'T' => Some(Keycode::T), 'U' => Some(Keycode::U),
                    'V' => Some(Keycode::V), 'W' => Some(Keycode::W), 'X' => Some(Keycode::X),
                    'Y' => Some(Keycode::Y), 'Z' => Some(Keycode::Z),
                    '0' => Some(Keycode::Key0), '1' => Some(Keycode::Key1),
                    '2' => Some(Keycode::Key2), '3' => Some(Keycode::Key3),
                    '4' => Some(Keycode::Key4), '5' => Some(Keycode::Key5),
                    '6' => Some(Keycode::Key6), '7' => Some(Keycode::Key7),
                    '8' => Some(Keycode::Key8), '9' => Some(Keycode::Key9),
                    _ => None,
                }
            } else {
                None
            }
        }
    }
}

/// Convert a Keycode back to a display name.
pub fn keycode_to_name(kc: &Keycode) -> String {
    match kc {
        Keycode::A => "A", Keycode::B => "B", Keycode::C => "C", Keycode::D => "D",
        Keycode::E => "E", Keycode::F => "F", Keycode::G => "G", Keycode::H => "H",
        Keycode::I => "I", Keycode::J => "J", Keycode::K => "K", Keycode::L => "L",
        Keycode::M => "M", Keycode::N => "N", Keycode::O => "O", Keycode::P => "P",
        Keycode::Q => "Q", Keycode::R => "R", Keycode::S => "S", Keycode::T => "T",
        Keycode::U => "U", Keycode::V => "V", Keycode::W => "W", Keycode::X => "X",
        Keycode::Y => "Y", Keycode::Z => "Z",
        Keycode::Key0 => "0", Keycode::Key1 => "1", Keycode::Key2 => "2",
        Keycode::Key3 => "3", Keycode::Key4 => "4", Keycode::Key5 => "5",
        Keycode::Key6 => "6", Keycode::Key7 => "7", Keycode::Key8 => "8",
        Keycode::Key9 => "9",
        Keycode::F1 => "F1", Keycode::F2 => "F2", Keycode::F3 => "F3",
        Keycode::F4 => "F4", Keycode::F5 => "F5", Keycode::F6 => "F6",
        Keycode::F7 => "F7", Keycode::F8 => "F8", Keycode::F9 => "F9",
        Keycode::F10 => "F10", Keycode::F11 => "F11", Keycode::F12 => "F12",
        Keycode::Space => "Space", Keycode::Tab => "Tab",
        Keycode::CapsLock => "CapsLock",
        Keycode::LShift => "LShift", Keycode::RShift => "RShift",
        Keycode::LControl => "LCtrl", Keycode::RControl => "RCtrl",
        Keycode::LAlt => "LAlt", Keycode::RAlt => "RAlt",
        Keycode::Escape => "Escape", Keycode::Enter => "Enter",
        Keycode::Backspace => "Backspace",
        Keycode::Up => "Up", Keycode::Down => "Down",
        Keycode::Left => "Left", Keycode::Right => "Right",
        Keycode::Grave => "`", Keycode::Minus => "-", Keycode::Equal => "=",
        Keycode::LeftBracket => "[", Keycode::RightBracket => "]",
        Keycode::BackSlash => "\\", Keycode::Semicolon => ";",
        Keycode::Apostrophe => "'", Keycode::Comma => ",",
        Keycode::Dot => ".", Keycode::Slash => "/",
        other => return format!("{:?}", other),
    }.to_string()
}

/// Poll for any input (key or mouse button press). Returns the first detected.
/// Used for the "press any key/button" PTT binding capture.
/// Filters out Mouse1 (left click) to avoid UI conflicts.
pub fn capture_any_input() -> Option<PttBind> {
    let device_state = DeviceState::new();

    // Check keyboard first
    let keys = device_state.get_keys();
    if let Some(kc) = keys.into_iter().next() {
        return Some(PttBind::Key(kc));
    }

    // Check mouse buttons (skip button 1 = left click)
    let mouse = device_state.get_mouse();
    for btn in 2..mouse.button_pressed.len() {
        if mouse.button_pressed[btn] {
            return Some(PttBind::Mouse(btn as u8));
        }
    }

    None
}

/// Polls global keyboard and mouse state in a background thread.
/// Updates `audio.ptt_active` based on the configured PTT binding.
pub fn spawn_global_hotkey_thread(
    audio: Arc<AudioState>,
    ptt_bind: Arc<std::sync::Mutex<PttBind>>,
    running: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || {
            let device_state = DeviceState::new();
            while running.load(Ordering::Relaxed) {
                let bind = ptt_bind.lock().unwrap_or_else(|e| e.into_inner()).clone();

                let pressed = match &bind {
                    PttBind::Key(key) => {
                        let keys = device_state.get_keys();
                        keys.contains(key)
                    }
                    PttBind::Mouse(btn) => {
                        let mouse = device_state.get_mouse();
                        let idx = *btn as usize;
                        idx < mouse.button_pressed.len() && mouse.button_pressed[idx]
                    }
                };
                audio.ptt_active.store(pressed, Ordering::Relaxed);

                // Poll at ~100Hz
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
        .ok();
}
