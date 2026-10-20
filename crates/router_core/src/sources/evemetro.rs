//! The EVE Metro source: the Pochven wormhole map from `POST https://api.evemetro.com/connections`,
//! converted to wormhole records. The service needs an API key, sent in the `x-api-key` header.
//! The body names a character, its corporation and its alliance. The service gives the map when
//! any of the three has access.
//!
//! The format comes from the service code (`calli-eve/eve-metro-api`, `src/handlers/connections.js`).
//! The router has not yet read a reply from the live service, so the parser accepts a number or a
//! text for each system ID and ignores each field that it does not know.

use crate::config::EveMetroConfig;
use crate::esi::client::Affiliation;
use crate::log::{self, LogEntry, LogEntryKind, LogOp};
use crate::sources::nexum::earlier;
use crate::sources::{self, FetchError, MassName, OverlayWarning};
use crate::wormhole::{Expiry, HOUR, Size, SourceData, SourceId, Wormhole, parse_utc};
use crate::wormhole_types::{WhCode, WormholeTypes};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

/// The fixed address of the service.
pub const URL: &str = "https://api.evemetro.com";

/// The env var for the API key. The build reads it, and the same variable at run time overrides it.
pub const KEY_VAR: &str = "EVE_ROUTER_METRO_KEY";

/// The key of the run time, else the key of the build. An empty value counts as no value.
fn pick_key(runtime: Option<String>, built: Option<&str>) -> Option<String> {
    let clean = |key: &str| Some(key.trim().to_owned()).filter(|key| !key.is_empty());
    runtime.as_deref().and_then(clean).or_else(|| built.and_then(clean))
}

/// The API key of the service. The repository and the config file hold no key. Without one, the
/// source stays off and the settings page hides it.
pub fn api_key() -> Option<String> {
    pick_key(std::env::var(KEY_VAR).ok(), option_env!("EVE_ROUTER_METRO_KEY"))
}

/// The text for the pilot when the app has no EVE client ID. The source needs a logged-in
/// character, like the other SSO features.
pub const NEEDS_CLIENT_ID: &str = "EVE Metro needs the EVE client ID. Set EVE_ROUTER_CLIENT_ID.";

/// A system ID. The service can give a number or a text, so the parser accepts both.
/// Any other value gives `None`.
#[derive(Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(from = "RawId")]
struct Id(Option<u32>);

#[derive(Deserialize)]
#[serde(untagged)]
enum RawId {
    Number(u64),
    Text(String),
    Other(serde::de::IgnoredAny),
}

impl From<RawId> for Id {
    fn from(raw: RawId) -> Id {
        let value = match raw {
            RawId::Number(n) => Some(n),
            RawId::Text(t) => t.trim().parse().ok(),
            RawId::Other(_) => None,
        };
        Id(value.and_then(|n| u32::try_from(n).ok()))
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MetroReply {
    /// False when no character, corporation or alliance of the request has access.
    #[serde(default)]
    access: bool,
    #[serde(default)]
    connections: Vec<MetroNode>,
}

/// A Pochven system with its wormholes.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MetroNode {
    #[serde(default)]
    system_id: Id,
    #[serde(default)]
    system_edges: Vec<MetroEdge>,
}

/// A wormhole from a Pochven system to a system outside Pochven.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MetroEdge {
    #[serde(default)]
    solar_system_id_dst: Id,
    /// The signature in the Pochven system.
    signature_src: Option<String>,
    signature_dst: Option<String>,
    wormhole_type_src: Option<String>,
    wormhole_type_dst: Option<String>,
    wormhole_mass: Option<MassName>,
    /// `critical` (less than one hour left) or `stable`.
    wormhole_eol: Option<String>,
    created_time: Option<String>,
}

pub fn parse(text: &str) -> Result<MetroReply, serde_json::Error> {
    serde_json::from_str(text)
}

/// The request body.
#[derive(Serialize)]
struct Query {
    character_id: u64,
    corporation_id: u64,
    /// A character in a corporation with no alliance gives no value.
    #[serde(skip_serializing_if = "Option::is_none")]
    alliance_id: Option<u64>,
}

/// The entries that the converter did not use, by reason.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct MetroReport {
    /// EVE system IDs that are not in the SDE.
    pub unknown: Vec<u32>,
    /// Wormholes with no system ID at one end.
    pub incomplete: usize,
    /// Wormholes that expired before the fetch.
    pub expired: usize,
    /// The service said that the character has no access.
    pub no_access: bool,
}

impl MetroReport {
    /// The count for the sidebar "Skipped" line.
    pub fn skipped(&self) -> usize {
        self.expired + self.incomplete
    }
}

/// The type of the end that is not a K162, for the life rule and the size. Two K162 ends, neither
/// identified yet, give `None`.
fn pick_type(src: &Option<String>, dst: &Option<String>) -> Option<WhCode> {
    let clean = |t: &Option<String>| t.as_deref().and_then(WhCode::parse);
    let real = |t: &WhCode| !t.is_k162();
    clean(src).filter(real).or(clean(dst).filter(real))
}

/// The text of a signature, or `None` for an empty text.
fn clean_sig(sig: &Option<String>) -> Option<String> {
    sig.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Convert the connections. `known` says if an EVE system ID is in the SDE.
pub fn convert(reply: &MetroReply, known: impl Fn(u32) -> bool, types: &WormholeTypes, fetched_at: u64) -> (SourceData, MetroReport) {
    let mut report = MetroReport { no_access: !reply.access, ..MetroReport::default() };
    let mut holes = Vec::new();
    for node in &reply.connections {
        for e in &node.system_edges {
            let (Some(a), Some(b)) = (node.system_id.0, e.solar_system_id_dst.0) else {
                report.incomplete += 1;
                continue;
            };
            let unknown: Vec<u32> = [a, b].into_iter().filter(|&id| !known(id)).collect();
            if !unknown.is_empty() {
                for id in unknown {
                    if !report.unknown.contains(&id) {
                        report.unknown.push(id);
                    }
                }
                continue;
            }
            let wh_type = pick_type(&e.wormhole_type_src, &e.wormhole_type_dst);
            // The creation time plus the life of the type, and the "critical" time mark: less than
            // one hour is left. The earlier limit wins.
            let created = e.created_time.as_deref().and_then(parse_utc);
            let by_life = wh_type
                .as_ref()
                .and_then(|t| types.life_hours(t, [a, b]))
                .zip(created)
                .map(|(hours, created)| Expiry { at: created.saturating_add((hours * 3600.0) as u64) });
            let by_mark = (e.wormhole_eol.as_deref() == Some("critical")).then(|| Expiry { at: fetched_at.saturating_add(HOUR) });
            let expiry = earlier(by_life, by_mark);
            if expiry.is_some_and(|x| x.at <= fetched_at) {
                report.expired += 1;
                continue;
            }
            let (size, max_jump_kg) = match wh_type.as_ref().and_then(|t| types.get(t)) {
                Some(t) => (Some(Size::from_jump_kg(t.max_jump_kg)), Some(t.max_jump_kg)),
                None => (None, None),
            };
            holes.push(Wormhole {
                size,
                max_jump_kg,
                mass: e.wormhole_mass.and_then(MassName::status),
                expiry,
                wh_type,
                ..Wormhole::new(a, b, clean_sig(&e.signature_src), clean_sig(&e.signature_dst), SourceId::EveMetro)
            });
        }
    }
    (SourceData { source: SourceId::EveMetro, fetched_at, origin: None, name: None, holes }, report)
}

/// The wormhole data of the EVE Metro source, for the startup and the sidebar.
#[derive(Default)]
pub struct Load {
    pub data: Option<SourceData>,
    pub report: MetroReport,
    /// The problems for the status line, for example "EVE Metro offline, map from 14:02".
    pub warnings: Vec<OverlayWarning>,
    /// The rows for the log: one for the fetch.
    pub log: Vec<LogEntry>,
}

/// The EVE Metro load, between the start and the end of the SDE load.
pub enum Pending {
    /// The source is off, or the key or the character is not set.
    None,
    /// A cache that is less than 5 minutes old. The router sends no request.
    Cache(SourceData),
    /// The settings are complete, but ESI has not given the affiliation of the character yet. The
    /// router sends no request and uses the cache, of any age. The affiliation starts a fetch.
    Waiting(Option<SourceData>),
    /// A fetch on a thread. The cache is the fallback for a failure.
    Fetch { thread: JoinHandle<Result<String, FetchError>>, started: u64, origin: String, cache: Option<SourceData>, cache_path: PathBuf },
}

/// The origin of the data at `base`. A cache is for one character only: the access depends on it.
fn origin_at(base: &str, character: u64) -> String {
    format!("{base}/connections#{character}")
}

/// The origin of the data of `cfg`. `None` when the source is off, or the key or the character is not set.
pub fn origin(cfg: &EveMetroConfig) -> Option<String> {
    cfg.complete().map(|(_, character)| origin_at(URL, character))
}

/// POST the query. `who` is the affiliation that ESI gave for the character. The service grants
/// access by these IDs, so the caller never gets them from a file.
fn fetch(base: &str, key: &str, character: u64, who: &Affiliation, timeout: Duration) -> Result<String, FetchError> {
    let query = Query { character_id: character, corporation_id: who.corporation_id, alliance_id: who.alliance_id };
    let body = serde_json::to_string(&query).map_err(|e| FetchError::Offline(e.to_string()))?;
    sources::post_json(&format!("{base}/connections"), key, &body, timeout)
}

/// Start the EVE Metro load: a fresh cache, else a fetch on a thread. `base` is `URL`. A test
/// gives the address of its own server.
pub fn start(cfg: &EveMetroConfig, base: &str, cache_path: PathBuf, now: u64) -> Pending {
    start_with(cfg, base, cache_path, now, true)
}

/// Start a fetch, also when the cache is fresh. A settings change needs this.
pub fn start_fetch(cfg: &EveMetroConfig, base: &str, cache_path: PathBuf, now: u64) -> Pending {
    start_with(cfg, base, cache_path, now, false)
}

/// Start the EVE Metro load. `use_fresh` false skips the fresh-cache check.
fn start_with(cfg: &EveMetroConfig, base: &str, cache_path: PathBuf, now: u64, use_fresh: bool) -> Pending {
    let Some((key, character)) = cfg.complete() else {
        return Pending::None;
    };
    let origin = origin_at(base, character);
    let cache = sources::read_cache(&cache_path).filter(|d| d.source == SourceId::EveMetro && d.origin.as_ref() == Some(&origin));
    if use_fresh && let Some(data) = cache.as_ref().filter(|d| sources::cache_is_fresh(d, now)) {
        return Pending::Cache(data.clone());
    }
    // No verified affiliation, no request. A pilot could otherwise name any corporation or alliance.
    let Some((_, _, who)) = cfg.request() else {
        return Pending::Waiting(cache);
    };
    let (base, key, who) = (base.to_string(), key.to_string(), who.clone());
    let thread = sources::spawn_fetch(SourceId::EveMetro, move || fetch(&base, &key, character, &who, sources::TIMEOUT));
    Pending::Fetch { thread, started: now, origin, cache, cache_path }
}

/// Finish the EVE Metro load after the SDE loads. A fetch problem gives the cache and a warning.
pub fn finish(pending: Pending, known: impl Fn(u32) -> bool, types: &WormholeTypes) -> Load {
    match pending {
        Pending::None => Load::default(),
        Pending::Cache(data) => {
            sources::log_cache_fresh(&data);
            let log = vec![LogEntry {
                time: crate::wormhole::now(),
                op: LogOp::EveMetroFetch,
                kind: LogEntryKind::CacheFresh,
                message: "cache is fresh".into(),
            }];
            Load { data: Some(data), log, ..Load::default() }
        }
        Pending::Waiting(cache) => {
            tracing::info!(cache_in_use = cache.is_some(), "EVE Metro waits for the affiliation of the character, so no request goes out");
            Load { data: cache, ..Load::default() }
        }
        Pending::Fetch { thread, started, origin, cache, cache_path } => {
            let fetched = thread.join().unwrap_or_else(|_| Err(FetchError::Offline("the fetch thread failed".into())));
            let mut report = MetroReport::default();
            let converted = fetched.and_then(|text| {
                let reply = parse(&text).map_err(|e| FetchError::Offline(format!("EVE Metro reply: {e}")))?;
                let (mut data, r) = convert(&reply, &known, types, started);
                data.origin = Some(origin);
                report = r;
                Ok(data)
            });
            // A reply that denies access is a successful fetch with no wormholes. It must not
            // replace a good cache with nothing, so it is treated like a fetch failure for the
            // cache: the cache stays as it was, with its own warning and log row.
            if report.no_access {
                tracing::warn!(cache_in_use = cache.is_some(), "EVE Metro reports no access for the linked character");
                let warnings = vec![OverlayWarning::MetroNoAccess];
                let log =
                    vec![LogEntry { time: started, op: LogOp::EveMetroFetch, kind: LogEntryKind::NoAccess, message: "no access".into() }];
                return Load { data: cache, report, warnings, log };
            }
            let chosen = sources::choose(SourceId::EveMetro, converted, cache, &cache_path);
            let (kind, message) = match &chosen.error {
                Some(error) => (error.kind(), error.message(SourceId::EveMetro)),
                None => (LogEntryKind::Loaded, log::loaded(chosen.data.as_ref().map_or(0, |d| d.holes.len()))),
            };
            let log = vec![LogEntry { time: started, op: LogOp::EveMetroFetch, kind, message }];
            Load { data: chosen.data, report, warnings: chosen.warning.into_iter().collect(), log }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiKey;
    use crate::test_support::serve;
    use crate::wormhole::MassStatus;
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

    fn fixture_text() -> String {
        std::fs::read_to_string(crate::test_support::fixture("evemetro-connections.json")).unwrap()
    }

    /// 39999999 is not a known system.
    fn fixture() -> (SourceData, MetroReport) {
        convert(&parse(&fixture_text()).unwrap(), |id| id != 39_999_999, &types(), FETCHED)
    }

    fn cfg() -> EveMetroConfig {
        EveMetroConfig { enabled: true, key: Some(ApiKey("metro_test_key".into())), character: Some(42), affiliation: Some(who()) }
    }

    fn who() -> Affiliation {
        Affiliation { corporation_id: 7, alliance_id: Some(9) }
    }

    #[test]
    fn convert_the_fixture() {
        let (data, report) = fixture();
        assert_eq!(data.source, SourceId::EveMetro);
        assert_eq!(data.holes.len(), 2, "{:?}", data.holes);
        assert_eq!(report, MetroReport { unknown: vec![39_999_999], incomplete: 1, expired: 1, no_access: false });
        assert_eq!(report.skipped(), 2);

        // A text ID and a stable wormhole. The K162 end gives way to the type F216.
        let jita = data.holes.iter().find(|h| h.a == 30_000_142).expect("no Jita wormhole");
        assert_eq!((jita.b, jita.wh_type.as_ref().map(WhCode::as_str)), (30_001_001, Some("F216")));
        assert_eq!((jita.mass, jita.expiry), (Some(MassStatus::Stable), None));
        assert_eq!(jita.sig_at(30_001_001).as_deref(), Some("ABC"));
        assert_eq!(jita.sig_at(30_000_142).as_deref(), Some("XYZ"));
        assert_eq!(jita.sources, vec![SourceId::EveMetro]);

        // A critical wormhole: less than one hour is left, and a critical mass.
        let amarr = data.holes.iter().find(|h| h.a == 30_001_001 && h.b == 30_002_187).expect("no Amarr wormhole");
        assert_eq!((amarr.mass, amarr.wh_type.as_ref().map(WhCode::as_str)), (Some(MassStatus::Critical), Some("C729")));
        assert_eq!(amarr.expiry, Some(Expiry { at: FETCHED + HOUR }));
        assert_eq!(amarr.sig_at(30_002_187), None);
        assert!(!amarr.usable(FETCHED, None));
    }

    #[test]
    fn a_known_type_gives_size_and_life() {
        let reply = parse(
            r#"{"access":true,"connections":[{"systemId":1,"systemEdges":[{"solarSystemIdDst":2,"wormholeTypeSrc":"K162","wormholeTypeDst":"B274","wormholeEol":"stable","createdTime":"2026-10-05T11:00:00Z"}]}]}"#,
        )
        .unwrap();
        let (data, _) = convert(&reply, |_| true, &types(), FETCHED);
        let hole = &data.holes[0];
        assert_eq!((hole.size, hole.max_jump_kg), (Some(Size::Large), Some(375_000_000.0)));
        // Created at 11:00, and the life of a B274 is 24 h.
        assert_eq!(hole.expiry, Some(Expiry { at: FETCHED - HOUR + 24 * HOUR }));
    }

    #[test]
    fn destabilized_mass_is_not_dropped() {
        let reply =
            parse(r#"{"access":true,"connections":[{"systemId":1,"systemEdges":[{"solarSystemIdDst":2,"wormholeMass":"destabilized"}]}]}"#)
                .unwrap();
        let (data, _) = convert(&reply, |_| true, &types(), FETCHED);
        assert_eq!(data.holes[0].mass, Some(MassStatus::Destabilized));
    }

    #[test]
    fn two_k162_ends_give_no_type() {
        // Neither end has been scanned to a real type yet.
        let reply = parse(
            r#"{"access":true,"connections":[{"systemId":1,"systemEdges":[{"solarSystemIdDst":2,"wormholeTypeSrc":"K162","wormholeTypeDst":"K162"}]}]}"#,
        )
        .unwrap();
        let (data, _) = convert(&reply, |_| true, &types(), FETCHED);
        assert_eq!(data.holes[0].wh_type, None);
        // One end is K162, the other is not yet known at all: still no real type.
        let reply =
            parse(r#"{"access":true,"connections":[{"systemId":1,"systemEdges":[{"solarSystemIdDst":2,"wormholeTypeSrc":"K162"}]}]}"#)
                .unwrap();
        let (data, _) = convert(&reply, |_| true, &types(), FETCHED);
        assert_eq!(data.holes[0].wh_type, None);
    }

    #[test]
    fn parse_is_tolerant() {
        // No fields at all, a wrong type for the ID, and a field that the router does not know.
        let reply = parse(r#"{"connections":[{"systemId":{"x":1},"systemEdges":[{"solarSystemIdDst":3}],"extra":true}]}"#).unwrap();
        let (data, report) = convert(&reply, |_| true, &types(), FETCHED);
        assert!(data.holes.is_empty());
        assert_eq!((report.incomplete, report.no_access), (1, true));
        assert!(parse("not json").is_err());
    }

    #[test]
    fn the_run_time_key_wins_over_the_build_key() {
        assert_eq!(pick_key(Some(" run ".into()), Some("built")).as_deref(), Some("run"));
        assert_eq!(pick_key(None, Some("built")).as_deref(), Some("built"));
        assert_eq!(pick_key(Some("  ".into()), Some("built")).as_deref(), Some("built"));
        assert_eq!(pick_key(Some("  ".into()), None), None);
        assert_eq!(pick_key(None, None), None);
        assert_eq!(KEY_VAR, "EVE_ROUTER_METRO_KEY");
    }

    #[test]
    fn the_client_id_text_names_the_env_var() {
        assert!(NEEDS_CLIENT_ID.contains(crate::esi::CLIENT_ID_VAR));
    }

    #[test]
    fn the_settings_must_be_complete() {
        assert_eq!(cfg().complete(), Some(("metro_test_key", 42)));
        assert_eq!(EveMetroConfig { enabled: false, ..cfg() }.complete(), None);
        assert_eq!(EveMetroConfig { key: None, ..cfg() }.complete(), None);
        assert_eq!(EveMetroConfig { character: None, ..cfg() }.complete(), None);
        assert!(matches!(start(&EveMetroConfig::default(), URL, PathBuf::from("none.json"), 0), Pending::None));
    }

    #[test]
    fn a_request_needs_the_affiliation() {
        assert_eq!(cfg().request(), Some(("metro_test_key", 42, &who())));
        assert_eq!(EveMetroConfig { affiliation: None, ..cfg() }.request(), None);
        assert_eq!(EveMetroConfig { enabled: false, ..cfg() }.request(), None);
    }

    #[test]
    fn fetch_sends_the_key_and_all_three_ids() {
        let (metro, request) = serve("200 OK", &fixture_text(), Duration::ZERO);
        let text = fetch(&metro, "metro_test_key", 42, &who(), sources::TIMEOUT).unwrap();
        assert!(text.contains("Kino"));
        let request = request.recv().unwrap();
        let lower = request.to_lowercase();
        assert!(request.starts_with("POST /connections HTTP/1.1\r\n"), "{request}");
        assert!(lower.contains("x-api-key: metro_test_key"), "{request}");
        assert!(lower.contains("content-type: application/json"), "{request}");
        let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, serde_json::json!({"character_id": 42, "corporation_id": 7, "alliance_id": 9}));
    }

    #[test]
    fn a_character_with_no_alliance_sends_no_alliance() {
        let (metro, request) = serve("200 OK", "{}", Duration::ZERO);
        fetch(&metro, "k", 42, &Affiliation { corporation_id: 7, alliance_id: None }, sources::TIMEOUT).unwrap();
        let request = request.recv().unwrap();
        let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, serde_json::json!({"character_id": 42, "corporation_id": 7}));
    }

    #[test]
    fn a_refused_key_and_a_rate_limit() {
        let (metro, _) = serve("403 Forbidden", "{}", Duration::ZERO);
        assert_eq!(fetch(&metro, "bad", 42, &who(), sources::TIMEOUT), Err(FetchError::Auth));
        let (metro, _) = serve("429 Too Many Requests", "{}", Duration::ZERO);
        assert_eq!(fetch(&metro, "k", 42, &who(), sources::TIMEOUT), Err(FetchError::RateLimit));
    }

    fn load(cfg: &EveMetroConfig, metro: &str, cache_path: PathBuf) -> Load {
        let pending = start_with(cfg, metro, cache_path, FETCHED, true);
        finish(pending, |id| id != 39_999_999, &types())
    }

    #[test]
    fn a_fetch_loads_and_writes_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eve-metro.json");
        let (metro, _) = serve("200 OK", &fixture_text(), Duration::ZERO);
        let result = load(&cfg(), &metro, path.clone());
        assert!(result.warnings.is_empty());
        assert_eq!(result.data.as_ref().map(|d| d.holes.len()), Some(2));
        assert_eq!(result.log[0].op, LogOp::EveMetroFetch);
        assert!(result.log[0].kind.is_ok());
        let cached = sources::read_cache(&path).expect("no cache file");
        assert_eq!(cached.origin, Some(origin_at(&metro, 42)));
        assert_eq!(origin(&cfg()), Some(format!("{URL}/connections#42")));
        assert_eq!(origin(&EveMetroConfig::default()), None);
        // A fresh cache for the same character stops the fetch. Another character does not use it.
        assert!(matches!(start_with(&cfg(), &metro, path.clone(), FETCHED, true), Pending::Cache(_)));
        let other = EveMetroConfig { character: Some(43), ..cfg() };
        assert!(matches!(start_with(&other, &metro, path, FETCHED, true), Pending::Fetch { cache: None, .. }));
    }

    #[test]
    fn no_affiliation_means_no_request_and_the_cache_of_any_age() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eve-metro.json");
        let (metro, _) = serve("200 OK", &fixture_text(), Duration::ZERO);
        load(&cfg(), &metro, path.clone());
        // Old cache, and a server that must not see a request. The origin names that server.
        let (silent, requests) = serve("200 OK", "{}", Duration::ZERO);
        let old =
            SourceData { fetched_at: FETCHED - 2 * HOUR, origin: Some(origin_at(&silent, 42)), ..sources::read_cache(&path).unwrap() };
        sources::write_cache(&path, &old).unwrap();
        let unverified = EveMetroConfig { affiliation: None, ..cfg() };
        for use_fresh in [true, false] {
            let pending = start_with(&unverified, &silent, path.clone(), FETCHED, use_fresh);
            assert!(matches!(pending, Pending::Waiting(Some(_))));
            let result = finish(pending, |_| true, &types());
            assert_eq!(result.data.as_ref().map(|d| d.holes.len()), Some(2));
            assert!(result.warnings.is_empty());
        }
        assert!(requests.recv_timeout(Duration::from_millis(300)).is_err(), "a request went out with no verified affiliation");
        // No cache and no affiliation: nothing to show.
        let none = start_with(&unverified, &silent, tmp.path().join("other.json"), FETCHED, true);
        assert_eq!(finish(none, |_| true, &types()).data, None);
    }

    #[test]
    fn no_access_gives_a_warning_and_no_wormholes() {
        let tmp = tempfile::tempdir().unwrap();
        let (metro, _) = serve("200 OK", r#"{"access":false}"#, Duration::ZERO);
        // No cache exists yet, so there is nothing to keep.
        let result = load(&cfg(), &metro, tmp.path().join("eve-metro.json"));
        assert_eq!(result.warnings, [OverlayWarning::MetroNoAccess]);
        assert_eq!(result.data, None);
        assert!(result.report.no_access);
        assert_eq!(result.log[0].kind, LogEntryKind::NoAccess);
    }

    /// A character can lose access for a moment (a late payment, a corp move). The empty reply
    /// must not overwrite a good cache with nothing, unlike a genuine fetch failure, which already
    /// falls back to the cache through `sources::choose`.
    #[test]
    fn no_access_keeps_a_good_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eve-metro.json");
        let (metro, _) = serve("200 OK", &fixture_text(), Duration::ZERO);
        load(&cfg(), &metro, path.clone());
        let before = sources::read_cache(&path).expect("no cache file");
        assert_eq!(before.holes.len(), 2);

        let (metro, _) = serve("200 OK", r#"{"access":false}"#, Duration::ZERO);
        // The cache must be old, else `load` takes it with no fetch, and the test proves nothing.
        let old = SourceData { fetched_at: FETCHED - 2 * HOUR, origin: Some(origin_at(&metro, 42)), ..before.clone() };
        sources::write_cache(&path, &old).unwrap();
        let result = load(&cfg(), &metro, path.clone());
        assert_eq!(result.data.as_ref().map(|d| d.holes.len()), Some(2), "the cached map stays");
        assert_eq!(result.warnings, [OverlayWarning::MetroNoAccess]);
        // The cache file on disk is untouched too, not just the in-memory result.
        assert_eq!(sources::read_cache(&path).as_ref().map(|d| d.holes.len()), Some(2));
    }

    #[test]
    fn a_failure_uses_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eve-metro.json");
        let (metro, _) = serve("200 OK", &fixture_text(), Duration::ZERO);
        load(&cfg(), &metro, path.clone());
        // The cache is old now, and the server refuses the key.
        let (metro, _) = serve("403 Forbidden", "{}", Duration::ZERO);
        let old = SourceData { fetched_at: FETCHED - 2 * HOUR, origin: Some(origin_at(&metro, 42)), ..sources::read_cache(&path).unwrap() };
        sources::write_cache(&path, &old).unwrap();
        let result = load(&cfg(), &metro, path);
        assert_eq!(result.data.as_ref().map(|d| d.holes.len()), Some(2));
        assert_eq!(sources::warning_text(&result.warnings).as_deref(), Some("EVE Metro key rejected"));
        assert_eq!(result.log[0].kind, LogEntryKind::Auth);
        assert_eq!(result.log[0].message, "bad API key");
    }
}
