//! The EVE-Scout source: the public Thera and Turnur signatures from
//! `GET https://api.eve-scout.com/v2/public/signatures`, converted to wormhole records.
//! The feed needs no key. The router fetches it at each startup, so the Thera and Turnur
//! switches work at once, with no restart.

use crate::sources::{self, FetchError};
use crate::wormhole::{Expiry, Size, SourceData, SourceId, Wormhole, parse_utc};
use crate::wormhole_types::WormholeTypes;
use serde::Deserialize;
use std::path::PathBuf;
use std::thread::JoinHandle;

pub const URL: &str = "https://api.eve-scout.com/v2/public/signatures";

/// One entry of the feed. The router reads only these fields.
#[derive(Deserialize)]
pub struct Signature {
    pub signature_type: String,
    /// False while a scout has not yet found the other end.
    #[serde(default)]
    pub completed: bool,
    /// The type of the end that is not a K162, for example "J377".
    pub wh_type: Option<String>,
    /// `small`, `medium`, `large`, `xlarge` or `capital`.
    pub max_ship_size: Option<String>,
    pub expires_at: Option<String>,
    /// Thera or Turnur.
    pub out_system_id: u32,
    pub out_signature: Option<String>,
    pub in_system_id: Option<u32>,
    pub in_signature: Option<String>,
}

pub fn parse(text: &str) -> Result<Vec<Signature>, String> {
    serde_json::from_str(text).map_err(|e| format!("EVE-Scout signatures: {e}"))
}

/// The entries that the converter did not use, by reason.
#[derive(Default, Debug, PartialEq)]
pub struct EveScoutReport {
    /// EVE system IDs that are not in the SDE.
    pub unknown: Vec<u32>,
    /// Entries that are not a wormhole, or that have no second end yet.
    pub incomplete: usize,
    /// Entries that expired before the fetch.
    pub expired: usize,
}

impl EveScoutReport {
    /// The count for the sidebar "Skipped" line.
    pub fn skipped(&self) -> usize {
        self.expired
    }
}

pub(crate) fn parse_size(size: &str) -> Option<Size> {
    match size {
        "small" => Some(Size::Small),
        "medium" => Some(Size::Medium),
        "large" => Some(Size::Large),
        "xlarge" => Some(Size::XLarge),
        "capital" => Some(Size::Capital),
        _ => None,
    }
}

/// Convert the feed. `known` says if an EVE system ID is in the SDE.
pub fn convert(sigs: &[Signature], known: impl Fn(u32) -> bool, types: &WormholeTypes, fetched_at: u64) -> (SourceData, EveScoutReport) {
    let mut report = EveScoutReport::default();
    let mut holes = Vec::new();
    for s in sigs {
        let Some(in_id) = s.in_system_id.filter(|_| s.signature_type == "wormhole" && s.completed) else {
            report.incomplete += 1;
            continue;
        };
        let unknown: Vec<u32> = [s.out_system_id, in_id].into_iter().filter(|&id| !known(id)).collect();
        if !unknown.is_empty() {
            report.unknown.extend(unknown);
            continue;
        }
        let expiry = s.expires_at.as_deref().and_then(parse_utc).map(|at| Expiry { at, exact: true });
        if expiry.is_some_and(|e| e.at <= fetched_at) {
            report.expired += 1;
            continue;
        }
        // An empty type text counts as no type. A known type gives the exact limit.
        // Else the scout's ship size gives the class.
        let wh_type = s.wh_type.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let (size, max_jump_kg) = match wh_type.and_then(|t| types.get(t)) {
            Some(t) => (Some(Size::from_jump_kg(t.max_jump_kg)), Some(t.max_jump_kg)),
            None => (s.max_ship_size.as_deref().and_then(parse_size), None),
        };
        holes.push(Wormhole {
            size,
            max_jump_kg,
            expiry,
            wh_type: wh_type.map(str::to_string),
            ..Wormhole::new(s.out_system_id, in_id, s.out_signature.clone(), s.in_signature.clone(), SourceId::EveScout)
        });
    }
    (SourceData { source: SourceId::EveScout, fetched_at, origin: None, holes }, report)
}

/// The wormhole data of the EVE-Scout source, for the startup and the sidebar.
#[derive(Default)]
pub struct Load {
    pub data: Option<SourceData>,
    pub report: EveScoutReport,
    /// A problem for the status line, for example "EVE-Scout offline, feed from 14:02".
    pub warning: Option<String>,
}

/// The EVE-Scout load, between the start and the end of the SDE load.
pub enum Pending {
    /// A cache that is less than 5 minutes old. The router sends no request.
    Cache(SourceData),
    /// A fetch on a thread. The cache is the fallback for a failure.
    Fetch { thread: JoinHandle<Result<String, FetchError>>, started: u64, origin: String, cache: Option<SourceData>, cache_path: PathBuf },
}

/// Start the EVE-Scout load: a fresh cache, else a fetch of `url` on a thread.
pub fn start(url: &str, cache_path: PathBuf, now: u64) -> Pending {
    let origin = url.to_string();
    let cache = sources::read_cache(&cache_path).filter(|d| d.source == SourceId::EveScout && d.origin.as_ref() == Some(&origin));
    if let Some(data) = cache.as_ref().filter(|d| sources::cache_is_fresh(d, now)) {
        return Pending::Cache(data.clone());
    }
    let thread_url = origin.clone();
    let thread = std::thread::spawn(move || sources::get(&thread_url, None, sources::TIMEOUT));
    Pending::Fetch { thread, started: now, origin, cache, cache_path }
}

/// Finish the EVE-Scout load after the SDE loads. A fetch problem gives the cache and a warning.
pub fn finish(pending: Pending, known: impl Fn(u32) -> bool, types: &WormholeTypes) -> Load {
    match pending {
        Pending::Cache(data) => Load { data: Some(data), ..Load::default() },
        Pending::Fetch { thread, started, origin, cache, cache_path } => {
            let fetched = thread.join().unwrap_or_else(|_| Err(FetchError::Offline("the fetch thread failed".into())));
            let mut report = EveScoutReport::default();
            let converted = fetched.and_then(|text| {
                let sigs = parse(&text).map_err(FetchError::Offline)?;
                let (mut data, r) = convert(&sigs, &known, types, started);
                data.origin = Some(origin);
                report = r;
                Ok(data)
            });
            let chosen = sources::choose(SourceId::EveScout, converted, cache, &cache_path);
            Load { data: chosen.data, report, warning: chosen.warning }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::serve;
    use crate::wormhole::{THERA, TURNUR};
    use crate::wormhole_types::WormholeType;
    use std::time::Duration;

    /// 2026-10-05T17:51:16Z, the time of the fixture.
    const FETCHED: u64 = 1_791_222_676;

    /// The J377 only. The other types in the fixture are not known, so they use the ship size.
    fn types() -> WormholeTypes {
        let j377 =
            WormholeType { code: "J377".into(), max_jump_kg: 62_000_000.0, max_life_h: 16.0, total_mass_kg: None, target_class: None };
        WormholeTypes { build: None, types: vec![j377] }
    }

    fn fixture_text() -> String {
        std::fs::read_to_string(crate::test_support::fixture("eve-scout.json")).unwrap()
    }

    fn fixture(known: impl Fn(u32) -> bool, at: u64) -> (SourceData, EveScoutReport) {
        convert(&parse(&fixture_text()).unwrap(), known, &types(), at)
    }

    fn find(data: &SourceData, a: u32, b: u32) -> &Wormhole {
        data.holes.iter().find(|h| (h.a, h.b) == (a.min(b), a.max(b))).unwrap()
    }

    #[test]
    fn converts_the_fixture() {
        let (data, report) = fixture(|_| true, FETCHED);
        assert_eq!(data.source, SourceId::EveScout);
        assert_eq!(data.holes.len(), 28);
        assert_eq!(report, EveScoutReport::default());
        assert_eq!(data.holes.iter().filter(|h| h.a == THERA || h.b == THERA).count(), 18);
        assert_eq!(data.holes.iter().filter(|h| h.a == TURNUR || h.b == TURNUR).count(), 10);
        // Turnur to J231245: a known type, so the exact limit.
        let j377 = find(&data, TURNUR, 31001232);
        assert_eq!((j377.size, j377.max_jump_kg), (Some(Size::Medium), Some(62_000_000.0)));
        assert_eq!(j377.wh_type.as_deref(), Some("J377"));
        assert_eq!(j377.expiry, Some(Expiry { at: parse_utc("2026-10-05T19:38:34Z").unwrap(), exact: true }));
        assert_eq!(j377.mass, None);
        assert_eq!(j377.sources, vec![SourceId::EveScout]);
        // Turnur has the lower ID, so its signature comes first.
        assert_eq!((j377.sig_a.as_deref(), j377.sig_b.as_deref()), (Some("IHS-280"), Some("YUR-252")));
        // Thera to Hulmate: Thera has the higher ID, so its signature comes second.
        let v898 = find(&data, THERA, 30003805);
        assert_eq!((v898.sig_a.as_deref(), v898.sig_b.as_deref()), (Some("SOB-950"), Some("NTM-070")));
        // An unknown type uses the ship size, with no exact limit.
        assert_eq!((v898.size, v898.max_jump_kg), (Some(Size::XLarge), None));
        assert_eq!(find(&data, TURNUR, 30045348).size, Some(Size::Capital));
    }

    #[test]
    fn expired_and_unknown_are_skipped() {
        // At 20:00 the two J377 holes that expire at 19:38 are gone.
        let (data, report) = fixture(|_| true, parse_utc("2026-10-05T20:00:00Z").unwrap());
        assert_eq!((data.holes.len(), report.expired, report.skipped()), (26, 2, 2));
        let (data, report) = fixture(|id| id != 31001232, FETCHED);
        assert_eq!((data.holes.len(), report.unknown), (27, vec![31001232]));
    }

    #[test]
    fn incomplete_entries_are_skipped() {
        let text = r#"[
            {"signature_type": "wormhole", "completed": false, "out_system_id": 31000005, "in_system_id": null},
            {"signature_type": "combat", "completed": true, "out_system_id": 31000005, "in_system_id": 30000142}
        ]"#;
        let (data, report) = convert(&parse(text).unwrap(), |_| true, &types(), FETCHED);
        assert!(data.holes.is_empty());
        assert_eq!(report.incomplete, 2);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("eve-scout.json")
    }

    #[test]
    fn fetch_sends_no_key_and_writes_the_cache() {
        let (url, request) = serve("200 OK", &fixture_text(), Duration::ZERO);
        let path = temp("eve-router-test-scout-fetch");
        let load = finish(start(&url, path.clone(), FETCHED), |_| true, &types());
        let request = request.recv().unwrap();
        assert!(request.starts_with("GET / HTTP/1.1"), "{request}");
        assert!(!request.to_lowercase().contains("authorization"), "{request}");
        let data = load.data.unwrap();
        assert_eq!((data.holes.len(), data.fetched_at), (28, FETCHED));
        assert_eq!(data.origin.as_deref(), Some(url.as_str()));
        assert_eq!(load.warning, None);
        assert_eq!(sources::read_cache(&path), Some(data));
    }

    #[test]
    fn fresh_cache_stops_the_fetch() {
        let path = temp("eve-router-test-scout-fresh");
        // Port 9 has no server, so a fetch would fail.
        let url = "http://127.0.0.1:9";
        let cached = SourceData { source: SourceId::EveScout, fetched_at: FETCHED - 60, origin: Some(url.into()), holes: Vec::new() };
        sources::write_cache(&path, &cached).unwrap();
        assert!(matches!(start(url, path, FETCHED), Pending::Cache(ref d) if *d == cached));
    }

    #[test]
    fn failed_fetch_uses_an_old_cache() {
        let (url, _) = serve("503 Service Unavailable", "{}", Duration::ZERO);
        let path = temp("eve-router-test-scout-stale");
        let old = SourceData { source: SourceId::EveScout, fetched_at: FETCHED - 3600, origin: Some(url.clone()), holes: Vec::new() };
        sources::write_cache(&path, &old).unwrap();
        let load = finish(start(&url, path, FETCHED), |_| true, &types());
        assert_eq!(load.data, Some(old));
        let warning = load.warning.unwrap();
        assert!(warning.starts_with("EVE-Scout offline, feed from "), "{warning}");
    }

    #[test]
    fn bad_json_counts_as_offline() {
        let (url, _) = serve("200 OK", "<html>", Duration::ZERO);
        let load = finish(start(&url, temp("eve-router-test-scout-bad"), FETCHED), |_| true, &types());
        assert_eq!(load.data, None);
        assert_eq!(load.warning.as_deref(), Some("EVE-Scout offline"));
    }
}
