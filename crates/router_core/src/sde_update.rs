//! Keep the local SDE files up to date.
//!
//! Source: https://developers.eveonline.com/docs/services/static-data/#automation
//! `latest.jsonl` gives the latest build number. The router then reads the zip index
//! from the end of the build zip, and fetches only the entries it needs with HTTP
//! range requests. This downloads about 1.8 MB instead of the full 99 MB zip.

use crate::ships;
use flate2::Crc;
use flate2::read::DeflateDecoder;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// The SDE files that the router needs. `_sde.jsonl` comes last, so an interrupted
/// update leaves the old build number, and the next start tries again.
pub const FILES: [&str; 4] = ["mapSolarSystems.jsonl", "mapStargates.jsonl", "mapRegions.jsonl", "_sde.jsonl"];

/// Set this variable to `1` to skip the check and use the local SDE as it is.
pub const SKIP_ENV: &str = "EVE_ROUTER_SKIP_SDE_CHECK";

const BASE_URL: &str = "https://developers.eveonline.com/static-data/tranquility";

pub enum Outcome {
    Skipped,
    UpToDate,
    Updated {
        from: Option<u32>,
        to: u32,
        bytes: u64,
    },
    /// The check failed, but a local SDE exists. The router uses it.
    Offline {
        local: u32,
        error: String,
    },
}

impl Outcome {
    /// The status line for the user, if the check did more than confirm the local build.
    pub fn message(&self, dir: &Path) -> Option<String> {
        match self {
            Outcome::Updated { from, to, bytes } => {
                let mb = *bytes as f64 / 1e6;
                Some(match from {
                    // Same build: only a file was missing, for example wormholes.json from an older version.
                    Some(b) if b == to => format!("Completed SDE build {to} ({mb:.1} MB) in {}", dir.display()),
                    Some(b) => format!("Updated the SDE from build {b} to build {to} ({mb:.1} MB) in {}", dir.display()),
                    None => format!("Downloaded SDE build {to} ({mb:.1} MB) to {}", dir.display()),
                })
            }
            Outcome::Offline { local, error } => Some(format!("Cannot check for a new SDE. Using local build {local}. Cause: {error}")),
            Outcome::Skipped | Outcome::UpToDate => None,
        }
    }
}

/// Make sure that `dir` holds the latest SDE files.
pub fn ensure(dir: &Path) -> Result<Outcome, String> {
    if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
        return Ok(Outcome::Skipped);
    }
    let local = local_build(dir);
    let agent = agent();
    let latest = match latest_build(&agent) {
        Ok(build) => build,
        Err(error) => {
            return match local {
                Some(local) => Ok(Outcome::Offline { local, error }),
                None => Err(format!("Cannot download the SDE to {}: {error}", dir.display())),
            };
        }
    };
    if local == Some(latest) {
        return Ok(Outcome::UpToDate);
    }
    let source = HttpSource { agent, url: format!("{BASE_URL}/eve-online-static-data-{latest}-jsonl.zip") };
    install(&source, dir, latest)
}

/// Download build `latest` into `dir`. `from` is the local build before the download.
fn install(src: &dyn RangeSource, dir: &Path, latest: u32) -> Result<Outcome, String> {
    // The local build can exist without ships.json or wormholes.json, from a version before these derived tables.
    let from = crate::sde::build_number(dir);
    let bytes = download(src, dir)?;
    Ok(Outcome::Updated { from, to: latest, bytes })
}

/// The build number of the local SDE, if all files are present.
fn local_build(dir: &Path) -> Option<u32> {
    let derived = [&crate::ships::FILE, &crate::wormhole_types::FILE];
    let present = FILES.iter().chain(derived).all(|f| dir.join(f).is_file());
    if present { crate::sde::build_number(dir) } else { None }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(concat!("eve-router/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn latest_build(agent: &ureq::Agent) -> Result<u32, String> {
    let url = format!("{BASE_URL}/latest.jsonl");
    let text = agent.get(&url).call().and_then(|mut r| r.body_mut().read_to_string()).map_err(|e| format!("{url}: {e}"))?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v["_key"] == "sde")
        .and_then(|v| v["buildNumber"].as_u64())
        .and_then(|b| u32::try_from(b).ok())
        .ok_or_else(|| format!("{url}: no \"sde\" record with a build number"))
}

/// A file that gives byte ranges: a remote zip, or bytes in memory for tests.
pub(crate) trait RangeSource: Sync {
    fn len(&self) -> Result<u64, String>;
    fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, String>;
}

struct HttpSource {
    agent: ureq::Agent,
    url: String,
}

/// The HTTP `Range` value for `len` bytes from `start`. `None` for zero bytes, which has no valid range.
fn range_header(start: u64, len: u64) -> Option<String> {
    let last = start + len.checked_sub(1)?;
    Some(format!("bytes={start}-{last}"))
}

impl RangeSource for HttpSource {
    fn len(&self) -> Result<u64, String> {
        let resp = self.agent.head(&self.url).call().map_err(|e| format!("{}: {e}", self.url))?;
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()?.parse().ok())
            .ok_or_else(|| format!("{}: no Content-Length", self.url))
    }

    fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, String> {
        let Some(range) = range_header(start, len) else { return Ok(Vec::new()) };
        let mut resp = self.agent.get(&self.url).header("Range", &range).call().map_err(|e| format!("{}: {e}", self.url))?;
        // 206 Partial Content. A 200 is the full 99 MB zip, so stop.
        if resp.status().as_u16() != 206 {
            return Err(format!("{}: the server ignored the range request ({})", self.url, resp.status()));
        }
        // The ureq limit fails when the body reaches it, so allow one more byte. The check below
        // still catches a wrong length.
        let body = resp.body_mut().with_config().limit(len + 1).read_to_vec().map_err(|e| e.to_string())?;
        if body.len() as u64 != len {
            return Err(format!("{}: asked for {len} bytes, got {}", self.url, body.len()));
        }
        Ok(body)
    }
}

/// One file in the zip central directory.
struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
/// The largest buffer that `extract` reserves before it decompresses an entry.
const MAX_RESERVE: usize = 256 * 1024 * 1024;

/// Read the central directory. The end record is in the last 64 KiB + 22 bytes of the zip.
fn read_directory(src: &dyn RangeSource) -> Result<Vec<Entry>, String> {
    let total = src.len()?;
    let tail_len = total.min(65_557);
    let tail = src.read(total - tail_len, tail_len)?;
    let end = tail.windows(4).rposition(|w| w == EOCD_SIG).ok_or("no zip end record")?;
    if tail.len() < end + 22 {
        return Err("short zip end record".into());
    }
    let (size, offset) = (u32_at(&tail, end + 12), u32_at(&tail, end + 16));
    if offset == u32::MAX {
        return Err("zip64 archives are not supported".into());
    }
    let dir = src.read(offset.into(), size.into())?;
    let mut entries = Vec::new();
    let mut i = 0;
    while i + 46 <= dir.len() && u32_at(&dir, i) == CENTRAL_SIG {
        let name_len = u16_at(&dir, i + 28) as usize;
        let extra_len = u16_at(&dir, i + 30) as usize;
        let comment_len = u16_at(&dir, i + 32) as usize;
        let name = dir.get(i + 46..i + 46 + name_len).ok_or("short zip directory")?;
        entries.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method: u16_at(&dir, i + 10),
            crc: u32_at(&dir, i + 16),
            compressed: u32_at(&dir, i + 20).into(),
            size: u32_at(&dir, i + 24).into(),
            offset: u32_at(&dir, i + 42).into(),
        });
        i += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// Fetch and decompress one entry. Make sure that its size and CRC-32 are correct.
fn extract(src: &dyn RangeSource, entry: &Entry) -> Result<Vec<u8>, String> {
    let err = |msg: &str| format!("{}: {msg}", entry.name);
    let header = src.read(entry.offset, 30)?;
    if u32_at(&header, 0) != LOCAL_SIG {
        return Err(err("bad local header"));
    }
    // The local extra field can differ from the central one, so read its length here.
    let data_start = entry.offset + 30 + u64::from(u16_at(&header, 26)) + u64::from(u16_at(&header, 28));
    let data = src.read(data_start, entry.compressed)?;
    let out = match entry.method {
        0 => data,
        8 => inflate(&data, entry.size).map_err(|e| err(&e.to_string()))?,
        m => return Err(err(&format!("compression method {m} is not supported"))),
    };
    let mut crc = Crc::new();
    crc.update(&out);
    if out.len() as u64 != entry.size || crc.sum() != entry.crc {
        return Err(err("size or CRC-32 does not match"));
    }
    Ok(out)
}

/// Decompress deflate data. The size comes from the zip, so the function trusts it only as a
/// limit. It stops at `size + 1` bytes, and reserves at most `MAX_RESERVE`. A bad entry then
/// fails the size check in `extract`, and does not fill the memory first.
fn inflate(data: &[u8], size: u64) -> std::io::Result<Vec<u8>> {
    let capacity = usize::try_from(size).unwrap_or(usize::MAX).min(MAX_RESERVE);
    let mut out = Vec::with_capacity(capacity);
    DeflateDecoder::new(data).take(size.saturating_add(1)).read_to_end(&mut out)?;
    Ok(out)
}

/// Fetch `FILES` from the zip into `dir`, and make `ships.json` from the ship source files.
/// The source files are not kept. Return the number of compressed bytes.
pub(crate) fn download(src: &dyn RangeSource, dir: &Path) -> Result<u64, String> {
    let entries = read_directory(src)?;
    let names: Vec<&str> = FILES.iter().chain(&ships::SOURCES).copied().collect();
    let wanted: Vec<&Entry> = names
        .iter()
        .map(|&f| entries.iter().find(|e| e.name == f).ok_or(format!("{f} is not in the SDE zip")))
        .collect::<Result<_, _>>()?;
    // The entries are independent, so fetch them at the same time.
    let mut files: HashMap<&str, Vec<u8>> =
        wanted.par_iter().map(|e| extract(src, e).map(|data| (e.name.as_str(), data))).collect::<Result<_, _>>()?;

    let build = files.get("_sde.jsonl").and_then(|d| build_from(d));
    let source = |name| files.get(name).map(Vec::as_slice).unwrap_or_default();
    let sources = ships::parse(source("types.jsonl"), source("typeDogma.jsonl"), source("groups.jsonl"))?;
    let ship_data = ships::derive(build, &sources)?;
    let wormhole_data = crate::wormhole_types::derive(build, &sources);
    drop(sources);

    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    ships::save(dir, &ship_data)?;
    crate::wormhole_types::save(dir, &wormhole_data)?;
    // FILES ends with _sde.jsonl, so the build number changes last.
    for name in FILES {
        let data = files.remove(name).unwrap_or_default();
        // Write a temporary file, then rename it, so a reader never sees half a file.
        let path = dir.join(name);
        let tmp = path.with_extension("jsonl.part");
        fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(wanted.iter().map(|e| e.compressed).sum())
}

/// The build number in the `_sde.jsonl` data.
fn build_from(data: &[u8]) -> Option<u32> {
    let line = std::str::from_utf8(data).ok()?.lines().next()?;
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value["buildNumber"].as_u64()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip in memory.
    impl RangeSource for Vec<u8> {
        fn len(&self) -> Result<u64, String> {
            Ok(Vec::len(self) as u64)
        }

        fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, String> {
            self.get(start as usize..(start + len) as usize).map(<[u8]>::to_vec).ok_or("range out of bounds".into())
        }
    }

    /// A small zip with the 7 files, an extra file, and both stored and deflated entries.
    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/sde-small.zip");

    #[test]
    fn downloads_only_wanted_files() {
        let dir = std::env::temp_dir().join("eve-router-test-sde-update");
        let _ = fs::remove_dir_all(&dir);
        download(&FIXTURE.to_vec(), &dir).unwrap();
        let mut names: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(
            names,
            ["_sde.jsonl", "mapRegions.jsonl", "mapSolarSystems.jsonl", "mapStargates.jsonl", "ships.json", "wormholes.json"]
        );
        // The fixture has no wormhole types, so the table is empty.
        assert!(crate::wormhole_types::load(&dir).unwrap().types.is_empty());
        let ships = ships::load(&dir).unwrap();
        assert_eq!(ships.build, Some(123));
        assert_eq!(ships.ansiblex.capacitor_gj, 1_250_000.0);
        let cost = |name: &str| ships.ships.iter().find(|s| s.name == name).unwrap().bridge_cost_gj;
        assert_eq!(cost("Sin"), Some(18000.0));
        assert_eq!(cost("Avatar"), None);
        // Category 22 is not a ship category, so the drone is not in the table.
        assert!(ships.ships.iter().all(|s| s.name != "Drone"));
        assert_eq!(crate::sde::build_number(&dir), Some(123));
        assert_eq!(local_build(&dir), Some(123));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Make `ships.json` and `wormholes.json` in the repository `sde/` directory again, from the
    /// SDE build in `sde/_sde.jsonl`. It needs the network, so it runs only on request:
    /// `cargo test regenerate_repo_sde -- --ignored`
    #[test]
    #[ignore]
    fn regenerate_repo_sde() {
        let dir = &crate::test_support::sde_dir();
        let build = crate::sde::build_number(dir).unwrap();
        let source = HttpSource { agent: agent(), url: format!("{BASE_URL}/eve-online-static-data-{build}-jsonl.zip") };
        download(&source, dir).unwrap();
    }

    #[test]
    fn install_reports_the_build_before_the_download() {
        let dir = std::env::temp_dir().join("eve-router-test-sde-install");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("_sde.jsonl"), "{\"_key\": \"sde\", \"buildNumber\": 100}\n").unwrap();
        // The fixture holds build 123.
        let Outcome::Updated { from, to, .. } = install(&FIXTURE.to_vec(), &dir, 123).unwrap() else { panic!("not Updated") };
        assert_eq!((from, to), (Some(100), 123));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn range_header_values() {
        assert_eq!(range_header(10, 5).as_deref(), Some("bytes=10-14"));
        assert_eq!(range_header(10, 1).as_deref(), Some("bytes=10-10"));
        // No bytes: "bytes=10-9" is not a valid range.
        assert_eq!(range_header(10, 0), None);
    }

    #[test]
    fn outcome_messages() {
        let dir = Path::new("sde");
        let text = |o: Outcome| o.message(dir);
        assert_eq!(text(Outcome::Skipped), None);
        assert_eq!(text(Outcome::UpToDate), None);
        let updated = |from| Outcome::Updated { from, to: 7, bytes: 1_840_000 };
        assert_eq!(text(updated(Some(7))).unwrap(), "Completed SDE build 7 (1.8 MB) in sde");
        assert_eq!(text(updated(Some(6))).unwrap(), "Updated the SDE from build 6 to build 7 (1.8 MB) in sde");
        assert_eq!(text(updated(None)).unwrap(), "Downloaded SDE build 7 (1.8 MB) to sde");
        let offline = Outcome::Offline { local: 6, error: "timeout".into() };
        assert_eq!(text(offline).unwrap(), "Cannot check for a new SDE. Using local build 6. Cause: timeout");
    }

    #[test]
    fn inflate_stops_after_the_given_size() {
        use flate2::{Compression, write::DeflateEncoder};
        use std::io::Write;
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&vec![0u8; 1024 * 1024]).unwrap();
        let data = enc.finish().unwrap();
        // The entry says 10 bytes, but the data gives 1 MiB. Read only one byte past the size.
        assert_eq!(inflate(&data, 10).unwrap().len(), 11);
        assert_eq!(inflate(&data, 1024 * 1024).unwrap().len(), 1024 * 1024);
    }

    #[test]
    fn corrupt_data_fails() {
        let mut bad = FIXTURE.to_vec();
        // Change one byte inside the first entry data. It starts at byte 51.
        bad[60] ^= 0xff;
        let dir = std::env::temp_dir().join("eve-router-test-sde-corrupt");
        assert!(download(&bad, &dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
