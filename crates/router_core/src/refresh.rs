//! The live refresh of the wormhole sources. Each refresh builds a full new map from the base
//! map. Thus a route of the old map keeps correct edge indices until the app swaps the map.

use crate::config::NexumConfig;
use crate::labels::Shortcuts;
use crate::overlay::OverlayReport;
use crate::sources::{self, evescout, nexum};
use crate::universe::Universe;
use crate::wormhole::{self, SourceData, SourceId};
use crate::wormhole_types::WormholeTypes;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

/// The time between two refreshes: the age at which a cache stops being fresh.
pub const INTERVAL: Duration = Duration::from_secs(sources::CACHE_FRESH_SECS);
/// The status note when the worker stops, for example after a panic.
pub const STOPPED: &str = "Wormhole refresh stopped";
/// The status text after a Nexum settings change, while the worker loads the new map.
pub const NEXUM_LOADING: &str = "Nexum settings saved. Loading the new map…";

/// A map with the wormholes of one load.
pub struct Snapshot {
    pub uni: Arc<Universe>,
    /// The counts of the Shortcuts box, and the fetch warnings for the status line.
    pub shortcuts: Shortcuts,
    /// The wormhole data of each source.
    pub all: Vec<SourceData>,
}

/// Build a map: a copy of `base` with the merged wormholes of `wh` and `scout` at `now`.
/// The startup and each refresh use this function, so both build the same map. The copy keeps
/// the `NodeIndex` of each system. Return the snapshot, and the count of wormholes that the map got.
pub fn build(base: &Universe, report: &OverlayReport, wh: &nexum::Load, scout: &evescout::Load, now: u64) -> (Snapshot, usize) {
    let mut uni = base.clone();
    let all: Vec<SourceData> = wh.data.iter().chain(&scout.data).cloned().collect();
    let count = uni.add_wormholes(&wormhole::merge(&all, now));
    let shortcuts = Shortcuts::new(&uni, report, wh, scout);
    (Snapshot { uni: Arc::new(uni), shortcuts, all }, count)
}

/// What the worker needs for each refresh.
pub struct Setup {
    /// The map with the stargates and the jump bridges, before the wormholes.
    pub base: Arc<Universe>,
    /// The jump bridge report of the startup, for the Shortcuts box.
    pub report: OverlayReport,
    pub types: WormholeTypes,
    pub nexum: NexumConfig,
    pub nexum_cache: PathBuf,
    pub scout_url: String,
    pub scout_cache: PathBuf,
    /// The time between two refreshes.
    pub interval: Duration,
}

impl Setup {
    /// The setup of an app: the public EVE-Scout feed, the cache files of the config at
    /// `cfg_path`, and a refresh each `INTERVAL`.
    pub fn new(base: Arc<Universe>, report: OverlayReport, types: WormholeTypes, nexum: NexumConfig, cfg_path: &std::path::Path) -> Setup {
        Setup {
            base,
            report,
            types,
            nexum,
            nexum_cache: sources::cache_path(SourceId::Nexum, cfg_path),
            scout_url: evescout::URL.into(),
            scout_cache: sources::cache_path(SourceId::EveScout, cfg_path),
            interval: INTERVAL,
        }
    }
}

/// A message from the app to the worker.
pub enum Control {
    /// Use these Nexum settings. Refresh at once with no fresh-cache check, then start a new interval.
    Nexum(NexumConfig),
}

/// The handle of the refresh worker. A drop of the handle stops the worker.
pub struct Refresher {
    control: Sender<Control>,
    snapshots: Receiver<Snapshot>,
}

impl Refresher {
    /// Start the worker on a thread. It waits `setup.interval`, refreshes, and sends a snapshot.
    /// `clock` gives the time in Unix seconds. The worker calls `wake` after it sends each snapshot.
    pub fn start(setup: Setup, clock: impl Fn() -> u64 + Send + 'static, wake: impl Fn() + Send + 'static) -> Refresher {
        let (control, control_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let worker = Worker { setup, last_nexum: None, last_scout: None };
        std::thread::spawn(move || worker.run(control_rx, snapshot_tx, clock, wake));
        Refresher { control, snapshots }
    }

    /// A handle with no worker, for the tests of the apps. The test sends the snapshots, and
    /// reads the control messages. A dropped sender acts as a stopped worker.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fake() -> (Refresher, Sender<Snapshot>, Receiver<Control>) {
        let (control, control_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        (Refresher { control, snapshots }, snapshot_tx, control_rx)
    }

    /// Use new Nexum settings, and refresh at once. A stopped worker ignores the message.
    pub fn nexum(&self, cfg: NexumConfig) {
        let _ = self.control.send(Control::Nexum(cfg));
    }

    /// The newest snapshot since the last call. An older snapshot that waits goes away.
    /// `Disconnected`: the worker stopped.
    pub fn try_recv(&self) -> Result<Snapshot, TryRecvError> {
        let mut newest = self.snapshots.try_recv()?;
        while let Ok(next) = self.snapshots.try_recv() {
            newest = next;
        }
        Ok(newest)
    }

    /// Wait up to `timeout` for the next snapshot.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Snapshot, RecvTimeoutError> {
        self.snapshots.recv_timeout(timeout)
    }
}

/// The state of the worker thread. A failed fetch with no cache uses the data of the last refresh.
struct Worker {
    setup: Setup,
    last_nexum: Option<SourceData>,
    last_scout: Option<SourceData>,
}

impl Worker {
    fn run(mut self, control: Receiver<Control>, snapshots: Sender<Snapshot>, clock: impl Fn() -> u64, wake: impl Fn()) {
        loop {
            let force = match control.recv_timeout(self.setup.interval) {
                Ok(Control::Nexum(cfg)) => {
                    self.setup.nexum = cfg;
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                // The app dropped the Refresher.
                Err(RecvTimeoutError::Disconnected) => return,
            };
            if snapshots.send(self.tick(clock(), force)).is_err() {
                return;
            }
            wake();
        }
    }

    /// One refresh at `now`. `force` skips the fresh-cache check of Nexum.
    fn tick(&mut self, now: u64, force: bool) -> Snapshot {
        let s = &self.setup;
        let pending = if force {
            nexum::start_fetch(&s.nexum, s.nexum_cache.clone(), now)
        } else {
            nexum::start(&s.nexum, s.nexum_cache.clone(), now)
        };
        let scout_pending = evescout::start(&s.scout_url, s.scout_cache.clone(), now);
        let known = |id: u32| s.base.by_id.contains_key(&id);
        let mut wh = nexum::finish(pending, known, &s.types);
        let mut scout = evescout::finish(scout_pending, known, &s.types);
        // A failed fetch with no cache keeps the data of the last refresh. As for the cache,
        // the data of another Nexum map does not count.
        let origin = nexum::map_url(&s.nexum);
        wh.data = wh.data.take().or_else(|| self.last_nexum.take().filter(|d| d.origin == origin));
        scout.data = scout.data.take().or_else(|| self.last_scout.take());
        self.last_nexum.clone_from(&wh.data);
        self.last_scout.clone_from(&scout.data);
        build(&s.base, &s.report, &wh, &scout, now).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiKey;
    use crate::sources::CACHE_FRESH_SECS;
    use crate::test_support::{FIXTURE_TIME, hole, serve, serve_live, serve_routes, shared_universe, universe};
    use crate::universe::Link;
    use crate::wormhole::{SourceId, THERA};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    const JITA: u32 = 30000142;
    const AMARR: u32 = 30002187;

    /// EVE-Scout data with one wormhole from Thera to Jita.
    fn scout_load() -> evescout::Load {
        // Read the record from JSON, so the test does not depend on each field of `SourceData`.
        let json = serde_json::json!({ "source": SourceId::EveScout, "fetched_at": 0, "holes": [hole(THERA, JITA)] });
        let data: SourceData = serde_json::from_value(json).unwrap();
        evescout::Load { data: Some(data), ..Default::default() }
    }

    #[test]
    fn build_keeps_the_node_of_each_system() {
        let base = universe();
        let (snap, count) = build(base, &OverlayReport::default(), &nexum::Load::default(), &scout_load(), 0);
        assert_eq!(count, 1);
        assert_eq!(snap.uni.graph.node_count(), base.graph.node_count());
        for (id, &node) in &base.by_id {
            assert_eq!(snap.uni.by_id[id], node);
            assert_eq!(snap.uni.system(node).id, *id);
        }
        // The copy has the two directions of the wormhole. The base map does not change.
        assert_eq!(snap.uni.graph.edge_count(), base.graph.edge_count() + 2);
        assert_eq!((snap.shortcuts.wormholes, snap.shortcuts.thera), (1, 1));
        assert_eq!(snap.all.len(), 1);
    }

    /// An EVE-Scout feed with one Thera wormhole to `to`. It has no expiry, so the test clock cannot end it.
    fn feed(to: u32) -> String {
        format!(
            r#"[{{"signature_type":"wormhole","completed":true,"out_system_id":{THERA},"out_signature":"ABC-123","in_system_id":{to},"in_signature":"DEF-456","max_ship_size":"large"}}]"#
        )
    }

    /// A setup with no Nexum settings, and the cache files in a new temporary directory.
    fn setup(name: &str, scout_url: String, interval: Duration) -> Setup {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        Setup {
            base: shared_universe(),
            report: OverlayReport::default(),
            types: WormholeTypes::default(),
            nexum: NexumConfig::default(),
            nexum_cache: dir.join("nexum.json"),
            scout_url,
            scout_cache: dir.join("eve-scout.json"),
            interval,
        }
    }

    /// A clock that moves 5 minutes ahead at each call, so a cache is never fresh.
    fn moving_clock() -> impl Fn() -> u64 + Send + 'static {
        let t = AtomicU64::new(FIXTURE_TIME);
        move || t.fetch_add(CACHE_FRESH_SECS, Ordering::Relaxed)
    }

    /// True if the map has a wormhole between the systems with the IDs `a` and `b`.
    fn has_hole(uni: &Universe, a: u32, b: u32) -> bool {
        uni.graph.edges_connecting(uni.by_id[&a], uni.by_id[&b]).any(|e| matches!(e.weight(), Link::Wormhole(_)))
    }

    /// Wait for a snapshot that `want` accepts. Fail after 10 s.
    fn wait_for(r: &Refresher, want: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let snap = r.recv_timeout(end.saturating_duration_since(Instant::now())).expect("no snapshot in 10 s");
            if want(&snap) {
                return snap;
            }
        }
    }

    #[test]
    fn a_changed_feed_gives_the_new_wormhole() {
        let routes = Arc::new(Mutex::new(HashMap::from([("/scout".to_string(), feed(JITA))])));
        let (url, _) = serve_live(routes.clone());
        let setup = setup("eve-router-test-refresh-feed", format!("{url}/scout"), Duration::from_millis(50));
        let r = Refresher::start(setup, moving_clock(), || {});
        let first = wait_for(&r, |s| has_hole(&s.uni, THERA, JITA));
        assert_eq!(first.shortcuts.thera, 1);
        routes.lock().unwrap().insert("/scout".into(), feed(AMARR));
        let changed = wait_for(&r, |s| has_hole(&s.uni, THERA, AMARR));
        assert!(!has_hole(&changed.uni, THERA, JITA));
        assert_eq!(changed.shortcuts.warning, None);
    }

    #[test]
    fn a_failed_fetch_keeps_the_last_good_data() {
        let routes = Arc::new(Mutex::new(HashMap::from([("/scout".to_string(), feed(JITA))])));
        let (url, _) = serve_live(routes.clone());
        let setup = setup("eve-router-test-refresh-500", format!("{url}/scout"), Duration::from_millis(50));
        let cache = setup.scout_cache.clone();
        let r = Refresher::start(setup, moving_clock(), || {});
        wait_for(&r, |s| has_hole(&s.uni, THERA, JITA));
        // A path that is not in the routes gets a 500.
        routes.lock().unwrap().clear();
        // First the cache gives the data, and the warning gives the time of the cache.
        let cached = wait_for(&r, |s| s.shortcuts.warning.is_some());
        assert!(has_hole(&cached.uni, THERA, JITA));
        // With no cache, the worker uses the data of the last refresh.
        std::fs::remove_file(&cache).unwrap();
        let kept = wait_for(&r, |s| s.shortcuts.warning.as_deref() == Some("EVE-Scout offline"));
        assert!(has_hole(&kept.uni, THERA, JITA));
        assert_eq!(kept.all.len(), 1);
    }

    #[test]
    fn a_nexum_message_refreshes_at_once_and_skips_the_fresh_cache() {
        let map = std::fs::read_to_string(crate::test_support::fixture("nexum-api.json")).unwrap();
        let (url, _) = serve_routes(HashMap::from([("/api/v1/maps/m1".to_string(), map)]));
        // One hour: a snapshot in this test comes from the message, not from the interval.
        let setup = setup("eve-router-test-refresh-nexum", format!("{url}/scout"), Duration::from_secs(3600));
        let cfg = NexumConfig { url: Some(url.clone()), key: Some(ApiKey("nxm_test".into())), map_id: Some("m1".into()) };
        // A fresh cache of the same map, with no wormholes. A normal start uses it.
        let cached = SourceData {
            source: SourceId::Nexum,
            fetched_at: FIXTURE_TIME - 60,
            origin: nexum::map_url(&cfg),
            name: None,
            holes: Vec::new(),
        };
        sources::write_cache(&setup.nexum_cache, &cached).unwrap();
        let r = Refresher::start(setup, || FIXTURE_TIME, || {});
        r.nexum(cfg);
        let snap = r.recv_timeout(Duration::from_secs(10)).expect("no refresh after the message");
        let data = snap.all.iter().find(|d| d.source == SourceId::Nexum).expect("no Nexum data");
        assert!(!data.holes.is_empty());
        assert_eq!(data.name.as_deref(), Some("Test Map"));
    }

    #[test]
    fn a_dropped_refresher_stops_the_thread() {
        let (url, _) = serve_routes(HashMap::from([("/scout".to_string(), feed(JITA))]));
        let (alive, stopped) = mpsc::channel::<()>();
        // The worker owns `wake`, so the channel closes when the thread ends.
        let wake = move || {
            let _ = alive.send(());
        };
        let setup = setup("eve-router-test-refresh-drop", format!("{url}/scout"), Duration::from_millis(50));
        let r = Refresher::start(setup, moving_clock(), wake);
        stopped.recv_timeout(Duration::from_secs(10)).expect("no first refresh");
        drop(r);
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            match stopped.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(()) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("the worker did not stop"),
            }
        }
    }

    /// Review focus: a map change, then a failed fetch of the new map.
    #[test]
    fn kept_data_of_another_nexum_map_is_not_used() {
        // The server has no routes, so each fetch gets a 500, and no cache file exists.
        let (url, _) = serve_routes(HashMap::new());
        let mut worker = Worker {
            setup: setup("eve-router-test-refresh-other-map", format!("{url}/scout"), INTERVAL),
            last_nexum: None,
            last_scout: None,
        };
        worker.setup.nexum = NexumConfig { url: Some(url.clone()), key: Some(ApiKey("nxm_test".into())), map_id: Some("m1".into()) };
        let kept = |map: &str| SourceData {
            source: SourceId::Nexum,
            fetched_at: FIXTURE_TIME,
            origin: Some(format!("{url}/api/v1/maps/{map}")),
            name: None,
            holes: vec![hole(JITA, AMARR)],
        };
        // The kept data of the current map fills the gap.
        worker.last_nexum = Some(kept("m1"));
        assert_eq!(worker.tick(FIXTURE_TIME, false).all.len(), 1);
        // The kept data of the old map does not.
        worker.last_nexum = Some(kept("m2"));
        assert!(worker.tick(FIXTURE_TIME, false).all.is_empty());
    }

    /// Review focus: several snapshots wait while the app is busy.
    #[test]
    fn try_recv_gives_the_newest_snapshot() {
        let (r, snapshots, _control) = Refresher::fake();
        assert!(matches!(r.try_recv(), Err(TryRecvError::Empty)));
        snapshots.send(build(universe(), &OverlayReport::default(), &nexum::Load::default(), &evescout::Load::default(), 0).0).unwrap();
        snapshots.send(build(universe(), &OverlayReport::default(), &nexum::Load::default(), &scout_load(), 0).0).unwrap();
        assert_eq!(r.try_recv().unwrap().shortcuts.wormholes, 1);
        assert!(matches!(r.try_recv(), Err(TryRecvError::Empty)));
        // A dropped sender acts as a stopped worker.
        drop(snapshots);
        assert!(matches!(r.try_recv(), Err(TryRecvError::Disconnected)));
    }

    /// Review focus: the app quits while a fetch runs.
    #[test]
    fn a_drop_does_not_wait_for_a_fetch() {
        // The server answers after 3 s, so the first refresh is in its fetch at the drop.
        let (url, _) = serve("200 OK", "[]", Duration::from_secs(3));
        let r = Refresher::start(setup("eve-router-test-refresh-slow", url, Duration::from_millis(10)), moving_clock(), || {});
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        drop(r);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn the_app_setup_uses_the_public_feed_and_a_5_minute_interval() {
        let cfg_path = std::path::Path::new("eve-router.json");
        let s = Setup::new(shared_universe(), OverlayReport::default(), WormholeTypes::default(), NexumConfig::default(), cfg_path);
        assert_eq!(s.interval, Duration::from_secs(CACHE_FRESH_SECS));
        assert_eq!(s.scout_url, evescout::URL);
        assert_eq!(s.nexum_cache, sources::cache_path(SourceId::Nexum, cfg_path));
        assert_eq!(s.scout_cache, sources::cache_path(SourceId::EveScout, cfg_path));
    }
}
