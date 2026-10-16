//! The startup load: the wormhole fetches, the SDE update, the map and the overlays.
//!
//! `load` starts the Nexum and EVE-Scout fetches first, so they run at the same time as the SDE
//! update. It tells the caller about each step with `Progress`, for the splash screen.

use crate::config::{self, AppPaths, Config};
use crate::overlay::{self, OverlayReport};
use crate::refresh::{self, Snapshot};
use crate::sde_update::{self, Outcome};
use crate::sources::{evescout, nexum};
use crate::universe::Universe;
use crate::wormhole::{self, SourceId};
use crate::wormhole_types::WormholeTypes;
use crate::{ansiblex, sde, wormhole_types};
use std::sync::Arc;

/// A step of the load, for the splash screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Ask the server for the latest SDE build.
    CheckingSde,
    /// Download the SDE files. `done` and `total` count compressed bytes.
    DownloadingSde { done: u64, total: u64 },
    /// Read the SDE files, and add the jump bridges.
    LoadingMap,
    /// Wait for the Nexum and EVE-Scout fetches, and add their wormholes.
    AddingWormholes,
}

impl Progress {
    /// The text for the splash screen, for example "Downloading the SDE: 12.3 of 31.0 MB".
    pub fn text(&self) -> String {
        match self {
            Progress::CheckingSde => "Checking for a new SDE…".into(),
            Progress::DownloadingSde { done, total } => {
                format!("Downloading the SDE: {:.1} of {:.1} MB", *done as f64 / 1e6, *total as f64 / 1e6)
            }
            Progress::LoadingMap => "Loading the map…".into(),
            Progress::AddingWormholes => "Adding the wormholes…".into(),
        }
    }

    /// The share of the work that is done, from 0.0 to 1.0, for a progress bar. `None` for a step
    /// with no known length.
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Progress::DownloadingSde { done, total } if *total > 0 => Some((*done as f64 / *total as f64) as f32),
            _ => None,
        }
    }
}

/// The loaded map, with the overlays.
pub struct Loaded {
    /// What the SDE update did. `Outcome::message` gives the status line.
    pub sde: Outcome,
    /// The ships and the groups of the SDE.
    pub hulls: Arc<ansiblex::HullTable>,
    /// The map with the wormholes, its Shortcuts box, and the wormhole data of each source.
    pub snapshot: Snapshot,
    /// The map with the stargates and the jump bridges, before the wormholes. Each refresh copies it.
    pub base: Arc<Universe>,
    pub report: OverlayReport,
    /// The wormhole types of the SDE. Each refresh converts the source data with them.
    pub types: WormholeTypes,
}

/// Update the SDE in `paths.sde`, load the map and add the overlays and the wormholes.
/// `skip_sde_check` skips the network check of the SDE. `progress` gets each step. It can run on
/// any thread of the load, so it must be quick.
pub fn load(cfg: &Config, paths: &AppPaths, skip_sde_check: bool, progress: impl Fn(Progress) + Sync) -> Result<Loaded, String> {
    // Nexum data comes only from the API. EVE-Scout is public, so the router always fetches it.
    // The Thera and Turnur switches act at route time, so a switch works with no restart.
    let nexum = nexum::start(&cfg.nexum, paths.cache_file(SourceId::Nexum), wormhole::now());
    let scout = evescout::start(evescout::URL, paths.cache_file(SourceId::EveScout), wormhole::now());

    let sde_dir = paths.sde.as_path();
    progress(Progress::CheckingSde);
    let outcome = sde_update::ensure(sde_dir, skip_sde_check, &|done, total| progress(Progress::DownloadingSde { done, total }))?;
    let hulls = ansiblex::HullTable::load(sde_dir)?;

    progress(Progress::LoadingMap);
    let mut uni = Universe::from_sde(sde::load(sde_dir)?);
    let mut report = OverlayReport::default();
    if let Some(path) = config::overlay_path(&paths.config, config::BRIDGES_FILE) {
        report = overlay::load_bridges(&mut uni, &path)?;
    }
    let types = wormhole_types::load(sde_dir)?;

    progress(Progress::AddingWormholes);
    let wh = nexum::finish(nexum, |id| uni.by_id.contains_key(&id), &types);
    let scout = evescout::finish(scout, |id| uni.by_id.contains_key(&id), &types);
    let (snapshot, _) = refresh::build(&uni, &report, &wh, &scout, wormhole::now());
    Ok(Loaded { sde: outcome, hulls, snapshot, base: Arc::new(uni), report, types })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load from an empty directory: download the SDE, with progress. It needs the network, so it
    /// runs only on request: `cargo test fresh_start_downloads_the_sde -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn fresh_start_downloads_the_sde() {
        use std::sync::Mutex;
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::in_dir(tmp.path());
        let steps = Mutex::new(Vec::new());
        let loaded = load(&Config::default(), &paths, false, |p| steps.lock().unwrap().push(p)).unwrap();
        let steps = steps.into_inner().unwrap();
        let downloads: Vec<_> = steps.iter().filter(|p| matches!(p, Progress::DownloadingSde { .. })).collect();
        println!("{} download reports, first {:?}, last {:?}", downloads.len(), downloads.first(), downloads.last());
        assert_eq!(steps.first(), Some(&Progress::CheckingSde));
        assert!(matches!(downloads.last(), Some(Progress::DownloadingSde { done, total }) if done == total));
        assert!(matches!(loaded.sde, Outcome::Updated { from: None, .. }));
        assert!(loaded.snapshot.uni.build.is_some());
    }

    #[test]
    fn a_download_shows_the_megabytes_and_a_fraction() {
        let p = Progress::DownloadingSde { done: 12_300_000, total: 31_000_000 };
        assert_eq!(p.text(), "Downloading the SDE: 12.3 of 31.0 MB");
        assert!((p.fraction().unwrap() - 0.3968).abs() < 1e-3);
        assert_eq!(Progress::DownloadingSde { done: 0, total: 0 }.fraction(), None);
        assert_eq!(Progress::LoadingMap.fraction(), None);
        assert_eq!(Progress::CheckingSde.text(), "Checking for a new SDE…");
    }
}
