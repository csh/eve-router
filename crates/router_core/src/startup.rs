//! The startup load: the wormhole fetches, the SDE update, the map and the overlays.
//!
//! The load has two calls. `begin` starts the Nexum and EVE-Scout fetches, so they run at the
//! same time as the SDE update in `finish`.

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
use std::time::{Duration, Instant};

/// The fetches that `begin` started.
pub struct Pending {
    nexum: nexum::Pending,
    scout: evescout::Pending,
    paths: AppPaths,
}

/// The loaded map, with the overlays.
pub struct Loaded {
    /// The ships and the groups of the SDE.
    pub hulls: Arc<ansiblex::HullTable>,
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
pub fn begin(cfg: &Config, paths: &AppPaths) -> Pending {
    // Nexum data comes only from the API.
    let nexum = nexum::start(&cfg.nexum, paths.cache_file(SourceId::Nexum), wormhole::now());
    // EVE-Scout is public, so the router always fetches it. The Thera and Turnur switches
    // act at route time, so a switch works with no restart.
    let scout = evescout::start(evescout::URL, paths.cache_file(SourceId::EveScout), wormhole::now());
    Pending { nexum, scout, paths: paths.clone() }
}

/// Update the SDE in `paths.sde`, load the map and add the overlays. `on_sde` gets the update
/// result before the map load starts.
pub fn finish(pending: Pending, skip_sde_check: bool, on_sde: impl FnOnce(&Outcome)) -> Result<Loaded, String> {
    let sde_dir = pending.paths.sde.as_path();
    on_sde(&sde_update::ensure(sde_dir, skip_sde_check)?);
    let hulls = ansiblex::HullTable::load(sde_dir)?;

    let started = Instant::now();
    let mut uni = Universe::from_sde(sde::load(sde_dir)?);
    let bridges_path = config::overlay_path(&pending.paths.config, config::BRIDGES_FILE);
    let mut report = OverlayReport::default();
    if let Some(path) = &bridges_path {
        report = overlay::load_bridges(&mut uni, path)?;
    }
    let types = wormhole_types::load(sde_dir)?;
    let wh = nexum::finish(pending.nexum, |id| uni.by_id.contains_key(&id), &types);
    let scout = evescout::finish(pending.scout, |id| uni.by_id.contains_key(&id), &types);
    let (snapshot, wormhole_count) = refresh::build(&uni, &report, &wh, &scout, wormhole::now());
    let load_time = started.elapsed();
    Ok(Loaded { hulls, snapshot, base: Arc::new(uni), report, types, wh, scout, wormhole_count, load_time })
}
