//! The window state, the startup load and the route search.

use crate::theme;
use crate::view::View;
use petgraph::graph::NodeIndex;
use router_core::config::{self, Config};
use router_core::esi::active::active_path;
use router_core::esi::pilots::Pilots;
use router_core::labels::Shortcuts;
use router_core::log::{self, LogEntry};
use router_core::refresh::{self, Refresher, Setup, Snapshot, kept_route, route_note};
use router_core::route::{Route, Stop};
use router_core::settings::{Settings, split_systems};
use router_core::sources::nexum::{self, MapInfo};
use router_core::startup;
use router_core::universe::Universe;
use router_core::wormhole::SourceData;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

enum State {
    /// The startup load runs on a thread, because it can take up to 35 s.
    Loading(Receiver<Result<Session, String>>),
    Ready(Box<Session>, Box<View>),
    Failed(String),
}

pub struct RouterApp {
    state: State,
    /// The start of the last frame, for the frame cap.
    last_frame: Option<Instant>,
}

/// The shortest time between two frames: about 60 frames per second at most. eframe draws a frame
/// only after input or a repaint request, so the window draws no frames while it is idle.
const MIN_FRAME_TIME: Duration = Duration::from_millis(16);

/// The quiet time after the last change of a dragged value, before the app searches and saves.
pub const APPLY_DELAY: Duration = Duration::from_millis(500);

/// The least time between two manual refreshes. A Nexum map costs one request for each system.
pub const REFRESH_GAP: Duration = Duration::from_secs(15);
pub const REFRESHING: &str = "Refreshing the wormholes…";
pub const REFRESH_WAIT: &str = "Refreshed less than 15 s ago";
pub const REFRESH_STOPPED: &str = "The refresh worker stopped";

/// The error when the load thread stops without a result. A panic in the load does this.
const LOAD_STOPPED: &str = "The startup load stopped with an internal error. Start the router from a terminal to see the cause.";

impl RouterApp {
    /// Set the theme and start the load. `ctx` gets a repaint request when the load ends.
    pub fn new(ctx: egui::Context) -> Self {
        theme::apply(&ctx);
        theme::set_font(&ctx);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load(ctx.clone()));
            ctx.request_repaint();
        });
        RouterApp { state: State::Loading(rx), last_frame: None }
    }

    /// Take the load result, if the thread sent it.
    fn poll(&mut self, ctx: &egui::Context) {
        let State::Loading(rx) = &self.state else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(LOAD_STOPPED.into()),
        };
        self.state = match result {
            Ok(session) => {
                // For example "EVE Router - SDE 3579973".
                if let Some(build) = session.uni.build {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("EVE Router - SDE {build}")));
                }
                let view = View::new(&session);
                State::Ready(Box::new(session), Box::new(view))
            }
            Err(e) => State::Failed(e),
        };
    }
}

/// The same load as the TUI, with the config file and the default SDE directory. The arguments
/// of the command line are the first waypoints, for example `eve-router-egui Jita "New Caldari"`.
fn load(ctx: egui::Context) -> Result<Session, String> {
    let cfg_path = config::default_path();
    let cfg = Config::load(&cfg_path)?;
    let pending = startup::begin(&cfg, &cfg_path);
    let sde_dir = config::default_sde_dir();
    let mut startup_lines = Vec::new();
    let startup::Loaded { snapshot, base, report, types, .. } =
        startup::finish(pending, &sde_dir, None, |outcome| startup_lines.extend(outcome.message(&sde_dir)))?;
    let Snapshot { uni, shortcuts, all, log } = snapshot;
    let settings = Settings::from_config(&cfg, &uni)?;
    // The worker gets the wormholes again each 5 minutes, and wakes the window after each refresh.
    let wake = ctx.clone();
    let mut setup = Setup::new(base, report, types, cfg.nexum.clone(), &cfg_path);
    setup.last = all.clone();
    let refresher = Refresher::start(setup, router_core::wormhole::now, move || wake.request_repaint());
    let mut session = Session::new(uni, settings, cfg, cfg_path.clone(), shortcuts);
    session.wormhole_data = all;
    session.log = log;
    session.refresher = Some(refresher);
    session.startup_lines = startup_lines;
    // A tracker event or a login result repaints the window.
    session.pilots = Pilots::open(&cfg_path, Arc::new(move || ctx.request_repaint()));
    // The command line can give the start, the midpoints and the destination, as for the TUI.
    let systems: Vec<String> = std::env::args().skip(1).collect();
    if !systems.is_empty() {
        session.add_list(&systems.join(","));
    }
    Ok(session)
}

/// The loaded map, the settings, the waypoints and the routes. It holds no UI state.
pub struct Session {
    pub uni: Arc<Universe>,
    pub settings: Settings,
    pub cfg: Config,
    cfg_path: PathBuf,
    pub shortcuts: Shortcuts,
    /// The wormhole data of each source, from the startup or the last refresh.
    pub wormhole_data: Vec<SourceData>,
    /// The rows of the Log window: the fetches since the start, newest last.
    pub log: Vec<LogEntry>,
    /// The refresh worker. `None` in the tests, and after the worker stops.
    refresher: Option<Refresher>,
    /// The time of the last manual refresh.
    refreshed_at: Option<Instant>,
    /// The note about the selected route after a refresh. It stays until the next search or
    /// refresh, because the status bar timer does not clear it.
    pub note: String,
    /// The SDE update messages of the startup load.
    pub startup_lines: Vec<String>,
    /// The start, the midpoints and the destination, in route order.
    pub waypoints: Vec<NodeIndex>,
    pub routes: Vec<Route>,
    /// The selected route. It is 0 when there are no routes.
    pub selected: usize,
    /// The selected step in the route table.
    pub selected_step: Option<usize>,
    /// The jumps from the start to each favourite.
    pub hubs: Vec<(NodeIndex, Option<u64>)>,
    /// The time of the last route search, with the favourite search.
    pub route_time: Option<Duration>,
    /// The time of the last route search, in Unix seconds. The labels use it.
    pub now: u64,
    /// The clock of the route search. The tests set a fixed time.
    clock: fn() -> u64,
    pub status: String,
    /// The time of the last change that waits for `APPLY_DELAY`, for example a drag in the settings.
    apply_at: Option<Instant>,
    /// The last Nexum map list, for the map name in the settings.
    pub map_names: Vec<MapInfo>,
    /// The logins, the tracking and the active route.
    pub pilots: Pilots,
}

impl Session {
    pub fn new(uni: Arc<Universe>, settings: Settings, cfg: Config, cfg_path: PathBuf, shortcuts: Shortcuts) -> Self {
        let mut session = Session {
            uni,
            settings,
            cfg,
            cfg_path: cfg_path.clone(),
            shortcuts,
            wormhole_data: Vec::new(),
            log: Vec::new(),
            refresher: None,
            refreshed_at: None,
            note: String::new(),
            startup_lines: Vec::new(),
            waypoints: Vec::new(),
            routes: Vec::new(),
            selected: 0,
            selected_step: None,
            hubs: Vec::new(),
            route_time: None,
            now: router_core::wormhole::now(),
            clock: router_core::wormhole::now,
            status: String::new(),
            apply_at: None,
            map_names: Vec::new(),
            pilots: Pilots::offline(active_path(&cfg_path)),
        };
        session.recompute();
        if let Some(warning) = session.shortcuts.warning.clone() {
            session.append_status(warning);
        }
        session
    }

    fn append_status(&mut self, text: String) {
        self.status = if self.status.is_empty() { text } else { format!("{} · {text}", self.status) };
    }

    pub fn selected_route(&self) -> Option<&Route> {
        self.routes.get(self.selected)
    }

    /// The part that waypoint `i` plays in the route.
    pub fn stop(&self, i: usize) -> Stop {
        match i {
            0 => Stop::Start,
            i if i + 1 == self.waypoints.len() => Stop::Destination,
            i => Stop::Midpoint(i),
        }
    }

    /// Search and save after the last change of a value that a pilot drags. Each call at `now`
    /// restarts the quiet time, so a drag gives one search and one write.
    pub fn apply_later(&mut self, now: Instant) {
        self.apply_at = Some(now);
    }

    /// Search and save if the quiet time of `apply_later` is over at `now`. Return the time left,
    /// or `None` if nothing waits.
    pub fn apply_due(&mut self, now: Instant) -> Option<Duration> {
        let left = APPLY_DELAY.saturating_sub(now.saturating_duration_since(self.apply_at?));
        if left.is_zero() {
            self.flush_apply();
            return None;
        }
        Some(left)
    }

    /// Search and save at once if a change waits. Call this before a window closes.
    pub fn flush_apply(&mut self) {
        if self.apply_at.take().is_some() {
            self.recompute();
            self.save();
        }
    }

    /// Find the routes and the favourite distances again.
    pub fn recompute(&mut self) {
        self.routes.clear();
        self.selected = 0;
        self.selected_step = None;
        self.status.clear();
        self.note.clear();
        if let Some(reason) = self.settings.rules.blocked_reason() {
            self.status = format!("Jump bridges off: {reason}");
        }
        let Some(&origin) = self.waypoints.first() else {
            self.hubs.clear();
            return;
        };
        let started = Instant::now();
        self.now = (self.clock)();
        let router = self.settings.router(&self.uni, self.now);
        let nodes = &self.waypoints;
        let settings = &self.settings;
        // The favourite search and the route search are independent, so they run at the same time.
        let (hubs, routes) = rayon::join(
            || router.jumps_to(origin, &settings.favourites),
            || {
                (nodes.len() >= 2).then(|| {
                    let (order, changed) = settings.order(&router, nodes)?;
                    router.routes(&order, settings.top).map(|routes| (routes, changed))
                })
            },
        );
        self.route_time = Some(started.elapsed());
        self.hubs = hubs;
        match routes {
            None => {}
            // The route table shows the optimized order, so the status line does not.
            Some(Ok((routes, _))) => self.routes = routes,
            Some(Err(e)) => self.status = e,
        }
    }

    /// Swap in a new map from the refresh worker. Search again with the waypoints. Keep the
    /// selected route and step if `kept_route` finds the route. Else select the first route.
    /// With no waypoints, only swap the map.
    pub fn apply_snapshot(&mut self, snap: Snapshot) {
        let old_uni = std::mem::replace(&mut self.uni, snap.uni);
        self.shortcuts = snap.shortcuts;
        self.wormhole_data = snap.all;
        log::push(&mut self.log, snap.log);
        if !self.waypoints.is_empty() {
            let old = self.selected_route().map(|r| r.path.clone());
            let old_step = self.selected_step;
            self.recompute();
            let kept = old.as_ref().and_then(|path| kept_route(&self.routes, &self.uni, &old_uni, path));
            if let Some(i) = kept {
                self.selected = i;
                self.selected_step = old_step;
            }
            if let Some(path) = &old {
                self.note = route_note(&old_uni, path, &self.uni, kept.is_some()).unwrap_or_default();
            }
        }
        // The status line shows a fetch problem, as at startup. While the text shows, a refresh does
        // not add it again. The status bar clears it after `STATUS_TIME`, so each refresh shows it again.
        if let Some(warning) = self.shortcuts.warning.clone()
            && !self.status.contains(&warning)
        {
            self.append_status(warning);
        }
    }

    /// Take the newest map from the refresh worker, if it sent one. Call this each frame.
    /// A stopped worker gives a note one time, and the map stays.
    pub fn poll_refresh(&mut self) {
        let Some(refresher) = &self.refresher else { return };
        match refresher.try_recv() {
            Ok(snap) => self.apply_snapshot(snap),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.refresher = None;
                self.note = refresh::STOPPED.into();
                let row = LogEntry { time: router_core::wormhole::now(), op: "refresh", ok: false, reason: "the worker stopped".into() };
                log::push(&mut self.log, [row]);
            }
        }
    }

    /// Ask the worker to refresh the wormholes now, also when a cache is fresh. Two calls in
    /// `REFRESH_GAP` send one message.
    pub fn refresh_now(&mut self, now: Instant) {
        let Some(refresher) = &self.refresher else {
            self.status = REFRESH_STOPPED.into();
            return;
        };
        if self.refreshed_at.is_some_and(|at| now.saturating_duration_since(at) < REFRESH_GAP) {
            self.status = REFRESH_WAIT.into();
            return;
        }
        self.refreshed_at = Some(now);
        refresher.refresh();
        self.status = REFRESHING.into();
    }

    /// The text of the Nexum map row: the name in the map list, else the name in the Nexum
    /// data of the current map, else the map ID.
    pub fn map_name(&self) -> String {
        nexum::map_label(&self.cfg.nexum, &self.map_names, &self.wormhole_data)
    }

    /// Add a waypoint at the end. The same system two times in a row is not a waypoint.
    pub fn add_waypoint(&mut self, node: NodeIndex) {
        if self.waypoints.last() == Some(&node) {
            self.status = format!("{} is already the last waypoint", self.uni.name(node));
            return;
        }
        self.waypoints.push(node);
        self.recompute();
    }

    /// Add the systems of a pasted list at the end, with one route search. Return the names
    /// that matched no system or more than one system. The other names are added.
    pub fn add_list(&mut self, text: &str) -> Vec<String> {
        let mut problems = Vec::new();
        let mut added = 0;
        for name in split_systems(text) {
            match self.uni.resolve(&name) {
                Ok(node) if self.waypoints.last() == Some(&node) => {}
                Ok(node) => {
                    self.waypoints.push(node);
                    added += 1;
                }
                Err(c) if c.is_empty() => problems.push(format!("Unknown system \"{name}\"")),
                Err(c) => problems.push(format!("\"{name}\" matches more than one system: {}", c.join(", "))),
            }
        }
        if added > 0 {
            self.recompute();
        }
        let s = if added == 1 { "" } else { "s" };
        let mut text = format!("Added {added} system{s}");
        if !problems.is_empty() {
            text = format!("{text} · {}", problems.join(" · "));
        }
        self.append_status(text);
        problems
    }

    /// Make `node` the start. A waypoint that is the same system moves to the start.
    pub fn set_start(&mut self, node: NodeIndex) {
        if self.waypoints.first() == Some(&node) {
            return;
        }
        if let Some(i) = self.waypoints.iter().position(|&n| n == node) {
            self.waypoints.remove(i);
        }
        self.waypoints.insert(0, node);
        self.recompute();
    }

    pub fn remove_waypoint(&mut self, i: usize) {
        if i < self.waypoints.len() {
            self.waypoints.remove(i);
            self.recompute();
        }
    }

    /// Swap waypoint `i` with waypoint `i - 1`.
    pub fn move_up(&mut self, i: usize) {
        if i > 0 && i < self.waypoints.len() {
            self.waypoints.swap(i, i - 1);
            self.recompute();
        }
    }

    pub fn move_down(&mut self, i: usize) {
        self.move_up(i + 1);
    }

    pub fn reverse(&mut self) {
        self.waypoints.reverse();
        self.recompute();
    }

    pub fn clear(&mut self) {
        self.waypoints.clear();
        self.recompute();
    }

    /// Replace the waypoints with a pasted list. Return the problems, as `add_list` does.
    /// `clear` recomputes, so no old route stays when no name resolves.
    pub fn replace_list(&mut self, text: &str) -> Vec<String> {
        self.clear();
        self.add_list(text)
    }

    /// Add a favourite. Return false if it is already a favourite.
    pub fn add_favourite(&mut self, node: NodeIndex) -> bool {
        if self.settings.favourites.contains(&node) {
            self.status = format!("{} is already a favourite", self.uni.name(node));
            return false;
        }
        self.settings.favourites.push(node);
        self.recompute();
        self.save();
        true
    }

    /// Save the config, and show the result on the status line.
    pub fn save(&mut self) {
        self.status = match self.write_config() {
            Ok(()) => "Config saved!".into(),
            Err(e) => e,
        };
    }

    /// Save the config. Show only an error on the status line.
    pub fn save_quietly(&mut self) {
        if let Err(e) = self.write_config() {
            self.status = e;
        }
    }

    /// Save the config after a change to a Nexum value. The refresh worker loads the new map at once.
    pub fn nexum_saved(&mut self) {
        self.status = match self.write_config() {
            Ok(()) => match &self.refresher {
                Some(refresher) => {
                    refresher.nexum(self.cfg.nexum.clone());
                    refresh::NEXUM_LOADING.into()
                }
                // No worker: a test, or a worker that stopped.
                None => "Nexum settings saved. Restart to load the new map.".into(),
            },
            Err(e) => e,
        };
    }

    /// Copy the settings to the config, and write the config file.
    fn write_config(&mut self) -> Result<(), String> {
        self.settings.store(&self.uni, &mut self.cfg);
        self.cfg.save(&self.cfg_path)
    }
}

impl eframe::App for RouterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(elapsed) = self.last_frame.map(|t| t.elapsed())
            && elapsed < MIN_FRAME_TIME
        {
            std::thread::sleep(MIN_FRAME_TIME - elapsed);
        }
        self.last_frame = Some(Instant::now());
        self.poll(ui.ctx());
        // The refresh worker sends a new map each 5 minutes.
        if let State::Ready(session, _) = &mut self.state {
            session.poll_refresh();
        }
        match &mut self.state {
            State::Loading(_) => crate::view::splash(ui, None),
            State::Failed(e) => crate::view::splash(ui, Some(e)),
            State::Ready(session, view) => view.show(ui, session),
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::config::NexumConfig;
    use router_core::labels::{jumps_label, route_extras};
    use router_core::log::LogEntry;
    use router_core::refresh::{Control, NEXUM_LOADING, Refresher, STOPPED};
    use router_core::test_support::{FIXTURE_TIME, hole, overlay_universe, settings, snapshot};
    use router_core::wormhole::{SourceData, SourceId, Wormhole};

    fn session(name: &str) -> Session {
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        let path = std::env::temp_dir().join(format!("eve-router-egui-test-{name}.json"));
        let mut s = Session::new(Arc::new(uni), settings, Config::default(), path, shortcuts);
        s.clock = || FIXTURE_TIME;
        s
    }

    const JITA: u32 = 30000142;
    const AMARR: u32 = 30002187;
    const PERIMETER: u32 = 30000144;

    /// A wormhole from Jita to Amarr, with the signature ABC at Jita. It gives a 1-jump route.
    fn jita_amarr() -> Wormhole {
        Wormhole { sig_a: Some("ABC-123".into()), ..hole(JITA, AMARR) }
    }

    #[test]
    fn a_refresh_keeps_the_same_path_and_step() {
        let mut s = session("refresh-keep");
        s.uni = snapshot(Vec::new()).uni;
        s.add_list("Jita, Amarr");
        assert!(s.routes.len() >= 2, "{}", s.status);
        s.selected = 1;
        s.selected_step = Some(3);
        let nodes = s.routes[1].path.nodes.clone();
        s.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(s.selected_route().unwrap().path.nodes, nodes);
        assert_eq!(s.selected_step, Some(3));
        assert_eq!(s.note, "");
    }

    #[test]
    fn a_refresh_selects_the_new_index_of_the_kept_path() {
        let mut s = session("refresh-moved");
        s.uni = snapshot(Vec::new()).uni;
        s.add_list("Jita, Amarr");
        s.selected_step = Some(2);
        let nodes = s.routes[0].path.nodes.clone();
        // The new wormhole route is first, so the kept path moves down.
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!(s.routes[0].wormholes, 1, "{}", s.status);
        assert_eq!(s.selected, 1);
        assert_eq!(s.selected_route().unwrap().path.nodes, nodes);
        assert_eq!(s.selected_step, Some(2));
    }

    /// Review focus: a stargate and a wormhole between the same two systems.
    #[test]
    fn a_refresh_keeps_the_selected_parallel_route() {
        let perimeter = || Wormhole { sig_a: Some("ABC-123".into()), ..hole(JITA, PERIMETER) };
        let mut s = session("refresh-parallel");
        s.uni = snapshot(vec![perimeter()]).uni;
        s.add_list("Jita, Perimeter");
        assert_eq!((s.routes[0].wormholes, s.routes[1].wormholes), (0, 1), "{}", s.status);
        s.selected = 1;
        s.apply_snapshot(snapshot(vec![perimeter()]));
        assert_eq!(s.selected, 1);
        assert_eq!(s.selected_route().unwrap().wormholes, 1);
    }

    #[test]
    fn a_closed_wormhole_gives_a_note_until_the_next_search() {
        let mut s = session("refresh-closed");
        s.settings.top = 1;
        s.uni = snapshot(vec![jita_amarr()]).uni;
        s.add_list("Jita, Amarr");
        assert_eq!(s.routes[0].wormholes, 1, "{}", s.status);
        s.selected_step = Some(1);
        s.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(s.routes[0].wormholes, 0);
        assert_eq!((s.selected, s.selected_step), (0, None));
        assert_eq!(s.note, "Wormhole ABC closed: the route changed");
        s.recompute();
        assert_eq!(s.note, "");
    }

    #[test]
    fn a_new_first_route_gives_a_note() {
        let mut s = session("refresh-new");
        s.settings.top = 1;
        s.uni = snapshot(Vec::new()).uni;
        s.add_list("Jita, Amarr");
        assert_eq!(s.routes[0].wormholes, 0);
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!(s.routes[0].wormholes, 1);
        assert_eq!(s.note, "Wormholes updated: a new route is first");
    }

    #[test]
    fn a_refresh_with_no_waypoints_only_swaps_the_map() {
        let mut s = session("refresh-none");
        s.status = "keep".into();
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!((s.shortcuts.wormholes, s.wormhole_data.len()), (1, 1));
        assert!(s.routes.is_empty());
        assert_eq!((s.status.as_str(), s.note.as_str()), ("keep", ""));
    }

    #[test]
    fn the_poll_applies_a_snapshot_and_reports_a_stopped_worker() {
        let (refresher, snapshots, _control) = Refresher::fake();
        let mut s = session("poll-refresh");
        s.refresher = Some(refresher);
        snapshots.send(snapshot(vec![jita_amarr()])).unwrap();
        s.poll_refresh();
        assert_eq!(s.shortcuts.wormholes, 1);
        drop(snapshots);
        s.poll_refresh();
        assert_eq!(s.note, STOPPED);
        assert!(s.refresher.is_none());
    }

    #[test]
    fn a_refresh_adds_its_rows_to_the_log() {
        let mut s = session("log-rows");
        let row = |time| LogEntry { time, op: "nexum.fetch", ok: true, reason: "Loaded 1 wormhole connection".into() };
        let mut snap = snapshot(Vec::new());
        snap.log = vec![row(1)];
        s.apply_snapshot(snap);
        let mut snap = snapshot(Vec::new());
        snap.log = vec![row(2)];
        s.apply_snapshot(snap);
        assert_eq!(s.log, [row(1), row(2)]);
    }

    #[test]
    fn a_stopped_worker_gives_a_log_row() {
        let (refresher, snapshots, _control) = Refresher::fake();
        let mut s = session("log-stopped");
        s.refresher = Some(refresher);
        drop(snapshots);
        s.poll_refresh();
        assert_eq!(s.log.len(), 1);
        assert_eq!((s.log[0].op, s.log[0].ok, s.log[0].reason.as_str()), ("refresh", false, "the worker stopped"));
    }

    #[test]
    fn refresh_now_asks_the_worker_and_waits_between_two_calls() {
        let (refresher, _snapshots, control) = Refresher::fake();
        let mut s = session("refresh-now");
        s.refresher = Some(refresher);
        let start = Instant::now();
        s.refresh_now(start);
        assert!(matches!(control.try_recv(), Ok(Control::Refresh)));
        assert_eq!(s.status, REFRESHING);
        // A second call soon after sends nothing.
        s.refresh_now(start + Duration::from_secs(5));
        assert!(control.try_recv().is_err());
        assert_eq!(s.status, REFRESH_WAIT);
        // After the gap, the call sends a message again.
        s.refresh_now(start + REFRESH_GAP);
        assert!(matches!(control.try_recv(), Ok(Control::Refresh)));
    }

    #[test]
    fn refresh_now_without_a_worker_says_so() {
        let mut s = session("refresh-none-worker");
        s.refresh_now(Instant::now());
        assert_eq!(s.status, REFRESH_STOPPED);
    }

    #[test]
    fn a_nexum_change_asks_the_worker_for_the_new_map() {
        let (refresher, _snapshots, control) = Refresher::fake();
        let mut s = session("nexum-refresh");
        s.refresher = Some(refresher);
        s.cfg.nexum.url = Some("https://nexum.example".into());
        s.nexum_saved();
        assert_eq!(s.status, NEXUM_LOADING);
        let Ok(Control::Nexum(sent)) = control.try_recv() else { panic!("no Nexum message") };
        assert_eq!(sent.url.as_deref(), Some("https://nexum.example"));
    }

    #[test]
    fn map_name_uses_the_name_in_the_wormhole_data() {
        let mut s = session("map-name-data");
        s.cfg.nexum = NexumConfig { url: Some("https://nexum.example".into()), key: None, map_id: Some("m1".into()) };
        assert_eq!(s.map_name(), "m1");
        let origin = router_core::sources::nexum::map_url(&s.cfg.nexum);
        s.wormhole_data = vec![SourceData { source: SourceId::Nexum, fetched_at: 0, origin, name: Some("Home".into()), holes: Vec::new() }];
        assert_eq!(s.map_name(), "Home");
    }

    #[test]
    fn a_dragged_value_applies_once_after_the_quiet_time() {
        let mut s = session("apply-later");
        let _ = std::fs::remove_file(&s.cfg_path);
        s.add_list("Jita, Amarr");
        s.status = "unread".into();
        let start = Instant::now();
        s.settings.costs.cap_weight = 1.0;
        s.apply_later(start);
        // A second change restarts the quiet time.
        s.settings.costs.cap_weight = 2.0;
        s.apply_later(start + Duration::from_millis(300));
        assert_eq!(s.apply_due(start + Duration::from_millis(600)), Some(APPLY_DELAY - Duration::from_millis(300)));
        assert_eq!(s.status, "unread", "no search yet");
        assert!(!s.cfg_path.exists(), "no write yet");
        assert_eq!(s.apply_due(start + APPLY_DELAY + Duration::from_millis(300)), None);
        assert_eq!(s.status, "Config saved!");
        assert!(std::fs::read_to_string(&s.cfg_path).unwrap().contains("2.0"));
        // Nothing waits now.
        s.status.clear();
        assert_eq!(s.apply_due(start + Duration::from_secs(10)), None);
        assert_eq!(s.status, "");
    }

    #[test]
    fn closing_a_window_applies_a_waiting_change_at_once() {
        let mut s = session("apply-flush");
        let _ = std::fs::remove_file(&s.cfg_path);
        s.flush_apply();
        assert!(!s.cfg_path.exists(), "nothing waited");
        s.settings.costs.unknown_sig_penalty = 7.0;
        s.apply_later(Instant::now());
        s.flush_apply();
        assert!(std::fs::read_to_string(&s.cfg_path).unwrap().contains("7.0"));
        assert_eq!(s.apply_due(Instant::now() + Duration::from_secs(10)), None);
    }

    fn names(s: &Session) -> Vec<&str> {
        s.waypoints.iter().map(|&n| s.uni.name(n)).collect()
    }

    /// The same routes as the `--print` snapshot of the TUI crate.
    #[test]
    fn replace_with_no_good_name_clears_the_routes() {
        let mut s = session("replace");
        s.add_list("Jita, Amarr");
        assert!(!s.routes.is_empty(), "{}", s.status);
        let problems = s.replace_list("Nowhere");
        assert_eq!(problems.len(), 1);
        assert!(s.waypoints.is_empty() && s.routes.is_empty());
    }

    #[test]
    fn a_lost_load_thread_shows_an_error() {
        let (tx, rx) = mpsc::channel::<Result<Session, String>>();
        drop(tx);
        let mut app = RouterApp { state: State::Loading(rx), last_frame: None };
        app.poll(&egui::Context::default());
        let State::Failed(text) = &app.state else { panic!("the state is not Failed") };
        assert_eq!(text, LOAD_STOPPED);
    }

    #[test]
    fn waypoints_give_the_print_routes() {
        let mut s = session("routes");
        let jita = s.uni.exact("Jita").unwrap();
        let ualx = s.uni.exact("UALX-3").unwrap();
        s.add_waypoint(jita);
        assert!(s.routes.is_empty());
        assert_eq!(s.hubs.len(), 0);
        s.set_start(ualx);
        assert_eq!(names(&s), ["UALX-3", "Jita"]);
        let lines: Vec<String> = s.routes.iter().map(|r| format!("{}{}", jumps_label(r.jumps), route_extras(r))).collect();
        assert_eq!(lines.len(), 3, "{}", s.status);
        // The list shows three different routes. A route that only detours the first one is not in it.
        assert_eq!(lines, ["30 jumps", "31 jumps", "41 jumps"]);
    }

    #[test]
    fn waypoint_operations() {
        let mut s = session("ops");
        for name in ["Jita", "Amarr", "Dodixie"] {
            let node = s.uni.exact(name).unwrap();
            s.add_waypoint(node);
        }
        // The same system two times in a row is not added.
        let dodixie = s.uni.exact("Dodixie").unwrap();
        s.add_waypoint(dodixie);
        assert_eq!(names(&s), ["Jita", "Amarr", "Dodixie"]);
        assert!(s.status.contains("already the last waypoint"), "{}", s.status);
        assert_eq!(s.stop(1), Stop::Midpoint(1));
        assert_eq!(s.stop(2), Stop::Destination);

        s.move_up(2);
        assert_eq!(names(&s), ["Jita", "Dodixie", "Amarr"]);
        s.move_down(0);
        assert_eq!(names(&s), ["Dodixie", "Jita", "Amarr"]);
        // The first waypoint cannot move up, and the last cannot move down.
        s.move_up(0);
        s.move_down(2);
        assert_eq!(names(&s), ["Dodixie", "Jita", "Amarr"]);
        s.reverse();
        assert_eq!(names(&s), ["Amarr", "Jita", "Dodixie"]);
        // A waypoint that becomes the start moves, so it is not in the route two times.
        s.set_start(dodixie);
        assert_eq!(names(&s), ["Dodixie", "Amarr", "Jita"]);
        s.remove_waypoint(1);
        assert_eq!(names(&s), ["Dodixie", "Jita"]);
        assert_eq!(s.routes.len(), 3);
        s.clear();
        assert!(s.waypoints.is_empty() && s.routes.is_empty());
    }

    #[test]
    fn pasted_list_adds_waypoints() {
        let mut s = session("list");
        let problems = s.add_list("UALX-3\r\nUALX-3\nNowhere\n\njita\n");
        assert_eq!(names(&s), ["UALX-3", "Jita"]);
        assert_eq!(problems, ["Unknown system \"Nowhere\""]);
        assert_eq!(s.routes.len(), 3);
        assert!(s.status.contains("Added 2 systems"), "{}", s.status);
        // A name that matches more than one system is not added.
        let problems = s.add_list("Ama, Amarr");
        assert_eq!(names(&s), ["UALX-3", "Jita", "Amarr"]);
        assert!(problems[0].contains("matches more than one system"), "{problems:?}");
    }

    /// A list of Jove Observatory systems, as a user pastes it: one name for each line.
    const JOVE: &str = "0P-U0Q\n16AM-3\n3L3N-X\n9-980U\nBW-WJ2\nF-ZBO0\nFE-6YQ\nH-HWQR\nJI1-SY\nKW-OAM\nMS1-KJ\nNZW-ZO\nPEK-8Z\nT-AKQZ\nWB-AYY\nY-EQ0C\nY-ORBJ\nZD1-Z2\nZO-P5K\n";

    #[test]
    fn jove_list_with_line_breaks_and_commas() {
        let mut s = session("jove");
        assert_eq!(s.add_list(JOVE), Vec::<String>::new(), "{}", s.status);
        assert_eq!(s.waypoints.len(), 19);
        assert!(!s.routes.is_empty(), "{}", s.status);
        let mut csv = session("jove-csv");
        csv.add_list(&JOVE.trim().replace('\n', ", "));
        assert_eq!(csv.waypoints, s.waypoints);
        // 17 midpoints: "Optimize order" changes their order and keeps the start and the destination.
        s.settings.optimize = true;
        s.recompute();
        assert!(!s.routes.is_empty(), "{}", s.status);
        let route = &s.routes[0];
        assert_eq!(route.path.nodes[0], s.waypoints[0]);
        assert_eq!(*route.path.nodes.last().unwrap(), s.waypoints[18]);
    }

    #[test]
    fn favourites_save_the_config() {
        let mut s = session("favourites");
        let _ = std::fs::remove_file(&s.cfg_path);
        let rens = s.uni.exact("Rens").unwrap();
        assert!(s.add_favourite(rens));
        assert!(!s.add_favourite(rens));
        let saved = Config::load(&s.cfg_path).unwrap();
        assert_eq!(saved.favourites, Some(vec!["Rens".to_string()]));
        assert_eq!(saved.capital.as_deref(), Some("JK-Q77"));
        std::fs::remove_file(&s.cfg_path).unwrap();
    }
}
