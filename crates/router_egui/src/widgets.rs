//! The widgets that egui has no version of, in the style of the Rift app.

use egui::{
    Color32, Response, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, Ui, Vec2, Widget, WidgetInfo, WidgetText, WidgetType, vec2,
};

use crate::theme;

/// The kind of a button. A primary button is the main action of a view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Primary,
    Secondary,
}

/// A button with square corners. A primary button has an accent fill. The others have the fill
/// and the line of the egui button.
pub struct AppButton {
    text: WidgetText,
    kind: Kind,
    selected: bool,
    min_size: Vec2,
}

impl AppButton {
    pub fn new(text: impl Into<WidgetText>) -> Self {
        AppButton { text: text.into(), kind: Kind::Secondary, selected: false, min_size: Vec2::ZERO }
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

impl Widget for AppButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let galley = self.text.into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Button);
        let padding = ui.spacing().button_padding;
        let size = (galley.size() + padding * 2.0).max(self.min_size).max(vec2(0.0, ui.spacing().interact_size.y));
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
                // The colors of the primary button of Rift: a dark fill, light under the pointer, white pressed.
                match (pressed, hovered) {
                    (true, _) => (Color32::from_rgb(0xEA, 0xEA, 0xEA), Color32::WHITE, Color32::from_rgb(0x59, 0x59, 0x59)),
                    (_, true) => (theme::accent_mid(ui), theme::accent_light(ui), Color32::WHITE),
                    _ => (theme::accent_dark(ui), theme::accent_line(ui), theme::TEXT),
                }
            } else if self.selected {
                (ui.visuals().selection.bg_fill, accent, Color32::WHITE)
            } else {
                (visuals.bg_fill, visuals.bg_stroke.color, visuals.fg_stroke.color)
            };
            if hovered {
                // egui has no blur, so a wide faint line around the button stands in for the glow.
                ui.painter().rect_stroke(rect.expand(1.0), 0.0, Stroke::new(2.0, accent.gamma_multiply(0.3)), StrokeKind::Inside);
            }
            ui.painter().rect(rect, 0.0, fill, Stroke::new(1.0, line), StrokeKind::Inside);
            let pos = rect.center() - galley.size() / 2.0;
            ui.painter().galley(pos, galley, text);
        }
        response
    }
}

/// The short ways to add a button of the app.
pub trait UiExt {
    /// A button.
    fn app_button(&mut self, text: impl Into<WidgetText>) -> Response;
}

impl UiExt for Ui {
    fn app_button(&mut self, text: impl Into<WidgetText>) -> Response {
        self.add(AppButton::new(text))
    }
}
