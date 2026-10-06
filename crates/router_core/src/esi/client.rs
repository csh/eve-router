//! The ESI calls of the router: one waypoint POST and three character GETs.
//!
//! Each request sends `X-Compatibility-Date`. Each response gives its `Expires` time, so the
//! tracker does not ask again before ESI has new data, and the remaining error budget
//! (`X-ESI-Error-Limit-Remain`), so the tracker can slow down before ESI blocks it.

use oauth2::AccessToken;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::time::{Duration, SystemTime};

pub const ESI_URL: &str = "https://esi.evetech.net";
/// The image server. It needs no token.
pub const IMAGES_URL: &str = "https://images.evetech.net";
/// The ESI version that the router expects. ESI uses the newest version on or before this date.
pub const COMPATIBILITY_DATE: &str = "2025-08-26";
/// The timeout of one ESI request.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The longest wait after a 420 or 429. A larger header value is not real, and it can overflow `Instant`.
pub const MAX_RETRY: Duration = Duration::from_secs(3600);
/// The largest ESI response body. The router reads only small objects.
const MAX_BODY: u64 = 64 * 1024;

/// `GET /characters/{id}/location`.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub solar_system_id: u32,
    pub station_id: Option<u32>,
    pub structure_id: Option<u64>,
}

/// `GET /characters/{id}/ship`.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Ship {
    pub ship_type_id: u32,
    pub ship_item_id: u64,
    pub ship_name: String,
}

/// `GET /characters/{id}/online`.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Online {
    pub online: bool,
}

/// A response value, with the cache and error-budget headers.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched<T> {
    pub value: T,
    /// ESI has no new data before this time.
    pub expires: Option<SystemTime>,
    /// `X-ESI-Error-Limit-Remain`: the errors left in the current window.
    pub errors_left: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub enum EsiError {
    /// 401 or 403: the token is expired, revoked or has no scope for the call.
    Auth,
    /// 420 or 429: ESI blocks the router for a time.
    Limited { retry_after: Option<Duration> },
    /// A timeout, a network error, a 5xx status or bad JSON.
    Offline(String),
    /// Any other status.
    Status(u16),
}

impl fmt::Display for EsiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EsiError::Auth => f.write_str("ESI refused the login"),
            EsiError::Limited { .. } => f.write_str("ESI limited — slowing updates"),
            EsiError::Offline(e) => write!(f, "ESI is not reachable: {e}"),
            EsiError::Status(code) => write!(f, "ESI gave status {code}"),
        }
    }
}

impl std::error::Error for EsiError {}

/// The ESI client. One agent keeps its connections open between polls.
#[derive(Clone)]
pub struct Esi {
    agent: ureq::Agent,
    base: String,
}

impl Default for Esi {
    fn default() -> Self {
        Esi::new(ESI_URL)
    }
}

impl Esi {
    /// `base` is `ESI_URL`, or a test server.
    pub fn new(base: &str) -> Esi {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("eve-router/", env!("CARGO_PKG_VERSION")))
            // The status and the headers of an error response are needed.
            .http_status_as_error(false)
            .build()
            .new_agent();
        Esi { agent, base: base.trim_end_matches('/').to_owned() }
    }

    pub fn location(&self, token: &AccessToken, character: u64) -> Result<Fetched<Location>, EsiError> {
        self.get(token, &format!("/characters/{character}/location"))
    }

    pub fn ship(&self, token: &AccessToken, character: u64) -> Result<Fetched<Ship>, EsiError> {
        self.get(token, &format!("/characters/{character}/ship"))
    }

    pub fn online(&self, token: &AccessToken, character: u64) -> Result<Fetched<Online>, EsiError> {
        self.get(token, &format!("/characters/{character}/online"))
    }

    /// Add one waypoint at the end of the in-game route of the token's character.
    /// `clear` removes the other waypoints first. ESI needs the game client to be online.
    pub fn add_waypoint(&self, token: &AccessToken, system: u32, clear: bool) -> Result<(), EsiError> {
        let request = self
            .agent
            .post(format!("{}/ui/autopilot/waypoint", self.base))
            .query("destination_id", system.to_string())
            .query("add_to_beginning", "false")
            .query("clear_other_waypoints", clear.to_string())
            .header("Authorization", format!("Bearer {}", token.secret()))
            .header("X-Compatibility-Date", COMPATIBILITY_DATE);
        let response = request.send_empty().map_err(|e| EsiError::Offline(e.to_string()))?;
        check_status(&response).map(|_| ())
    }

    fn get<T: DeserializeOwned>(&self, token: &AccessToken, path: &str) -> Result<Fetched<T>, EsiError> {
        let request = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("Authorization", format!("Bearer {}", token.secret()))
            .header("X-Compatibility-Date", COMPATIBILITY_DATE);
        let mut response = request.call().map_err(|e| EsiError::Offline(e.to_string()))?;
        let errors_left = check_status(&response)?;
        let expires = header(&response, "expires").and_then(|v| httpdate::parse_http_date(&v).ok());
        let text = response.body_mut().with_config().limit(MAX_BODY).read_to_string().map_err(|e| EsiError::Offline(e.to_string()))?;
        let value = serde_json::from_str(&text).map_err(|e| EsiError::Offline(format!("bad JSON: {e}")))?;
        Ok(Fetched { value, expires, errors_left })
    }
}

/// The portrait of a character (JPEG), from `IMAGES_URL` or a test server. `size` is a power of 2
/// from 32 to 1024.
pub fn portrait(base: &str, id: u64, size: u32) -> Result<Vec<u8>, EsiError> {
    let url = format!("{}/characters/{id}/portrait?size={size}", base.trim_end_matches('/'));
    let mut response = crate::sources::agent(TIMEOUT).get(&url).call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => EsiError::Status(code),
        e => EsiError::Offline(e.to_string()),
    })?;
    response.body_mut().with_config().limit(MAX_BODY * 4).read_to_vec().map_err(|e| EsiError::Offline(e.to_string()))
}

fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

/// Map an error status to an `EsiError`. A success gives the error budget.
fn check_status(response: &ureq::http::Response<ureq::Body>) -> Result<Option<u32>, EsiError> {
    let errors_left = header(response, "x-esi-error-limit-remain").and_then(|v| v.trim().parse().ok());
    match response.status().as_u16() {
        200..=299 => Ok(errors_left),
        401 | 403 => Err(EsiError::Auth),
        420 | 429 => {
            let retry_after = header(response, "retry-after")
                .or_else(|| header(response, "x-esi-error-limit-reset"))
                .and_then(|v| v.trim().parse().ok())
                .map(|secs| Duration::from_secs(secs).min(MAX_RETRY));
            Err(EsiError::Limited { retry_after })
        }
        code @ 500..=599 => Err(EsiError::Offline(format!("status {code}"))),
        code => Err(EsiError::Status(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{serve, serve_with_headers};

    fn token() -> AccessToken {
        AccessToken::new("t0ken".into())
    }

    #[test]
    fn location_with_headers() {
        let headers = [("Expires", "Tue, 06 Oct 2026 14:00:05 GMT"), ("X-ESI-Error-Limit-Remain", "97")];
        let (url, rx) = serve_with_headers("200 OK", r#"{"solar_system_id": 30000142, "station_id": 60003760}"#, &headers);
        let fetched = Esi::new(&url).location(&token(), 2112625428).unwrap();
        assert_eq!(fetched.value, Location { solar_system_id: 30000142, station_id: Some(60003760), structure_id: None });
        assert_eq!(fetched.errors_left, Some(97));
        assert_eq!(fetched.expires, Some(httpdate::parse_http_date("Tue, 06 Oct 2026 14:00:05 GMT").unwrap()));
        let request = rx.recv().unwrap().to_lowercase();
        assert!(request.starts_with("get /characters/2112625428/location http/1.1"), "{request}");
        assert!(request.contains("authorization: bearer t0ken"), "{request}");
        assert!(request.contains(&format!("x-compatibility-date: {COMPATIBILITY_DATE}")), "{request}");
    }

    #[test]
    fn ship_and_online() {
        let (url, _) = serve("200 OK", r#"{"ship_type_id": 670, "ship_item_id": 1, "ship_name": "Pod"}"#, Duration::ZERO);
        assert_eq!(Esi::new(&url).ship(&token(), 1).unwrap().value.ship_type_id, 670);
        let (url, _) = serve("200 OK", r#"{"online": true, "last_login": "2026-10-06T12:00:00Z"}"#, Duration::ZERO);
        assert!(Esi::new(&url).online(&token(), 1).unwrap().value.online);
    }

    #[test]
    fn waypoint_query() {
        let (url, rx) = serve("204 No Content", "", Duration::ZERO);
        Esi::new(&url).add_waypoint(&token(), 30000142, true).unwrap();
        let request = rx.recv().unwrap();
        let line = request.lines().next().unwrap();
        assert!(line.starts_with("POST /ui/autopilot/waypoint?"), "{line}");
        for part in ["destination_id=30000142", "add_to_beginning=false", "clear_other_waypoints=true"] {
            assert!(line.contains(part), "{part} not in {line}");
        }
    }

    #[test]
    fn error_statuses() {
        let check = |status: &str, headers: &[(&str, &str)]| {
            let (url, _) = serve_with_headers(status, r#"{"error": "x"}"#, headers);
            Esi::new(&url).location(&token(), 1).unwrap_err()
        };
        assert_eq!(check("401 Unauthorized", &[]), EsiError::Auth);
        assert_eq!(check("403 Forbidden", &[]), EsiError::Auth);
        assert_eq!(
            check("420 Error Limited", &[("X-ESI-Error-Limit-Reset", "37")]),
            EsiError::Limited { retry_after: Some(Duration::from_secs(37)) }
        );
        assert_eq!(
            check("429 Too Many Requests", &[("Retry-After", "5")]),
            EsiError::Limited { retry_after: Some(Duration::from_secs(5)) }
        );
        // A huge header value waits at most one hour, so `Instant` does not overflow.
        assert_eq!(
            check("429 Too Many Requests", &[("Retry-After", "18446744073709551615")]),
            EsiError::Limited { retry_after: Some(MAX_RETRY) }
        );
        assert!(matches!(check("503 Service Unavailable", &[]), EsiError::Offline(_)));
        assert_eq!(check("404 Not Found", &[]), EsiError::Status(404));
    }

    #[test]
    fn portrait_bytes() {
        let (url, rx) = serve("200 OK", "JPEG", Duration::ZERO);
        assert_eq!(portrait(&url, 7, 64).unwrap(), b"JPEG");
        assert!(rx.recv().unwrap().starts_with("GET /characters/7/portrait?size=64 HTTP/1.1"));
        let (url, _) = serve("404 Not Found", "", Duration::ZERO);
        assert_eq!(portrait(&url, 7, 64), Err(EsiError::Status(404)));
    }

    #[test]
    fn bad_json_is_offline() {
        let (url, _) = serve("200 OK", "<html>", Duration::ZERO);
        assert!(matches!(Esi::new(&url).online(&token(), 1), Err(EsiError::Offline(_))));
    }
}
