//! The shared wormhole record. Each source converts its data to this record.
//! The router and the TUI read only this record, never the raw format of a source.

use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::time::{SystemTime, UNIX_EPOCH};

pub const HOUR: u64 = 3600;

/// The current time in Unix seconds.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SourceId {
    Nexum,
    EveScout,
}

impl SourceId {
    pub fn label(self) -> &'static str {
        match self {
            SourceId::Nexum => "Nexum",
            SourceId::EveScout => "EVE-Scout",
        }
    }

    /// The name of the cache file, without the extension.
    pub fn file_stem(self) -> &'static str {
        match self {
            SourceId::Nexum => "nexum",
            SourceId::EveScout => "eve-scout",
        }
    }
}

/// The size class of a wormhole. The order goes from small to large.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Size {
    Small,
    Medium,
    Large,
    XLarge,
    Capital,
}

impl Size {
    /// The lowest per-jump mass limit in the class, in kg. Thus the size check never sends a
    /// ship into a wormhole that is too small. Source: the SDE attribute 1385 of each type.
    pub fn max_jump_kg(self) -> f64 {
        match self {
            Size::Small => 5_000_000.0,
            Size::Medium => 62_000_000.0,
            Size::Large => 375_000_000.0,
            Size::XLarge => 1_000_000_000.0,
            Size::Capital => 2_000_000_000.0,
        }
    }

    /// The class of a per-jump limit. The limits between classes come from Nexum
    /// (`web/src/utils/wormholeSize.ts`), plus a Capital class at 2,000,000,000 kg.
    pub fn from_jump_kg(kg: f64) -> Size {
        match kg {
            kg if kg >= 2_000_000_000.0 => Size::Capital,
            kg if kg >= 1_000_000_000.0 => Size::XLarge,
            kg if kg >= 300_000_000.0 => Size::Large,
            kg if kg >= 62_000_000.0 => Size::Medium,
            _ => Size::Small,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
            Size::XLarge => "XL",
            Size::Capital => "Capital",
        }
    }
}

/// The mass status. The order goes from good to bad, so the worse status is the larger one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MassStatus {
    Stable,
    Destabilized,
    Critical,
}

impl MassStatus {
    pub fn label(self) -> &'static str {
        match self {
            MassStatus::Stable => "Stable",
            MassStatus::Destabilized => "Destabilized",
            MassStatus::Critical => "Critical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expiry {
    /// Unix seconds.
    pub at: u64,
    /// True: an exact expiry time. False: an upper limit, for example from "less than 4 hours".
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wormhole {
    /// The EVE system ID with the lower value.
    pub a: u32,
    /// The EVE system ID with the higher value.
    pub b: u32,
    /// `None`: the size is not known, so there is no size check.
    pub size: Option<Size>,
    /// The exact per-jump mass of a known wormhole type, in kg.
    pub max_jump_kg: Option<f64>,
    pub mass: Option<MassStatus>,
    pub expiry: Option<Expiry>,
    /// For example "K162".
    pub wh_type: Option<String>,
    /// The signature at `a` and the signature at `b`.
    pub sigs: Option<(String, String)>,
    pub sources: Vec<SourceId>,
}

impl Wormhole {
    /// The per-jump limit: the exact type value, else the lowest value of the size class.
    pub fn jump_limit_kg(&self) -> Option<f64> {
        self.max_jump_kg.or(self.size.map(Size::max_jump_kg))
    }

    /// True if a ship of `hull_kg` can use the wormhole at `now`.
    /// `None` for the hull: no hull is set, so there is no size check.
    pub fn usable(&self, now: u64, hull_kg: Option<f64>) -> bool {
        let fits = match (hull_kg, self.jump_limit_kg()) {
            (Some(hull), Some(limit)) => hull <= limit,
            _ => true,
        };
        self.mass != Some(MassStatus::Critical) && self.expiry.is_none_or(|e| e.at > now) && fits
    }
}

/// Parse an RFC 3339 time, for example `2026-10-05T10:18:57.562Z`, to Unix seconds.
/// A bad format, or a time before 1970, gives `None`.
pub fn parse_utc(s: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(s).ok()?.timestamp().try_into().ok()
}

/// "2h 10m", or "10m" for less than one hour.
fn duration_text(secs: u64) -> String {
    let minutes = secs / 60;
    if minutes >= 60 { format!("{}h {}m", minutes / 60, minutes % 60) } else { format!("{minutes}m") }
}

/// The remaining time of a wormhole, for the TUI.
pub fn expiry_text(e: Expiry, now: u64) -> String {
    if e.at <= now {
        return "Expired".into();
    }
    let left = duration_text(e.at - now);
    if e.exact { format!("Expires in {left}") } else { format!("Less than {left} remaining") }
}

/// The age of the data, for example "2 min ago".
pub fn age_text(fetched_at: u64, now: u64) -> String {
    match now.saturating_sub(fetched_at) / 60 {
        0 => "just now".into(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 48 * 60 => format!("{} h ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

/// The clock time of a Unix time in a time zone, for example "14:02".
pub fn clock_time<Tz: TimeZone>(t: u64, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let utc = i64::try_from(t).ok().and_then(|t| DateTime::from_timestamp(t, 0));
    utc.map_or_else(String::new, |d| d.with_timezone(tz).format("%H:%M").to_string())
}

/// The clock time of a Unix time in the local time zone of the user, for example "14:02".
pub fn local_time(t: u64) -> String {
    clock_time(t, &Local)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A wormhole between two systems, with no known values.
    pub fn hole(a: u32, b: u32) -> Wormhole {
        Wormhole {
            a: a.min(b),
            b: a.max(b),
            size: None,
            max_jump_kg: None,
            mass: None,
            expiry: None,
            wh_type: None,
            sigs: None,
            sources: vec![SourceId::Nexum],
        }
    }

    #[test]
    fn parses_utc_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-10-05T10:18:57.562Z"), Some(1_791_195_537));
        assert_eq!(parse_utc("2026-10-05T12:00:00Z"), Some(1_791_201_600));
        // A leap day, and a year that is a multiple of 100 but not of 400.
        assert_eq!(parse_utc("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_utc("2100-03-01T00:00:00.000Z"), Some(4_107_542_400));
        // An offset converts to UTC.
        assert_eq!(parse_utc("2026-10-05T12:18:57+02:00"), Some(1_791_195_537));
        assert_eq!(parse_utc("2023-02-29T00:00:00Z"), None);
        assert_eq!(parse_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc("2026-10-05T24:00:00Z"), None);
        // A time before 1970 has no Unix seconds.
        assert_eq!(parse_utc("1969-12-31T23:59:59Z"), None);
        assert_eq!(parse_utc("yesterday"), None);
        assert_eq!(parse_utc(""), None);
    }

    #[test]
    fn size_classes() {
        assert_eq!(Size::from_jump_kg(5_000_000.0), Size::Small);
        assert_eq!(Size::from_jump_kg(62_000_000.0), Size::Medium);
        assert_eq!(Size::from_jump_kg(300_000_000.0), Size::Large);
        assert_eq!(Size::from_jump_kg(375_000_000.0), Size::Large);
        assert_eq!(Size::from_jump_kg(410_000_000.0), Size::Large);
        assert_eq!(Size::from_jump_kg(1_000_000_000.0), Size::XLarge);
        assert_eq!(Size::from_jump_kg(2_000_000_000.0), Size::Capital);
        assert_eq!(Size::Large.max_jump_kg(), 375_000_000.0);
        assert_eq!(Size::XLarge.label(), "XL");
        assert!(Size::Small < Size::Capital);
        assert!(MassStatus::Stable < MassStatus::Critical);
    }

    #[test]
    fn time_text() {
        let now = 1_000_000;
        assert_eq!(expiry_text(Expiry { at: now + 2 * HOUR + 600, exact: true }, now), "Expires in 2h 10m");
        assert_eq!(expiry_text(Expiry { at: now + 3 * HOUR + 600, exact: false }, now), "Less than 3h 10m remaining");
        assert_eq!(expiry_text(Expiry { at: now + 300, exact: false }, now), "Less than 5m remaining");
        assert_eq!(expiry_text(Expiry { at: now, exact: true }, now), "Expired");
        assert_eq!(age_text(now - 30, now), "just now");
        assert_eq!(age_text(now - 120, now), "2 min ago");
        assert_eq!(age_text(now - 3 * HOUR, now), "3 h ago");
        assert_eq!(age_text(now - 72 * HOUR, now), "3 days ago");
        // A time after now (a clock change) does not give a negative age.
        assert_eq!(age_text(now + 600, now), "just now");
        // 2026-10-05T14:02:00Z.
        let t = 1_791_208_920;
        assert_eq!(clock_time(t, &chrono::Utc), "14:02");
        assert_eq!(clock_time(t, &chrono::FixedOffset::east_opt(2 * 3600).unwrap()), "16:02");
        assert_eq!(clock_time(t, &chrono::FixedOffset::west_opt(5 * 3600).unwrap()), "09:02");
    }

    #[test]
    fn usable_rules() {
        let now = 1_000_000;
        let medium = Wormhole { size: Some(Size::Medium), ..hole(1, 2) };
        // Sin: 106,300,000 kg. Rifter: 1,067,000 kg.
        assert!(!medium.usable(now, Some(106_300_000.0)));
        assert!(medium.usable(now, Some(1_067_000.0)));
        assert!(medium.usable(now, None));
        // An exact type value wins over the size class.
        let typed = Wormhole { size: Some(Size::Large), max_jump_kg: Some(410_000_000.0), ..hole(1, 2) };
        assert_eq!(typed.jump_limit_kg(), Some(410_000_000.0));
        assert!(typed.usable(now, Some(400_000_000.0)));
        // No size: no size check.
        assert!(hole(1, 2).usable(now, Some(2_400_000_000.0)));
        let critical = Wormhole { mass: Some(MassStatus::Critical), ..hole(1, 2) };
        assert!(!critical.usable(now, None));
        let destabilized = Wormhole { mass: Some(MassStatus::Destabilized), ..hole(1, 2) };
        assert!(destabilized.usable(now, None));
        let ends = Wormhole { expiry: Some(Expiry { at: now, exact: true }), ..hole(1, 2) };
        assert!(!ends.usable(now, None));
        assert!(ends.usable(now - 1, None));
    }
}
