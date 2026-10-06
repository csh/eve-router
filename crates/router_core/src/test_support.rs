//! Helpers for the tests.

use crate::ansiblex::{BridgeRules, find_hull};
use crate::route::Mode;
use crate::settings::Settings;
use crate::sources::nexum;
use crate::universe::Universe;
use crate::wormhole::{self, SourceId, Wormhole};
use crate::{overlay, sde, wormhole_types};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
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
        min_life: 0,
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

/// Serve one canned HTTP response on 127.0.0.1. Return the base URL, and a channel that
/// gives the request text.
pub fn serve(status: &str, body: &str, delay: Duration) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
        std::thread::sleep(delay);
        let _ = stream.write_all(response.as_bytes());
    });
    (url, rx)
}

/// Serve canned 200 responses on 127.0.0.1, one for each path, until the test ends.
/// A path that is not in `routes` gets a 500. The channel gives each request text.
pub fn serve_routes(routes: HashMap<String, String>) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = request.split(' ').nth(1).unwrap_or("").to_string();
            let (status, body) = match routes.get(&path) {
                Some(body) => ("200 OK", body.as_str()),
                None => ("500 Internal Server Error", "{}"),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tx.send(request);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (url, rx)
}
