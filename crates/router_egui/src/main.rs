// A release build opens no console window next to the app window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The egui window of EVE Router. This first version loads the map and shows the route summaries.

mod app;

use app::RouterApp;

/// mimalloc is faster than the system allocator for the many small maps of the route searches.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions::default();
    eframe::run_native("EVE Router", options, Box::new(|cc| Ok(Box::new(RouterApp::new(cc.egui_ctx.clone())))))
}
