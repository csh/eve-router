//! The Nexum source: the map from `GET /api/v1/maps/:mapId`, converted to wormhole records.
//! The router reads Nexum data only from the API.
//! The size and expiry rules copy Nexum (`server/src/routes/maps.ts` and
//! `server/src/data/whLifetimes.ts`).

use crate::config::NexumConfig;
use crate::sources::{self, FetchError};
use crate::wormhole::{Expiry, HOUR, MassStatus, Size, SourceData, SourceId, Wormhole, parse_utc};
use crate::wormhole_types::WormholeTypes;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

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
    /// The system name, for the `whLeadsTo` match of a signature.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, Clone)]
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
    /// The row ID of the signature in the source system, when a scout linked it.
    pub source_signature_id: Option<String>,
    pub target_signature_id: Option<String>,
}

/// One signature of a system, from `GET /api/v1/maps/:mapId/systems/:systemId/signatures`.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NexumSig {
    /// The row ID. A connection links a signature by this ID.
    pub id: String,
    /// For example "ABC-123".
    pub sig_id: Option<String>,
    /// For example "wormhole" or "data".
    pub sig_type: Option<String>,
    /// Free text from the scout: a system name, a class such as "NS", or "Drifter".
    pub wh_leads_to: Option<String>,
}

/// The signatures of each system, by the map-local system ID.
pub type SystemSigs = HashMap<String, Vec<NexumSig>>;

/// The number of signature requests that run at the same time.
const SIG_WORKERS: usize = 8;

pub fn parse_map(text: &str) -> Result<NexumMap, String> {
    serde_json::from_str(text).map_err(|e| format!("Nexum map: {e}"))
}

/// One map in `GET /api/v1/maps`.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct MapInfo {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct MapList {
    maps: Vec<MapInfo>,
}

/// The API base: the URL without a trailing slash, plus `/api/v1`.
fn api_base(cfg: &NexumConfig) -> Option<String> {
    Some(format!("{}/api/v1", cfg.url.as_deref()?.trim_end_matches('/')))
}

/// The URL of the configured map.
pub fn map_url(cfg: &NexumConfig) -> Option<String> {
    Some(format!("{}/maps/{}", api_base(cfg)?, cfg.map_id.as_deref()?))
}

/// GET the configured map, as text. The caller parses it after the SDE loads.
pub fn fetch_map(cfg: &NexumConfig, timeout: Duration) -> Result<String, FetchError> {
    let (url, key) = (map_url(cfg), cfg.key.as_ref());
    let (Some(url), Some(key)) = (url, key) else {
        return Err(FetchError::Offline("the Nexum settings are not complete".into()));
    };
    sources::get(&url, Some(&key.0), timeout)
}

/// The map-local IDs of the systems at the ends of the wormhole connections, sorted.
/// A placeholder has no EVE system, so it has no signatures to fetch.
pub fn sig_systems(map: &NexumMap) -> Vec<String> {
    let real: HashSet<&str> = map.systems.iter().filter(|s| s.eve_system_id.is_some()).map(|s| s.id.as_str()).collect();
    let mut ids: Vec<String> = map
        .connections
        .iter()
        .filter(|c| c.connection_type == "standard" && !c.broken)
        .flat_map(|c| [c.source_id.as_str(), c.target_id.as_str()])
        .filter(|id| real.contains(id))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// GET the signatures of each system in `ids`, with `SIG_WORKERS` requests at a time.
/// Return the signatures, and the number of systems with a failed request.
pub fn fetch_sigs(cfg: &NexumConfig, ids: &[String], timeout: Duration) -> (SystemSigs, usize) {
    let (Some(map), Some(key)) = (map_url(cfg), cfg.key.as_ref()) else {
        return (SystemSigs::new(), ids.len());
    };
    // One agent for all workers, so the requests reuse the connections.
    let agent = sources::agent(timeout);
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(ids.len()));
    std::thread::scope(|scope| {
        for _ in 0..SIG_WORKERS.min(ids.len()) {
            scope.spawn(|| {
                while let Some(id) = ids.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let url = format!("{map}/systems/{id}/signatures");
                    let sigs =
                        sources::get_with(&agent, &url, Some(&key.0)).ok().and_then(|t| serde_json::from_str::<Vec<NexumSig>>(&t).ok());
                    results.lock().unwrap().push((id.clone(), sigs));
                }
            });
        }
    });
    let mut out = SystemSigs::new();
    let mut failed = 0;
    for (id, sigs) in results.into_inner().unwrap() {
        match sigs {
            Some(sigs) => {
                out.insert(id, sigs);
            }
            None => failed += 1,
        }
    }
    (out, failed)
}

/// GET the map list, for the settings page. It needs the URL and the key, but no map ID.
pub fn fetch_maps(cfg: &NexumConfig, timeout: Duration) -> Result<Vec<MapInfo>, FetchError> {
    let (Some(base), Some(key)) = (api_base(cfg), cfg.key.as_ref()) else {
        return Err(FetchError::Offline("set the Nexum URL and key first".into()));
    };
    let text = sources::get(&format!("{base}/maps"), Some(&key.0), timeout)?;
    let list: MapList = serde_json::from_str(&text).map_err(|e| FetchError::Offline(format!("Nexum map list: {e}")))?;
    Ok(list.maps)
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
/// creation time plus the type life. `ends` are the EVE system IDs, for the K162 life. The time status is at most one hour old, so it can give
/// an earlier limit. The earlier value wins.
fn expiry(c: &NexumConnection, ends: [u32; 2], types: &WormholeTypes, fetched_at: u64, report: &mut NexumReport) -> Option<Expiry> {
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
        Some(Expiry { at: eol.saturating_add(4 * HOUR), exact: false })
    } else {
        match c.wh_type.as_deref().and_then(|t| types.life_hours(t, ends)) {
            Some(hours) => time(&c.created_at).map(|created| Expiry { at: created.saturating_add((hours * 3600.0) as u64), exact: false }),
            None => None,
        }
    };
    // The server text gives any number, so the math saturates.
    let status = c
        .time_status
        .as_deref()
        .and_then(status_hours)
        .map(|h| Expiry { at: fetched_at.saturating_add(h.saturating_mul(HOUR)), exact: false });
    earlier(base, status)
}

/// The signature of a connection in the system `from`. A linked signature row comes first.
/// Else, the one wormhole signature in `from` whose `whLeadsTo` is the name of `to`.
/// `single` is false when more connections join the same two systems: one name match
/// cannot then tell which connection the signature belongs to.
fn end_sig(link: Option<&str>, from: &str, to: Option<&str>, sigs: &SystemSigs, single: bool) -> Option<String> {
    let list = sigs.get(from)?;
    if let Some(sig) = link.and_then(|link| list.iter().find(|s| s.id == link)) {
        return sig.sig_id.clone();
    }
    let to = to.filter(|_| single)?.trim();
    let mut found = list.iter().filter(|s| {
        s.sig_type.as_deref() == Some("wormhole") && s.wh_leads_to.as_deref().is_some_and(|w| w.trim().eq_ignore_ascii_case(to))
    });
    let first = found.next()?;
    if found.next().is_some() { None } else { first.sig_id.clone() }
}

/// Convert a Nexum map. `known` says if an EVE system ID is in the SDE.
/// `sigs` gives the signatures of each system. A map file has none.
/// The converter never reads or changes the graph.
pub fn convert(
    map: &NexumMap,
    sigs: &SystemSigs,
    known: impl Fn(u32) -> bool,
    types: &WormholeTypes,
    fetched_at: u64,
) -> (SourceData, NexumReport) {
    let mut report = NexumReport::default();
    let names: HashMap<&str, &str> = map.systems.iter().filter_map(|s| Some((s.id.as_str(), s.name.as_deref()?))).collect();
    // The number of wormhole connections between each two systems.
    let mut pair_count: HashMap<(&str, &str), usize> = HashMap::new();
    for c in map.connections.iter().filter(|c| c.connection_type == "standard" && !c.broken) {
        let (x, y) = (c.source_id.as_str(), c.target_id.as_str());
        *pair_count.entry((x.min(y), x.max(y))).or_default() += 1;
    }
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
        let expiry = expiry(c, [a, b], types, fetched_at, &mut report);
        if expiry.is_some_and(|e| e.at <= fetched_at) {
            report.expired += 1;
            continue;
        }
        // An empty type text counts as no type.
        let wh_type = c.wh_type.as_deref().map(str::trim).filter(|t| !t.is_empty());
        // Nexum takes the size from a known type. A K162 or a hole with no type keeps the stored size.
        let (size, max_jump_kg) = match wh_type.and_then(|t| types.get(t)) {
            Some(t) => (Some(Size::from_jump_kg(t.max_jump_kg)), Some(t.max_jump_kg)),
            None => (c.size.as_deref().and_then(parse_size), None),
        };
        let (x, y) = (c.source_id.as_str(), c.target_id.as_str());
        let single = pair_count.get(&(x.min(y), x.max(y))) == Some(&1);
        let source_sig = end_sig(c.source_signature_id.as_deref(), x, names.get(y).copied(), sigs, single);
        let target_sig = end_sig(c.target_signature_id.as_deref(), y, names.get(x).copied(), sigs, single);
        holes.push(Wormhole {
            size,
            max_jump_kg,
            mass: c.mass_status.as_deref().and_then(parse_mass),
            expiry,
            wh_type: wh_type.map(str::to_string),
            ..Wormhole::new(a, b, source_sig, target_sig, SourceId::Nexum)
        });
    }
    (SourceData { source: SourceId::Nexum, fetched_at, origin: None, holes }, report)
}

/// The wormhole data of the Nexum source, for the startup and the sidebar.
#[derive(Default)]
pub struct Load {
    pub data: Option<SourceData>,
    pub report: NexumReport,
    /// A problem for the status line, for example "Nexum offline, map from 14:02".
    pub warning: Option<String>,
}

/// The Nexum load, between the start and the end of the SDE load.
pub enum Pending {
    /// No complete Nexum settings.
    None,
    /// A cache that is less than 5 minutes old. The router sends no request.
    Cache(SourceData),
    /// A fetch on a thread. The cache is the fallback for a failure.
    Fetch {
        thread: JoinHandle<Result<Fetched, FetchError>>,
        started: u64,
        origin: Option<String>,
        cache: Option<SourceData>,
        cache_path: PathBuf,
    },
}

/// The result of the fetch thread: the map, and the signatures of its wormhole ends.
pub struct Fetched {
    map: NexumMap,
    sigs: SystemSigs,
    /// The number of systems with a failed signature request.
    sig_failures: usize,
}

/// GET the map, then the signatures of its wormhole ends. A failed signature request
/// drops only the signatures of that system.
fn fetch_all(cfg: &NexumConfig) -> Result<Fetched, FetchError> {
    let map = parse_map(&fetch_map(cfg, sources::TIMEOUT)?).map_err(FetchError::Offline)?;
    let (sigs, sig_failures) = fetch_sigs(cfg, &sig_systems(&map), sources::TIMEOUT);
    Ok(Fetched { map, sigs, sig_failures })
}

/// Start the Nexum load: a fresh cache, else a fetch on a thread.
pub fn start(cfg: &NexumConfig, cache_path: PathBuf, now: u64) -> Pending {
    if cfg.complete().is_none() {
        return Pending::None;
    }
    // A cache is for one map. Another source, another map or no origin counts as no cache.
    let origin = map_url(cfg);
    let cache = sources::read_cache(&cache_path).filter(|d| d.source == SourceId::Nexum && d.origin == origin);
    if let Some(data) = cache.as_ref().filter(|d| sources::cache_is_fresh(d, now)) {
        return Pending::Cache(data.clone());
    }
    let cfg = cfg.clone();
    let thread = std::thread::spawn(move || fetch_all(&cfg));
    Pending::Fetch { thread, started: now, origin, cache, cache_path }
}

/// Finish the Nexum load after the SDE loads. A fetch problem gives the cache and a warning.
pub fn finish(pending: Pending, known: impl Fn(u32) -> bool, types: &WormholeTypes) -> Load {
    match pending {
        Pending::None => Load::default(),
        Pending::Cache(data) => Load { data: Some(data), ..Load::default() },
        Pending::Fetch { thread, started, origin, cache, cache_path } => {
            let fetched = thread.join().unwrap_or_else(|_| Err(FetchError::Offline("the fetch thread failed".into())));
            let mut report = NexumReport::default();
            let mut sig_failures = 0;
            let converted = fetched.map(|f| {
                let (mut data, r) = convert(&f.map, &f.sigs, &known, types, started);
                data.origin = origin;
                report = r;
                sig_failures = f.sig_failures;
                data
            });
            let chosen = sources::choose(SourceId::Nexum, converted, cache, &cache_path);
            let sig_warning = match sig_failures {
                0 => None,
                1 => Some("Nexum signatures: 1 system not loaded".to_string()),
                n => Some(format!("Nexum signatures: {n} systems not loaded")),
            };
            let warnings: Vec<String> = chosen.warning.into_iter().chain(sig_warning).collect();
            Load { data: chosen.data, report, warning: (!warnings.is_empty()).then(|| warnings.join(" · ")) }
        }
    }
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
        fixture_with(&SystemSigs::new())
    }

    fn fixture_with(sigs: &SystemSigs) -> (SourceData, NexumReport) {
        let text = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let map = parse_map(&text).unwrap();
        // 39999999 is not a known system.
        convert(&map, sigs, |id| id != 39_999_999, &types(), FETCHED)
    }

    fn fixture_sigs() -> SystemSigs {
        serde_json::from_str(&std::fs::read_to_string(crate::test_support::fixture("nexum-sigs.json")).unwrap()).unwrap()
    }

    fn sig_pair(w: &Wormhole) -> (Option<&str>, Option<&str>) {
        (w.sig_a.as_deref(), w.sig_b.as_deref())
    }

    #[test]
    fn signatures_by_link_and_by_leads_to() {
        let (data, _) = fixture_with(&fixture_sigs());
        // Jita to J134702: each end has one signature that leads to the other end.
        // The name match ignores case and spaces.
        assert_eq!(sig_pair(find(&data, 30000142, 31002230)), (Some("BTA-111"), Some("JIT-202")));
        // J134702 to Amarr: the connection links the J134702 signature by its row ID.
        assert_eq!(sig_pair(find(&data, 30002187, 31002230)), (None, Some("KXA-101")));
        // Rens to J134702: two signatures lead to Rens, so the converter does not guess.
        assert_eq!(sig_pair(find(&data, 30002510, 31002230)), (None, None));
        // Perimeter to Thera: the data site that "leads to" Thera is not a wormhole.
        // The Thera signature leads to "Drifter", so it does not match.
        assert_eq!(sig_pair(find(&data, 30000144, 31000005)), (Some("THE-222"), None));
    }

    #[test]
    fn two_connections_on_one_pair_get_no_name_match() {
        let text = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let mut map = parse_map(&text).unwrap();
        // A second Perimeter to Thera connection. One "Thera" signature cannot belong to both.
        let i = map.connections.iter().position(|c| (c.source_id.as_str(), c.target_id.as_str()) == ("s-perimeter", "s-thera")).unwrap();
        map.connections.push(map.connections[i].clone());
        let (data, _) = convert(&map, &fixture_sigs(), |_| true, &types(), FETCHED);
        let pairs: Vec<_> = data.holes.iter().filter(|h| (h.a, h.b) == (30000144, 31000005)).map(sig_pair).collect();
        assert_eq!(pairs, [(None, None), (None, None)]);
    }

    #[test]
    fn empty_type_is_no_type() {
        // The API gives an empty text for a type that a scout did not set.
        let text = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let mut map = parse_map(&text).unwrap();
        let i = map.connections.iter().position(|c| c.wh_type.as_deref() == Some("B274")).unwrap();
        map.connections[i].wh_type = Some(" ".into());
        let (data, _) = convert(&map, &SystemSigs::new(), |_| true, &types(), FETCHED);
        let hole = find(&data, 30000142, 31002230);
        assert_eq!(hole.wh_type, None);
        // With no type, the stored size gives the class.
        assert_eq!((hole.size, hole.max_jump_kg), (Some(Size::Medium), None));
    }

    #[test]
    fn signature_systems_are_the_wormhole_ends() {
        let text = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let ids = sig_systems(&parse_map(&text).unwrap());
        // No placeholder, no broken connection, and no gate, jumpgate or cyno connection.
        let expected = ["s-amarr", "s-dodixie", "s-hek", "s-j134702", "s-jita", "s-perimeter", "s-rens", "s-thera", "s-unknown"];
        assert_eq!(ids, expected);
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
        assert_eq!(report, NexumReport { placeholders: 1, unknown: vec![39_999_999], missing_end: 1, broken: 1, expired: 2, bad_time: 1 });
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
        assert_eq!((typed.sig_a.as_deref(), typed.sig_b.as_deref()), (None, None));
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
    fn a_huge_time_status_does_not_overflow() {
        let c: NexumConnection = serde_json::from_value(serde_json::json!({
            "sourceId": "1",
            "targetId": "2",
            "connectionType": "wormhole",
            "timeStatus": format!("lessThan{}h", u64::MAX),
        }))
        .unwrap();
        let mut report = NexumReport::default();
        let e = expiry(&c, [1, 2], &WormholeTypes::default(), 1_000, &mut report).unwrap();
        assert_eq!(e.at, u64::MAX);
    }

    #[test]
    fn k162_to_a_drifter_system_lives_16h() {
        // createdAt is 3h 19m before the fetch, as with the KVW-404 hole to Liberated Barbican.
        let created = FETCHED - 3 * HOUR - 19 * 60;
        let created_text = chrono::DateTime::from_timestamp(created as i64, 0).unwrap().to_rfc3339();
        let c: NexumConnection = serde_json::from_value(serde_json::json!({
            "sourceId": "1",
            "targetId": "2",
            "connectionType": "standard",
            "type": "K162",
            "timeStatus": "fresh",
            "createdAt": created_text,
        }))
        .unwrap();
        let mut report = NexumReport::default();
        let drifter = expiry(&c, [30002481, 31000002], &WormholeTypes::default(), FETCHED, &mut report);
        assert_eq!(drifter, Some(Expiry { at: created + 16 * HOUR, exact: false }));
        assert_eq!(crate::wormhole::expiry_text(drifter.unwrap(), FETCHED), "Less than 12h 41m remaining");
        // A K162 with no Drifter end keeps the 48 h limit.
        let other = expiry(&c, [30002481, 31002230], &WormholeTypes::default(), FETCHED, &mut report);
        assert_eq!(other, Some(Expiry { at: created + 48 * HOUR, exact: false }));
        // A manual time wins over the K162 life.
        let mut manual = c.clone();
        manual.lifetime_expires_at = Some(chrono::DateTime::from_timestamp((FETCHED + 20 * HOUR) as i64, 0).unwrap().to_rfc3339());
        let manual = expiry(&manual, [30002481, 31000002], &WormholeTypes::default(), FETCHED, &mut report);
        assert_eq!(manual, Some(Expiry { at: FETCHED + 20 * HOUR, exact: true }));
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

    use crate::config::{ApiKey, NexumConfig};
    use crate::test_support::serve;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn cfg(url: &str) -> NexumConfig {
        NexumConfig { url: Some(url.into()), key: Some(ApiKey("nxm_test".into())), map_id: Some("m1".into()) }
    }

    /// The map URL of `cfg("http://127.0.0.1:9")`.
    const MAP1: &str = "http://127.0.0.1:9/api/v1/maps/m1";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nexum.json")
    }

    #[test]
    fn partial_settings_do_not_fetch() {
        let mut partial = cfg("http://127.0.0.1:9");
        partial.key = None;
        assert!(matches!(start(&partial, temp("eve-router-test-partial"), FETCHED), Pending::None));
    }

    #[test]
    fn fresh_cache_stops_the_fetch() {
        let path = temp("eve-router-test-fresh");
        let cached = SourceData { source: SourceId::Nexum, fetched_at: FETCHED - 60, origin: Some(MAP1.into()), holes: Vec::new() };
        crate::sources::write_cache(&path, &cached).unwrap();
        // Port 9 has no server, so a fetch would fail.
        let pending = start(&cfg("http://127.0.0.1:9"), path.clone(), FETCHED);
        let load = finish(pending, |_| true, &types());
        assert_eq!(load.data, Some(cached));
        assert_eq!(load.warning, None);
    }

    #[test]
    fn fetch_converts_and_writes_the_cache() {
        let body = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let (url, request) = serve("200 OK", &body, Duration::ZERO);
        let path = temp("eve-router-test-fetch");
        let load = finish(start(&cfg(&url), path.clone(), FETCHED), |id| id != 39_999_999, &types());
        assert!(request.recv().unwrap().starts_with("GET /api/v1/maps/m1 HTTP/1.1"));
        let data = load.data.unwrap();
        assert_eq!(data.holes.len(), 5);
        assert_eq!(data.fetched_at, FETCHED);
        assert_eq!(load.report.placeholders, 1);
        assert_eq!(crate::sources::read_cache(&path), Some(data));
    }

    #[test]
    fn failed_fetch_uses_an_old_cache() {
        let (url, _) = serve("401 Unauthorized", "{}", Duration::ZERO);
        let path = temp("eve-router-test-stale");
        let old = SourceData {
            source: SourceId::Nexum,
            fetched_at: FETCHED - 3600,
            origin: Some(format!("{url}/api/v1/maps/m1")),
            holes: Vec::new(),
        };
        crate::sources::write_cache(&path, &old).unwrap();
        let load = finish(start(&cfg(&url), path, FETCHED), |_| true, &types());
        assert_eq!(load.data, Some(old));
        assert_eq!(load.warning.as_deref(), Some("Nexum key rejected"));
    }

    #[test]
    fn cache_of_another_map_starts_a_fetch() {
        let body = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let (url, request) = serve("200 OK", &body, Duration::ZERO);
        let path = temp("eve-router-test-other-map");
        // A fresh cache, but for map m2.
        let other = SourceData {
            source: SourceId::Nexum,
            fetched_at: FETCHED - 60,
            origin: Some(format!("{url}/api/v1/maps/m2")),
            holes: Vec::new(),
        };
        crate::sources::write_cache(&path, &other).unwrap();
        let pending = start(&cfg(&url), path, FETCHED);
        assert!(matches!(pending, Pending::Fetch { .. }));
        let load = finish(pending, |id| id != 39_999_999, &types());
        assert!(request.recv().unwrap().starts_with("GET /api/v1/maps/m1 HTTP/1.1"));
        let data = load.data.unwrap();
        assert_eq!(data.holes.len(), 5);
        assert_eq!(data.origin, Some(format!("{url}/api/v1/maps/m1")));
    }

    #[test]
    fn failed_fetch_ignores_a_cache_of_another_map() {
        let (url, _) = serve("404 Not Found", "{}", Duration::ZERO);
        let path = temp("eve-router-test-other-map-fail");
        let other = SourceData {
            source: SourceId::Nexum,
            fetched_at: FETCHED - 3600,
            origin: Some(format!("{url}/api/v1/maps/m2")),
            holes: Vec::new(),
        };
        crate::sources::write_cache(&path, &other).unwrap();
        let load = finish(start(&cfg(&url), path, FETCHED), |_| true, &types());
        assert_eq!(load.data, None);
        assert_eq!(load.warning.as_deref(), Some("Nexum map not found"));
    }

    #[test]
    fn cache_without_origin_is_not_used() {
        let (url, request) = serve("404 Not Found", "{}", Duration::ZERO);
        let path = temp("eve-router-test-no-origin");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = format!(r#"{{"source":"Nexum","fetched_at":{},"holes":[]}}"#, FETCHED - 60);
        std::fs::write(&path, old).unwrap();
        assert!(crate::sources::read_cache(&path).is_some());
        let pending = start(&cfg(&url), path, FETCHED);
        assert!(matches!(pending, Pending::Fetch { cache: None, .. }));
        let load = finish(pending, |_| true, &types());
        assert!(request.recv().unwrap().starts_with("GET /api/v1/maps/m1"));
        assert_eq!(load.data, None);
    }

    /// The map, the fixture signatures, and an empty list for each other system except Dodixie.
    /// The Dodixie request gets a 500.
    fn sig_routes() -> HashMap<String, String> {
        let map = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let mut routes = HashMap::from([("/api/v1/maps/m1".to_string(), map)]);
        let text = std::fs::read_to_string(crate::test_support::fixture("nexum-sigs.json")).unwrap();
        let sigs: HashMap<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
        for id in ["s-amarr", "s-hek", "s-j134702", "s-jita", "s-perimeter", "s-rens", "s-thera", "s-unknown"] {
            let body = sigs.get(id).map_or("[]".to_string(), |v| v.to_string());
            routes.insert(format!("/api/v1/maps/m1/systems/{id}/signatures"), body);
        }
        routes
    }

    #[test]
    fn fetch_gets_the_signatures() {
        let (url, requests) = crate::test_support::serve_routes(sig_routes());
        let path = temp("eve-router-test-sigs");
        let load = finish(start(&cfg(&url), path.clone(), FETCHED), |id| id != 39_999_999, &types());
        // One map request, and one signature request for each of the 9 wormhole ends.
        // All are GET requests with the key.
        let requests: Vec<String> = requests.try_iter().collect();
        assert_eq!(requests.len(), 10);
        for r in &requests {
            assert!(r.starts_with("GET /api/v1/maps/m1"), "{r}");
            assert!(r.to_lowercase().contains("authorization: bearer nxm_test"), "{r}");
        }
        let data = load.data.unwrap();
        assert_eq!(sig_pair(find(&data, 30000142, 31002230)), (Some("BTA-111"), Some("JIT-202")));
        // The Dodixie request failed. The Dodixie wormhole stays, with no signature, and the status says so.
        assert_eq!(sig_pair(find(&data, 30002659, 31000005)), (None, None));
        assert_eq!(load.warning.as_deref(), Some("Nexum signatures: 1 system not loaded"));
        // The cache holds the signatures.
        assert_eq!(crate::sources::read_cache(&path), Some(data));
    }

    #[test]
    fn bad_json_counts_as_offline() {
        let (url, _) = serve("200 OK", "<html>", Duration::ZERO);
        let load = finish(start(&cfg(&url), temp("eve-router-test-badjson"), FETCHED), |_| true, &types());
        assert_eq!(load.data, None);
        assert_eq!(load.warning.as_deref(), Some("Nexum offline"));
    }
}
