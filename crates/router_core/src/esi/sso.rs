//! EVE SSO: OAuth 2.0 with PKCE, for a native app with no client secret. The `oauth2` crate makes
//! the PKCE pair, the state, the authorize URL and the token requests.
//!
//! A login has three steps:
//! 1. `Sso::start_login` gives the URL, and listens on `127.0.0.1:21404`.
//! 2. The browser sends the code to the listener. Or the user pastes the redirected URL, and
//!    `Login::paste` reads the code from it. Both paths work at the same time.
//! 3. `Sso::exchange` sends the code and the PKCE verifier to SSO, and gets the tokens.
//!
//! EVE SSO gives no OpenID Connect ID token. The character is in the claims of the access token,
//! a JWT. The router does not check the JWT signature: it gets each token directly from the SSO
//! token endpoint over TLS, so TLS proves where the token came from. The router still checks the
//! issuer, the audience, the expiry and the subject.

use super::{Character, SCOPES};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD_INDIFFERENT;
use oauth2::basic::BasicClient;
use oauth2::helpers::deserialize_optional_string_or_vec_string;
use oauth2::url::Url;
use oauth2::{
    AccessToken, AuthType, AuthUrl, AuthorizationCode, ClientId, CsrfToken, EndpointNotSet, EndpointSet, ErrorResponseType, HttpRequest,
    HttpResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, RequestTokenError, RevocationUrl, Scope,
    StandardErrorResponse, StandardRevocableToken, SyncHttpClient, TokenResponse, TokenUrl,
};
use serde::Deserialize;
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
/// The largest SSO response body.
const MAX_BODY: u64 = 64 * 1024;
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
    /// SSO refused the code or the refresh token. The character must log in again.
    Rejected(String),
    /// A timeout, a network error, a 5xx status or a bad response.
    Offline(String),
    /// The token is not valid: a wrong issuer, audience or subject, or an expired token.
    BadToken(String),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginError::Denied(e) => write!(f, "EVE SSO did not give access ({e}). Start the login again."),
            LoginError::StateMismatch => f.write_str("This URL is not from the current login. Use the URL from the newest login tab."),
            LoginError::NoCode => f.write_str("The URL has no login code. Paste the full URL from the address bar."),
            LoginError::Rejected(e) => write!(f, "EVE SSO refused the login ({e}). Log in again."),
            LoginError::Offline(e) => write!(f, "EVE SSO is not reachable: {e}"),
            LoginError::BadToken(e) => write!(f, "EVE SSO sent a token that is not valid: {e}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// The token requests and the revoke request have different error response types.
impl<T: ErrorResponseType + fmt::Display + 'static> From<RequestTokenError<ureq::Error, StandardErrorResponse<T>>> for LoginError {
    fn from(e: RequestTokenError<ureq::Error, StandardErrorResponse<T>>) -> Self {
        type E<T> = RequestTokenError<ureq::Error, StandardErrorResponse<T>>;
        match e {
            E::ServerResponse(r) => LoginError::Rejected(r.error().to_string()),
            E::Request(e) => LoginError::Offline(e.to_string()),
            E::Parse(e, _) => LoginError::Offline(format!("bad response: {e}")),
            E::Other(e) => LoginError::Offline(e),
        }
    }
}

/// The HTTP client for `oauth2`, on the ureq agent of the router.
struct Http(ureq::Agent);

impl Http {
    fn new() -> Http {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("eve-router/", env!("CARGO_PKG_VERSION")))
            // `oauth2` reads the error body of a 4xx response.
            .http_status_as_error(false)
            // A redirect from the token endpoint is not followed.
            .max_redirects(0)
            .build();
        Http(config.new_agent())
    }
}

impl SyncHttpClient for Http {
    type Error = ureq::Error;

    fn call(&self, request: HttpRequest) -> Result<HttpResponse, ureq::Error> {
        let mut response = self.0.run(request)?;
        let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec()?;
        let (parts, _) = response.into_parts();
        Ok(HttpResponse::from_parts(parts, body))
    }
}

type EveClient = BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet, EndpointSet>;

/// The EVE SSO client of one developer app.
pub struct Sso {
    client: EveClient,
    http: Http,
}

impl Sso {
    pub fn new(client_id: &str) -> Sso {
        Self::with_urls(client_id, AUTHORIZE_URL, TOKEN_URL, REVOKE_URL, &redirect_uri())
    }

    /// The SSO URLs are constants, so a parse error is a bug.
    fn with_urls(client_id: &str, authorize: &str, token: &str, revoke: &str, redirect: &str) -> Sso {
        let client = BasicClient::new(ClientId::new(client_id.to_owned()))
            .set_auth_uri(AuthUrl::new(authorize.to_owned()).expect("authorize URL"))
            .set_token_uri(TokenUrl::new(token.to_owned()).expect("token URL"))
            .set_revocation_url(RevocationUrl::new(revoke.to_owned()).expect("revoke URL"))
            .set_redirect_uri(RedirectUrl::new(redirect.to_owned()).expect("redirect URL"))
            // A native app has no secret. The client ID goes in the form body.
            .set_auth_type(AuthType::RequestBody);
        Sso { client, http: Http::new() }
    }

    /// Make the PKCE pair and the state, and listen on the callback port. A listener failure
    /// does not stop the login: the user can paste the redirected URL.
    pub fn start_login(&self) -> Login {
        self.start_login_on(CALLBACK_PORT)
    }

    fn start_login_on(&self, port: u16) -> Login {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state) = self
            .client
            .authorize_url(CsrfToken::new_random)
            .add_scopes(SCOPES.iter().map(|s| Scope::new((*s).to_owned())))
            .set_pkce_challenge(challenge)
            .url();
        let listener = Listener::bind(port, state.secret().clone()).map_err(|e| match e.kind() {
            io::ErrorKind::AddrInUse => format!("Port {port} is in use. Paste the redirected URL instead."),
            _ => format!("Cannot listen on port {port}: {e}. Paste the redirected URL instead."),
        });
        Login { url: url.into(), state, verifier, listener }
    }

    /// Send the code and the verifier, and get the tokens.
    pub fn exchange(&self, code: AuthorizationCode, verifier: PkceCodeVerifier) -> Result<Tokens, LoginError> {
        let response = self.client.exchange_code(code).set_pkce_verifier(verifier).request(&self.http)?;
        self.tokens(&response, None, now())
    }

    /// Get a new access token with the refresh token. If SSO gives no new refresh token, the old
    /// one stays valid.
    pub fn refresh(&self, refresh: &RefreshToken) -> Result<Tokens, LoginError> {
        let response = self.client.exchange_refresh_token(refresh).request(&self.http)?;
        self.tokens(&response, Some(refresh), now())
    }

    /// Revoke a refresh token at SSO. After this, the token gives no access.
    pub fn revoke(&self, refresh: &RefreshToken) -> Result<(), LoginError> {
        let token = StandardRevocableToken::RefreshToken(refresh.clone());
        let request = self.client.revoke_token(token).map_err(|e| LoginError::Offline(e.to_string()))?;
        request.request(&self.http).map_err(LoginError::from)
    }

    fn tokens(&self, response: &impl TokenResponse, old_refresh: Option<&RefreshToken>, now: u64) -> Result<Tokens, LoginError> {
        let access = response.access_token();
        let character = read_claims(access, self.client.client_id(), now)?;
        let refresh = match (response.refresh_token(), old_refresh) {
            (Some(token), _) => token.clone(),
            (None, Some(old)) => old.clone(),
            (None, None) => return Err(LoginError::BadToken("no refresh token".into())),
        };
        let expires_in = response.expires_in().map_or(0, |d| d.as_secs());
        Ok(Tokens { character, access: access.clone(), refresh, expires_at: now + expires_in })
    }
}

/// Read the code from a callback URL, a callback request target or a bare query string.
/// The URL must have the `state` of this login.
pub fn parse_callback(text: &str, state: &str) -> Result<AuthorizationCode, LoginError> {
    let text = text.trim();
    let query = match text.split_once('?') {
        Some((_, query)) => query,
        None if text.contains('=') => text,
        None => return Err(LoginError::NoCode),
    };
    // Any base gives the same query pairs.
    let url = Url::parse(&format!("http://localhost/?{query}")).map_err(|_| LoginError::NoCode)?;
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    if param("state").as_deref() != Some(state) {
        return Err(LoginError::StateMismatch);
    }
    if let Some(error) = param("error") {
        return Err(LoginError::Denied(error));
    }
    param("code").filter(|c| !c.is_empty()).map(AuthorizationCode::new).ok_or(LoginError::NoCode)
}

/// The loopback listener. A thread accepts connections until a request gives a code or an
/// error, or until the listener drops. A request with a wrong `state` does not stop it.
pub struct Listener {
    port: u16,
    rx: Receiver<Result<AuthorizationCode, LoginError>>,
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
    pub fn try_recv(&self) -> Option<Result<AuthorizationCode, LoginError>> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Answer one HTTP request. `None` means "keep listening".
fn handle(mut stream: TcpStream, state: &str) -> Option<Result<AuthorizationCode, LoginError>> {
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
    state: CsrfToken,
    verifier: PkceCodeVerifier,
    listener: Result<Listener, String>,
}

impl Login {
    /// The reason the listener did not start, if it did not.
    pub fn listen_error(&self) -> Option<&str> {
        self.listener.as_ref().err().map(String::as_str)
    }

    /// The code from the listener, when the browser sent it.
    pub fn poll(&self) -> Option<Result<AuthorizationCode, LoginError>> {
        self.listener.as_ref().ok()?.try_recv()
    }

    /// The code from a pasted URL.
    pub fn paste(&self, text: &str) -> Result<AuthorizationCode, LoginError> {
        parse_callback(text, self.state.secret())
    }

    /// The PKCE verifier, for `Sso::exchange`.
    pub fn verifier(&self) -> PkceCodeVerifier {
        PkceCodeVerifier::new(self.verifier.secret().clone())
    }
}

/// The tokens of one character.
#[derive(Debug)]
pub struct Tokens {
    pub character: Character,
    pub access: AccessToken,
    /// SSO can give a new refresh token at each refresh. Store the new one before the next use.
    pub refresh: RefreshToken,
    /// The expiry of the access token, in Unix seconds.
    pub expires_at: u64,
}

/// The claims of an EVE access token (a JWT). `oauth2` has no JWT type, and its introspection
/// response needs an `active` field and a `scope` string, which an EVE token does not have.
#[derive(Deserialize)]
struct Claims {
    iss: String,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec_string")]
    aud: Option<Vec<String>>,
    exp: u64,
    sub: String,
    name: String,
    /// One scope comes as a string, and more scopes as a list.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec_string")]
    scp: Option<Vec<String>>,
}

/// Read the character from the access token, and check the issuer, the audience, the expiry
/// and the subject. See the module comment about the signature.
pub fn read_claims(token: &AccessToken, client_id: &ClientId, now: u64) -> Result<Character, LoginError> {
    let bad = |text: &str| LoginError::BadToken(text.to_owned());
    let mut parts = token.secret().split('.');
    let (Some(_), Some(payload), Some(_), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(bad("not a JWT"));
    };
    let json = URL_SAFE_NO_PAD_INDIFFERENT.decode(payload).map_err(|_| bad("bad base64"))?;
    let claims: Claims = serde_json::from_slice(&json).map_err(|e| LoginError::BadToken(e.to_string()))?;
    if !ISSUERS.contains(&claims.iss.as_str()) {
        return Err(LoginError::BadToken(format!("wrong issuer {}", claims.iss)));
    }
    let audience = claims.aud.unwrap_or_default();
    if !audience.iter().any(|a| a == client_id.as_str()) || !audience.iter().any(|a| a == AUDIENCE) {
        return Err(bad("wrong audience"));
    }
    if claims.exp <= now {
        return Err(bad("expired"));
    }
    let id = claims.sub.strip_prefix(SUBJECT_PREFIX).and_then(|id| id.parse().ok()).ok_or_else(|| bad("bad subject"))?;
    Ok(Character { id, name: claims.name, scopes: claims.scp.unwrap_or_default().into_iter().map(Scope::new).collect() })
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    const CLIENT: &str = "client-123";
    const NOW: u64 = 1_800_000_000;

    /// A JWT with these claims. The header and the signature are not read.
    fn jwt(claims: serde_json::Value) -> String {
        let part = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.c2ln", part(&json!({"alg": "RS256", "kid": "JWT-Signature-Key"})), part(&claims))
    }

    fn claims(exp: u64) -> serde_json::Value {
        json!({
            "iss": "https://login.eveonline.com",
            "aud": [CLIENT, "EVE Online"],
            "exp": exp,
            "sub": "CHARACTER:EVE:2112625428",
            "name": "Alice Ander",
            "scp": ["esi-ui.write_waypoint.v1", "esi-location.read_location.v1"],
        })
    }

    fn claims_of(token: String, now: u64) -> Result<Character, LoginError> {
        read_claims(&AccessToken::new(token), &ClientId::new(CLIENT.into()), now)
    }

    /// The code text of a parse result. `AuthorizationCode` has no `PartialEq`.
    fn text(result: Result<AuthorizationCode, LoginError>) -> Result<String, LoginError> {
        result.map(|code| code.secret().clone())
    }

    /// An `Sso` whose token and revoke URLs are a test server.
    fn sso_at(url: &str) -> Sso {
        Sso::with_urls(CLIENT, AUTHORIZE_URL, url, url, &redirect_uri())
    }

    #[test]
    fn authorize_url_has_every_parameter() {
        let login = Sso::new(CLIENT).start_login_on(0);
        let url = Url::parse(&login.url).unwrap();
        assert_eq!(url.as_str().split('?').next(), Some(AUTHORIZE_URL));
        let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        assert_eq!(param("response_type").as_deref(), Some("code"));
        assert_eq!(param("client_id").as_deref(), Some(CLIENT));
        assert_eq!(param("redirect_uri").as_deref(), Some("http://localhost:21404/callback"));
        assert_eq!(param("scope"), Some(SCOPES.join(" ")));
        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(param("state").as_deref(), Some(login.state.secret().as_str()));
        assert!(param("code_challenge").is_some_and(|c| c.len() == 43));
    }

    #[test]
    fn each_login_has_a_new_state_and_verifier() {
        let sso = Sso::new(CLIENT);
        let (a, b) = (sso.start_login_on(0), sso.start_login_on(0));
        assert_ne!(a.state.secret(), b.state.secret());
        assert_ne!(a.verifier().secret(), b.verifier().secret());
    }

    #[test]
    fn parse_pasted_urls() {
        let full = "http://localhost:21404/callback?code=abc%2Bdef&state=s1";
        assert_eq!(text(parse_callback(full, "s1")), Ok("abc+def".into()));
        assert_eq!(text(parse_callback(&format!("  {full}#frag \n"), "s1")), Ok("abc+def".into()));
        assert_eq!(text(parse_callback("code=xyz&state=s1", "s1")), Ok("xyz".into()));
        assert_eq!(text(parse_callback(full, "other")), Err(LoginError::StateMismatch));
        assert_eq!(text(parse_callback("http://localhost:21404/callback?state=s1", "s1")), Err(LoginError::NoCode));
        assert_eq!(text(parse_callback("hello", "s1")), Err(LoginError::NoCode));
        assert_eq!(text(parse_callback("/callback?error=access_denied&state=s1", "s1")), Err(LoginError::Denied("access_denied".into())));
        // An error with no matching state is not from this login.
        assert_eq!(text(parse_callback("/callback?error=access_denied", "s1")), Err(LoginError::StateMismatch));
    }

    #[test]
    fn exchange_gives_the_tokens() {
        let body = json!({"access_token": jwt(claims(u64::MAX / 2)), "expires_in": 1199, "refresh_token": "r1", "token_type": "Bearer"});
        let (url, rx) = crate::test_support::serve("200 OK", &body.to_string(), Duration::ZERO);
        let tokens = sso_at(&url).exchange(AuthorizationCode::new("c0de".into()), PkceCodeVerifier::new("v".repeat(43))).unwrap();
        assert_eq!(tokens.character.id, 2112625428);
        assert_eq!(tokens.refresh.secret(), "r1");
        assert!(rx.recv().unwrap().starts_with("POST / HTTP/1.1"));
    }

    #[test]
    fn refresh_keeps_the_old_token_if_sso_gives_none() {
        let body = json!({"access_token": jwt(claims(u64::MAX / 2)), "expires_in": 1199, "token_type": "Bearer"});
        let (url, _rx) = crate::test_support::serve("200 OK", &body.to_string(), Duration::ZERO);
        let tokens = sso_at(&url).refresh(&RefreshToken::new("old".into())).unwrap();
        assert_eq!(tokens.refresh.secret(), "old");
    }

    #[test]
    fn refused_refresh_token_is_rejected() {
        let body = r#"{"error":"invalid_grant","error_description":"Invalid refresh token."}"#;
        let (url, _rx) = crate::test_support::serve("400 Bad Request", body, Duration::ZERO);
        assert_eq!(sso_at(&url).refresh(&RefreshToken::new("old".into())).unwrap_err(), LoginError::Rejected("invalid_grant".into()));
    }

    /// `oauth2` sends a revoke only to an HTTPS URL. The test server has no TLS, so the test
    /// examines that guard.
    #[test]
    fn revoke_needs_https() {
        let err = sso_at("http://127.0.0.1:9/revoke").revoke(&RefreshToken::new("old".into())).unwrap_err();
        assert!(matches!(&err, LoginError::Offline(e) if e.contains("HTTPS")), "{err:?}");
    }

    #[test]
    fn server_error_is_offline() {
        let (url, _rx) = crate::test_support::serve("503 Service Unavailable", "down", Duration::ZERO);
        assert!(matches!(sso_at(&url).refresh(&RefreshToken::new("old".into())), Err(LoginError::Offline(_))));
    }

    #[test]
    fn claims_give_the_character() {
        let character = claims_of(jwt(claims(NOW + 1200)), NOW).unwrap();
        assert_eq!(character.id, 2112625428);
        assert_eq!(character.name, "Alice Ander");
        assert_eq!(character.scopes, [Scope::new("esi-ui.write_waypoint.v1".into()), Scope::new("esi-location.read_location.v1".into())]);
    }

    #[test]
    fn one_scope_as_a_string() {
        let mut c = claims(NOW + 1200);
        c["scp"] = json!("esi-ui.write_waypoint.v1");
        assert_eq!(claims_of(jwt(c), NOW).unwrap().scopes, [Scope::new("esi-ui.write_waypoint.v1".into())]);
    }

    #[test]
    fn bad_claims_are_refused() {
        let check = |key: &str, value: serde_json::Value| {
            let mut c = claims(NOW + 1200);
            c[key] = value;
            assert!(matches!(claims_of(jwt(c), NOW), Err(LoginError::BadToken(_))), "{key}");
        };
        check("iss", json!("https://evil.example"));
        check("aud", json!(["other-client", "EVE Online"]));
        check("aud", json!(CLIENT));
        check("exp", json!(NOW));
        check("sub", json!("CORPORATION:EVE:1"));
        assert!(matches!(claims_of("a.b".into(), NOW), Err(LoginError::BadToken(_))));
        assert!(matches!(claims_of("a.!!!.c".into(), NOW), Err(LoginError::BadToken(_))));
    }

    /// Send one request to the listener and return the response text.
    fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn wait(listener: &Listener) -> Result<AuthorizationCode, LoginError> {
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
        assert_eq!(text(wait(&listener)), Ok("good".into()));
    }

    #[test]
    fn listener_reports_a_denied_login() {
        let listener = Listener::bind(0, "s1".into()).unwrap();
        assert!(get(listener.port(), "/callback?error=access_denied&state=s1").starts_with("HTTP/1.1 400"));
        assert_eq!(text(wait(&listener)), Err(LoginError::Denied("access_denied".into())));
    }

    #[test]
    fn busy_port_gives_the_paste_path() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = busy.local_addr().unwrap().port();
        let login = Sso::new(CLIENT).start_login_on(port);
        assert_eq!(login.listen_error(), Some(format!("Port {port} is in use. Paste the redirected URL instead.").as_str()));
        assert!(login.poll().is_none());
        let state = login.state.secret();
        assert_eq!(text(login.paste(&format!("http://localhost:{port}/callback?code=c0de&state={state}"))), Ok("c0de".into()));
    }
}
