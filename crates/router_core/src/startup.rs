//! The startup load: the wormhole fetches, the SDE update, the map and the overlays.
//!
//! The load has two calls. `begin` starts the Nexum and EVE-Scout fetches, so they run at the
//! same time as the SDE update in `finish`.

use crate::config::{self, Config};
use crate::overlay::{self, OverlayReport};
use crate::refresh::{self, Snapshot};
use crate::sde_update::{self, Outcome};
use crate::sources::{self, evescout, nexum};
use crate::universe::Universe;
use crate::wormhole::{self, SourceId};
use crate::wormhole_types::WormholeTypes;
use crate::{ansiblex, sde, wormhole_types};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The fetches that `begin` started.
pub struct Pending {
    nexum: nexum::Pending,
    scout: evescout::Pending,
    cfg_path: PathBuf,
}

/// The loaded map, with the overlays.
pub struct Loaded {
    /// The map with the wormholes, its Shortcuts box, and the wormhole data of each source.
    pub snapshot: Snapshot,
    /// The map with the stargates and the jump bridges, before the wormholes. Each refresh copies it.
    pub base: Arc<Universe>,
    pub report: OverlayReport,
    /// The wormhole types of the SDE. Each refresh converts the source data with them.
    pub types: WormholeTypes,
    pub wh: nexum::Load,
    pub scout: evescout::Load,
    /// The wormholes that the map got after the merge.
    pub wormhole_count: usize,
    /// The time from the SDE load to the end, without the SDE update.
    pub load_time: Duration,
}

/// Start the wormhole fetches. Call this before `finish`, so the fetches run during the SDE update.
pub fn begin(cfg: &Config, cfg_path: &Path) -> Pending {
    // Nexum data comes only from the API.
    let cache_path = sources::cache_path(SourceId::Nexum, cfg_path);
    let nexum = nexum::start(&cfg.nexum, cache_path, wormhole::now());
    // EVE-Scout is public, so the router always fetches it. The Thera and Turnur switches
    // act at route time, so a switch works with no restart.
    let scout_cache = sources::cache_path(SourceId::EveScout, cfg_path);
    let scout = evescout::start(evescout::URL, scout_cache, wormhole::now());
    Pending { nexum, scout, cfg_path: cfg_path.to_path_buf() }
}

/// Update the SDE in `sde_dir`, load the map and add the overlays. `on_sde` gets the update
/// result before the map load starts. `bridges` is the jump bridge file of the CLI flag, if any.
pub fn finish(pending: Pending, sde_dir: &Path, bridges: Option<PathBuf>, on_sde: impl FnOnce(&Outcome)) -> Result<Loaded, String> {
    on_sde(&sde_update::ensure(sde_dir)?);
    ansiblex::init(sde_dir)?;

    let started = Instant::now();
    let mut uni = Universe::from_sde(sde::load(sde_dir)?);
    let bridges_path = config::overlay_path(bridges, &pending.cfg_path, config::BRIDGES_FILE);
    let mut report = OverlayReport::default();
    if let Some(path) = &bridges_path {
        report = overlay::load_bridges(&mut uni, path)?;
    }
    let types = wormhole_types::load(sde_dir)?;
    let wh = nexum::finish(pending.nexum, |id| uni.by_id.contains_key(&id), &types);
    let scout = evescout::finish(pending.scout, |id| uni.by_id.contains_key(&id), &types);
    let (snapshot, wormhole_count) = refresh::build(&uni, &report, &wh, &scout, wormhole::now());
    let load_time = started.elapsed();
    Ok(Loaded { snapshot, base: Arc::new(uni), report, types, wh, scout, wormhole_count, load_time })
}
