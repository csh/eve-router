//! The scale of the minimum wormhole life (minutes), for a slider.
//!
//! The first positions step by 1 minute from 30 to 60 minutes. The next positions are the stops
//! 2, 3, 4, 6, 8, 12, 16 and 24 hours. The config holds minutes, so a value that is not on the
//! scale (a hand-edited file) shows at the nearest position and stays until the user moves the
//! slider.

/// Minutes at the first position.
pub const FIRST: u64 = 30;
/// Minutes at the end of the 1-minute steps.
const LAST_STEP: u64 = 60;
/// The stops after the 1-minute steps, in minutes: 2, 3, 4, 6, 8, 12, 16 and 24 hours.
const STOPS: [u64; 8] = [120, 180, 240, 360, 480, 720, 960, 1440];
/// The last position. The first is 0.
pub const MAX_POS: u32 = (LAST_STEP - FIRST) as u32 + STOPS.len() as u32;

/// The minutes at a position. A position past `MAX_POS` gives the last stop.
pub fn life_at(pos: u32) -> u64 {
    let steps = (LAST_STEP - FIRST) as u32;
    match pos.checked_sub(steps + 1) {
        None => FIRST + u64::from(pos),
        Some(stop) => STOPS[(stop as usize).min(STOPS.len() - 1)],
    }
}

/// The position that is nearest to `minutes`.
pub fn pos_of(minutes: u64) -> u32 {
    (0..=MAX_POS).min_by_key(|&p| life_at(p).abs_diff(minutes)).unwrap_or(0)
}

/// The text of a time: minutes under an hour, else hours. For example "45 minutes", "1 hour",
/// "4 hours" or "1.5 hours".
pub fn label(minutes: u64) -> String {
    let plural = |n: u64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    match minutes {
        m if m < 60 => plural(m, "minute"),
        m if m % 60 == 0 => plural(m / 60, "hour"),
        m => format!("{:.1} hours", m as f64 / 60.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_steps_by_a_minute_then_jumps() {
        assert_eq!(life_at(0), 30);
        assert_eq!(life_at(30), 60);
        assert_eq!([31, 32, 33, 34, 35, 36, 37, 38].map(life_at), STOPS);
        assert_eq!(life_at(MAX_POS + 5), 1440);
        assert_eq!(MAX_POS, 38);
    }

    #[test]
    fn a_value_snaps_to_the_nearest_position() {
        for pos in 0..=MAX_POS {
            assert_eq!(pos_of(life_at(pos)), pos);
        }
        assert_eq!(pos_of(0), 0);
        assert_eq!(pos_of(45), 15);
        // Between 60 minutes and 4 hours: the nearer end wins.
        assert_eq!(life_at(pos_of(80)), 60);
        assert_eq!(life_at(pos_of(100)), 120);
        assert_eq!(life_at(pos_of(100_000)), 1440);
    }

    #[test]
    fn the_label_uses_minutes_then_hours() {
        assert_eq!(label(45), "45 minutes");
        assert_eq!(label(1), "1 minute");
        assert_eq!(label(60), "1 hour");
        assert_eq!(label(240), "4 hours");
        assert_eq!(label(1440), "24 hours");
        assert_eq!(label(90), "1.5 hours");
    }
}
