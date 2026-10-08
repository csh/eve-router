//! The route strip: one square for each system of a route, in the colors of the security scale, as
//! the game draws a route. A system that the pilot gave (the start, a midpoint, the destination)
//! shows as a plus. A wormhole jump or a bridge jump shows as a colored bar below the square.
//! The strip draws no border, so it adds no padding and hides nothing.

use crate::theme;
use egui::{Painter, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use router_core::route::Route;
use router_core::universe::{Link, Universe, display_sec};

/// The widest space between the centers of two squares.
const MAX_PITCH: f32 = 12.0;
/// The narrowest space. A route with more systems than this allows draws past the rectangle, so
/// the strip never gets unreadable.
const MIN_PITCH: f32 = 4.0;

/// The space between the centers of two squares, for `count` squares in `width`.
pub fn pitch(width: f32, count: usize) -> f32 {
    (width / count.max(1) as f32).clamp(MIN_PITCH, MAX_PITCH)
}

/// The side of a square for a `pitch`: 10 px at the widest space, with a gap of 2 px.
pub fn square_side(pitch: f32) -> f32 {
    if pitch >= 6.0 { pitch - 2.0 } else { pitch - 1.0 }
}

/// The index of the square under `x`, for a strip that starts at `left`. `None` outside the squares.
pub fn dot_at(left: f32, width: f32, count: usize, x: f32) -> Option<usize> {
    let index = ((x - left) / pitch(width, count)).floor();
    (index >= 0.0 && (index as usize) < count).then_some(index as usize)
}

/// The center of square `index`.
fn center(rect: Rect, count: usize, index: usize) -> Pos2 {
    pos2(rect.left() + pitch(rect.width(), count) * (index as f32 + 0.5), rect.center().y)
}

/// Draw the strip of `route` in `rect`, which is at least 18 px high. `selected` gets a bar above
/// its square in the accent color.
pub fn paint(painter: &Painter, uni: &Universe, route: &Route, rect: Rect, selected: Option<usize>) {
    let nodes = &route.path.nodes;
    let count = nodes.len();
    let pitch = pitch(rect.width(), count);
    let side = square_side(pitch);
    let half = side / 2.0;
    for (i, &node) in nodes.iter().enumerate() {
        let at = center(rect, count, i);
        let color = theme::sec_color(uni.system(node).security);
        if route.stop_at(i).is_some() {
            // The game marks a waypoint with a plus in the place of the square.
            let stroke = Stroke::new((side / 4.0).max(1.5), theme::TEXT);
            painter.line_segment([at - vec2(half, 0.0), at + vec2(half, 0.0)], stroke);
            painter.line_segment([at - vec2(0.0, half), at + vec2(0.0, half)], stroke);
        } else {
            painter.rect_filled(Rect::from_center_size(at, vec2(side, side)), 0.0, color);
        }
        // How the route enters this system, if it is not a gate: a bar below the square.
        let link = match i.checked_sub(1).map(|e| &uni.graph[route.path.edges[e]]) {
            Some(Link::Wormhole(_)) => Some(theme::WORMHOLE),
            Some(Link::JumpBridge) => Some(theme::BRIDGE),
            _ => None,
        };
        if let Some(link) = link {
            let bar = Rect::from_min_size(pos2(at.x - half - (pitch - side), at.y + half + 1.5), vec2(pitch, 2.0));
            painter.rect_filled(bar, 0.0, link);
        }
        if selected == Some(i) {
            let bar = Rect::from_min_size(pos2(at.x - half, at.y - half - 3.5), vec2(side, 2.0));
            painter.rect_filled(bar, 0.0, theme::ACCENT);
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
    fn a_square_keeps_a_gap() {
        assert_eq!(square_side(12.0), 10.0);
        assert_eq!(square_side(6.0), 4.0);
        assert_eq!(square_side(4.0), 3.0);
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
