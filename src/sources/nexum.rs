//! The Nexum source: the map from `GET /api/v1/maps/:mapId`, converted to wormhole records.
//! A manual map export has the same shape, so `--nexum <file>` uses the same converter.
//! The size and expiry rules copy Nexum (`server/src/routes/maps.ts` and
//! `server/src/data/whLifetimes.ts`).

use crate::wormhole::{Expiry, HOUR, MassStatus, Size, SourceData, SourceId, Wormhole, now, parse_utc};
use crate::wormhole_types::WormholeTypes;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NexumMap {
    pub systems: Vec<NexumSystem>,
    pub connections: Vec<NexumConnection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NexumSystem {
    /// The map-local ID.
    pub id: String,
    /// `None` for a placeholder: a custom node with no EVE system.
    pub eve_system_id: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NexumConnection {
    pub source_id: String,
    pub target_id: String,
    pub connection_type: String,
    pub size: Option<String>,
    #[serde(rename = "type")]
    pub wh_type: Option<String>,
    pub mass_status: Option<String>,
    pub time_status: Option<String>,
    pub lifetime_expires_at: Option<String>,
    pub eol_at: Option<String>,
    pub created_at: Option<String>,
    #[serde(default)]
    pub broken: bool,
}

pub fn parse_map(text: &str) -> Result<NexumMap, String> {
    serde_json::from_str(text).map_err(|e| format!("Nexum map: {e}"))
}

/// The connections that the converter did not use, by reason.
#[derive(Default, Debug, PartialEq)]
pub struct NexumReport {
    /// Connections with a placeholder system at one end.
    pub placeholders: usize,
    /// EVE system IDs that are not in the SDE.
    pub unknown: Vec<u32>,
    /// Connections with an end that is not a known system.
    pub missing_end: usize,
    pub broken: usize,
    /// Connections that expired before the fetch.
    pub expired: usize,
    /// Times in a format that the parser does not know.
    pub bad_time: usize,
}

impl NexumReport {
    /// The count for the sidebar "Skipped" line.
    pub fn skipped(&self) -> usize {
        self.broken + self.expired + self.missing_end
    }
}

/// The time status gives an upper limit: "lessThan4h" gives 4. "fresh" and "expired" give `None`.
fn status_hours(status: &str) -> Option<u64> {
    status.strip_prefix("lessThan")?.strip_suffix('h')?.parse().ok()
}

fn parse_size(size: &str) -> Option<Size> {
    match size {
        "small" => Some(Size::Small),
        "medium" => Some(Size::Medium),
        "large" => Some(Size::Large),
        "xl" => Some(Size::XLarge),
        _ => None,
    }
}

fn parse_mass(mass: &str) -> Option<MassStatus> {
    match mass {
        "stable" => Some(MassStatus::Stable),
        "destabilized" => Some(MassStatus::Destabilized),
        "critical" => Some(MassStatus::Critical),
        _ => None,
    }
}

/// The earlier of two expiry values.
fn earlier(a: Option<Expiry>, b: Option<Expiry>) -> Option<Expiry> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.at < x.at { y } else { x }),
        (x, y) => x.or(y),
    }
}

/// The expiry, in the Nexum order: the manual time, then the EOL mark plus 4 h, then the
/// creation time plus the type life. The time status is at most one hour old, so it can give
/// an earlier limit. The earlier value wins.
fn expiry(c: &NexumConnection, types: &WormholeTypes, fetched_at: u64, report: &mut NexumReport) -> Option<Expiry> {
    let mut time = |value: &Option<String>| {
        let parsed = value.as_deref().map(parse_utc)?;
        if parsed.is_none() {
            report.bad_time += 1;
        }
        parsed
    };
    let base = if let Some(at) = time(&c.lifetime_expires_at) {
        Some(Expiry { at, exact: true })
    } else if let Some(eol) = time(&c.eol_at) {
        Some(Expiry { at: eol + 4 * HOUR, exact: false })
    } else {
        match c.wh_type.as_deref().and_then(|t| types.life_hours(t)) {
            Some(hours) => time(&c.created_at).map(|created| Expiry { at: created + (hours * 3600.0) as u64, exact: false }),
            None => None,
        }
    };
    let status = c.time_status.as_deref().and_then(status_hours).map(|h| Expiry { at: fetched_at + h * HOUR, exact: false });
    earlier(base, status)
}

/// Convert a Nexum map. `known` says if an EVE system ID is in the SDE.
/// The converter never reads or changes the graph.
pub fn convert(map: &NexumMap, known: impl Fn(u32) -> bool, types: &WormholeTypes, fetched_at: u64) -> (SourceData, NexumReport) {
    let mut report = NexumReport::default();
    let mut placeholders: HashSet<&str> = HashSet::new();
    let mut ids: HashMap<&str, u32> = HashMap::new();
    for s in &map.systems {
        match s.eve_system_id {
            None => {
                placeholders.insert(s.id.as_str());
            }
            Some(id) if known(id) => {
                ids.insert(s.id.as_str(), id);
            }
            Some(id) => report.unknown.push(id),
        }
    }

    let mut holes = Vec::new();
    for c in map.connections.iter().filter(|c| c.connection_type == "standard") {
        if c.broken {
            report.broken += 1;
            continue;
        }
        if c.time_status.as_deref() == Some("expired") {
            report.expired += 1;
            continue;
        }
        if placeholders.contains(c.source_id.as_str()) || placeholders.contains(c.target_id.as_str()) {
            report.placeholders += 1;
            continue;
        }
        let (Some(&a), Some(&b)) = (ids.get(c.source_id.as_str()), ids.get(c.target_id.as_str())) else {
            report.missing_end += 1;
            continue;
        };
        let expiry = expiry(c, types, fetched_at, &mut report);
        if expiry.is_some_and(|e| e.at <= fetched_at) {
            report.expired += 1;
            continue;
        }
        // Nexum takes the size from a known type. A K162 or a hole with no type keeps the stored size.
        let (size, max_jump_kg) = match c.wh_type.as_deref().and_then(|t| types.get(t)) {
            Some(t) => (Some(Size::from_jump_kg(t.max_jump_kg)), Some(t.max_jump_kg)),
            None => (c.size.as_deref().and_then(parse_size), None),
        };
        holes.push(Wormhole {
            a: a.min(b),
            b: a.max(b),
            size,
            max_jump_kg,
            mass: c.mass_status.as_deref().and_then(parse_mass),
            expiry,
            wh_type: c.wh_type.clone(),
            sigs: None,
            sources: vec![SourceId::Nexum],
        });
    }
    (SourceData { source: SourceId::Nexum, fetched_at, holes }, report)
}

/// The wormhole data of the Nexum source, for the startup and the sidebar.
#[derive(Default)]
pub struct Load {
    pub data: Option<SourceData>,
    pub report: NexumReport,
    /// A problem for the status line, for example "Nexum offline, map from 14:02".
    pub warning: Option<String>,
}

/// Read a map file (`--nexum`, or an export next to the config file).
/// The time of the file is the fetch time.
pub fn load_file(path: &Path, known: impl Fn(u32) -> bool, types: &WormholeTypes) -> Result<Load, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let map = parse_map(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let fetched_at = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or_else(now, |d| d.as_secs());
    let (data, report) = convert(&map, known, types, fetched_at);
    Ok(Load { data: Some(data), report, warning: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wormhole_types::WormholeType;

    /// 2026-10-05T12:00:00Z.
    const FETCHED: u64 = 1_791_201_600;

    fn types() -> WormholeTypes {
        let b274 = WormholeType {
            code: "B274".into(),
            max_jump_kg: 375_000_000.0,
            max_life_h: 24.0,
            total_mass_kg: Some(2_000_000_000.0),
            target_class: Some(7),
        };
        WormholeTypes { build: None, types: vec![b274] }
    }

    fn fixture() -> (SourceData, NexumReport) {
        let text = std::fs::read_to_string("tests/fixtures/nexum-api.json").unwrap();
        let map = parse_map(&text).unwrap();
        // 39999999 is not a known system.
        convert(&map, |id| id != 39_999_999, &types(), FETCHED)
    }

    fn find(data: &SourceData, a: u32, b: u32) -> &Wormhole {
        data.holes.iter().find(|h| (h.a, h.b) == (a.min(b), a.max(b))).unwrap()
    }

    #[test]
    fn converts_the_fixture() {
        let (data, report) = fixture();
        assert_eq!(data.source, SourceId::Nexum);
        assert_eq!(data.fetched_at, FETCHED);
        // typed, k162, critical, eol, badtime.
        assert_eq!(data.holes.len(), 5);
        assert_eq!(
            report,
            NexumReport { placeholders: 1, unknown: vec![39_999_999], missing_end: 1, broken: 1, expired: 2, bad_time: 1 }
        );
        assert_eq!(report.skipped(), 4);
        // No jumpgate, gate or cyno connection: Perimeter-Amarr, Jita-Perimeter and Jita-Hek are not wormholes.
        for (a, b) in [(30000144, 30002187), (30000142, 30000144), (30000142, 30002053)] {
            assert!(!data.holes.iter().any(|h| (h.a, h.b) == (a, b)));
        }
    }

    #[test]
    fn type_sets_size_and_expiry() {
        let (data, _) = fixture();
        let typed = find(&data, 30000142, 31002230);
        // The B274 type wins over the stored "medium".
        assert_eq!(typed.size, Some(Size::Large));
        assert_eq!(typed.max_jump_kg, Some(375_000_000.0));
        assert_eq!(typed.mass, Some(MassStatus::Stable));
        assert_eq!(typed.wh_type.as_deref(), Some("B274"));
        // createdAt 02:00 + 24 h is before the time status limit (12:00 + 24 h).
        assert_eq!(typed.expiry, Some(Expiry { at: FETCHED - 10 * HOUR + 24 * HOUR, exact: false }));
        assert_eq!(typed.sources, vec![SourceId::Nexum]);
        assert_eq!(typed.sigs, None);
    }

    #[test]
    fn k162_keeps_stored_size() {
        let (data, _) = fixture();
        let k162 = find(&data, 30002187, 31002230);
        assert_eq!(k162.size, Some(Size::Medium));
        assert_eq!(k162.max_jump_kg, None);
        assert_eq!(k162.mass, Some(MassStatus::Destabilized));
        // createdAt + 48 h is later than the time status limit (12:00 + 4 h).
        assert_eq!(k162.expiry, Some(Expiry { at: FETCHED + 4 * HOUR, exact: false }));
    }

    #[test]
    fn expiry_sources() {
        let (data, _) = fixture();
        let critical = find(&data, 30002510, 31002230);
        assert_eq!(critical.mass, Some(MassStatus::Critical));
        assert_eq!(critical.size, Some(Size::Small));
        assert_eq!(critical.expiry, Some(Expiry { at: FETCHED + 8 * HOUR + 1800, exact: true }));
        let eol = find(&data, 30002659, 31000005);
        assert_eq!(eol.size, Some(Size::XLarge));
        assert_eq!(eol.mass, None);
        assert_eq!(eol.expiry, Some(Expiry { at: FETCHED - HOUR + 4 * HOUR, exact: false }));
        // A bad time and no type: no expiry.
        let bad = find(&data, 30000144, 31000005);
        assert_eq!(bad.size, Some(Size::Large));
        assert_eq!(bad.expiry, None);
    }

    #[test]
    fn value_mapping() {
        assert_eq!(parse_size("xl"), Some(Size::XLarge));
        assert_eq!(parse_size("capital"), None);
        assert_eq!(parse_size("huge"), None);
        assert_eq!(parse_mass("critical"), Some(MassStatus::Critical));
        assert_eq!(parse_mass("verge"), None);
        assert_eq!(status_hours("lessThan1h"), Some(1));
        assert_eq!(status_hours("lessThan24h"), Some(24));
        assert_eq!(status_hours("fresh"), None);
        assert_eq!(status_hours("expired"), None);
    }

    #[test]
    fn old_export_still_parses() {
        // The first fixture has fewer fields. Each missing field is `None`.
        let text = std::fs::read_to_string("tests/fixtures/nexum.json").unwrap();
        let (data, report) = convert(&parse_map(&text).unwrap(), |_| true, &types(), FETCHED);
        assert_eq!(data.holes.len(), 1);
        assert_eq!(report.broken, 1);
        assert_eq!(report.expired, 1);
    }
}
