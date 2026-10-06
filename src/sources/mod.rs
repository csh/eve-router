//! Wormhole sources. Each source has one file, and converts its data to `Wormhole` records.
//! This file holds the HTTP GET, the fetch errors and the disk cache.

pub mod evescout;
pub mod nexum;

use crate::wormhole::{SourceData, SourceId, local_time};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The timeout of one request.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// A cache that is younger than this stops the fetch at startup.
pub const CACHE_FRESH_SECS: u64 = 300;
/// The largest response body. A large alliance map is a few MB.
const MAX_BODY: u64 = 64 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub enum FetchError {
    /// 401 or 403: the key is wrong, or it has no access to the map.
    Auth,
    /// 404: the map ID is wrong.
    NotFound,
    /// A timeout, a network error, a 5xx status or bad JSON.
    Offline(String),
}

impl FetchError {
    /// The status line text. `cached_at` is the fetch time of the cache in use.
    pub fn status(&self, source: SourceId, cached_at: Option<u64>) -> String {
        let name = source.label();
        // Nexum gives a map. EVE-Scout gives a public feed, with no key.
        let data = match source {
            SourceId::Nexum => "map",
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
/// The router never sends any other method to a source.
pub fn get_with(agent: &ureq::Agent, url: &str, key: Option<&str>) -> Result<String, FetchError> {
    let mut request = agent.get(url);
    if let Some(key) = key {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    match request.call() {
        Ok(mut resp) => resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|e| FetchError::Offline(e.to_string())),
        Err(ureq::Error::StatusCode(401 | 403)) => Err(FetchError::Auth),
        Err(ureq::Error::StatusCode(404)) => Err(FetchError::NotFound),
        Err(e) => Err(FetchError::Offline(e.to_string())),
    }
}

/// The cache file of a source: `nexum.json` in the platform cache directory, for example
/// `%LOCALAPPDATA%\com.smrkn.eve-router\nexum.json` on Windows. If the platform gives no
/// cache directory, use a `cache` directory next to the config file.
pub fn cache_path(source: SourceId, cfg_path: &Path) -> PathBuf {
    let name = format!("{}.json", source.file_stem());
    dirs::cache_dir()
        .map_or_else(|| cfg_path.with_file_name("cache").join(&name), |dir| dir.join(crate::config::APP_DIR).join(&name))
}

/// Read a cache file. A missing, corrupt or old-format file gives `None`.
pub fn read_cache(path: &Path) -> Option<SourceData> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Write a cache file: a temporary file, then a rename, so a reader never sees half a file.
pub fn write_cache(path: &Path, data: &SourceData) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.part");
    let text = serde_json::to_string(data).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// True if the cache is less than 5 minutes old. A fetch time after `now` is not fresh.
pub fn cache_is_fresh(data: &SourceData, now: u64) -> bool {
    data.fetched_at <= now && now - data.fetched_at < CACHE_FRESH_SECS
}

/// The data that the router uses, and the text for the status line.
pub struct Chosen {
    pub data: Option<SourceData>,
    pub warning: Option<String>,
}

/// Choose between a fetch result and the cache. A success writes the cache.
/// A failure uses the cache. A failed fetch never stops the router.
pub fn choose(source: SourceId, fetched: Result<SourceData, FetchError>, cache: Option<SourceData>, cache_path: &Path) -> Chosen {
    match fetched {
        Ok(data) => {
            let warning = write_cache(cache_path, &data).err().map(|e| format!("Cannot write the wormhole cache: {e}"));
            Chosen { data: Some(data), warning }
        }
        Err(e) => {
            let warning = Some(e.status(source, cache.as_ref().map(|c| c.fetched_at)));
            Chosen { data: cache, warning }
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
        NexumConfig { url: Some(url.into()), key: Some(ApiKey("nxm_test".into())), map_id: Some("m1".into()) }
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
        for (status, expected) in [
            ("401 Unauthorized", FetchError::Auth),
            ("403 Forbidden", FetchError::Auth),
            ("404 Not Found", FetchError::NotFound),
        ] {
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
        assert_eq!(maps, vec![nexum::MapInfo { id: "m1".into(), name: "Home".into() }, nexum::MapInfo { id: "m2".into(), name: "Scanning".into() }]);
        assert!(request.recv().unwrap().starts_with("GET /api/v1/maps HTTP/1.1"));
        let (url, _) = serve("200 OK", "not json", Duration::ZERO);
        assert!(matches!(nexum::fetch_maps(&nexum(&url), TIMEOUT), Err(FetchError::Offline(_))));
    }

    #[test]
    fn map_url_has_one_slash() {
        let mut cfg = nexum("https://nexum.example/");
        assert_eq!(nexum::map_url(&cfg).as_deref(), Some("https://nexum.example/api/v1/maps/m1"));
        cfg.url = Some("https://nexum.example".into());
        assert_eq!(nexum::map_url(&cfg).as_deref(), Some("https://nexum.example/api/v1/maps/m1"));
        cfg.map_id = None;
        assert_eq!(nexum::map_url(&cfg), None);
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
        SourceData { source: SourceId::Nexum, fetched_at, origin: None, holes: vec![hole(30000142, 31002230)] }
    }

    #[test]
    fn cache_round_trip_and_age() {
        let dir = std::env::temp_dir().join("eve-router-test-cache");
        let _ = fs::remove_dir_all(&dir);
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
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn choose_uses_the_cache_on_failure() {
        let dir = std::env::temp_dir().join("eve-router-test-choose");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nexum.json");
        // A success writes the cache.
        let chosen = choose(SourceId::Nexum, Ok(sample(5000)), None, &path);
        assert_eq!(chosen.data, Some(sample(5000)));
        assert_eq!(chosen.warning, None);
        assert_eq!(read_cache(&path), Some(sample(5000)));
        // A failure uses the cache and gives a status line.
        let chosen = choose(SourceId::Nexum, Err(FetchError::Auth), Some(sample(4000)), &path);
        assert_eq!(chosen.data, Some(sample(4000)));
        assert_eq!(chosen.warning.as_deref(), Some("Nexum key rejected"));
        // A failure with no cache gives no wormholes.
        let chosen = choose(SourceId::Nexum, Err(FetchError::Offline("timeout".into())), None, &path);
        assert_eq!(chosen.data, None);
        assert_eq!(chosen.warning.as_deref(), Some("Nexum offline"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
