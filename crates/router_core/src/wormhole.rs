//! The shared wormhole record. Each source converts its data to this record.
//! The router reads only this record, never the raw format of a source.

use crate::wormhole_space::Stance;
use crate::wormhole_types::WhCode;
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
    EveMetro,
}

impl SourceId {
    pub fn label(self) -> &'static str {
        match self {
            SourceId::Nexum => "Nexum",
            SourceId::EveScout => "EVE-Scout",
            SourceId::EveMetro => "EVE Metro",
        }
    }

    /// The name of the cache file, without the extension.
    pub fn file_stem(self) -> &'static str {
        match self {
            SourceId::Nexum => "nexum",
            SourceId::EveScout => "eve-scout",
            SourceId::EveMetro => "eve-metro",
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
    /// Unix seconds. A source can give an upper limit, for example from "less than 4 hours".
    pub at: u64,
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
    pub wh_type: Option<WhCode>,
    /// The signature in system `a`, for example "ABC-123". A scout can give only "ABC".
    #[serde(default)]
    pub sig_a: Option<String>,
    /// The signature in system `b`.
    #[serde(default)]
    pub sig_b: Option<String>,
    pub sources: Vec<SourceId>,
}

/// The three letters of a signature, in capitals: "abc-123" gives "ABC". Empty text gives `None`.
pub fn sig_letters(sig: &str) -> Option<String> {
    let letters: String = sig.trim().chars().take_while(char::is_ascii_alphabetic).take(3).collect();
    (!letters.is_empty()).then(|| letters.to_ascii_uppercase())
}

impl Wormhole {
    /// A wormhole between the systems `x` and `y`, with the signature in each system.
    /// The function puts the lower ID in `a`, so `merge` can match records from all sources.
    /// The other fields start as `None`.
    pub fn new(x: u32, y: u32, sig_x: Option<String>, sig_y: Option<String>, source: SourceId) -> Wormhole {
        let ((a, sig_a), (b, sig_b)) = if x <= y { ((x, sig_x), (y, sig_y)) } else { ((y, sig_y), (x, sig_x)) };
        Wormhole { a, b, size: None, max_jump_kg: None, mass: None, expiry: None, wh_type: None, sig_a, sig_b, sources: vec![source] }
    }

    /// The three letters of the signature in the system `id`, if `id` is an end and a scout
    /// gave the signature. A route uses the signature in the system that the jump leaves.
    pub fn sig_at(&self, id: u32) -> Option<String> {
        let sig = if id == self.a {
            &self.sig_a
        } else if id == self.b {
            &self.sig_b
        } else {
            &None
        };
        sig.as_deref().and_then(sig_letters)
    }

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

/// Thera, the wormhole hub in J-space. It has no stargates.
pub const THERA: u32 = 31000005;
/// Turnur, the wormhole hub in lowsec. It has stargates.
pub const TURNUR: u32 = 30002086;

/// The stances of the wormhole hubs. Each hub has a different risk, so each has its own stance.
/// A stance acts on each wormhole with an end in the hub, from all sources. Gates stay open.
/// A file of an old version holds `true` or `false` for a hub, and they read as Allow and Block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hubs {
    #[serde(with = "crate::wormhole_space::switch")]
    pub thera: Stance,
    #[serde(with = "crate::wormhole_space::switch")]
    pub turnur: Stance,
}

impl Default for Hubs {
    fn default() -> Self {
        Hubs { thera: Stance::Allow, turnur: Stance::Allow }
    }
}

impl Hubs {
    /// The stance for a wormhole: the strictest stance of the hubs at its two ends.
    pub fn stance(self, w: &Wormhole) -> Stance {
        let ends_in = |id| w.a == id || w.b == id;
        let at = |id, stance| if ends_in(id) { stance } else { Stance::Allow };
        at(THERA, self.thera).max(at(TURNUR, self.turnur))
    }
}

/// The converted data of one source. The disk cache holds this record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceData {
    pub source: SourceId,
    /// Unix seconds.
    pub fetched_at: u64,
    /// Where the data came from, for example the Nexum map URL. A cache is for one origin only.
    /// An old cache file has no origin.
    #[serde(default)]
    pub origin: Option<String>,
    /// A name for the data, for example the Nexum map name. An old cache file has no name.
    #[serde(default)]
    pub name: Option<String>,
    pub holes: Vec<Wormhole>,
}

/// Join the wormholes of all sources. This function does no I/O.
///
/// - A record with an expiry at or before `now` is dropped.
/// - The records of one source never merge with each other. One source can list two
///   wormholes between the same systems.
/// - Records from two sources merge when they have the same pair, and no end has two known
///   signatures with different letters. A record merges at most one time with each later source.
pub fn merge(sources: &[SourceData], now: u64) -> Vec<Wormhole> {
    let mut out: Vec<Wormhole> = Vec::new();
    for data in sources {
        let earlier = out.len();
        let mut taken = vec![false; earlier];
        for hole in &data.holes {
            if hole.expiry.is_some_and(|e| e.at <= now) {
                continue;
            }
            let matched = (0..earlier).find(|&i| !taken[i] && same_hole(&out[i], hole));
            match matched {
                Some(i) => {
                    taken[i] = true;
                    combine(&mut out[i], hole);
                }
                None => out.push(hole.clone()),
            }
        }
    }
    out
}

fn same_hole(a: &Wormhole, b: &Wormhole) -> bool {
    (a.a, a.b) == (b.a, b.b) && ends_agree(&a.sig_a, &b.sig_a) && ends_agree(&a.sig_b, &b.sig_b)
}

/// An end with a known signature in both records must have the same three letters.
fn ends_agree(x: &Option<String>, y: &Option<String>) -> bool {
    match (x.as_deref().and_then(sig_letters), y.as_deref().and_then(sig_letters)) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// A wormhole that the pilot marked as closed. The mapping tool can still list it. One entry
/// removes one hole, so a second hole between the same systems stays open.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedHole {
    /// The system IDs, as in `Wormhole`: `a` is the lower value.
    pub a: u32,
    pub b: u32,
    /// The signature letters at each end, as known when the pilot closed the hole.
    pub sig_a: Option<String>,
    pub sig_b: Option<String>,
    /// These two only break a tie between holes with the same signatures.
    pub size: Option<Size>,
    pub expiry: Option<Expiry>,
    /// The end that the pilot jumped from, for the lists: `a` or `b`. It plays no part in `matches`.
    pub start: u32,
    /// The count of matching holes when `apply_closed` first used the entry. `None` before that.
    pub seen: Option<usize>,
}

impl ClosedHole {
    /// The entry for a hole of the map. The start is `a`: use `with_start` for the other end.
    pub fn of(w: &Wormhole) -> ClosedHole {
        ClosedHole {
            a: w.a,
            b: w.b,
            sig_a: w.sig_a.as_deref().and_then(sig_letters),
            sig_b: w.sig_b.as_deref().and_then(sig_letters),
            size: w.size,
            expiry: w.expiry,
            start: w.a,
            seen: None,
        }
    }

    /// The same entry with the system `id` as the start. An `id` that is not an end changes nothing.
    pub fn with_start(mut self, id: u32) -> ClosedHole {
        if id == self.a || id == self.b {
            self.start = id;
        }
        self
    }

    /// The start and the end, each with its system ID and its signature letters.
    pub fn ends(&self) -> [(u32, Option<&str>); 2] {
        let (a, b) = ((self.a, self.sig_a.as_deref()), (self.b, self.sig_b.as_deref()));
        if self.start == self.b { [b, a] } else { [a, b] }
    }

    /// The entry for a jump from the system `from` to the system `to`. `sig` is the signature
    /// in `from`. The active route holds only this data of a wormhole.
    pub fn from_jump(from: u32, to: u32, sig: Option<&str>, size: Option<Size>, expiry: Option<Expiry>) -> ClosedHole {
        let sig = sig.and_then(sig_letters);
        let (a, b, sig_a, sig_b) = if from <= to { (from, to, sig, None) } else { (to, from, None, sig) };
        ClosedHole { a, b, sig_a, sig_b, size, expiry, start: from, seen: None }
    }

    /// True if the hole can be the closed one: the same pair, and no end has two known
    /// signatures that differ.
    pub fn matches(&self, w: &Wormhole) -> bool {
        (self.a, self.b) == (w.a, w.b) && ends_agree(&self.sig_a, &w.sig_a) && ends_agree(&self.sig_b, &w.sig_b)
    }

    /// How close a matching hole is to the closed one. A higher value is closer.
    fn closeness(&self, w: &Wormhole) -> (usize, bool) {
        let known = |x: &Option<String>, y: &Option<String>| x.is_some() && x.as_deref() == y.as_deref().and_then(sig_letters).as_deref();
        let exact = usize::from(known(&self.sig_a, &w.sig_a)) + usize::from(known(&self.sig_b, &w.sig_b));
        (exact, self.size == w.size && self.expiry == w.expiry)
    }
}

/// Take the closed holes out of `holes`. Each entry removes the one hole that is closest to it.
/// An entry retires, and leaves `closed`, when fewer holes match than at its first use: the
/// closed hole probably left the map, so a look-alike must stay open.
pub fn apply_closed(mut holes: Vec<Wormhole>, closed: &mut Vec<ClosedHole>) -> Vec<Wormhole> {
    closed.retain_mut(|c| {
        let found: Vec<usize> = (0..holes.len()).filter(|&i| c.matches(&holes[i])).collect();
        let seen = *c.seen.get_or_insert(found.len());
        // The earliest hole wins a tie.
        let best = found.iter().copied().rev().max_by_key(|&i| c.closeness(&holes[i]));
        match best {
            Some(i) if found.len() >= seen => {
                holes.remove(i);
                true
            }
            _ => false,
        }
    });
    holes
}

/// Merge `h` into `o`: the smaller size and limit, the worse mass status, the earlier expiry,
/// the first type and signatures, and all sources.
fn combine(o: &mut Wormhole, h: &Wormhole) {
    o.size = match (o.size, h.size) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    };
    o.max_jump_kg = match (o.max_jump_kg, h.max_jump_kg) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    };
    o.mass = o.mass.max(h.mass);
    o.expiry = match (o.expiry, h.expiry) {
        (Some(x), Some(y)) => Some(if y.at < x.at { y } else { x }),
        (x, y) => x.or(y),
    };
    if o.wh_type.is_none() {
        o.wh_type = h.wh_type.clone();
    }
    if o.sig_a.is_none() {
        o.sig_a = h.sig_a.clone();
    }
    if o.sig_b.is_none() {
        o.sig_b = h.sig_b.clone();
    }
    for s in &h.sources {
        if !o.sources.contains(s) {
            o.sources.push(*s);
        }
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

/// The remaining time of a wormhole.
pub fn expiry_text(e: Expiry, now: u64) -> String {
    if e.at <= now {
        return "Expired".into();
    }
    let left = duration_text(e.at - now);
    format!("Expires in {left}")
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
    use crate::test_support::hole;

    #[test]
    fn hubs_give_a_stance_by_end() {
        use Stance::{Allow, Avoid, Block};
        let all = Hubs::default();
        assert_eq!(all, Hubs { thera: Allow, turnur: Allow });
        let no_thera = Hubs { thera: Block, ..all };
        let no_turnur = Hubs { turnur: Block, ..all };
        // Jita to Thera, Turnur to Thera, and Jita to Amarr.
        let (to_thera, both, other) = (hole(30000142, THERA), hole(TURNUR, THERA), hole(30000142, 30002187));
        assert!(all.stance(&to_thera) == Allow && all.stance(&both) == Allow);
        assert!(no_thera.stance(&to_thera) == Block && no_thera.stance(&both) == Block);
        assert!(no_turnur.stance(&to_thera) == Allow && no_turnur.stance(&both) == Block);
        assert!(no_thera.stance(&other) == Allow && no_turnur.stance(&other) == Allow);
        // The strictest stance of the two ends wins: Block over Avoid over Allow.
        let avoid_thera = Hubs { thera: Avoid, ..all };
        assert_eq!((avoid_thera.stance(&to_thera), avoid_thera.stance(&other)), (Avoid, Allow));
        assert_eq!(Hubs { thera: Avoid, turnur: Block }.stance(&both), Block);
        assert_eq!(Hubs { thera: Block, turnur: Avoid }.stance(&both), Block);
    }

    /// A wormhole between Jita and Amarr with the signature letters at each end.
    fn signed(sig_a: Option<&str>, sig_b: Option<&str>) -> Wormhole {
        Wormhole::new(30000142, 30002187, sig_a.map(Into::into), sig_b.map(Into::into), SourceId::Nexum)
    }

    fn sigs(holes: &[Wormhole]) -> Vec<(Option<String>, Option<String>)> {
        holes.iter().map(|h| (h.sig_a.clone(), h.sig_b.clone())).collect()
    }

    #[test]
    fn a_closed_hole_removes_only_the_hole_with_its_signature() {
        let holes = vec![signed(Some("ABC-123"), None), signed(Some("DEF-456"), None)];
        let mut closed = vec![ClosedHole::of(&holes[0])];
        let kept = apply_closed(holes, &mut closed);
        assert_eq!(sigs(&kept), vec![(Some("DEF-456".into()), None)]);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].seen, Some(1));
    }

    #[test]
    fn a_closed_hole_prefers_the_exact_signature_over_an_unknown_one() {
        let holes = vec![signed(None, None), signed(Some("ABC-123"), None)];
        let mut closed = vec![ClosedHole::of(&holes[1])];
        let kept = apply_closed(holes, &mut closed);
        assert_eq!(sigs(&kept), vec![(None, None)]);
    }

    #[test]
    fn two_identical_holes_keep_one_after_one_closes() {
        let holes = vec![signed(None, None), signed(None, None)];
        let mut closed = vec![ClosedHole::of(&holes[0])];
        assert_eq!(apply_closed(holes.clone(), &mut closed).len(), 1);
        // Closing both in turn leaves none.
        let mut both = vec![ClosedHole::of(&holes[0]), ClosedHole::of(&holes[1])];
        assert!(apply_closed(holes, &mut both).is_empty());
    }

    #[test]
    fn a_learned_signature_keeps_the_hole_closed() {
        let mut closed = vec![ClosedHole::of(&signed(Some("ABC-123"), None))];
        let kept = apply_closed(vec![signed(Some("ABC-123"), Some("XYZ-999")), signed(Some("DEF-456"), None)], &mut closed);
        assert_eq!(sigs(&kept), vec![(Some("DEF-456".into()), None)]);
    }

    #[test]
    fn an_entry_retires_when_fewer_holes_match_than_before() {
        let two = vec![signed(None, None), signed(None, None)];
        let mut closed = vec![ClosedHole::of(&two[0])];
        let first = apply_closed(two, &mut closed);
        assert_eq!(first.len(), 1);
        // The bad hole left the map. The look-alike must stay open.
        let after = apply_closed(first, &mut closed);
        assert_eq!(after.len(), 1);
        assert!(closed.is_empty());
    }

    #[test]
    fn a_closed_hole_ignores_other_pairs() {
        let other = Wormhole::new(30000142, 30002659, None, None, SourceId::Nexum);
        let mut closed = vec![ClosedHole::of(&signed(None, None))];
        let kept = apply_closed(vec![other, signed(None, None)], &mut closed);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].b, 30002659);
    }

    #[test]
    fn a_closed_hole_lists_the_start_first() {
        let hole = signed(Some("ABC-123"), Some("DEF-456"));
        // The start is Jita, the lower ID, until the caller sets another one.
        let closed = ClosedHole::of(&hole);
        assert_eq!(closed.ends(), [(30000142, Some("ABC")), (30002187, Some("DEF"))]);
        let back = closed.with_start(30002187);
        assert_eq!(back.ends(), [(30002187, Some("DEF")), (30000142, Some("ABC"))]);
        // A system that is not an end changes nothing.
        assert_eq!(back.clone().with_start(1), back);
        // The start does not change what the entry matches.
        assert!(back.matches(&hole));
        let jump = ClosedHole::from_jump(30002187, 30000142, Some("DEF"), None, None);
        assert_eq!(jump.ends(), [(30002187, Some("DEF")), (30000142, None)]);
    }

    #[test]
    fn a_closed_hole_from_a_jump_names_the_signature_of_the_leaving_end() {
        // The jump leaves Amarr (the higher ID), so the signature belongs to the `b` end.
        let closed = ClosedHole::from_jump(30002187, 30000142, Some("abc-123"), None, None);
        assert_eq!((closed.a, closed.b), (30000142, 30002187));
        assert_eq!((closed.sig_a, closed.sig_b), (None, Some("ABC".into())));
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
        assert_eq!(expiry_text(Expiry { at: now + 2 * HOUR + 600 }, now), "Expires in 2h 10m");
        assert_eq!(expiry_text(Expiry { at: now + 3 * HOUR + 600 }, now), "Expires in 3h 10m");
        assert_eq!(expiry_text(Expiry { at: now + 300 }, now), "Expires in 5m");
        assert_eq!(expiry_text(Expiry { at: now }, now), "Expired");
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
        let ends = Wormhole { expiry: Some(Expiry { at: now }), ..hole(1, 2) };
        assert!(!ends.usable(now, None));
        assert!(ends.usable(now - 1, None));
    }

    fn data(source: SourceId, holes: Vec<Wormhole>) -> SourceData {
        SourceData { source, fetched_at: 0, origin: None, name: None, holes }
    }

    #[test]
    fn merge_drops_expired() {
        let gone = Wormhole { expiry: Some(Expiry { at: 100 }), ..hole(1, 2) };
        let open = Wormhole { expiry: Some(Expiry { at: 101 }), ..hole(3, 4) };
        let merged = merge(&[data(SourceId::Nexum, vec![gone, open.clone()])], 100);
        assert_eq!(merged, vec![open]);
    }

    #[test]
    fn merge_keeps_two_holes_from_one_source() {
        let merged = merge(&[data(SourceId::Nexum, vec![hole(1, 2), hole(1, 2)])], 0);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_joins_sources() {
        let nexum = Wormhole {
            size: Some(Size::Large),
            max_jump_kg: Some(410_000_000.0),
            mass: Some(MassStatus::Destabilized),
            expiry: Some(Expiry { at: 500 }),
            wh_type: None,
            ..hole(1, 2)
        };
        let scout = Wormhole {
            size: Some(Size::Medium),
            max_jump_kg: None,
            mass: Some(MassStatus::Stable),
            expiry: Some(Expiry { at: 400 }),
            wh_type: Some("K162".into()),
            sig_a: Some("ABC-123".into()),
            sig_b: Some("DEF-456".into()),
            sources: vec![SourceId::EveScout],
            ..hole(1, 2)
        };
        let merged = merge(&[data(SourceId::Nexum, vec![nexum]), data(SourceId::EveScout, vec![scout])], 0);
        assert_eq!(merged.len(), 1);
        let m = &merged[0];
        assert_eq!(m.size, Some(Size::Medium));
        assert_eq!(m.max_jump_kg, Some(410_000_000.0));
        assert_eq!(m.mass, Some(MassStatus::Destabilized));
        assert_eq!(m.expiry, Some(Expiry { at: 400 }));
        assert_eq!(m.wh_type.as_ref().map(WhCode::as_str), Some("K162"));
        assert_eq!((m.sig_a.as_deref(), m.sig_b.as_deref()), (Some("ABC-123"), Some("DEF-456")));
        assert_eq!(m.sources, vec![SourceId::Nexum, SourceId::EveScout]);
    }

    #[test]
    fn merge_keeps_holes_with_other_signatures() {
        let first = Wormhole { sig_a: Some("ABC-123".into()), sig_b: Some("DEF-456".into()), ..hole(1, 2) };
        let second =
            Wormhole { sig_a: Some("XYZ-789".into()), sig_b: Some("DEF-456".into()), sources: vec![SourceId::EveScout], ..hole(1, 2) };
        let merged = merge(&[data(SourceId::Nexum, vec![first]), data(SourceId::EveScout, vec![second])], 0);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_compares_the_three_letters_of_known_ends() {
        // Nexum knows only the signature at a, as "abc". EVE-Scout knows both ends.
        let nexum = Wormhole { sig_a: Some("abc".into()), ..hole(1, 2) };
        let scout =
            Wormhole { sig_a: Some("ABC-123".into()), sig_b: Some("DEF-456".into()), sources: vec![SourceId::EveScout], ..hole(1, 2) };
        let merged = merge(&[data(SourceId::Nexum, vec![nexum]), data(SourceId::EveScout, vec![scout.clone()])], 0);
        assert_eq!(merged.len(), 1);
        // The first source keeps its value. The second source fills the unknown end.
        assert_eq!((merged[0].sig_a.as_deref(), merged[0].sig_b.as_deref()), (Some("abc"), Some("DEF-456")));
        // A signature at b that disagrees stops the merge.
        let other = Wormhole { sig_b: Some("XYZ-789".into()), ..hole(1, 2) };
        assert_eq!(merge(&[data(SourceId::Nexum, vec![other]), data(SourceId::EveScout, vec![scout])], 0).len(), 2);
    }

    #[test]
    fn signature_at_an_end() {
        let w = Wormhole { sig_a: Some("ABC-123".into()), sig_b: Some("de".into()), ..hole(10, 20) };
        assert_eq!(w.sig_at(10).as_deref(), Some("ABC"));
        assert_eq!(w.sig_at(20).as_deref(), Some("DE"));
        assert_eq!(w.sig_at(30), None);
        assert_eq!(Wormhole { sig_a: Some("  ".into()), ..hole(10, 20) }.sig_at(10), None);
    }

    #[test]
    fn merge_uses_each_record_one_time_per_source() {
        // Source 2 lists two holes for the pair. Only one of them merges with the source 1 hole.
        let two = vec![
            Wormhole { sources: vec![SourceId::EveScout], ..hole(1, 2) },
            Wormhole { sources: vec![SourceId::EveScout], ..hole(1, 2) },
        ];
        let merged = merge(&[data(SourceId::Nexum, vec![hole(1, 2)]), data(SourceId::EveScout, two)], 0);
        assert_eq!(merged.len(), 2);
    }

    /// A cache file from an older version stores the type code as plain text, in any case.
    #[test]
    fn an_old_cache_code_loads_as_a_code() {
        let mut json = serde_json::to_value(Wormhole::new(1, 2, None, None, SourceId::Nexum)).unwrap();
        json["wh_type"] = " b274 ".into();
        let hole: Wormhole = serde_json::from_value(json).unwrap();
        assert_eq!(hole.wh_type, Some(WhCode::from("B274")));
        assert_eq!(serde_json::to_value(&hole).unwrap()["wh_type"], "B274");
    }
}
