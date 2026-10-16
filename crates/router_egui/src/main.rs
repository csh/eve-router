// A release build opens no console window next to the app window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The egui window of EVE Router.

mod app;
mod chrome;
mod log_window;
mod notice;
mod pilots_view;
mod search;
mod settings_window;
mod strip;
mod theme;
mod view;
mod widgets;

use app::RouterApp;

/// mimalloc is faster than the system allocator for the many small maps of the route searches.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> eframe::Result {
    // The window needs the setting before it opens. A config that does not load gives the default.
    let chrome =
        router_core::config::Config::load(&router_core::config::AppPaths::detect().config).map(|cfg| !cfg.os_frame).unwrap_or(true);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EVE Router")
            .with_decorations(!chrome)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([320.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native("EVE Router", options, Box::new(|cc| Ok(Box::new(RouterApp::new(cc.egui_ctx.clone(), chrome)))))
}
