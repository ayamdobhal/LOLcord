#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui_simple;

use iced::{Application, Settings};

fn main() -> iced::Result {
    ui_simple::App::run(Settings::default())
}