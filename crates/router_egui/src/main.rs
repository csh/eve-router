// A release build opens no console window next to the app window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The egui window of EVE Router.

mod app;
mod app_update;
mod chrome;
mod log_window;
mod logging;
mod notice;
mod overlay;
mod pilots_view;
mod search;
mod settings_window;
mod setup_wizard;
mod strip;
#[cfg(test)]
mod test_ui;
mod theme;
mod view;
mod welcome;
mod widgets;
#[cfg(windows)]
mod win32;

use app::RouterApp;

/// mimalloc is faster than the system allocator for the many small maps of the route searches.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> eframe::Result {
    // Keep the value until the program ends. Its drop flushes the log file.
    let _log = logging::init(&router_core::config::AppPaths::detect());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EVE Router")
            .with_decorations(false)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([1280.0, 748.0]),
        ..Default::default()
    };
    eframe::run_native("EVE Router", options, Box::new(|cc| Ok(Box::new(RouterApp::new(cc.egui_ctx.clone())))))
}
