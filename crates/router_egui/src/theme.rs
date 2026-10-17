//! The look of the window: the colors, the fonts and the panel frame, in the style of the
//! EVE Online "Photon" UI, as the Rift app draws it. Dark panels, 1 px lines, one accent.

use egui::epaint::text::VariationCoords;
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Pos2, RichText, Shadow, Stroke, TextStyle, Ui,
    Vec2, vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x07, 0x07, 0x07);
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(0x0D, 0x0D, 0x0D, 0xEB);
pub const HEADER: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const LINE: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
/// The 1 px border of the window: darker than the lines inside it.
pub const WINDOW_BORDER: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
pub const TEXT: Color32 = Color32::from_rgb(0xC1, 0xC1, 0xC1);
/// Between `TEXT` and `TEXT_DIM`: the summary of a route in the route list.
pub const TEXT_SOFT: Color32 = Color32::from_rgb(0xA0, 0xA0, 0xA0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x83, 0x83, 0x83);
/// The text of a disabled widget.
pub const TEXT_OFF: Color32 = Color32::from_rgb(0x4F, 0x4F, 0x4F);
pub const OK: Color32 = Color32::from_rgb(0x8D, 0xC1, 0x69);
pub const WARN: Color32 = Color32::from_rgb(0xF3, 0x90, 0x58);
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x45, 0x4B);
/// A muted red for the icon of a failure screen. `ERROR` is too bright for a large mark.
pub const ERROR_SOFT: Color32 = Color32::from_rgb(0xC9, 0x5A, 0x5A);
pub const WORMHOLE: Color32 = Color32::from_rgb(0xB0, 0x70, 0xE0);
pub const BRIDGE: Color32 = Color32::from_rgb(0x60, 0xA0, 0xFF);

/// The fill of a stop row in the route table, and of a hovered row.
pub const ROW_FILL: Color32 = Color32::from_rgb(0x10, 0x12, 0x13);

/// The Photon UI faction theme presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FactionTheme {
    #[default]
    Caldari,
    Amarr,
    Gallente,
    Minmatar,
}

impl FactionTheme {
    pub const ALL: [FactionTheme; 4] = [FactionTheme::Caldari, FactionTheme::Amarr, FactionTheme::Gallente, FactionTheme::Minmatar];

    pub fn name(self) -> &'static str {
        match self {
            FactionTheme::Caldari => "caldari",
            FactionTheme::Amarr => "amarr",
            FactionTheme::Gallente => "gallente",
            FactionTheme::Minmatar => "minmatar",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FactionTheme::Caldari => "Caldari",
            FactionTheme::Amarr => "Amarr",
            FactionTheme::Gallente => "Gallente",
            FactionTheme::Minmatar => "Minmatar",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name.trim().to_lowercase().as_str() {
            "amarr" => FactionTheme::Amarr,
            "gallente" => FactionTheme::Gallente,
            "minmatar" => FactionTheme::Minmatar,
            _ => FactionTheme::Caldari,
        }
    }

    pub fn accent(self) -> Color32 {
        match self {
            FactionTheme::Caldari => Color32::from_rgb(0x58, 0xA7, 0xBF),
            FactionTheme::Amarr => Color32::from_rgb(0xE5, 0xA9, 0x3C),
            FactionTheme::Gallente => Color32::from_rgb(0x45, 0xB8, 0x78),
            FactionTheme::Minmatar => Color32::from_rgb(0xE0, 0x58, 0x38),
        }
    }

    pub fn selection_bg(self) -> Color32 {
        match self {
            FactionTheme::Caldari => Color32::from_rgb(0x35, 0x58, 0x66),
            FactionTheme::Amarr => Color32::from_rgb(0x3D, 0x2E, 0x15),
            FactionTheme::Gallente => Color32::from_rgb(0x18, 0x38, 0x24),
            FactionTheme::Minmatar => Color32::from_rgb(0x40, 0x1C, 0x15),
        }
    }

    pub fn hovered_bg(self) -> Color32 {
        match self {
            FactionTheme::Caldari => Color32::from_rgb(0x13, 0x1C, 0x1F),
            FactionTheme::Amarr => Color32::from_rgb(0x2A, 0x20, 0x12),
            FactionTheme::Gallente => Color32::from_rgb(0x12, 0x28, 0x1C),
            FactionTheme::Minmatar => Color32::from_rgb(0x2C, 0x16, 0x12),
        }
    }

    /// The 1 px line of a button at rest: a dark accent. A button has no fill at rest, as in Rift.
    pub fn button_border(self) -> Color32 {
        mix(Color32::BLACK, self.accent(), 0.35)
    }

    /// The line of a hovered button: the light accent.
    pub fn button_border_hover(self) -> Color32 {
        accent_light_of(self.accent())
    }

    pub fn active_bg(self) -> Color32 {
        self.selection_bg()
    }

    pub fn open_bg(self) -> Color32 {
        match self {
            FactionTheme::Caldari => Color32::from_rgb(0x0F, 0x17, 0x1A),
            FactionTheme::Amarr => Color32::from_rgb(0x20, 0x18, 0x10),
            FactionTheme::Gallente => Color32::from_rgb(0x10, 0x1C, 0x14),
            FactionTheme::Minmatar => Color32::from_rgb(0x22, 0x12, 0x10),
        }
    }
}

/// The mix of two opaque colors. `t` is the share of `b`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

/// The accent in a dark shade: the fill of a panel header and of a primary button at rest.
pub fn accent_dark(ui: &Ui) -> Color32 {
    mix(Color32::BLACK, accent(ui), 0.22)
}

/// The fill of a panel header, and the line of the top bar and the status bar.
pub fn header_fill(ui: &Ui) -> Color32 {
    mix(Color32::BLACK, accent(ui), 0.15)
}

/// The fill of the top bar and the status bar: a faint accent, darker than a panel header.
pub fn bar_fill(ui: &Ui) -> Color32 {
    mix(Color32::BLACK, accent(ui), 0.065)
}

/// The accent in a middle shade: the fill of a primary button under the pointer.
pub fn accent_mid(ui: &Ui) -> Color32 {
    mix(Color32::BLACK, accent(ui), 0.5)
}

/// The accent in a muted shade: the line of a primary button at rest.
pub fn accent_line(ui: &Ui) -> Color32 {
    mix(Color32::BLACK, accent(ui), 0.72)
}

/// The accent in a lighter shade: the line of a focused field and of a hovered primary button.
pub fn accent_light(ui: &Ui) -> Color32 {
    accent_light_of(accent(ui))
}

fn accent_light_of(accent: Color32) -> Color32 {
    mix(accent, Color32::WHITE, 0.25)
}

/// The current accent color from the active UI visuals.
pub fn accent(ui: &Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

/// Whether compact layout mode is currently active.
pub fn is_compact(ui: &Ui) -> bool {
    ui.spacing().interact_size.y < 20.0
}

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

/// Oxanium (SIL Open Font License) for all text, and the Phosphor icons (MIT) for the icons. The egui default fonts stay as the fallback,
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
    // Phosphor icons: the regular face is the fallback of all text, so a string can hold an icon.
    egui_phosphor_icons::add_fonts(&mut fonts);
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("phosphor-icons".into());
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

/// Set the colors and the spacing of the dark theme with the given faction tint and compact setting.
pub fn apply(ctx: &egui::Context, theme: FactionTheme, compact: bool) {
    ctx.set_theme(egui::Theme::Dark);
    let accent = theme.accent();
    let sel_bg = theme.selection_bg();
    let hov_bg = theme.hovered_bg();
    let act_bg = theme.active_bg();
    let opn_bg = theme.open_bg();

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
        v.extreme_bg_color = Color32::from_rgb(0x05, 0x05, 0x05);
        v.text_edit_bg_color = Some(Color32::from_rgb(0x05, 0x05, 0x05));
        v.faint_bg_color = ROW_FILL;
        v.code_bg_color = HEADER;
        v.hyperlink_color = accent;
        v.weak_text_color = Some(TEXT_DIM);
        v.warn_fg_color = WARN;
        v.error_fg_color = ERROR;
        v.selection.bg_fill = sel_bg;
        v.selection.stroke = Stroke::new(1.0, accent_light_of(accent));
        v.window_highlight_topmost = false;

        let w = &mut v.widgets;
        for (state, fill, stroke, text) in [
            (&mut w.noninteractive, PANEL, LINE, TEXT),
            (&mut w.inactive, Color32::TRANSPARENT, theme.button_border(), TEXT),
            (&mut w.hovered, hov_bg, theme.button_border_hover(), Color32::WHITE),
            (&mut w.active, act_bg, accent, Color32::WHITE),
            (&mut w.open, opn_bg, accent, TEXT),
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

        // A thin bar that floats over the content and takes no room, as in the Rift app.
        style.spacing.scroll = egui::style::ScrollStyle { bar_width: 8.0, handle_min_length: 30.0, ..egui::style::ScrollStyle::floating() };
        style.interaction.tooltip_delay = 0.5;

        let s = &mut style.spacing;
        if compact {
            s.item_spacing = vec2(4.0, 2.0);
            s.button_padding = vec2(6.0, 2.0);
            s.interact_size.y = 18.0;
            s.window_margin = Margin::same(6);
            s.menu_margin = Margin::same(2);
        } else {
            s.item_spacing = vec2(6.0, 4.0);
            s.button_padding = vec2(8.0, 3.0);
            s.interact_size.y = 22.0;
            s.window_margin = Margin::same(10);
            s.menu_margin = Margin::same(4);
        }
    });
}

/// Set default dark theme.
pub fn apply_default(ctx: &egui::Context) {
    apply(ctx, FactionTheme::default(), false);
}

/// The uppercase, letter-spaced text of a panel header.
pub fn header_text(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).color(TEXT_DIM).size(11.0).extra_letter_spacing(1.5)
}

/// The title of a panel: plain text, as the section titles of Rift.
fn panel_title(text: &str) -> RichText {
    RichText::new(text).color(TEXT).extra_letter_spacing(0.5)
}

/// A panel: a header strip with `title` and `info`, then the content. `fill` makes the panel
/// as high as the space that is left.
pub fn panel<R>(ui: &mut Ui, title: &str, info: &str, fill: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let info = |ui: &mut Ui| _ = ui.label(RichText::new(info).color(TEXT_DIM).size(11.0));
    panel_with(ui, title, info, fill, add)
}

/// A panel with the widgets of `header` at the right of the header strip, from right to left.
pub fn panel_with<R>(ui: &mut Ui, title: &str, header: impl FnOnce(&mut Ui), fill: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = Frame::new().stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::ZERO);
    let response = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let header_margin = if is_compact(ui) { Margin::symmetric(6, 2) } else { Margin::symmetric(8, 4) };
        Frame::new().fill(header_fill(ui)).inner_margin(header_margin).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(panel_title(title));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), header);
            });
        });
        let inner_margin = if is_compact(ui) { Margin::same(4) } else { Margin::same(8) };
        Frame::new()
            .inner_margin(inner_margin)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if fill {
                    ui.set_min_height(ui.available_height());
                }
                add(ui)
            })
            .inner
    });
    // Flat 1px border Photon styling without corner ticks
    response.inner
}

/// The bar at the left edge of a selected row.
pub fn selection_bar(ui: &Ui, rect: egui::Rect) {
    let bar = egui::Rect::from_min_size(Pos2::new(rect.left(), rect.top()), Vec2::new(3.0, rect.height()));
    ui.painter().rect_filled(bar, 0.0, accent(ui));
}

/// The width of a window: `want`, or less when the window of the app is narrower.
pub fn modal_width(ui: &Ui, want: f32) -> f32 {
    want.min((ui.ctx().content_rect().width() - 48.0).max(240.0))
}

/// A tab of the client style: text only, bright when active, with an accent underline.
/// `size` is the room of the tab. Returns the response, which is clicked when the tab is picked.
pub fn underline_tab(ui: &mut Ui, active: bool, text: &str, size: Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let color = if active {
        Color32::WHITE
    } else if response.hovered() {
        TEXT
    } else {
        TEXT_DIM
    };
    let p = ui.painter();
    p.text(rect.center() - vec2(0.0, 1.0), egui::Align2::CENTER_CENTER, text, egui::TextStyle::Body.resolve(ui.style()), color);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, LINE));
    if active {
        // egui has no blur, so a wide faint line above the accent line stands in for the glow.
        let glow = Stroke::new(3.0, accent(ui).gamma_multiply(0.25));
        p.line_segment([rect.left_bottom() - vec2(0.0, 2.0), rect.right_bottom() - vec2(0.0, 2.0)], glow);
        p.line_segment([rect.left_bottom() - vec2(0.0, 0.5), rect.right_bottom() - vec2(0.0, 0.5)], Stroke::new(1.0, accent(ui)));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_theme_has_its_own_button_colors() {
        for (i, a) in FactionTheme::ALL.iter().enumerate() {
            for b in &FactionTheme::ALL[i + 1..] {
                assert_ne!(a.button_border(), b.button_border());
            }
        }
    }

    #[test]
    fn caldari_uses_the_rift_cryo_blue() {
        assert_eq!(FactionTheme::Caldari.accent(), Color32::from_rgb(0x58, 0xA7, 0xBF));
        for theme in FactionTheme::ALL {
            assert_ne!(accent_light_of(theme.accent()), theme.accent());
        }
    }
}
