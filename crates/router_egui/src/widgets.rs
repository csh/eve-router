//! The widgets that egui has no version of, in the style of the Rift app.

use egui::epaint::PathShape;
use egui::{Color32, Pos2, Response, Sense, Stroke, TextStyle, TextWrapMode, Ui, Vec2, Widget, WidgetInfo, WidgetText, WidgetType, vec2};

use crate::theme;

/// The longest cut at the bottom right corner of a button, as in the Rift app.
const MAX_CUT: f32 = 9.0;

/// The kind of a button. A primary button is the main action of a view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Primary,
    Secondary,
}

/// A button. A primary button has an accent fill and the bottom right corner cut at 45 degrees.
/// The others have square corners, and the fill and the line of the egui button.
pub struct CutButton {
    text: WidgetText,
    kind: Kind,
    selected: bool,
    min_size: Vec2,
}

impl CutButton {
    pub fn new(text: impl Into<WidgetText>) -> Self {
        CutButton { text: text.into(), kind: Kind::Secondary, selected: false, min_size: Vec2::ZERO }
    }

    /// The main action of a view.
    pub fn primary(mut self) -> Self {
        self.kind = Kind::Primary;
        self
    }

    /// Show the button as on, with the selection color.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }
}

/// The corners of the button shape, clockwise from the top left. The cut is at the bottom right.
/// A cut of 0 gives a plain rectangle.
fn outline(rect: egui::Rect, cut: f32) -> Vec<Pos2> {
    if cut == 0.0 {
        return vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    }
    vec![rect.left_top(), rect.right_top(), rect.right_bottom() - vec2(0.0, cut), rect.right_bottom() - vec2(cut, 0.0), rect.left_bottom()]
}

/// The length of the cut for a button of this height: smaller buttons get a smaller cut.
fn cut_for(height: f32) -> f32 {
    (height * 0.4).clamp(4.0, MAX_CUT)
}

impl Widget for CutButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let galley = self.text.into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Button);
        let padding = ui.spacing().button_padding;
        // Room for the cut, so the text keeps clear of the corner.
        let size = (galley.size() + padding * 2.0 + vec2(4.0, 0.0)).max(self.min_size).max(vec2(0.0, ui.spacing().interact_size.y));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, galley.text()));

        if ui.is_rect_visible(rect) {
            let accent = theme::accent(ui);
            let pressed = response.is_pointer_button_down_on();
            let hovered = response.hovered() && enabled;
            let visuals = ui.style().interact(&response);
            let (fill, line, text) = if !enabled {
                (Color32::TRANSPARENT, theme::LINE, theme::TEXT_OFF)
            } else if self.kind == Kind::Primary {
                match (pressed, hovered) {
                    (true, _) => (accent, theme::accent_light(ui), Color32::WHITE),
                    (_, true) => (theme::accent_dark(ui).lerp_to_gamma(accent, 0.5), theme::accent_light(ui), Color32::WHITE),
                    _ => (theme::accent_dark(ui), accent, Color32::WHITE),
                }
            } else if self.selected {
                (ui.visuals().selection.bg_fill, accent, Color32::WHITE)
            } else {
                (visuals.bg_fill, visuals.bg_stroke.color, visuals.fg_stroke.color)
            };
            // The line runs inside the rect, so a 1 px line covers whole pixels.
            let inner = rect.shrink(0.5);
            let cut = if self.kind == Kind::Primary { cut_for(rect.height()) } else { 0.0 };
            let shape = PathShape::convex_polygon(outline(inner, cut), fill, Stroke::new(1.0, line));
            ui.painter().add(shape);
            let pos = rect.center() - galley.size() / 2.0 - vec2(1.0, 0.0);
            ui.painter().galley(pos, galley, text);
        }
        response
    }
}

/// The short ways to add a button of the app.
pub trait UiExt {
    /// A button with a cut corner.
    fn cut_button(&mut self, text: impl Into<WidgetText>) -> Response;
}

impl UiExt for Ui {
    fn cut_button(&mut self, text: impl Into<WidgetText>) -> Response {
        self.add(CutButton::new(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Rect;

    #[test]
    fn the_outline_has_five_corners_and_the_cut_is_at_the_bottom_right() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 30.0));
        let corners = outline(rect, 9.0);
        assert_eq!(corners.len(), 5);
        assert!(!corners.contains(&rect.right_bottom()));
        assert!(corners.contains(&Pos2::new(100.0, 21.0)));
        assert!(corners.contains(&Pos2::new(91.0, 30.0)));
    }

    #[test]
    fn a_zero_cut_gives_a_rectangle() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 30.0));
        assert_eq!(outline(rect, 0.0), [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()]);
    }

    #[test]
    fn the_cut_grows_with_the_height_up_to_the_rift_value() {
        assert_eq!(cut_for(10.0), 4.0);
        assert!((cut_for(22.0) - 8.8).abs() < 1e-6);
        assert_eq!(cut_for(40.0), MAX_CUT);
    }
}
