//! Helpers for the draw tests: a context with the theme and the font, and a reader for the texts of a frame.

use crate::theme;

/// A context with the default theme and the fonts of the app. A page uses the bold font, so a draw test needs both.
pub fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply_default(&ctx);
    theme::set_font(&ctx);
    ctx
}

/// The texts that a frame draws, with the position and the size of each.
pub fn drawn_texts(output: &egui::FullOutput) -> Vec<(String, egui::Pos2, egui::Vec2)> {
    fn walk(shape: &egui::epaint::Shape, found: &mut Vec<(String, egui::Pos2, egui::Vec2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => found.push((text.galley.text().to_string(), text.pos, text.galley.size())),
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    output.shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut found));
    found
}

/// The plain shapes that a frame paints (lines, rects, circles and so on), with the groups opened. Texts are included.
pub fn drawn_shapes(output: &egui::FullOutput) -> Vec<egui::epaint::Shape> {
    fn walk(shape: &egui::epaint::Shape, found: &mut Vec<egui::epaint::Shape>) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, found)),
            other => found.push(other.clone()),
        }
    }
    let mut found = Vec::new();
    output.shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut found));
    found
}

/// The outer rects of the cards that `theme::panel` draws, from the top to the bottom and from the
/// left to the right. A card has the 1 point border of the theme and no rounded corner. A rect
/// that lies inside another card, and a rect that is too small to be a card, do not count.
pub fn card_rects(output: &egui::FullOutput) -> Vec<egui::Rect> {
    let rects: Vec<egui::Rect> = drawn_shapes(output)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Rect(r)
                if r.stroke == egui::Stroke::new(1.0, theme::LINE)
                    && r.corner_radius == egui::CornerRadius::ZERO
                    && r.rect.width() > 100.0
                    && r.rect.height() > 40.0 =>
            {
                Some(r.rect)
            }
            _ => None,
        })
        .collect();
    let mut cards: Vec<egui::Rect> =
        rects.iter().copied().filter(|rect| !rects.iter().any(|other| other != rect && other.contains_rect(rect.shrink(0.5)))).collect();
    cards.sort_by(|a, b| (a.min.y, a.min.x).partial_cmp(&(b.min.y, b.min.x)).expect("no NaN"));
    cards
}
