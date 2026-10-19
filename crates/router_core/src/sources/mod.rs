//! Wormhole sources. Each source has one file, and converts its data to `Wormhole` records.
//! This file holds the HTTP GET, the fetch errors and the disk cache.

pub mod evemetro;
pub mod evescout;
pub mod nexum;

use crate::error::{FileError, write_atomic};
use crate::wormhole::{MassStatus, SourceData, SourceId, local_time};
use serde::Deserialize;
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// The kind of a signature in the Nexum and EVE-Scout data. A new kind is `Other`.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SigType {
    Wormhole,
    #[serde(other)]
    Other,
}

/// The mass status in the Nexum and EVE Metro data. A new name is `Unknown`.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MassName {
    Stable,
    Destabilized,
    Critical,
    #[serde(other)]
    Unknown,
}

impl MassName {
    pub fn status(self) -> Option<MassStatus> {
        match self {
            MassName::Stable => Some(MassStatus::Stable),
            MassName::Destabilized => Some(MassStatus::Destabilized),
            MassName::Critical => Some(MassStatus::Critical),
            MassName::Unknown => None,
        }
    }
}

/// The timeout of one request.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// A cache that is younger than this stops the fetch at startup.
pub const CACHE_FRESH_SECS: u64 = 300;
/// The largest response body. A large alliance map is a few MB.
const MAX_BODY: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// 401 or 403: the key is wrong, or it has no access to the map.
    #[error("the server refused the request")]
    Auth,
    /// 404: the map ID is wrong.
    #[error("not found")]
    NotFound,
    /// A timeout, a network error, a 5xx status or bad JSON.
    #[error("{0}")]
    Offline(String),
}

impl FetchError {
    /// The cause, for the log. The status line has a longer text.
    pub fn reason(&self, source: SourceId) -> String {
        match (self, source) {
            (FetchError::Auth, SourceId::EveScout) => "request refused".into(),
            (FetchError::Auth, SourceId::Nexum) => "bad API key or no access to the map".into(),
            (FetchError::Auth, SourceId::EveMetro) => "bad API key".into(),
            (FetchError::NotFound, _) => "map not found".into(),
            (FetchError::Offline(cause), _) => cause.clone(),
        }
    }

    /// The status line text. `cached_at` is the fetch time of the cache in use.
    pub fn status(&self, source: SourceId, cached_at: Option<u64>) -> String {
        let name = source.label();
        // Nexum and EVE Metro give a map. EVE-Scout gives a public feed, with no key.
        let data = match source {
            SourceId::Nexum | SourceId::EveMetro => "map",
            SourceId::EveScout => "feed",
        };
        match (self, cached_at) {
            (FetchError::Auth, _) if source == SourceId::EveScout => format!("{name} refused the request"),
            (FetchError::Auth, _) => format!("{name} key rejected"),
            (FetchError::NotFound, _) => format!("{name} {data} not found"),
            (FetchError::Offline(_), Some(t)) => format!("{name} offline, {data} from {}", local_time(t)),
            (FetchError::Offline(_), None) => format!("{name} offline"),
        }
    }
}

/// An HTTP agent with a timeout for each request. One agent keeps its connections open,
/// so a batch of requests to one server uses one agent.
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("eve-router/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// Send one GET request with a new agent. See `get_with`.
pub fn get(url: &str, key: Option<&str>, timeout: Duration) -> Result<String, FetchError> {
    get_with(&agent(timeout), url, key)
}

/// Send one GET request, with a bearer key if `key` is set, and return the body.
/// A source gets its data with GET. The one exception is `post_json`.
pub fn get_with(agent: &ureq::Agent, url: &str, key: Option<&str>) -> Result<String, FetchError> {
    let mut request = agent.get(url);
    if let Some(key) = key {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    read_body(request.call())
}

/// Send one POST request with a JSON body and the key in the `x-api-key` header, and return the
/// body. Only EVE Metro uses it. Its query is a POST that reads data and changes nothing.
pub fn post_json(url: &str, key: &str, body: &str, timeout: Duration) -> Result<String, FetchError> {
    let request = agent(timeout).post(url).header("x-api-key", key).header("Content-Type", "application/json");
    read_body(request.send(body))
}

/// The body of a response, or the fetch error of its status.
fn read_body(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<String, FetchError> {
    match result {
        Ok(mut resp) => resp.body_mut().with_config().limit(MAX_BODY).read_to_string().map_err(|e| FetchError::Offline(e.to_string())),
        Err(ureq::Error::StatusCode(401 | 403)) => Err(FetchError::Auth),
        Err(ureq::Error::StatusCode(404)) => Err(FetchError::NotFound),
        Err(e) => Err(FetchError::Offline(e.to_string())),
    }
}

/// Read a cache file. A missing, corrupt or old-format file gives `None`.
pub fn read_cache(path: &Path) -> Option<SourceData> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Write a cache file: a temporary file, then a rename, so a reader never sees half a file.
pub fn write_cache(path: &Path, data: &SourceData) -> Result<(), FileError> {
    let text = serde_json::to_string(data).map_err(FileError::json(path))?;
    write_atomic(path, &path.with_extension("json.part"), &text)
}

/// True if the cache is less than 5 minutes old. A fetch time after `now` is not fresh.
pub fn cache_is_fresh(data: &SourceData, now: u64) -> bool {
    data.fetched_at <= now && now - data.fetched_at < CACHE_FRESH_SECS
}

/// A problem with the overlay data, for the status line and the notices. Two equal values are the
/// same problem, so a notice shows one time for each problem.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayWarning {
    /// The Ansiblex file names systems that the SDE does not know.
    BridgeUnknownSystems(Vec<String>),
    /// The Ansiblex file has a bridge with the same system at both ends.
    BridgeSameSystem(Vec<String>),
    /// A source names EVE system IDs that the SDE does not know.
    UnknownIds { source: SourceId, ids: Vec<u32> },
    /// A failed fetch. `cached_at` is the fetch time of the cache that the router uses instead.
    Fetch { source: SourceId, error: FetchError, cached_at: Option<u64> },
    /// The cache file did not write. The text is the cause.
    CacheWrite(String),
    /// Nexum did not give the signatures of this many systems.
    NexumSignatures { failed: usize },
    /// EVE Metro denies access to the linked character.
    MetroNoAccess,
}

impl fmt::Display for OverlayWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OverlayWarning::BridgeUnknownSystems(names) => write!(f, "Bridges: unknown systems: {}", names.join(", ")),
            OverlayWarning::BridgeSameSystem(names) => write!(f, "Bridges: same system at both ends: {}", names.join(", ")),
            OverlayWarning::UnknownIds { source, ids } => {
                let ids: Vec<String> = ids.iter().map(u32::to_string).collect();
                write!(f, "{}: unknown system IDs: {}", source.label(), ids.join(", "))
            }
            OverlayWarning::Fetch { source, error, cached_at } => f.write_str(&error.status(*source, *cached_at)),
            OverlayWarning::CacheWrite(cause) => write!(f, "Cannot write the wormhole cache: {cause}"),
            OverlayWarning::NexumSignatures { failed: 1 } => f.write_str("Nexum signatures: 1 system not loaded"),
            OverlayWarning::NexumSignatures { failed } => write!(f, "Nexum signatures: {failed} systems not loaded"),
            OverlayWarning::MetroNoAccess => f.write_str("EVE Metro: this character has no access"),
        }
    }
}

/// The warnings as one line, or `None` for no warning.
pub fn warning_text(warnings: &[OverlayWarning]) -> Option<String> {
    (!warnings.is_empty()).then(|| warnings.iter().map(ToString::to_string).collect::<Vec<_>>().join(" · "))
}

/// The data that the router uses, and the problem for the status line.
pub struct Chosen {
    pub data: Option<SourceData>,
    pub warning: Option<OverlayWarning>,
    /// The failed fetch. The log shows its cause.
    pub error: Option<FetchError>,
}

/// Choose between a fetch result and the cache. A success writes the cache.
/// A failure uses the cache. A failed fetch never stops the router.
pub fn choose(source: SourceId, fetched: Result<SourceData, FetchError>, cache: Option<SourceData>, cache_path: &Path) -> Chosen {
    match fetched {
        Ok(data) => {
            let warning = write_cache(cache_path, &data).err().map(|e| OverlayWarning::CacheWrite(e.to_string()));
            Chosen { data: Some(data), warning, error: None }
        }
        Err(e) => {
            let warning = Some(OverlayWarning::Fetch { source, error: e.clone(), cached_at: cache.as_ref().map(|c| c.fetched_at) });
            Chosen { data: cache, warning, error: Some(e) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiKey, NexumConfig};
    use crate::test_support::{hole, serve};
    use std::time::Duration;

    const MAPS: &str = r#"{"maps":[{"id":"m1","name":"Home","isCorpMap":false},{"id":"m2","name":"Scanning"}]}"#;

    fn nexum(url: &str) -> NexumConfig {
        NexumConfig { url: Some(url.into()), key: Some(ApiKey("nxm_test".into())), map_ids: vec!["m1".into()], keep_unsigned: true }
    }

    #[test]
    fn get_sends_one_get_with_the_key() {
        let (url, request) = serve("200 OK", "{}", Duration::ZERO);
        assert_eq!(get(&format!("{url}/x"), Some("nxm_test"), TIMEOUT), Ok("{}".into()));
        let request = request.recv().unwrap();
        assert!(request.starts_with("GET /x HTTP/1.1\r\n"), "{request}");
        assert!(request.to_lowercase().contains("authorization: bearer nxm_test"), "{request}");
    }

    #[test]
    fn status_codes_give_fetch_errors() {
        for (status, expected) in
            [("401 Unauthorized", FetchError::Auth), ("403 Forbidden", FetchError::Auth), ("404 Not Found", FetchError::NotFound)]
        {
            let (url, _) = serve(status, "{}", Duration::ZERO);
            assert_eq!(get(&url, Some("k"), TIMEOUT), Err(expected), "{status}");
        }
        let (url, _) = serve("500 Internal Server Error", "{}", Duration::ZERO);
        assert!(matches!(get(&url, Some("k"), TIMEOUT), Err(FetchError::Offline(_))));
    }

    #[test]
    fn slow_server_times_out() {
        let (url, _) = serve("200 OK", "{}", Duration::from_secs(2));
        assert!(matches!(get(&url, Some("k"), Duration::from_millis(300)), Err(FetchError::Offline(_))));
    }

    #[test]
    fn map_list_and_bad_json() {
        let (url, request) = serve("200 OK", MAPS, Duration::ZERO);
        let maps = nexum::fetch_maps(&nexum(&url), TIMEOUT).unwrap();
        assert_eq!(
            maps,
            vec![nexum::MapInfo { id: "m1".into(), name: "Home".into() }, nexum::MapInfo { id: "m2".into(), name: "Scanning".into() }]
        );
        assert!(request.recv().unwrap().starts_with("GET /api/v1/maps HTTP/1.1"));
        let (url, _) = serve("200 OK", "not json", Duration::ZERO);
        assert!(matches!(nexum::fetch_maps(&nexum(&url), TIMEOUT), Err(FetchError::Offline(_))));
    }

    #[test]
    fn map_url_has_one_slash() {
        let mut cfg = nexum("https://nexum.example/");
        assert_eq!(nexum::map_url(&cfg, "m1").as_deref(), Some("https://nexum.example/api/v1/maps/m1"));
        cfg.url = Some("https://nexum.example".into());
        assert_eq!(nexum::map_url(&cfg, "m1").as_deref(), Some("https://nexum.example/api/v1/maps/m1"));
        cfg.url = None;
        assert_eq!(nexum::map_url(&cfg, "m1"), None);
    }

    #[test]
    fn status_text() {
        // 2026-10-05T14:02:00Z. The status shows it in the local time zone of the machine.
        let t = 1_791_208_920;
        assert_eq!(FetchError::Auth.status(SourceId::Nexum, Some(t)), "Nexum key rejected");
        assert_eq!(FetchError::NotFound.status(SourceId::Nexum, None), "Nexum map not found");
        let offline = FetchError::Offline("x".into()).status(SourceId::Nexum, Some(t));
        assert_eq!(offline, format!("Nexum offline, map from {}", crate::wormhole::local_time(t)));
        assert_eq!(FetchError::Offline("x".into()).status(SourceId::Nexum, None), "Nexum offline");
        assert_eq!(FetchError::Auth.status(SourceId::EveScout, None), "EVE-Scout refused the request");
        assert_eq!(FetchError::NotFound.status(SourceId::EveScout, None), "EVE-Scout feed not found");
    }

    fn sample(fetched_at: u64) -> SourceData {
        SourceData { source: SourceId::Nexum, fetched_at, origin: None, name: None, holes: vec![hole(30000142, 31002230)] }
    }

    #[test]
    fn cache_round_trip_and_age() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join("nexum.json");
        assert_eq!(read_cache(&path), None);
        write_cache(&path, &sample(1000)).unwrap();
        assert_eq!(read_cache(&path), Some(sample(1000)));
        assert!(cache_is_fresh(&sample(1000), 1000 + CACHE_FRESH_SECS - 1));
        assert!(!cache_is_fresh(&sample(1000), 1000 + CACHE_FRESH_SECS));
        // A fetch time in the future (a clock change) is not fresh.
        assert!(!cache_is_fresh(&sample(2000), 1000));
        // A corrupt or old-format file counts as no cache.
        fs::write(&path, "{\"holes\": 3}").unwrap();
        assert_eq!(read_cache(&path), None);
    }

    #[test]
    fn choose_uses_the_cache_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join("nexum.json");
        // A success writes the cache.
        let chosen = choose(SourceId::Nexum, Ok(sample(5000)), None, &path);
        assert_eq!(chosen.data, Some(sample(5000)));
        assert_eq!(chosen.warning, None);
        assert_eq!(read_cache(&path), Some(sample(5000)));
        // A failure uses the cache and gives a status line.
        let chosen = choose(SourceId::Nexum, Err(FetchError::Auth), Some(sample(4000)), &path);
        assert_eq!(chosen.data, Some(sample(4000)));
        assert_eq!(chosen.warning.map(|w| w.to_string()).as_deref(), Some("Nexum key rejected"));
        // A failure with no cache gives no wormholes.
        let chosen = choose(SourceId::Nexum, Err(FetchError::Offline("timeout".into())), None, &path);
        assert_eq!(chosen.data, None);
        assert_eq!(chosen.warning.map(|w| w.to_string()).as_deref(), Some("Nexum offline"));
    }

    #[test]
    fn an_old_cache_with_no_name_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nexum.json");
        // A cache file from before the name field.
        let old = r#"{"source":"Nexum","fetched_at":1000,"origin":"https://nexum.example/api/v1/maps/m1","holes":[]}"#;
        fs::write(&path, old).unwrap();
        let data = read_cache(&path).unwrap();
        assert_eq!((data.name, data.fetched_at), (None, 1000));
    }
}
