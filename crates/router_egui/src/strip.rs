//! The route strip: one dot for each system of a route, in the colors of the security scale, as the
//! game draws a route. A wormhole jump or a bridge jump shows as a colored bar between two dots.

use crate::theme;
use egui::{Color32, Painter, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use router_core::route::Route;
use router_core::universe::{Link, Universe, display_sec};

/// The widest space between two dots.
const MAX_PITCH: f32 = 12.0;
/// The narrowest space between two dots. A route with more systems than this allows draws past the
/// rectangle, so the strip never gets unreadable.
const MIN_PITCH: f32 = 3.0;

/// The space between the centers of two dots, for `count` dots in `width`.
pub fn pitch(width: f32, count: usize) -> f32 {
    (width / count.max(1) as f32).clamp(MIN_PITCH, MAX_PITCH)
}

/// The index of the dot under `x`, for a strip that starts at `left`. `None` outside the dots.
pub fn dot_at(left: f32, width: f32, count: usize, x: f32) -> Option<usize> {
    let index = ((x - left) / pitch(width, count)).floor();
    (index >= 0.0 && (index as usize) < count).then_some(index as usize)
}

/// The center of dot `index`.
fn center(rect: Rect, count: usize, index: usize) -> Pos2 {
    pos2(rect.left() + pitch(rect.width(), count) * (index as f32 + 0.5), rect.center().y)
}

/// Draw the strip of `route` in `rect`. `selected` gets a ring in the accent color.
pub fn paint(painter: &Painter, uni: &Universe, route: &Route, rect: Rect, selected: Option<usize>) {
    let nodes = &route.path.nodes;
    let count = nodes.len();
    let radius = (pitch(rect.width(), count) * 0.38).clamp(1.2, 3.5);
    for (i, &edge) in route.path.edges.iter().enumerate() {
        let (color, width) = match uni.graph[edge] {
            Link::Stargate => (theme::LINE, 1.0),
            Link::Wormhole(_) => (theme::WORMHOLE, 2.5),
            Link::JumpBridge => (theme::BRIDGE, 2.5),
        };
        painter.line_segment([center(rect, count, i), center(rect, count, i + 1)], Stroke::new(width, color));
    }
    for (i, &node) in nodes.iter().enumerate() {
        let at = center(rect, count, i);
        painter.circle_filled(at, radius, theme::sec_color(uni.system(node).security));
        if route.stop_at(i).is_some() {
            painter.circle_stroke(at, radius + 2.0, Stroke::new(1.0, Color32::WHITE));
        }
        if selected == Some(i) {
            painter.circle_stroke(at, radius + 3.5, Stroke::new(1.5, theme::ACCENT));
        }
    }
}

/// The text of the tooltip of a dot: the system, its security and how the route enters it.
fn tip(uni: &Universe, route: &Route, step: usize) -> String {
    let sys = uni.system(route.path.nodes[step]);
    let sec = format!("{:.1}", display_sec(sys.security));
    match step.checked_sub(1).map(|i| &uni.graph[route.path.edges[i]]) {
        Some(Link::Wormhole(_)) => format!("{} {sec} · {} · by wormhole", sys.name, sys.region),
        Some(Link::JumpBridge) => format!("{} {sec} · {} · by jump bridge", sys.name, sys.region),
        _ => format!("{} {sec} · {}", sys.name, sys.region),
    }
}

/// The strip as a widget of `height`, as wide as the free space. Return the step that a click picked.
pub fn show(ui: &mut Ui, uni: &Universe, route: &Route, height: f32, selected: Option<usize>) -> Option<usize> {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    paint(&ui.painter_at(rect), uni, route, rect, selected);
    let count = route.path.nodes.len();
    let step = response.interact_pointer_pos().or_else(|| response.hover_pos()).and_then(|p| dot_at(rect.left(), rect.width(), count, p.x));
    if let Some(step) = step.filter(|_| response.hovered()) {
        response.clone().on_hover_text_at_pointer(tip(uni, route, step));
    }
    step.filter(|_| response.clicked())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pitch_fits_the_width_and_stays_readable() {
        assert_eq!(pitch(840.0, 42), MAX_PITCH);
        assert_eq!(pitch(210.0, 42), 5.0);
        assert_eq!(pitch(100.0, 200), MIN_PITCH);
        assert_eq!(pitch(100.0, 0), MAX_PITCH);
    }

    #[test]
    fn a_click_finds_its_dot() {
        // 4 dots in 40 px: the pitch is 10.
        assert_eq!(dot_at(100.0, 40.0, 4, 100.0), Some(0));
        assert_eq!(dot_at(100.0, 40.0, 4, 129.9), Some(2));
        assert_eq!(dot_at(100.0, 40.0, 4, 140.0), None);
        assert_eq!(dot_at(100.0, 40.0, 4, 99.0), None);
    }
}
