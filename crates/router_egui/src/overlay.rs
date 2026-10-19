//! The routing overlay. It works on Windows only: it reads the title and the program of the focused
//! window with the `windows` crate. Other systems get an `Overlay` that does nothing.

#[cfg(windows)]
mod mode;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use mode::{DisplayMode, Geometry};
#[cfg(windows)]
pub use window::Overlay;

#[cfg(not(windows))]
#[derive(Default)]
pub struct Overlay;

#[cfg(not(windows))]
impl Overlay {
    pub fn logic(&mut self, _ctx: &egui::Context, _s: &crate::app::Session) {}

    pub fn show(&mut self, _ctx: &egui::Context, _s: &crate::app::Session) {}
}
