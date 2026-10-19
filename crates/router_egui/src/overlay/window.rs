//! The overlay window: a small window above the game that helps with a departure. It shows what to
//! do at the next wormhole or filament of the active route.
//!
//! The window exists while the overlay is on and the pilot is in the system that the next hop leaves.
//! It shows only
//! while the client of the route character has the focus. The window never takes the focus, and
//! a click on it does not take the focus, so the client keeps the focus and the title that the probe reads.
//!
//! The probe runs in `logic`. eframe calls `logic` also while the main window is minimized, and
//! a command to show the overlay window then starts the pass that draws it. The window is drawn in `show`.

use super::mode::DisplayMode;
use super::win;
use crate::app::Session;
use crate::pilots_view::system_name;
use crate::theme;
use egui::{Context, CursorIcon, Frame, Id, Margin, Pos2, RichText, Sense, TextStyle, ViewportBuilder, ViewportCommand, ViewportId, vec2};
use router_core::config::{Place, Pos};
use router_core::routing_overlay::{DEFAULT_POS, content, content_for_title, parse_client_title, resolve_position};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The title of the window. It cannot match the title of a client, which starts with "EVE - ".
const TITLE: &str = "eve-router overlay";

/// The time between two reads of the focused window.
const POLL: Duration = Duration::from_millis(250);

/// The title of the dummy window of the position editor. It cannot match the title of a client either.
const EDITOR_TITLE: &str = "eve-router overlay position";

/// The size of the window, in points.
const SIZE: [f32; 2] = [260.0, 58.0];

fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("routing-overlay")
}

fn editor_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("routing-overlay-editor")
}

#[derive(Default)]
pub struct Overlay {
    /// The window exists: `show` ran in the last pass that drew it. A new window starts hidden.
    alive: bool,
    /// The last command that `logic` sent was `Visible(true)`.
    shown: bool,
    last_poll: Option<Instant>,
    /// The display mode of the client of each character, from the last read. The key is the character name.
    modes: HashMap<String, DisplayMode>,
    /// The position editor, while the user moves the dummy window.
    editor: Option<Editor>,
}

/// The position editor: a dummy window that the user drags to the place for the overlay.
struct Editor {
    place: Place,
    /// The position of the dummy window in the last pass that drew it. `None` before the first pass.
    at: Option<Pos>,
}

/// The position of a window in the config: the corner of the window, in whole points.
fn pos_of(corner: Pos2) -> Pos {
    Pos { x: corner.x.round() as i32, y: corner.y.round() as i32 }
}

/// The command for a change of what the overlay wants. `None`: nothing to send.
fn transition(want: bool, alive: bool, shown: bool) -> Option<bool> {
    match (want, shown) {
        (true, false) if alive => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// The system of the route character, from the location that the tracker polls.
fn here(s: &Session, route: &router_core::esi::active::ActiveRoute) -> Option<u32> {
    s.pilots.live.get(&route.character).and_then(|live| live.system)
}

impl Overlay {
    /// Open the dummy window for a place. A dummy window for another place closes.
    pub fn begin_edit(&mut self, place: Place) {
        self.editor = Some(Editor { place, at: None });
    }

    /// The place that the user moves, while the dummy window is open.
    pub fn editing(&self) -> Option<Place> {
        self.editor.as_ref().map(|e| e.place)
    }

    /// Close the dummy window and keep the old position.
    pub fn cancel_edit(&mut self) {
        self.editor = None;
    }

    /// Close the dummy window. The result is the place and the position of the window, for the
    /// caller to save. A window that no pass has drawn gives `None`.
    pub fn finish_edit(&mut self) -> Option<(Place, Pos)> {
        let editor = self.editor.take()?;
        Some((editor.place, editor.at?))
    }

    /// The display mode that the last read of the client of this character gave. A character that
    /// the overlay did not see in front yet gives `None`.
    pub fn display_mode(&self, character_name: &str) -> Option<DisplayMode> {
        self.modes.get(character_name).copied()
    }

    /// Read the focused window, and show or hide the overlay window. With the overlay off, or with
    /// no active route, no call to Windows happens. During a route, the read of the focused window
    /// repeats each `POLL`, and it records the display mode of a client that it finds.
    pub fn logic(&mut self, ctx: &Context, s: &Session) {
        let config = &s.cfg.routing_overlay;
        let route = s.pilots.active.as_ref().filter(|_| config.enabled && self.editor.is_none());
        let Some(route) = route else {
            // The dummy window replaces the overlay window while the editor is open.
            self.alive = false;
            self.last_poll = None;
            self.send(ctx, false, None);
            return;
        };
        ctx.request_repaint_after(POLL);
        let here = here(s, route);
        let departing = content(route, here).is_some();
        if !departing {
            // `show` draws no window, so no window exists. The next one starts hidden.
            self.alive = false;
        }
        if self.last_poll.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        // Read the title first. Ask for the program only when the title is the title of a client.
        let client = win::foreground().and_then(|fg| {
            let title = fg.title()?;
            let name = parse_client_title(&title)?.to_owned();
            fg.is_eve_client().then_some((fg, title, name))
        });
        if let Some((fg, _, name)) = &client
            && let Some(geometry) = fg.geometry()
        {
            self.modes.insert(name.clone(), geometry.mode());
        }
        let want = departing && client.is_some_and(|(_, title, _)| content_for_title(&title, Some(route), here).is_some());
        self.send(ctx, want, resolve_position(config, route.character));
    }

    fn send(&mut self, ctx: &Context, want: bool, position: Option<Pos>) {
        let Some(visible) = transition(want, self.alive, self.shown) else { return };
        if visible {
            // Square corners, as the other windows of the app have. A click on the window must not
            // take the focus from the client.
            crate::chrome::square_window_titled(TITLE);
            win::keep_focus_off(TITLE);
            let Pos { x, y } = position.unwrap_or(DEFAULT_POS);
            ctx.send_viewport_cmd_to(viewport_id(), ViewportCommand::OuterPosition(Pos2::new(x as f32, y as f32)));
        }
        ctx.send_viewport_cmd_to(viewport_id(), ViewportCommand::Visible(visible));
        self.shown = visible;
    }

    /// Draw the window, while the overlay is on and the pilot is in the system that the next
    /// wormhole or filament leaves. The window starts hidden. `logic` shows it.
    pub fn show(&mut self, ctx: &Context, s: &Session) {
        let config = &s.cfg.routing_overlay;
        if let Some(editor) = &mut self.editor {
            show_editor(ctx, config, editor);
            return;
        }
        let Some(route) = s.pilots.active.as_ref().filter(|_| config.enabled) else { return };
        let Some(content) = content(route, here(s, route)) else { return };
        let at = resolve_position(config, route.character).unwrap_or(DEFAULT_POS);
        let builder = window_builder(TITLE, at)
            // The client keeps the focus: the window does not activate when it shows.
            .with_active(false)
            // No click-through and no transparency: both make Windows treat the window as a layered
            // window, and the window then draws nothing. `keep_focus_off` keeps the focus on the client.
            .with_visible(false);
        let (title, detail) = content.lines(|id| system_name(s, id));
        ctx.show_viewport_immediate(viewport_id(), builder, |ui, _class| paint(ui, &title, &detail));
        self.alive = true;
    }
}

/// The builder of the overlay window and of the dummy window: the same size, no frame, always on top.
fn window_builder(title: &str, at: Pos) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(SIZE)
        .with_position(Pos2::new(at.x as f32, at.y as f32))
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
}

/// Draw the dummy window and read its position. The user drags the window with the primary button.
fn show_editor(ctx: &Context, config: &router_core::config::RoutingOverlayConfig, editor: &mut Editor) {
    let start = match editor.place {
        Place::Global => config.position,
        Place::Character(id) => resolve_position(config, id),
    };
    let builder = window_builder(EDITOR_TITLE, start.unwrap_or(DEFAULT_POS));
    ctx.show_viewport_immediate(editor_viewport_id(), builder, |ui, _class| {
        paint(ui, "Signature: ABC", "Drag to move");
        let drag = ui.interact(ui.ctx().content_rect(), Id::new("overlay-editor-drag"), Sense::drag());
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if let Some(rect) = ui.ctx().input(|i| i.viewport().outer_rect) {
            editor.at = Some(pos_of(rect.min));
        }
    });
}

/// Draw the two lines of the window.
fn paint(ui: &mut egui::Ui, title: &str, detail: &str) {
    // The window is the box: the dark fill of the app, the lit line of the main window on its top
    // edge, and the text styles of the app. The window never has the focus, so the line is always lit.
    let panel = Frame::new().fill(theme::BG).inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 8 });
    egui::CentralPanel::default().frame(panel).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        ui.label(RichText::new(title).text_style(TextStyle::Heading).color(theme::accent(ui)));
        ui.label(RichText::new(detail).color(theme::TEXT_DIM));
    });
    crate::chrome::paint_top_line(ui.ctx(), theme::accent(ui));
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

    #[test]
    fn the_corner_of_a_window_rounds_to_whole_points() {
        assert_eq!(pos_of(Pos2::new(40.4, 40.5)), Pos { x: 40, y: 41 });
        assert_eq!(pos_of(Pos2::new(-12.6, 3.0)), Pos { x: -13, y: 3 });
    }

    #[test]
    fn the_editor_gives_the_place_and_the_position_once() {
        let mut o = Overlay::default();
        assert_eq!(o.editing(), None);
        o.begin_edit(Place::Character(7));
        assert_eq!(o.editing(), Some(Place::Character(7)));
        o.editor.as_mut().unwrap().at = Some(Pos { x: 10, y: 20 });
        assert_eq!(o.finish_edit(), Some((Place::Character(7), Pos { x: 10, y: 20 })));
        assert_eq!(o.editing(), None);
        assert_eq!(o.finish_edit(), None);
    }

    #[test]
    fn a_dummy_window_that_never_drew_gives_no_position() {
        let mut o = Overlay::default();
        o.begin_edit(Place::Global);
        assert_eq!(o.finish_edit(), None);
        assert_eq!(o.editing(), None);
    }

    #[test]
    fn cancel_keeps_nothing() {
        let mut o = Overlay::default();
        o.begin_edit(Place::Global);
        o.editor.as_mut().unwrap().at = Some(Pos { x: 1, y: 1 });
        o.cancel_edit();
        assert_eq!(o.editing(), None);
        assert_eq!(o.finish_edit(), None);
    }

    #[test]
    fn a_second_edit_replaces_the_first() {
        let mut o = Overlay::default();
        o.begin_edit(Place::Global);
        o.editor.as_mut().unwrap().at = Some(Pos { x: 1, y: 1 });
        o.begin_edit(Place::Character(9));
        assert_eq!(o.editing(), Some(Place::Character(9)));
        assert_eq!(o.finish_edit(), None);
    }
}
