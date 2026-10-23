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
    radio: bool,
    min_size: Vec2,
}

impl AppButton {
    pub fn new(text: impl Into<WidgetText>) -> Self {
        AppButton { text: text.into(), kind: Kind::Secondary, selected: false, radio: false, min_size: Vec2::ZERO }
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

    /// The button is one choice of a group. A screen reader reads it as a radio button, and says
    /// whether it is the chosen one. The color shows the choice too.
    pub fn choice(mut self, chosen: bool) -> Self {
        self.selected = chosen;
        self.radio = true;
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
        response.widget_info(|| {
            if self.radio {
                WidgetInfo::selected(WidgetType::RadioButton, enabled, self.selected, galley.text())
            } else {
                WidgetInfo::labeled(WidgetType::Button, enabled, galley.text())
            }
        });

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

/// A slider over whole positions, in the style of the sliders of the game: a slim dark inset track
/// and a square handle that is taller than the track. It has no number box. The caller shows the value. A click or a
/// drag on the track sets the position, and the arrow keys move it by one while it has focus.
pub struct AppStepSlider<'a> {
    pos: &'a mut u32,
    max: u32,
    width: f32,
    text: String,
    name: String,
}

/// The height of the handle of a step slider, and of the space that it takes.
const SLIDER_HEIGHT: f32 = 14.0;
/// The height of the track of a step slider.
const SLIDER_TRACK: f32 = 6.0;
/// The width of the handle of a step slider.
const SLIDER_HANDLE: f32 = 8.0;

impl<'a> AppStepSlider<'a> {
    /// A slider over the positions `0..=max`. `name` is what the slider sets, and `text` is the
    /// value as the user reads it. A screen reader gets both, because the position alone means nothing.
    pub fn new(pos: &'a mut u32, max: u32, name: impl Into<String>, text: impl Into<String>) -> Self {
        AppStepSlider { pos, max, width: 200.0, text: text.into(), name: name.into() }
    }

    /// The width of the track and the handle, in points. The default is 200.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

impl Widget for AppStepSlider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, mut response) = ui.allocate_exact_size(vec2(self.width, SLIDER_HEIGHT), Sense::click_and_drag());
        let enabled = ui.is_enabled();
        let max = self.max.max(1);
        // The centre of the handle runs from half a handle in from each end of the track.
        let run = (rect.width() - SLIDER_HANDLE).max(1.0);
        let x_of = |pos: u32| rect.left() + SLIDER_HANDLE / 2.0 + run * pos.min(max) as f32 / max as f32;
        let before = *self.pos;
        if enabled {
            if let Some(p) = response.interact_pointer_pos() {
                let t = ((p.x - rect.left() - SLIDER_HANDLE / 2.0) / run).clamp(0.0, 1.0);
                // `t` is in 0..=1, so the product is in 0..=max and the cast cannot lose a value.
                *self.pos = (t * max as f32).round() as u32;
            }
            if response.has_focus() {
                // Without the filter, egui moves the focus to a neighbor widget on an arrow key.
                let filter = egui::EventFilter { horizontal_arrows: true, ..Default::default() };
                ui.memory_mut(|m| m.set_focus_lock_filter(response.id, filter));
                let (left, right) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
                if left {
                    *self.pos = self.pos.saturating_sub(1);
                }
                if right {
                    *self.pos = (*self.pos + 1).min(self.max);
                }
            }
        }
        if *self.pos != before {
            response.mark_changed();
        }
        let value = *self.pos;
        response.widget_info(|| WidgetInfo {
            current_text_value: Some(self.text.clone()),
            ..WidgetInfo::slider(enabled, f64::from(value), &self.name)
        });

        if ui.is_rect_visible(rect) {
            let accent = theme::accent(ui);
            let active = enabled && (response.hovered() || response.dragged() || response.has_focus());
            let line = match (enabled, active) {
                (false, _) => theme::LINE,
                (_, true) => theme::accent_light(ui),
                _ => theme::accent_line(ui),
            };
            let painter = ui.painter();
            let track = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), SLIDER_TRACK));
            painter.rect(track, 0.0, Color32::from_rgb(0x05, 0x05, 0x05), Stroke::new(1.0, line), StrokeKind::Inside);
            let x = x_of(value);
            let handle = egui::Rect::from_center_size(egui::pos2(x, rect.center().y), vec2(SLIDER_HANDLE, SLIDER_HEIGHT));
            let (fill, edge) = if !enabled {
                (theme::LINE, theme::LINE)
            } else if response.dragged() {
                (Color32::from_rgb(0xEA, 0xEA, 0xEA), Color32::WHITE)
            } else if active {
                (theme::accent_mid(ui), theme::accent_light(ui))
            } else {
                (theme::accent_dark(ui), accent)
            };
            if active {
                // The same stand-in for a glow as on the buttons.
                painter.rect_stroke(handle.expand(1.0), 0.0, Stroke::new(2.0, accent.gamma_multiply(0.3)), StrokeKind::Inside);
            }
            painter.rect(handle, 0.0, fill, Stroke::new(1.0, edge), StrokeKind::Inside);
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::{Role, Toggled};

    /// Run one frame with the widgets of `add` and return the accessibility nodes of it.
    fn nodes(add: impl FnOnce(&mut Ui)) -> Vec<egui::accesskit::Node> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut add = Some(add);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(add) = add.take() {
                add(ui);
            }
        });
        output.textures_delta.clear();
        output.platform_output.accesskit_update.expect("accesskit is on").nodes.into_iter().map(|(_, node)| node).collect()
    }

    #[test]
    fn a_choice_button_says_whether_it_is_chosen() {
        let nodes = nodes(|ui| {
            ui.add(AppButton::new("Allow").choice(false));
            ui.add(AppButton::new("Avoid").choice(true));
        });
        let radio = |label: &str| nodes.iter().find(|n| n.role() == Role::RadioButton && n.label() == Some(label)).unwrap();
        assert_eq!(radio("Allow").toggled(), Some(Toggled::False));
        assert_eq!(radio("Avoid").toggled(), Some(Toggled::True));
    }

    #[test]
    fn a_plain_button_is_a_button() {
        let nodes = nodes(|ui| {
            ui.add(AppButton::new("Save").selected(true));
        });
        assert!(nodes.iter().any(|n| n.role() == Role::Button && n.label() == Some("Save") && n.toggled().is_none()));
    }

    #[test]
    fn the_slider_gives_its_name_and_its_value_as_text() {
        let mut pos = 3;
        let nodes = nodes(|ui| {
            ui.add(AppStepSlider::new(&mut pos, 38, "Minimum wormhole lifetime", "4 hours"));
        });
        let slider = nodes.iter().find(|n| n.role() == Role::Slider).unwrap();
        assert_eq!(slider.label(), Some("Minimum wormhole lifetime"));
        assert_eq!(slider.value(), Some("4 hours"));
    }

    #[test]
    fn the_arrow_keys_move_the_slider_and_keep_the_focus() {
        let ctx = egui::Context::default();
        let mut pos = 3;
        let mut slider_id = None;
        let mut frame = |ctx: &egui::Context, pos: &mut u32, events: Vec<egui::Event>, focus: bool| {
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(AppStepSlider::new(pos, 38, "Lifetime", "text"));
                    slider_id = Some(r.id);
                    if focus {
                        r.request_focus();
                    }
                    ui.add(AppButton::new("Neighbor"));
                });
            });
            output.textures_delta.clear();
        };
        frame(&ctx, &mut pos, vec![], true);
        frame(&ctx, &mut pos, vec![], false);
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        frame(&ctx, &mut pos, vec![key(egui::Key::ArrowRight)], false);
        frame(&ctx, &mut pos, vec![key(egui::Key::ArrowRight)], false);
        assert_eq!(pos, 5);
        assert_eq!(ctx.memory(egui::Memory::focused), slider_id);
    }
}
