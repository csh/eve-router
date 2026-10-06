//! EVE SSO: OAuth 2.0 with PKCE, for a native app with no client secret.
//!
//! A login has three steps:
//! 1. `Login::start` makes the PKCE pair and the URL, and listens on `127.0.0.1:21404`.
//! 2. The browser sends the code to the listener. Or the user pastes the redirected URL, and
//!    `Login::paste` reads the code from it. Both paths work at the same time.
//! 3. `exchange` sends the code and the PKCE verifier to SSO, and gets the tokens.
//!
//! The router does not check the JWT signature. It gets each token directly from the SSO token
//! endpoint over TLS, so TLS proves where the token came from (OpenID Connect Core 3.1.3.7).
//! The router still checks the issuer, the audience, the expiry and the subject.

use super::{Character, SCOPES, Secret};
use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const AUTHORIZE_URL: &str = "https://login.eveonline.com/v2/oauth/authorize";
pub const TOKEN_URL: &str = "https://login.eveonline.com/v2/oauth/token";
pub const REVOKE_URL: &str = "https://login.eveonline.com/v2/oauth/revoke";

/// The port of the callback. EVE SSO matches the registered callback URL exactly, so the port
/// is fixed.
pub const CALLBACK_PORT: u16 = 21404;
pub const CALLBACK_PATH: &str = "/callback";
/// The two issuer values that EVE SSO uses.
const ISSUERS: [&str; 2] = ["login.eveonline.com", "https://login.eveonline.com"];
/// The audience value that each EVE SSO token has, next to the client ID.
const AUDIENCE: &str = "EVE Online";
const SUBJECT_PREFIX: &str = "CHARACTER:EVE:";
/// The timeout of one SSO request.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest HTTP request that the listener reads.
const MAX_REQUEST: usize = 8 * 1024;

/// The callback URL, as registered for the EVE developer app.
pub fn redirect_uri() -> String {
    format!("http://localhost:{CALLBACK_PORT}{CALLBACK_PATH}")
}

#[derive(Debug, PartialEq)]
pub enum LoginError {
    /// SSO sent an `error` to the callback, for example when the user cancels the login.
    Denied(String),
    /// The `state` is not the one of this login.
    StateMismatch,
    /// The text has no `code`.
    NoCode,
    /// SSO refused the code or the refresh token (400 or 401). The character must log in again.
    Rejected,
    /// A timeout, a network error, a 5xx status or bad JSON.
    Offline(String),
    /// The token is not valid: a wrong issuer, audience or subject, or an expired token.
    BadToken(String),
    /// The system random source failed.
    NoRandom(String),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginError::Denied(e) => write!(f, "EVE SSO did not give access ({e}). Start the login again."),
            LoginError::StateMismatch => f.write_str("This URL is not from the current login. Use the URL from the newest login tab."),
            LoginError::NoCode => f.write_str("The URL has no login code. Paste the full URL from the address bar."),
            LoginError::Rejected => f.write_str("EVE SSO refused the login. Log in again."),
            LoginError::Offline(e) => write!(f, "EVE SSO is not reachable: {e}"),
            LoginError::BadToken(e) => write!(f, "EVE SSO sent a token that is not valid: {e}"),
            LoginError::NoRandom(e) => write!(f, "The system random source failed: {e}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// The PKCE pair (RFC 7636). The verifier stays in the router. The challenge goes in the URL.
#[derive(Debug, PartialEq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// The verifier is the 32 bytes in base64url. The challenge is the SHA-256 of the verifier.
    pub fn from_bytes(bytes: &[u8; 32]) -> Pkce {
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge }
    }
}

/// 32 random bytes from the operating system.
fn random_bytes() -> Result<[u8; 32], LoginError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| LoginError::NoRandom(e.to_string()))?;
    Ok(bytes)
}

/// The URL that starts the login in the browser.
pub fn authorize_url(client_id: &str, state: &str, challenge: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("redirect_uri", &redirect_uri()),
        ("client_id", client_id),
        ("scope", &SCOPES.join(" ")),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    format!("{AUTHORIZE_URL}?{}", query.join("&"))
}

/// Percent-encode all but the unreserved characters of RFC 3986.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decode a query value: "%XX" gives a byte, and "+" gives a space. A bad escape stays as text.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match text.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                Some(byte) => {
                    out.push(byte);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read the code from a callback URL, a callback request target or a bare query string.
/// The URL must have the `state` of this login.
pub fn parse_callback(text: &str, state: &str) -> Result<String, LoginError> {
    let text = text.trim();
    let text = text.split('#').next().unwrap_or_default();
    let query = match text.split_once('?') {
        Some((_, query)) => query,
        None if text.contains('=') => text,
        None => return Err(LoginError::NoCode),
    };
    let mut code = None;
    let mut got_state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(decode(value)),
            "state" => got_state = Some(decode(value)),
            "error" => error = Some(decode(value)),
            _ => {}
        }
    }
    if got_state.as_deref() != Some(state) {
        return Err(LoginError::StateMismatch);
    }
    if let Some(error) = error {
        return Err(LoginError::Denied(error));
    }
    code.filter(|c| !c.is_empty()).ok_or(LoginError::NoCode)
}

/// The loopback listener. A thread accepts connections until a request gives a code or an
/// error, or until the listener drops. A request with a wrong `state` does not stop it.
pub struct Listener {
    port: u16,
    rx: Receiver<Result<String, LoginError>>,
    stop: Arc<AtomicBool>,
}

impl Listener {
    /// Listen on `127.0.0.1` only. Port 0 gives a free port, for the tests.
    pub fn bind(port: u16, state: String) -> io::Result<Listener> {
        let socket = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        socket.set_nonblocking(true)?;
        let port = socket.local_addr()?.port();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match socket.accept() {
                    Ok((stream, _)) => {
                        if let Some(result) = handle(stream, &state) {
                            let _ = tx.send(result);
                            return;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        Ok(Listener { port, rx, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The code or the error, when a request gave one.
    pub fn try_recv(&self) -> Option<Result<String, LoginError>> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Answer one HTTP request. `None` means "keep listening".
fn handle(mut stream: TcpStream, state: &str) -> Option<Result<String, LoginError>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    // Read only the request line.
    while !buf.windows(2).any(|w| w == b"\r\n") && buf.len() < MAX_REQUEST {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let request = String::from_utf8_lossy(&buf);
    let line = request.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let path = target.split('?').next().unwrap_or_default();
    if method != "GET" || path != CALLBACK_PATH {
        respond(&mut stream, "404 Not Found", "Not found.");
        return None;
    }
    match parse_callback(target, state) {
        Ok(code) => {
            respond(&mut stream, "200 OK", "Login received. Close this tab and go back to EVE Router.");
            Some(Ok(code))
        }
        Err(LoginError::StateMismatch) => {
            respond(&mut stream, "400 Bad Request", &LoginError::StateMismatch.to_string());
            None
        }
        Err(e) => {
            respond(&mut stream, "400 Bad Request", &e.to_string());
            Some(Err(e))
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>EVE Router</title>\
         <body style=\"font-family:sans-serif;background:#111;color:#ddd;padding:2em\"><p>{}</p></body>",
        message.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    );
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes()));
}

/// One login. Drop it to stop the listener.
pub struct Login {
    /// Open this URL in the browser, and show it for a copy.
    pub url: String,
    state: String,
    pkce: Pkce,
    listener: Result<Listener, String>,
}

impl Login {
    /// Make the PKCE pair and the state, and listen on the callback port. A listener failure
    /// does not stop the login: the user can paste the redirected URL.
    pub fn start(client_id: &str) -> Result<Login, LoginError> {
        Self::start_on(client_id, CALLBACK_PORT)
    }

    fn start_on(client_id: &str, port: u16) -> Result<Login, LoginError> {
        let pkce = Pkce::from_bytes(&random_bytes()?);
        let state = URL_SAFE_NO_PAD.encode(random_bytes()?);
        let url = authorize_url(client_id, &state, &pkce.challenge);
        let listener = Listener::bind(port, state.clone()).map_err(|e| match e.kind() {
            io::ErrorKind::AddrInUse => format!("Port {port} is in use. Paste the redirected URL instead."),
            _ => format!("Cannot listen on port {port}: {e}. Paste the redirected URL instead."),
        });
        Ok(Login { url, state, pkce, listener })
    }

    /// The reason the listener did not start, if it did not.
    pub fn listen_error(&self) -> Option<&str> {
        self.listener.as_ref().err().map(String::as_str)
    }

    /// The code from the listener, when the browser sent it.
    pub fn poll(&self) -> Option<Result<String, LoginError>> {
        self.listener.as_ref().ok()?.try_recv()
    }

    /// The code from a pasted URL.
    pub fn paste(&self, text: &str) -> Result<String, LoginError> {
        parse_callback(text, &self.state)
    }

    /// The PKCE verifier, for `exchange`.
    pub fn verifier(&self) -> &str {
        &self.pkce.verifier
    }
}

/// The tokens of one character.
#[derive(Debug)]
pub struct Tokens {
    pub character: Character,
    pub access: Secret,
    /// SSO can give a new refresh token at each refresh. Store the new one before the next use.
    pub refresh: Secret,
    /// The expiry of the access token, in Unix seconds.
    pub expires_at: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: String,
}

/// Send the code and the verifier, and get the tokens.
pub fn exchange(agent: &ureq::Agent, client_id: &str, code: &str, verifier: &str) -> Result<Tokens, LoginError> {
    let form = [("grant_type", "authorization_code"), ("code", code), ("client_id", client_id), ("code_verifier", verifier)];
    tokens(post_form(agent, TOKEN_URL, &form)?, client_id, now())
}

/// Get a new access token with the refresh token.
pub fn refresh(agent: &ureq::Agent, client_id: &str, refresh: &Secret) -> Result<Tokens, LoginError> {
    let form = [("grant_type", "refresh_token"), ("refresh_token", refresh.expose()), ("client_id", client_id)];
    tokens(post_form(agent, TOKEN_URL, &form)?, client_id, now())
}

/// Revoke a refresh token at SSO. After this, the token gives no access.
pub fn revoke(agent: &ureq::Agent, client_id: &str, refresh: &Secret) -> Result<(), LoginError> {
    let form = [("token_type_hint", "refresh_token"), ("token", refresh.expose()), ("client_id", client_id)];
    post_form(agent, REVOKE_URL, &form).map(|_| ())
}

fn post_form(agent: &ureq::Agent, url: &str, form: &[(&str, &str)]) -> Result<String, LoginError> {
    match agent.post(url).send_form(form.iter().copied()) {
        Ok(mut resp) => resp.body_mut().read_to_string().map_err(|e| LoginError::Offline(e.to_string())),
        Err(ureq::Error::StatusCode(400 | 401)) => Err(LoginError::Rejected),
        Err(e) => Err(LoginError::Offline(e.to_string())),
    }
}

fn tokens(body: String, client_id: &str, now: u64) -> Result<Tokens, LoginError> {
    let resp: TokenResponse = serde_json::from_str(&body).map_err(|e| LoginError::Offline(format!("bad token response: {e}")))?;
    let character = read_claims(&resp.access_token, client_id, now)?;
    Ok(Tokens {
        character,
        access: Secret::new(resp.access_token),
        refresh: Secret::new(resp.refresh_token),
        expires_at: now + resp.expires_in,
    })
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    #[serde(default)]
    aud: Audience,
    exp: u64,
    sub: String,
    name: String,
    #[serde(default)]
    scp: Scopes,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

/// SSO gives one scope as a string, and more scopes as a list.
#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Scopes {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

/// Read the character from the access token, and check the issuer, the audience, the expiry
/// and the subject. See the module comment about the signature.
pub fn read_claims(token: &str, client_id: &str, now: u64) -> Result<Character, LoginError> {
    let bad = |text: &str| LoginError::BadToken(text.to_owned());
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(bad("not a JWT"));
    };
    let json = URL_SAFE_NO_PAD_INDIFFERENT.decode(payload).map_err(|_| bad("bad base64"))?;
    let claims: Claims = serde_json::from_slice(&json).map_err(|e| LoginError::BadToken(e.to_string()))?;
    if !ISSUERS.contains(&claims.iss.as_str()) {
        return Err(LoginError::BadToken(format!("wrong issuer {}", claims.iss)));
    }
    let audience = match claims.aud {
        Audience::One(a) => vec![a],
        Audience::Many(list) => list,
        Audience::None => vec![],
    };
    if !audience.iter().any(|a| a == client_id) || !audience.iter().any(|a| a == AUDIENCE) {
        return Err(bad("wrong audience"));
    }
    if claims.exp <= now {
        return Err(bad("expired"));
    }
    let id = claims.sub.strip_prefix(SUBJECT_PREFIX).and_then(|id| id.parse().ok()).ok_or_else(|| bad("bad subject"))?;
    let scopes = match claims.scp {
        Scopes::One(s) => vec![s],
        Scopes::Many(list) => list,
        Scopes::None => vec![],
    };
    Ok(Character { id, name: claims.name, scopes })
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CLIENT: &str = "client-123";
    const NOW: u64 = 1_800_000_000;

    /// A JWT with these claims. The header and the signature are not read.
    fn jwt(claims: serde_json::Value) -> String {
        let part = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.c2ln", part(&json!({"alg": "RS256", "kid": "JWT-Signature-Key"})), part(&claims))
    }

    fn claims() -> serde_json::Value {
        json!({
            "iss": "https://login.eveonline.com",
            "aud": [CLIENT, "EVE Online"],
            "exp": NOW + 1200,
            "sub": "CHARACTER:EVE:2112625428",
            "name": "Alice Ander",
            "scp": ["esi-ui.write_waypoint.v1", "esi-location.read_location.v1"],
        })
    }

    #[test]
    fn pkce_matches_rfc_7636_appendix_b() {
        let bytes = [
            116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77, 105, 214, 191, 240, 91, 88, 5, 88,
            83, 132, 141, 121,
        ];
        let pkce = Pkce::from_bytes(&bytes);
        assert_eq!(pkce.verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(pkce.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn authorize_url_has_every_parameter() {
        let url = authorize_url(CLIENT, "st4te", "ch4llenge");
        assert_eq!(
            url,
            "https://login.eveonline.com/v2/oauth/authorize?response_type=code\
             &redirect_uri=http%3A%2F%2Flocalhost%3A21404%2Fcallback&client_id=client-123\
             &scope=esi-ui.write_waypoint.v1%20esi-location.read_location.v1%20esi-location.read_ship_type.v1%20esi-location.read_online.v1\
             &state=st4te&code_challenge=ch4llenge&code_challenge_method=S256"
        );
    }

    #[test]
    fn parse_pasted_urls() {
        let full = "http://localhost:21404/callback?code=abc%2Bdef&state=s1";
        assert_eq!(parse_callback(full, "s1"), Ok("abc+def".into()));
        assert_eq!(parse_callback(&format!("  {full}#frag \n"), "s1"), Ok("abc+def".into()));
        assert_eq!(parse_callback("code=xyz&state=s1", "s1"), Ok("xyz".into()));
        assert_eq!(parse_callback(full, "other"), Err(LoginError::StateMismatch));
        assert_eq!(parse_callback("http://localhost:21404/callback?state=s1", "s1"), Err(LoginError::NoCode));
        assert_eq!(parse_callback("hello", "s1"), Err(LoginError::NoCode));
        assert_eq!(parse_callback("/callback?error=access_denied&state=s1", "s1"), Err(LoginError::Denied("access_denied".into())));
        // An error with no matching state is not from this login.
        assert_eq!(parse_callback("/callback?error=access_denied", "s1"), Err(LoginError::StateMismatch));
    }

    #[test]
    fn decode_keeps_bad_escapes() {
        assert_eq!(decode("a%2"), "a%2");
        assert_eq!(decode("a%zz"), "a%zz");
        assert_eq!(decode("%41+b"), "A b");
    }

    #[test]
    fn claims_give_the_character() {
        let character = read_claims(&jwt(claims()), CLIENT, NOW).unwrap();
        assert_eq!(character.id, 2112625428);
        assert_eq!(character.name, "Alice Ander");
        assert_eq!(character.scopes, ["esi-ui.write_waypoint.v1", "esi-location.read_location.v1"]);
    }

    #[test]
    fn one_scope_as_a_string() {
        let mut c = claims();
        c["scp"] = json!("esi-ui.write_waypoint.v1");
        assert_eq!(read_claims(&jwt(c), CLIENT, NOW).unwrap().scopes, ["esi-ui.write_waypoint.v1"]);
    }

    #[test]
    fn bad_claims_are_refused() {
        let check = |key: &str, value: serde_json::Value| {
            let mut c = claims();
            c[key] = value;
            assert!(matches!(read_claims(&jwt(c), CLIENT, NOW), Err(LoginError::BadToken(_))), "{key}");
        };
        check("iss", json!("https://evil.example"));
        check("aud", json!(["other-client", "EVE Online"]));
        check("aud", json!(CLIENT));
        check("exp", json!(NOW));
        check("sub", json!("CORPORATION:EVE:1"));
        assert!(matches!(read_claims("a.b", CLIENT, NOW), Err(LoginError::BadToken(_))));
        assert!(matches!(read_claims("a.!!!.c", CLIENT, NOW), Err(LoginError::BadToken(_))));
    }

    #[test]
    fn token_response_gives_tokens() {
        let body = json!({"access_token": jwt(claims()), "expires_in": 1199, "refresh_token": "r1", "token_type": "Bearer"});
        let tokens = tokens(body.to_string(), CLIENT, NOW).unwrap();
        assert_eq!(tokens.character.name, "Alice Ander");
        assert_eq!(tokens.refresh.expose(), "r1");
        assert_eq!(tokens.expires_at, NOW + 1199);
    }

    #[test]
    fn post_form_sends_the_form() {
        let (url, rx) = crate::test_support::serve("200 OK", "{}", Duration::ZERO);
        let agent = crate::sources::agent(TIMEOUT);
        assert_eq!(post_form(&agent, &url, &[("grant_type", "refresh_token"), ("refresh_token", "r1")]), Ok("{}".into()));
        let request = rx.recv().unwrap();
        assert!(request.starts_with("POST / HTTP/1.1"), "{request}");
        // The helper reads one TCP segment, so the body may not be in `request`.
        assert!(request.contains("application/x-www-form-urlencoded"), "{request}");
    }

    #[test]
    fn refused_refresh_token_is_rejected() {
        let (url, _rx) = crate::test_support::serve("400 Bad Request", r#"{"error":"invalid_grant"}"#, Duration::ZERO);
        let agent = crate::sources::agent(TIMEOUT);
        assert_eq!(post_form(&agent, &url, &[("grant_type", "refresh_token")]), Err(LoginError::Rejected));
    }

    /// Send one request to the listener and return the response text.
    fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn wait(listener: &Listener) -> Result<String, LoginError> {
        for _ in 0..100 {
            if let Some(result) = listener.try_recv() {
                return result;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no result from the listener");
    }

    #[test]
    fn listener_gets_the_code() {
        let listener = Listener::bind(0, "s1".into()).unwrap();
        let port = listener.port();
        assert!(get(port, "/favicon.ico").starts_with("HTTP/1.1 404"));
        // A wrong state does not stop the listener.
        assert!(get(port, "/callback?code=bad&state=s2").starts_with("HTTP/1.1 400"));
        assert!(listener.try_recv().is_none());
        let response = get(port, "/callback?code=good&state=s1");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("Login received"));
        assert_eq!(wait(&listener), Ok("good".into()));
    }

    #[test]
    fn listener_reports_a_denied_login() {
        let listener = Listener::bind(0, "s1".into()).unwrap();
        assert!(get(listener.port(), "/callback?error=access_denied&state=s1").starts_with("HTTP/1.1 400"));
        assert_eq!(wait(&listener), Err(LoginError::Denied("access_denied".into())));
    }

    #[test]
    fn busy_port_gives_the_paste_path() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = busy.local_addr().unwrap().port();
        let login = Login::start_on(CLIENT, port).unwrap();
        assert_eq!(login.listen_error(), Some(format!("Port {port} is in use. Paste the redirected URL instead.").as_str()));
        assert!(login.poll().is_none());
        let state = login.url.split("state=").nth(1).unwrap().split('&').next().unwrap();
        assert_eq!(login.paste(&format!("http://localhost:{port}/callback?code=c0de&state={state}")), Ok("c0de".into()));
    }

    #[test]
    fn each_login_has_a_new_state_and_verifier() {
        let a = Login::start_on(CLIENT, 0).unwrap();
        let b = Login::start_on(CLIENT, 0).unwrap();
        assert_ne!(a.state, b.state);
        assert_ne!(a.verifier(), b.verifier());
        assert_eq!(a.verifier().len(), 43);
    }
}
