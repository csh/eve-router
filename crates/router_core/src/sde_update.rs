//! Keep the local SDE files up to date.
//!
//! Source: https://developers.eveonline.com/docs/services/static-data/#automation
//! `latest.jsonl` gives the latest build number. The router then reads the zip index
//! from the end of the build zip, and fetches only the entries it needs with HTTP
//! range requests. This downloads about 1.8 MB instead of the full 99 MB zip.

use crate::error::{FileError, SdeError};
use crate::ships;
use flate2::Crc;
use flate2::read::DeflateDecoder;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// The SDE files that the router needs. `_sde.jsonl` comes last, so an interrupted
/// update leaves the old build number, and the next start tries again.
pub const FILES: [&str; 4] = ["mapSolarSystems.jsonl", "mapStargates.jsonl", "mapRegions.jsonl", "_sde.jsonl"];

/// Set this variable to `1` to skip the check and use the local SDE as it is.
pub const SKIP_ENV: &str = "EVE_ROUTER_SKIP_SDE_CHECK";

const BASE_URL: &str = "https://developers.eveonline.com/static-data/tranquility";

/// Receives `(done, total)` in compressed bytes while the download runs. It runs on the threads
/// of the download, so it must be quick.
pub type OnBytes<'a> = &'a (dyn Fn(u64, u64) + Sync);

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
        error: SdeError,
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

/// Make sure that `dir` holds the latest SDE files. With `skip`, do not look at the network.
/// `on_bytes` reports the progress of a download.
pub fn ensure(dir: &Path, skip: bool, on_bytes: OnBytes) -> Result<Outcome, SdeError> {
    if skip {
        return Ok(Outcome::Skipped);
    }
    let local = local_build(dir);
    let agent = agent();
    let latest = match latest_build(&agent) {
        Ok(build) => build,
        Err(error) => {
            return match local {
                Some(local) => Ok(Outcome::Offline { local, error }),
                None => Err(SdeError::Download { dir: dir.to_path_buf(), source: Box::new(error) }),
            };
        }
    };
    if local == Some(latest) {
        return Ok(Outcome::UpToDate);
    }
    let source = HttpSource { agent, url: format!("{BASE_URL}/eve-online-static-data-{latest}-jsonl.zip") };
    install(&source, dir, latest, on_bytes)
}

/// Download build `latest` into `dir`. `from` is the local build before the download.
fn install(src: &dyn RangeSource, dir: &Path, latest: u32, on_bytes: OnBytes) -> Result<Outcome, SdeError> {
    // The local build can exist without ships.json or wormholes.json, from a version before these derived tables.
    let from = crate::sde::build_number(dir);
    let bytes = download(src, dir, on_bytes)?;
    Ok(Outcome::Updated { from, to: latest, bytes })
}

/// The build number of the local SDE, if all files are present and `ships.json` has the
/// current schema. A build number match alone is not enough: a schema change (a new field
/// such as `filament_ok`) needs a fresh `ships.json` even when the SDE build stays the same.
fn local_build(dir: &Path) -> Option<u32> {
    let derived = [&crate::ships::FILE, &crate::wormhole_types::FILE];
    let present = FILES.iter().chain(derived).all(|f| dir.join(f).is_file());
    let current_schema = ships::load(dir).is_ok_and(|d| d.schema == ships::SCHEMA);
    if present && current_schema { crate::sde::build_number(dir) } else { None }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(concat!("eve-router/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn latest_build(agent: &ureq::Agent) -> Result<u32, SdeError> {
    let url = format!("{BASE_URL}/latest.jsonl");
    let text = agent
        .get(&url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| SdeError::Http { url: url.clone(), source: Box::new(e) })?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v["_key"] == "sde")
        .and_then(|v| v["buildNumber"].as_u64())
        .and_then(|b| u32::try_from(b).ok())
        .ok_or(SdeError::NoBuild { url })
}

/// A file that gives byte ranges: a remote zip, or bytes in memory for tests.
pub(crate) trait RangeSource: Sync {
    fn len(&self) -> Result<u64, SdeError>;
    fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, SdeError>;
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

impl HttpSource {
    fn http(&self, source: ureq::Error) -> SdeError {
        SdeError::Http { url: self.url.clone(), source: Box::new(source) }
    }
}

impl RangeSource for HttpSource {
    fn len(&self) -> Result<u64, SdeError> {
        let resp = self.agent.head(&self.url).call().map_err(|e| self.http(e))?;
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()?.parse().ok())
            .ok_or_else(|| SdeError::NoContentLength { url: self.url.clone() })
    }

    fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, SdeError> {
        let Some(range) = range_header(start, len) else { return Ok(Vec::new()) };
        let mut resp = self.agent.get(&self.url).header("Range", &range).call().map_err(|e| self.http(e))?;
        // 206 Partial Content. A 200 is the full 99 MB zip, so stop.
        if resp.status().as_u16() != 206 {
            return Err(SdeError::RangeIgnored { url: self.url.clone(), status: resp.status().as_u16() });
        }
        // The ureq limit fails when the body reaches it, so allow one more byte. The check below
        // still catches a wrong length.
        let body = resp.body_mut().with_config().limit(len + 1).read_to_vec().map_err(|e| self.http(e))?;
        if body.len() as u64 != len {
            return Err(SdeError::ShortBody { url: self.url.clone(), want: len, got: body.len() as u64 });
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
fn read_directory(src: &dyn RangeSource) -> Result<Vec<Entry>, SdeError> {
    let total = src.len()?;
    let tail_len = total.min(65_557);
    let tail = src.read(total - tail_len, tail_len)?;
    let end = tail.windows(4).rposition(|w| w == EOCD_SIG).ok_or_else(|| SdeError::Zip("no zip end record".into()))?;
    if tail.len() < end + 22 {
        return Err(SdeError::Zip("short zip end record".into()));
    }
    let (size, offset) = (u32_at(&tail, end + 12), u32_at(&tail, end + 16));
    if offset == u32::MAX {
        return Err(SdeError::Zip("zip64 archives are not supported".into()));
    }
    let dir = src.read(offset.into(), size.into())?;
    let mut entries = Vec::new();
    let mut i = 0;
    while i + 46 <= dir.len() && u32_at(&dir, i) == CENTRAL_SIG {
        let name_len = u16_at(&dir, i + 28) as usize;
        let extra_len = u16_at(&dir, i + 30) as usize;
        let comment_len = u16_at(&dir, i + 32) as usize;
        let name = dir.get(i + 46..i + 46 + name_len).ok_or_else(|| SdeError::Zip("short zip directory".into()))?;
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

/// The most bytes in one range request. A big entry then reports its progress in steps.
const CHUNK: u64 = 1024 * 1024;

/// Read `len` bytes from `start` in requests of at most `CHUNK` bytes. `step` gets the size of each part.
fn read_chunks(src: &dyn RangeSource, start: u64, len: u64, step: &dyn Fn(u64)) -> Result<Vec<u8>, SdeError> {
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0).min(MAX_RESERVE));
    let mut at = 0;
    while at < len {
        let part = CHUNK.min(len - at);
        out.extend(src.read(start + at, part)?);
        step(part);
        at += part;
    }
    Ok(out)
}

/// Fetch and decompress one entry. Make sure that its size and CRC-32 are correct.
/// `step` gets the number of compressed bytes after each part of the download.
fn extract(src: &dyn RangeSource, entry: &Entry, step: &dyn Fn(u64)) -> Result<Vec<u8>, SdeError> {
    let err = |msg: &str| SdeError::Entry { name: entry.name.clone(), message: msg.to_string() };
    let header = src.read(entry.offset, 30)?;
    if u32_at(&header, 0) != LOCAL_SIG {
        return Err(err("bad local header"));
    }
    // The local extra field can differ from the central one, so read its length here.
    let data_start = entry.offset + 30 + u64::from(u16_at(&header, 26)) + u64::from(u16_at(&header, 28));
    let data = read_chunks(src, data_start, entry.compressed, step)?;
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
/// The source files are not kept. Return the number of compressed bytes. `on_bytes` gets
/// `(done, total)` at the start and after each part of the download.
pub(crate) fn download(src: &dyn RangeSource, dir: &Path, on_bytes: OnBytes) -> Result<u64, SdeError> {
    let entries = read_directory(src)?;
    let names: Vec<&str> = FILES.iter().chain(&ships::SOURCES).copied().collect();
    let wanted: Vec<&Entry> = names
        .iter()
        .map(|&f| entries.iter().find(|e| e.name == f).ok_or_else(|| SdeError::NotInZip(f.to_string())))
        .collect::<Result<_, _>>()?;
    let total: u64 = wanted.iter().map(|e| e.compressed).sum();
    let done = Mutex::new(0u64);
    on_bytes(0, total);
    // The lock covers the report, so the reports come in order and `done` never goes back.
    let step = |part: u64| {
        let mut done = done.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *done += part;
        on_bytes(*done, total);
    };
    // The entries are independent, so fetch them at the same time.
    let mut files: HashMap<&str, Vec<u8>> =
        wanted.par_iter().map(|e| extract(src, e, &step).map(|data| (e.name.as_str(), data))).collect::<Result<_, _>>()?;

    let build = files.get("_sde.jsonl").and_then(|d| build_from(d));
    let source = |name| files.get(name).map(Vec::as_slice).unwrap_or_default();
    let sources = ships::parse(source("types.jsonl"), source("typeDogma.jsonl"), source("groups.jsonl"), source("typeLists.jsonl"))?;
    let ship_data = ships::derive(build, &sources)?;
    let wormhole_data = crate::wormhole_types::derive(build, &sources);
    drop(sources);

    fs::create_dir_all(dir).map_err(FileError::io(dir))?;
    ships::save(dir, &ship_data)?;
    crate::wormhole_types::save(dir, &wormhole_data)?;
    // FILES ends with _sde.jsonl, so the build number changes last.
    for name in FILES {
        let data = files.remove(name).unwrap_or_default();
        // Write a temporary file, then rename it, so a reader never sees half a file.
        let path = dir.join(name);
        let tmp = path.with_extension("jsonl.part");
        fs::write(&tmp, data).map_err(FileError::io(&tmp))?;
        fs::rename(&tmp, &path).map_err(FileError::io(&path))?;
    }
    Ok(total)
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
        fn len(&self) -> Result<u64, SdeError> {
            Ok(Vec::len(self) as u64)
        }

        fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, SdeError> {
            self.get(start as usize..(start + len) as usize).map(<[u8]>::to_vec).ok_or_else(|| SdeError::Zip("range out of bounds".into()))
        }
    }

    /// A small zip with the 7 files, an extra file, and both stored and deflated entries.
    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/sde-small.zip");

    #[test]
    fn downloads_only_wanted_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        download(&FIXTURE.to_vec(), &dir, &|_, _| {}).unwrap();
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
    }

    #[test]
    fn a_download_reports_its_progress_up_to_the_total() {
        use std::sync::Mutex;
        let tmp = tempfile::tempdir().unwrap();
        let seen = Mutex::new(Vec::new());
        let total = download(&FIXTURE.to_vec(), tmp.path(), &|done, total| seen.lock().unwrap().push((done, total))).unwrap();
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&(0, total)), "the first report is the start");
        assert_eq!(seen.last(), Some(&(total, total)), "the last report is the end");
        // The files download at the same time, but the reports keep their order.
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "{seen:?}");
        assert!(seen.iter().all(|&(done, t)| t == total && done <= total));
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
        download(&source, dir, &|_, _| {}).unwrap();
    }

    #[test]
    fn install_reports_the_build_before_the_download() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("_sde.jsonl"), "{\"_key\": \"sde\", \"buildNumber\": 100}\n").unwrap();
        // The fixture holds build 123.
        let Outcome::Updated { from, to, .. } = install(&FIXTURE.to_vec(), &dir, 123, &|_, _| {}).unwrap() else { panic!("not Updated") };
        assert_eq!((from, to), (Some(100), 123));
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
        let offline = Outcome::Offline { local: 6, error: SdeError::Zip("timeout".into()) };
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        assert!(download(&bad, &dir, &|_, _| {}).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
