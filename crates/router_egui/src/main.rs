// A release build opens no console window next to the app window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The egui window of EVE Router. It has the layout and the functions of the TUI, with a
//! system search in place of the text input.

mod app;
mod pilots_view;
mod search;
mod settings_window;
mod theme;
mod view;

use app::RouterApp;

/// mimalloc is faster than the system allocator for the many small maps of the route searches.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EVE Router")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([1260.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("EVE Router", options, Box::new(|cc| Ok(Box::new(RouterApp::new(cc.egui_ctx.clone())))))
}
