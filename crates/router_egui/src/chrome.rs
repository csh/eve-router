//! The window frame that the app draws itself, in the style of the Rift app: a thin title bar
//! with the title at the left and the window buttons at the right, a 1 px border, and a lit line
//! on the top edge. The OS draws none of it, so the window has no OS title bar.

use egui::viewport::ResizeDirection;
use egui::{
    Color32, Context, CursorIcon, Frame, Id, LayerId, Margin, Order, Panel, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, ViewportCommand,
    vec2,
};

use crate::theme;

/// The height of the title bar.
const TITLE_HEIGHT: f32 = 32.0;
/// The width of the room of one window button: a 16 px icon box and an 8 px gap.
const BUTTON_WIDTH: f32 = 24.0;
/// The width of the zone at a window edge that starts a resize.
const EDGE: f32 = 4.0;
/// The length of the zone at a corner that starts a diagonal resize.
const CORNER: f32 = 10.0;
/// The icon of a window button.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Show the title bar: the title and the dim `detail` at the left, the window buttons at the right.
/// The bar moves the window when the pilot drags it. A double click maximizes the window.
pub fn title_bar(ui: &mut Ui, title: &str, detail: &str) {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
    let frame = Frame::new().fill(theme::BG).inner_margin(Margin::ZERO);
    Panel::top("title-bar").resizable(false).exact_size(TITLE_HEIGHT).frame(frame).show(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TITLE_HEIGHT), Sense::hover());
        // The drag area comes first, so the buttons on top of it win the pointer.
        let drag = ui.interact(rect, Id::new("title-drag"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        let accent = theme::accent(ui);
        let painter = ui.painter_at(rect);
        let middle = rect.center().y;
        paint_logo(&painter, Pos2::new(rect.left() + 20.0, middle), accent);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let title_rect = painter.text(Pos2::new(rect.left() + 38.0, middle), egui::Align2::LEFT_CENTER, title, font.clone(), theme::TEXT);
        if !detail.is_empty() {
            painter.text(Pos2::new(title_rect.right() + 8.0, middle), egui::Align2::LEFT_CENTER, detail, font, theme::TEXT_DIM);
        }

        let button = |from_right: f32| {
            Rect::from_min_size(Pos2::new(rect.right() - 8.0 - BUTTON_WIDTH * from_right, rect.top()), vec2(BUTTON_WIDTH, TITLE_HEIGHT))
        };
        if caption_button(ui, button(1.0), "close", Icon::Close) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        let toggle = if maximized { Icon::Restore } else { Icon::Maximize };
        if caption_button(ui, button(2.0), "maximize", toggle) {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        if caption_button(ui, button(3.0), "minimize", Icon::Minimize) {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    });
}

/// The mark of the app: three stops joined by a route line.
fn paint_logo(painter: &egui::Painter, center: Pos2, accent: Color32) {
    let stops = [center + vec2(-6.0, 3.0), center + vec2(0.0, -4.0), center + vec2(6.0, 3.0)];
    painter.line_segment([stops[0], stops[1]], Stroke::new(1.0, accent));
    painter.line_segment([stops[1], stops[2]], Stroke::new(1.0, accent));
    for stop in stops {
        painter.circle_filled(stop, 2.0, accent);
    }
}

/// One window button: a small icon with no box. Under the pointer the icon turns light and glows,
/// as the buttons of the Rift app do. Returns true when the pilot clicks it.
fn caption_button(ui: &mut Ui, rect: Rect, name: &str, icon: Icon) -> bool {
    let response = ui.interact(rect, Id::new(("caption", name)), Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    // Half a pixel off the center puts the 1 px lines on whole pixels.
    let c = Pos2::new(rect.center().x.floor() + 0.5, rect.center().y.floor() + 0.5);
    let draw = |stroke: Stroke| match icon {
        Icon::Minimize => _ = painter.line_segment([c + vec2(-4.0, 3.0), c + vec2(4.0, 3.0)], stroke),
        Icon::Maximize => _ = painter.rect_stroke(Rect::from_center_size(c, vec2(8.0, 8.0)), 0.0, stroke, StrokeKind::Middle),
        Icon::Restore => {
            painter.rect_stroke(Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(6.0, 6.0)), 0.0, stroke, StrokeKind::Middle);
            // The window behind: the top and the right line only.
            let back = Rect::from_center_size(c + vec2(1.0, -1.0), vec2(6.0, 6.0));
            painter.line_segment([back.left_top() + vec2(2.0, 0.0), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom() - vec2(0.0, 2.0)], stroke);
        }
        Icon::Close => {
            painter.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], stroke);
            painter.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], stroke);
        }
    };
    if hovered {
        // egui has no blur, so a wide faint copy under the icon stands in for the glow.
        draw(Stroke::new(4.0, theme::accent(ui).gamma_multiply(0.25)));
        draw(Stroke::new(1.0, theme::accent_light(ui)));
    } else {
        draw(Stroke::new(1.0, theme::TEXT_DIM));
    }
    response.clicked()
}

/// Draw the border of the window and the lit line on its top edge. Call it last in the frame.
/// The line has the accent color while the window has the focus, else the color of the border.
pub fn paint_border(ctx: &Context, accent: Color32) {
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("window-border")));
    let rect = ctx.content_rect();
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, theme::WINDOW_BORDER), StrokeKind::Inside);
    if ctx.input(|i| i.viewport().focused).unwrap_or(true) {
        let top = |y: f32| [Pos2::new(rect.left(), rect.top() + y), Pos2::new(rect.right(), rect.top() + y)];
        painter.line_segment(top(0.5), Stroke::new(1.0, accent));
        // egui has no blur, so two fading lines stand in for the glow.
        painter.line_segment(top(2.0), Stroke::new(2.0, accent.gamma_multiply(0.25)));
        painter.line_segment(top(4.0), Stroke::new(2.0, accent.gamma_multiply(0.10)));
    }
}

/// Tell Windows 11 not to round the corners of the window. The app frame is square, so the
/// rounding of the OS cuts into the border. Other systems and Windows 10 ignore the call. The
/// function acts one time, when the window has a handle.
pub fn square_corners(frame: &eframe::Frame) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use std::sync::atomic::{AtomicBool, Ordering};

        // DWMWA_WINDOW_CORNER_PREFERENCE, and the value DWMWCP_DONOTROUND.
        const ATTRIBUTE: u32 = 33;
        const DO_NOT_ROUND: u32 = 1;
        static DONE: AtomicBool = AtomicBool::new(false);
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmSetWindowAttribute(hwnd: *mut std::ffi::c_void, attribute: u32, value: *const u32, size: u32) -> i32;
        }

        if DONE.load(Ordering::Relaxed) {
            return;
        }
        let Ok(handle) = frame.window_handle() else { return };
        let RawWindowHandle::Win32(window) = handle.as_raw() else { return };
        DONE.store(true, Ordering::Relaxed);
        // SAFETY: the handle is the live window of this frame. The value is a `u32` of 4 bytes.
        // The call has no effect, and returns an error code, where the system has no such attribute.
        unsafe { DwmSetWindowAttribute(window.hwnd.get() as *mut std::ffi::c_void, ATTRIBUTE, &DO_NOT_ROUND, 4) };
    }
    #[cfg(not(windows))]
    let _ = frame;
}

/// Start a resize when the pilot presses the primary button at the edge of the window. The OS
/// draws no resize border on a window with no decorations.
pub fn resize_edges(ctx: &Context) {
    if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let Some(direction) = edge_direction(pos, ctx.content_rect()) else { return };
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// The resize direction for a pointer at `pos`, or `None` away from the edges of `window`.
fn edge_direction(pos: Pos2, window: Rect) -> Option<ResizeDirection> {
    let left = pos.x - window.left();
    let right = window.right() - pos.x;
    let top = pos.y - window.top();
    let bottom = window.bottom() - pos.y;
    let on_x = left < EDGE || right < EDGE;
    let on_y = top < EDGE || bottom < EDGE;
    if !on_x && !on_y {
        return None;
    }
    // A pointer on one edge, near a corner, counts for both edges of that corner.
    let west = left < EDGE || (on_y && left < CORNER);
    let east = right < EDGE || (on_y && right < CORNER);
    let north = top < EDGE || (on_x && top < CORNER);
    let south = bottom < EDGE || (on_x && bottom < CORNER);
    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (_, _, true, _) => ResizeDirection::West,
        _ => ResizeDirection::East,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))
    }

    #[test]
    fn the_middle_of_the_window_starts_no_resize() {
        assert_eq!(edge_direction(Pos2::new(200.0, 150.0), window()), None);
    }

    #[test]
    fn an_edge_gives_its_direction() {
        assert_eq!(edge_direction(Pos2::new(1.0, 150.0), window()), Some(ResizeDirection::West));
        assert_eq!(edge_direction(Pos2::new(399.0, 150.0), window()), Some(ResizeDirection::East));
        assert_eq!(edge_direction(Pos2::new(200.0, 1.0), window()), Some(ResizeDirection::North));
        assert_eq!(edge_direction(Pos2::new(200.0, 299.0), window()), Some(ResizeDirection::South));
    }

    #[test]
    fn a_corner_gives_a_diagonal_direction() {
        assert_eq!(edge_direction(Pos2::new(1.0, 1.0), window()), Some(ResizeDirection::NorthWest));
        assert_eq!(edge_direction(Pos2::new(399.0, 1.0), window()), Some(ResizeDirection::NorthEast));
        assert_eq!(edge_direction(Pos2::new(1.0, 299.0), window()), Some(ResizeDirection::SouthWest));
        assert_eq!(edge_direction(Pos2::new(399.0, 299.0), window()), Some(ResizeDirection::SouthEast));
        // On the top edge, but within the corner length of the left edge.
        assert_eq!(edge_direction(Pos2::new(8.0, 1.0), window()), Some(ResizeDirection::NorthWest));
    }
}
