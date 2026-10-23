//! The scales of the soft route costs (jumps), for a slider.
//!
//! A scale steps by a fixed amount from 0. The config holds jumps, so a value that is not on the
//! scale (a hand-edited file) shows at the nearest position and stays until the user moves the
//! slider.

/// A scale of costs in jumps: the positions `0..=max_pos`, each `step_tenths` tenths of a jump apart.
#[derive(Clone, Copy, Debug)]
pub struct CostScale {
    step_tenths: u32,
    /// The last position. The first is 0.
    pub max_pos: u32,
}

/// The bridge capacitor cost: 0 to 3 jumps for each 1% of a gate, in steps of 0.1.
pub const CAP: CostScale = CostScale { step_tenths: 1, max_pos: 30 };
/// The unknown signature cost: 0 to 20 jumps, in steps of 1.
pub const SIGNATURE: CostScale = CostScale { step_tenths: 10, max_pos: 20 };

impl CostScale {
    /// The cost at a position. A position past `max_pos` gives the last stop.
    pub fn at(self, pos: u32) -> f32 {
        (pos.min(self.max_pos) * self.step_tenths) as f32 / 10.0
    }

    /// The position that is nearest to `jumps`.
    pub fn pos_of(self, jumps: f32) -> u32 {
        let distance = |pos: u32| (self.at(pos) - jumps).abs();
        (0..=self.max_pos).min_by(|&a, &b| distance(a).total_cmp(&distance(b))).unwrap_or(0)
    }
}

/// The text of a cost: "4 jumps", "0.5 jumps" or "1 jump".
pub fn label(jumps: f32) -> String {
    let text = format!("{jumps:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text);
    format!("{text} {}", if text == "1" { "jump" } else { "jumps" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scale_steps_from_zero() {
        assert_eq!(CAP.at(0), 0.0);
        assert_eq!(CAP.at(6), 0.6);
        assert_eq!(CAP.at(CAP.max_pos), 3.0);
        assert_eq!(CAP.at(CAP.max_pos + 9), 3.0);
        assert_eq!(SIGNATURE.at(1), 1.0);
        assert_eq!(SIGNATURE.at(4), 4.0);
        assert_eq!(SIGNATURE.at(SIGNATURE.max_pos), 20.0);
    }

    #[test]
    fn a_value_snaps_to_the_nearest_position() {
        for scale in [CAP, SIGNATURE] {
            for pos in 0..=scale.max_pos {
                assert_eq!(scale.pos_of(scale.at(pos)), pos);
            }
        }
        assert_eq!(CAP.pos_of(0.64), 6);
        assert_eq!(SIGNATURE.pos_of(1000.0), SIGNATURE.max_pos);
        assert_eq!(SIGNATURE.pos_of(0.0), 0);
    }

    #[test]
    fn the_defaults_are_on_the_scales() {
        let costs = crate::settings::RouteCosts::default();
        assert_eq!(CAP.at(CAP.pos_of(costs.cap_weight)), costs.cap_weight);
        assert_eq!(SIGNATURE.at(SIGNATURE.pos_of(costs.unknown_sig_penalty)), costs.unknown_sig_penalty);
    }

    #[test]
    fn the_label_shows_whole_and_half_jumps() {
        assert_eq!(label(4.0), "4 jumps");
        assert_eq!(label(0.5), "0.5 jumps");
        assert_eq!(label(1.0), "1 jump");
        assert_eq!(label(0.0), "0 jumps");
    }
}
