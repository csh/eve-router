//! The widgets that egui has no version of, in the style of the Rift app.

use egui::{
    Color32, Response, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, Ui, Vec2, Widget, WidgetInfo, WidgetText, WidgetType, vec2,
};

use crate::theme;
use egui::text::LayoutJob;
use egui::{Align, FontSelection, RichText};
use egui_phosphor_icons::icons;

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

/// A checkbox in the style of the app: a square with a dark accent line, a check mark in the accent
/// when on, and the label at the right. The label takes the click too.
pub struct AppCheckbox<'a> {
    checked: &'a mut bool,
    text: WidgetText,
}

/// The side of the box of a checkbox.
const CHECK_BOX: f32 = 16.0;

impl Widget for AppCheckbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let galley = self.text.into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
        let gap = 8.0;
        let size = vec2(CHECK_BOX + gap + galley.size().x, CHECK_BOX.max(galley.size().y).max(ui.spacing().interact_size.y));
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
        if response.clicked() {
            *self.checked = !*self.checked;
            response.mark_changed();
        }
        let checked = *self.checked;
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, checked, galley.text()));
        if ui.is_rect_visible(rect) {
            let accent = theme::accent(ui);
            let hovered = response.hovered() && enabled;
            let boxed =
                egui::Rect::from_min_size(egui::Pos2::new(rect.left(), rect.center().y - CHECK_BOX / 2.0), vec2(CHECK_BOX, CHECK_BOX));
            let (fill, line, text) = if !enabled {
                (Color32::TRANSPARENT, theme::LINE, theme::TEXT_OFF)
            } else if hovered {
                (if checked { theme::accent_mid(ui) } else { theme::accent_dark(ui) }, theme::accent_light(ui), Color32::WHITE)
            } else {
                (if checked { theme::accent_dark(ui) } else { Color32::TRANSPARENT }, theme::accent_line(ui), theme::TEXT)
            };
            ui.painter().rect(boxed, 0.0, fill, Stroke::new(1.0, line), StrokeKind::Inside);
            if checked {
                let mark = RichText::new(icons::CHECK.as_str()).size(13.0).color(if enabled { accent } else { theme::TEXT_OFF });
                let mark = WidgetText::from(mark).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
                ui.painter().galley(boxed.center() - mark.size() / 2.0, mark, accent);
            }
            let pos = egui::Pos2::new(rect.left() + CHECK_BOX + gap, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(pos, galley, text);
        }
        response
    }
}

/// The label of a button: `icon`, then `text`. An empty text gives the icon alone. The icon keeps
/// its own font face, so a filled icon can stand next to the text.
pub fn icon_label(ui: &Ui, icon: RichText, text: &str) -> WidgetText {
    let mut job = LayoutJob::default();
    let font = FontSelection::Style(TextStyle::Button);
    icon.append_to(&mut job, ui.style(), font.clone(), Align::Center);
    if !text.is_empty() {
        RichText::new(format!(" {text}")).append_to(&mut job, ui.style(), font, Align::Center);
    }
    job.into()
}

/// A context menu for `response`. A second right click on the same widget opens a new menu there.
/// The egui menu would close in the same frame: the click lands outside the old menu, and the
/// old menu and the new one have the same ID. Each right click gets a new ID, so the old menu
/// closes and the new one stays.
pub fn context_menu(response: &Response, add_contents: impl FnOnce(&mut Ui)) {
    let key = response.id.with("menu-count");
    let mut count: u32 = response.ctx.data(|d| d.get_temp(key)).unwrap_or(0);
    if response.secondary_clicked() {
        count = count.wrapping_add(1);
        response.ctx.data_mut(|d| d.insert_temp(key, count));
    }
    egui::Popup::context_menu(response).id(response.id.with(("menu", count))).show(add_contents);
}

/// The short ways to add a button of the app.
pub trait UiExt {
    /// A button.
    fn app_button(&mut self, text: impl Into<WidgetText>) -> Response;
    /// A checkbox.
    fn app_checkbox(&mut self, checked: &mut bool, text: impl Into<WidgetText>) -> Response;
}

impl UiExt for Ui {
    fn app_button(&mut self, text: impl Into<WidgetText>) -> Response {
        self.add(AppButton::new(text))
    }

    fn app_checkbox(&mut self, checked: &mut bool, text: impl Into<WidgetText>) -> Response {
        self.add(AppCheckbox { checked, text: text.into() })
    }
}
