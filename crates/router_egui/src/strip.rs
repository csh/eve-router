//! The route strip: one square for each system of a route, in the colors of the security scale, as
//! the game draws a route. A system that the pilot gave (the start, a midpoint, the destination)
//! shows as a plus in the color of its security. A wormhole jump or a bridge jump shows as a
//! colored bar below the square. The strip draws no border, so it adds no padding and hides nothing.
//!
//! The strip works in whole device pixels, so every edge is sharp at any display scale.

use crate::theme;
use egui::{Painter, Rect, pos2, vec2};
use router_core::route::Route;
use router_core::universe::{Link, Universe};

/// The widest space between the centers of two squares, in points.
const MAX_PITCH: f32 = 12.0;
/// The narrowest space, in points. A route with more systems than this allows draws past the
/// rectangle, so the strip never gets unreadable.
const MIN_PITCH: f32 = 4.0;

/// The space between the centers of two squares, for `count` squares in `width` points.
pub fn pitch(width: f32, count: usize) -> f32 {
    (width / count.max(1) as f32).clamp(MIN_PITCH, MAX_PITCH)
}

/// The pitch and the side of a square in whole device pixels, for `count` squares in `width`
/// points at `ppp` pixels for each point. The gap is 2 points between large squares, 1 between small ones.
pub fn square_px(width: f32, count: usize, ppp: f32) -> (f32, f32) {
    let pitch_px = (pitch(width, count) * ppp).floor();
    let gap_points = if pitch_px >= 6.0 * ppp { 2.0 } else { 1.0 };
    let gap_px = (gap_points * ppp).round().max(1.0);
    (pitch_px, (pitch_px - gap_px).max(1.0))
}

/// Draw the strip of `route` in `rect`, which is at least 16 points high.
pub fn paint(painter: &Painter, uni: &Universe, route: &Route, rect: Rect) {
    let ppp = painter.pixels_per_point();
    let (pitch_px, side_px) = square_px(rect.width(), route.path.nodes.len(), ppp);
    // The plus has arms of 2 px, or 3 px when the side is odd, so the arms sit on whole pixels.
    let arm_px = if side_px % 2.0 == 0.0 { 2.0 } else { 3.0 };
    let left_px = (rect.left() * ppp).round();
    let top_px = (rect.center().y * ppp - side_px / 2.0).round();
    let at = |x_px: f32, y_px: f32, w_px: f32, h_px: f32| Rect::from_min_size(pos2(x_px / ppp, y_px / ppp), vec2(w_px / ppp, h_px / ppp));
    for (i, &node) in route.path.nodes.iter().enumerate() {
        let x = left_px + pitch_px * i as f32;
        let color = theme::sec_color(uni.system(node).security);
        if route.stop_at(i).is_some() {
            // The game marks a waypoint with a plus in the place of the square.
            let mid = (side_px - arm_px) / 2.0;
            painter.rect_filled(at(x, top_px + mid, side_px, arm_px), 0.0, color);
            painter.rect_filled(at(x + mid, top_px, arm_px, side_px), 0.0, color);
        } else {
            painter.rect_filled(at(x, top_px, side_px, side_px), 0.0, color);
        }
        // How the route enters this system, if it is not a gate: a bar as wide as the square, below it.
        let link = match i.checked_sub(1).map(|e| &uni.graph[route.path.edges[e]]) {
            Some(Link::Wormhole(_)) => Some(theme::WORMHOLE),
            Some(Link::JumpBridge) => Some(theme::BRIDGE),
            _ => None,
        };
        if let Some(link) = link {
            painter.rect_filled(at(x, top_px + side_px + 2.0 * ppp.round().max(1.0), side_px, 2.0), 0.0, link);
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
    fn a_square_is_whole_pixels_with_a_gap() {
        // At 1 pixel for each point: 10 px squares at the widest space.
        assert_eq!(square_px(840.0, 42, 1.0), (12.0, 10.0));
        assert_eq!(square_px(210.0, 42, 1.0), (5.0, 4.0));
        // At 1.5 pixels for each point: a pitch of 18 px, and a gap of 3 px.
        assert_eq!(square_px(840.0, 42, 1.5), (18.0, 15.0));
        // A fractional pitch rounds down to a whole pixel.
        assert_eq!(square_px(500.0, 41, 1.0).0, 12.0);
        assert_eq!(square_px(300.0, 41, 1.0).0, 7.0);
    }
}
