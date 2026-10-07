//! Helpers for the tests.

use crate::ansiblex::{BridgeRules, find_hull};
use crate::route::Mode;
use crate::settings::{HullSource, Settings};
use crate::sources::nexum;
use crate::universe::Universe;
use crate::wormhole::{self, SourceId, Wormhole};
use crate::{overlay, sde, wormhole_types};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// The repository `sde/` directory. The search goes up from this crate to the directory
/// that holds `Cargo.lock`, so the tests do not depend on the working directory.
pub fn sde_dir() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir.ancestors().find(|d| d.join("Cargo.lock").is_file()).expect("no Cargo.lock above the crate");
    root.join("sde")
}

/// A file in `tests/fixtures/` of this crate.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

/// The real SDE, loaded one time for all tests.
pub fn universe() -> &'static Universe {
    static UNI: OnceLock<Universe> = OnceLock::new();
    UNI.get_or_init(|| Universe::from_sde(crate::sde::load(&sde_dir()).unwrap()))
}

/// 2026-10-05T12:00:00Z, the time of the Nexum fixture.
pub const FIXTURE_TIME: u64 = 1_791_201_600;

/// The SDE with the bridges and the Nexum wormholes of the fixtures, at `FIXTURE_TIME`.
pub fn overlay_universe() -> Universe {
    let mut uni = Universe::from_sde(sde::load(&sde_dir()).unwrap());
    overlay::load_bridges(&mut uni, &fixture("ansiblex.txt")).unwrap();
    let text = std::fs::read_to_string(fixture("nexum-api.json")).unwrap();
    let map = nexum::parse_map(&text).unwrap();
    let types = wormhole_types::load(&sde_dir()).unwrap();
    let (data, _) = nexum::convert(&map, &Default::default(), |id| uni.by_id.contains_key(&id), &types, FIXTURE_TIME);
    uni.add_wormholes(&wormhole::merge(&[data], FIXTURE_TIME));
    uni
}

/// Shortest routes, the top 3, all overlays on, the capital JK-Q77, and no favourites.
pub fn settings(uni: &Universe, hull: Option<&str>) -> Settings {
    Settings {
        mode: Mode::Shortest,
        optimize: false,
        top: 3,
        wormholes: true,
        hubs: Default::default(),
        bridges: true,
        rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: hull.map(|h| find_hull(h).unwrap()), max_cap: None },
        hull_source: HullSource::Manual,
        min_life: 0,
        costs: Default::default(),
        favourites: Vec::new(),
    }
}

/// A wormhole between two systems, with no known values.
pub fn hole(a: u32, b: u32) -> Wormhole {
    Wormhole::new(a, b, None, None, SourceId::Nexum)
}

/// Compare `text` with the snapshot file `tests/snapshots/<name>.txt` of the calling crate.
/// Set `UPDATE_SNAPSHOTS=1` to write the file instead.
#[macro_export]
macro_rules! assert_snapshot {
    ($name:expr, $text:expr) => {
        $crate::test_support::check_snapshot(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), $name, &$text)
    };
}

pub fn check_snapshot(manifest_dir: &Path, name: &str, text: &str) {
    let path = manifest_dir.join("tests").join("snapshots").join(format!("{name}.txt"));
    if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}. Run with UPDATE_SNAPSHOTS=1.", path.display()));
    // Git can change the line ends of the snapshot file on Windows.
    assert_eq!(text, expected.replace("\r\n", "\n"), "snapshot {} changed", path.display());
}

/// A test server on 127.0.0.1, at a free port, and its base URL.
fn test_server() -> (tiny_http::Server, String) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    (server, url)
}

/// The request as HTTP text: the request line, the headers and the body.
fn request_text(request: &mut tiny_http::Request) -> String {
    let mut text = format!("{} {} HTTP/{}\r\n", request.method(), request.url(), request.http_version());
    for header in request.headers() {
        text.push_str(&format!("{}: {}\r\n", header.field, header.value));
    }
    text.push_str("\r\n");
    let _ = request.as_reader().read_to_string(&mut text);
    text
}

/// Send a JSON response with extra headers. `status` is for example "404 Not Found".
fn respond_json(request: tiny_http::Request, status: &str, body: &str, headers: &[(String, String)]) {
    let code: u16 = status.split(' ').next().and_then(|c| c.parse().ok()).expect("status code");
    let header = |name: &str, value: &str| tiny_http::Header::from_bytes(name, value).unwrap();
    let mut response =
        tiny_http::Response::from_string(body).with_status_code(code).with_header(header("Content-Type", "application/json"));
    for (name, value) in headers {
        response = response.with_header(header(name, value));
    }
    let _ = request.respond(response);
}

/// Serve one canned HTTP response on 127.0.0.1. Return the base URL, and a channel that
/// gives the request text.
pub fn serve(status: &str, body: &str, delay: Duration) -> (String, Receiver<String>) {
    serve_full(status, body, &[], delay)
}

/// `serve` with extra response headers, and no delay.
pub fn serve_with_headers(status: &str, body: &str, headers: &[(&str, &str)]) -> (String, Receiver<String>) {
    serve_full(status, body, headers, Duration::ZERO)
}

fn serve_full(status: &str, body: &str, headers: &[(&str, &str)], delay: Duration) -> (String, Receiver<String>) {
    let (server, url) = test_server();
    let (status, body) = (status.to_owned(), body.to_owned());
    let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok(mut request) = server.recv() else { return };
        let _ = tx.send(request_text(&mut request));
        std::thread::sleep(delay);
        respond_json(request, &status, &body, &headers);
    });
    (url, rx)
}

/// Serve canned 200 responses on 127.0.0.1, one for each path, until the test ends.
/// A path that is not in `routes` gets a 500. The channel gives each request text.
pub fn serve_routes(routes: HashMap<String, String>) -> (String, Receiver<String>) {
    let (server, url) = test_server();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let (status, body) = match routes.get(request.url()) {
                Some(body) => ("200 OK", body.as_str()),
                None => ("500 Internal Server Error", "{}"),
            };
            let _ = tx.send(request_text(&mut request));
            respond_json(request, status, body, &[]);
        }
    });
    (url, rx)
}
