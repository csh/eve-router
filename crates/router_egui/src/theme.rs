//! The look of the window: the colors, the fonts and the panel frame, in the style of the
//! EVE Online "Photon" UI. Dark panels, 1 px lines, square corners, one blue accent.

use egui::epaint::text::VariationCoords;
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Pos2, RichText, Shadow, Stroke, TextStyle, Ui,
    Vec2, vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x07, 0x09, 0x0C);
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(0x0D, 0x12, 0x17, 0xEB);
pub const HEADER: Color32 = Color32::from_rgb(0x14, 0x1C, 0x22);
pub const LINE: Color32 = Color32::from_rgb(0x26, 0x34, 0x3D);
pub const ACCENT: Color32 = Color32::from_rgb(0x5A, 0xB4, 0xD2);
pub const ACCENT_DIM: Color32 = Color32::from_rgba_premultiplied(0x10, 0x20, 0x26, 0x2E);
pub const TEXT: Color32 = Color32::from_rgb(0xC8, 0xD2, 0xD8);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x6E, 0x7E, 0x88);
pub const OK: Color32 = Color32::from_rgb(0x4C, 0xC0, 0x70);
pub const WARN: Color32 = Color32::from_rgb(0xE0, 0xA0, 0x30);
pub const ERROR: Color32 = Color32::from_rgb(0xE0, 0x4A, 0x3A);
pub const WORMHOLE: Color32 = Color32::from_rgb(0xB0, 0x70, 0xE0);
pub const BRIDGE: Color32 = Color32::from_rgb(0x60, 0xA0, 0xFF);

/// The fill of a stop row in the route table, and of a hovered row.
pub const ROW_FILL: Color32 = Color32::from_rgb(0x12, 0x1A, 0x20);

/// The color of a security value.
pub fn sec_color(security: f64) -> Color32 {
    let rgb = router_core::labels::sec_rgb(security);
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The font family of the bold text. A font with no bold face uses its regular face.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// The text size of the body text.
const BODY_SIZE: f32 = 14.0;

/// Oxanium (SIL Open Font License) for all text. The egui default fonts stay as the fallback,
/// for the arrows and the icons that Oxanium does not have.
pub fn set_font(ctx: &egui::Context) {
    const OXANIUM: &[u8] = include_bytes!("../assets/fonts/Oxanium.ttf");
    // Oxanium is a variable font: the weight axis gives the regular and the bold face.
    // egui puts the line gap of the font (0.25 em) below the glyphs, so the text sits high in
    // its row. The offset moves the glyphs down to the middle of the row. The values come from
    // screenshots: the bold face is mostly uppercase text, which needs a larger offset.
    let face = |wght: f32, offset: f32| {
        let tweak = egui::FontTweak { coords: VariationCoords::new([(b"wght", wght)]), y_offset_factor: offset, ..Default::default() };
        Arc::new(FontData::from_static(OXANIUM).tweak(tweak))
    };
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("ui-regular".into(), face(450.0, 0.26));
    fonts.font_data.insert("ui-bold".into(), face(700.0, 0.28));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().insert(0, "ui-regular".into());
    }
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    fonts.families.insert(bold(), std::iter::once("ui-bold".to_string()).chain(fallback).collect());
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(BODY_SIZE - 2.0)),
            (TextStyle::Body, FontId::proportional(BODY_SIZE)),
            (TextStyle::Button, FontId::proportional(BODY_SIZE)),
            (TextStyle::Monospace, FontId::monospace(BODY_SIZE)),
            (TextStyle::Heading, FontId::new(BODY_SIZE + 4.0, bold())),
        ]
        .into();
    });
}

/// Set the colors and the spacing of the dark theme.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.extreme_bg_color = Color32::from_rgb(0x05, 0x07, 0x09);
        v.text_edit_bg_color = Some(Color32::from_rgb(0x05, 0x07, 0x09));
        v.faint_bg_color = ROW_FILL;
        v.code_bg_color = HEADER;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = WARN;
        v.error_fg_color = ERROR;
        v.selection.bg_fill = Color32::from_rgb(0x1C, 0x3A, 0x46);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_highlight_topmost = false;

        let w = &mut v.widgets;
        for (state, fill, stroke, text) in [
            (&mut w.noninteractive, PANEL, LINE, TEXT),
            (&mut w.inactive, Color32::from_rgb(0x12, 0x1A, 0x20), LINE, TEXT),
            (&mut w.hovered, Color32::from_rgb(0x18, 0x26, 0x2E), ACCENT, Color32::WHITE),
            (&mut w.active, Color32::from_rgb(0x1C, 0x3A, 0x46), ACCENT, Color32::WHITE),
            (&mut w.open, Color32::from_rgb(0x14, 0x1C, 0x22), ACCENT, TEXT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, text);
            state.corner_radius = CornerRadius::ZERO;
            state.expansion = 0.0;
        }
        // A label has no frame line.
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);

        let s = &mut style.spacing;
        s.item_spacing = vec2(6.0, 4.0);
        s.button_padding = vec2(8.0, 3.0);
        s.interact_size.y = 22.0;
        s.window_margin = Margin::same(10);
        s.menu_margin = Margin::same(4);
    });
}

/// The uppercase, letter-spaced text of a panel header.
pub fn header_text(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).color(TEXT_DIM).size(11.0).extra_letter_spacing(1.5)
}

/// A panel: a header strip with `title` and `info`, then the content. `fill` makes the panel
/// as high as the space that is left.
pub fn panel<R>(ui: &mut Ui, title: &str, info: &str, fill: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let info = |ui: &mut Ui| _ = ui.label(RichText::new(info).color(TEXT_DIM).size(11.0));
    panel_with(ui, title, info, fill, add)
}

/// A panel with the widgets of `header` at the right of the header strip, from right to left.
pub fn panel_with<R>(ui: &mut Ui, title: &str, header: impl FnOnce(&mut Ui), fill: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::ZERO);
    let response = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        Frame::new().fill(HEADER).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(header_text(title));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), header);
            });
        });
        Frame::new()
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if fill {
                    ui.set_min_height(ui.available_height());
                }
                add(ui)
            })
            .inner
    });
    corner_ticks(ui, response.response.rect);
    response.inner
}

/// The short accent lines at the top-left and the bottom-right corner of a frame.
pub fn corner_ticks(ui: &Ui, rect: egui::Rect) {
    const LEN: f32 = 10.0;
    let stroke = Stroke::new(2.0, ACCENT);
    let p = ui.painter();
    let tl = rect.left_top();
    p.line_segment([tl, tl + vec2(LEN, 0.0)], stroke);
    p.line_segment([tl, tl + vec2(0.0, LEN)], stroke);
    let br = rect.right_bottom();
    p.line_segment([br, br - vec2(LEN, 0.0)], stroke);
    p.line_segment([br, br - vec2(0.0, LEN)], stroke);
}

/// The bar at the left edge of a selected row.
pub fn selection_bar(ui: &Ui, rect: egui::Rect) {
    let bar = egui::Rect::from_min_size(Pos2::new(rect.left(), rect.top()), Vec2::new(3.0, rect.height()));
    ui.painter().rect_filled(bar, 0.0, ACCENT);
}
