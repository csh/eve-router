//! The overlay window: a small window above the game that shows the signature of the next
//! wormhole of the active route.
//!
//! The window exists while the overlay is on and the route has a wormhole ahead. It shows only
//! while the client of the route character has the focus. The window never takes the focus, and
//! mouse clicks go through it, so the client keeps the focus and the title that the probe reads.
//!
//! The probe runs in `logic`. eframe calls `logic` also while the main window is minimized, and
//! a command to show the overlay window then starts the pass that draws it. The window is drawn in `show`.

use super::win;
use crate::app::Session;
use crate::pilots_view::system_name;
use crate::theme;
use egui::{Context, Frame, Margin, Pos2, RichText, Stroke, ViewportBuilder, ViewportCommand, ViewportId, vec2};
use router_core::config::Pos;
use router_core::routing_overlay::{Content, Signature, content, content_for_title, resolve_position};
use std::time::{Duration, Instant};

/// The title of the window. It cannot match the title of a client, which starts with "EVE - ".
const TITLE: &str = "eve-router overlay";

/// The time between two reads of the focused window.
const POLL: Duration = Duration::from_millis(250);

/// The place of the window for a character that has none of its own, and no global place.
const DEFAULT_POS: Pos = Pos { x: 40, y: 40 };

fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("routing-overlay")
}

#[derive(Default)]
pub struct Overlay {
    /// The window exists: `show` ran in the last pass that drew it. A new window starts hidden.
    alive: bool,
    /// The last command that `logic` sent was `Visible(true)`.
    shown: bool,
    last_poll: Option<Instant>,
}

/// The command for a change of what the overlay wants. `None`: nothing to send.
fn transition(want: bool, alive: bool, shown: bool) -> Option<bool> {
    match (want, shown) {
        (true, false) if alive => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

impl Overlay {
    /// Read the focused window, and show or hide the overlay window. With the overlay off, or with
    /// no wormhole ahead, no call to Windows happens.
    pub fn logic(&mut self, ctx: &Context, s: &Session) {
        let config = &s.cfg.routing_overlay;
        let route = s.pilots.active.as_ref().filter(|r| config.enabled && content(r).is_some());
        let Some(route) = route else {
            self.alive = false;
            self.last_poll = None;
            self.send(ctx, false, None);
            return;
        };
        ctx.request_repaint_after(POLL);
        if self.last_poll.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        // Read the title first. Ask for the program only when the title is the title of the route character.
        let want = win::foreground()
            .is_some_and(|fg| fg.title().is_some_and(|title| content_for_title(&title, Some(route)).is_some()) && fg.is_eve_client());
        self.send(ctx, want, resolve_position(config, route.character));
    }

    fn send(&mut self, ctx: &Context, want: bool, position: Option<Pos>) {
        let Some(visible) = transition(want, self.alive, self.shown) else { return };
        if visible {
            let Pos { x, y } = position.unwrap_or(DEFAULT_POS);
            ctx.send_viewport_cmd_to(viewport_id(), ViewportCommand::OuterPosition(Pos2::new(x as f32, y as f32)));
        }
        ctx.send_viewport_cmd_to(viewport_id(), ViewportCommand::Visible(visible));
        self.shown = visible;
    }

    /// Draw the window, while the overlay is on and the route has a wormhole ahead. The window
    /// starts hidden. `logic` shows it.
    pub fn show(&mut self, ctx: &Context, s: &Session) {
        let config = &s.cfg.routing_overlay;
        let Some(route) = s.pilots.active.as_ref().filter(|_| config.enabled) else { return };
        let Some(content) = content(route) else { return };
        let Pos { x, y } = resolve_position(config, route.character).unwrap_or(DEFAULT_POS);
        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([260.0, 64.0])
            .with_position(Pos2::new(x as f32, y as f32))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            // The client keeps the focus: the window does not activate, and clicks go through it.
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_visible(false);
        ctx.show_viewport_immediate(viewport_id(), builder, |ui, _class| draw(ui, s, &content));
        self.alive = true;
    }
}

fn draw(ui: &mut egui::Ui, s: &Session, content: &Content) {
    let frame = Frame::new().fill(theme::BG).stroke(Stroke::new(1.0, theme::accent(ui))).inner_margin(Margin::symmetric(12, 8));
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            let line = match &content.signature {
                Signature::Known(letters) => RichText::new(letters).monospace().size(22.0).strong().color(theme::accent(ui)),
                Signature::CheckBookmarks => RichText::new("Check bookmarks").size(20.0).strong().color(theme::WARN),
            };
            ui.label(line);
            ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
            let route = format!("{} to {}", system_name(s, content.from), system_name(s, content.to));
            ui.label(RichText::new(route).size(12.0).color(theme::TEXT_DIM));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_shows_when_the_client_has_the_focus_and_the_window_exists() {
        assert_eq!(transition(true, true, false), Some(true));
    }

    #[test]
    fn a_window_that_does_not_exist_yet_gets_no_command() {
        assert_eq!(transition(true, false, false), None);
    }

    #[test]
    fn the_window_hides_when_the_client_loses_the_focus() {
        assert_eq!(transition(false, true, true), Some(false));
        // Also when the window is gone: the next window starts hidden.
        assert_eq!(transition(false, false, true), Some(false));
    }

    #[test]
    fn no_change_sends_nothing() {
        assert_eq!(transition(true, true, true), None);
        assert_eq!(transition(false, true, false), None);
        assert_eq!(transition(false, false, false), None);
    }
}
