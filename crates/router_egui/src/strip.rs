//! The route strip: one square for each system of a route, in the colors of the security scale, as
//! the game draws a route. A system that the pilot gave (the start, a midpoint, the destination)
//! shows as a plus in the color of its security. A wormhole jump or a bridge jump shows as a
//! colored bar below the square. The strip draws no border, so it adds no padding and hides nothing.

use crate::theme;
use egui::{Painter, Pos2, Rect, Stroke, pos2, vec2};
use router_core::route::Route;
use router_core::universe::{Link, Universe};

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

/// The center of square `index`.
fn center(rect: Rect, count: usize, index: usize) -> Pos2 {
    pos2(rect.left() + pitch(rect.width(), count) * (index as f32 + 0.5), rect.center().y)
}

/// Draw the strip of `route` in `rect`, which is at least 16 px high.
pub fn paint(painter: &Painter, uni: &Universe, route: &Route, rect: Rect) {
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
            let stroke = Stroke::new((side / 4.0).max(1.5), color);
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
    }
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
}
