//! The window state, the startup load and the route search.

use crate::notice::{Level, Notice};
use crate::theme;
use crate::view::View;
use petgraph::graph::NodeIndex;
use router_core::config::{AppPaths, Config};
use router_core::error::FileError;
use router_core::esi::pilots::Pilots;
use router_core::esi::store::{Keyring, StoreError, TokenStore};
use router_core::esi::{self, active::active_path};
use router_core::labels::Shortcuts;
use router_core::log::{self, LogEntry, LogEntryKind, LogOp};
use router_core::overlay::{self, OverlayReport, Pair};
use router_core::refresh::{self, Refresher, RouteChange, Setup, Snapshot, kept_route, route_change};
use router_core::route::{Route, Stop};
use router_core::settings::{Settings, split_systems};
use router_core::sources::nexum::MapInfo;
use router_core::sources::{OverlayWarning, evemetro};
use router_core::startup::StartupError;
use router_core::universe::Universe;
use router_core::wormhole::{ClosedHole, SourceData};
use router_core::{sde_update, startup};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// A message from the load thread.
enum LoadMsg {
    /// The load starts a new step.
    Progress(startup::Progress),
    /// The load ends.
    Done(Box<Result<Session, String>>),
}

/// The load thread and the step that it reported last.
struct Loading {
    rx: Receiver<LoadMsg>,
    step: Option<startup::Progress>,
}

enum State {
    /// The startup load runs on a thread, because it can take up to 35 s.
    Loading(Loading),
    Ready(Box<Session>, Box<View>),
    Failed(String),
}

pub struct RouterApp {
    state: State,
}

/// The quiet time after the last change of a dragged value, before the app searches and saves.
pub const APPLY_DELAY: Duration = Duration::from_millis(500);

/// The least time between two manual refreshes. A Nexum map costs one request for each system.
pub const REFRESH_GAP: Duration = Duration::from_secs(15);

/// What the status bar says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Ready,
    Refreshing,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Ready => "Ready",
            Status::Refreshing => "Refreshing the wormholes…",
        }
    }
}

pub const REFRESH_STOPPED: &str = "The refresh worker stopped";

/// The error when the load thread stops without a result. A panic in the load does this.
const LOAD_STOPPED: &str = "The startup load stopped with an internal error. Start the router from a terminal to see the cause.";

impl RouterApp {
    /// Set the theme and start the load. `ctx` gets a repaint request when the load ends.
    pub fn new(ctx: egui::Context) -> Self {
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = load(ctx.clone(), &tx).map_err(|e| e.to_string());
            let _ = tx.send(LoadMsg::Done(Box::new(result)));
            ctx.request_repaint();
        });
        RouterApp { state: State::Loading(Loading { rx, step: None }) }
    }

    /// Take the load result, if the thread sent it.
    fn poll(&mut self, ctx: &egui::Context) {
        let State::Loading(loading) = &mut self.state else { return };
        let result = loop {
            match loading.rx.try_recv() {
                Ok(LoadMsg::Progress(step)) => loading.step = Some(step),
                Ok(LoadMsg::Done(result)) => break *result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => break Err(LOAD_STOPPED.into()),
            }
        };
        self.state = match result {
            Ok(session) => {
                session.apply_theme(ctx);
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

/// The OS keyring as the token store, or the reason why there is none.
fn keyring() -> Result<Box<dyn TokenStore>, StoreError> {
    Keyring::open().map(|keyring| Box::new(keyring) as Box<dyn TokenStore>)
}

/// True if the env var is "1".
fn env_flag(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

/// The client ID of the EVE developer app. The env var at run time wins over the value at build
/// time. The repository holds no client ID. Without one, the router cannot log in.
fn client_id() -> Option<String> {
    let runtime = std::env::var(esi::CLIENT_ID_VAR).ok();
    runtime.or_else(|| option_env!("EVE_ROUTER_CLIENT_ID").map(str::to_owned)).map(|id| id.trim().to_owned()).filter(|id| !id.is_empty())
}

/// The startup load, with the config file and the default SDE directory. The arguments
/// of the command line are the first waypoints, for example `eve-router-egui Jita "New Caldari"`.
fn load(ctx: egui::Context, tx: &mpsc::Sender<LoadMsg>) -> Result<Session, StartupError> {
    let paths = AppPaths::detect();
    let cfg = Config::load(&paths.config)?;
    // Each step wakes the window, so the splash screen shows it.
    let progress = |step| {
        let _ = tx.send(LoadMsg::Progress(step));
        ctx.request_repaint();
    };
    // EVE Metro needs a logged-in character, so it stays off without an EVE client ID.
    let sso_ready = client_id().is_some();
    let startup_cfg = Config { eve_metro: cfg.eve_metro.gated(sso_ready), ..cfg.clone() };
    let startup::Loaded { sde, hulls, snapshot, base, report, types } =
        startup::load(&startup_cfg, &paths, env_flag(sde_update::SKIP_ENV), progress)?;
    let startup_lines: Vec<String> = sde.message().into_iter().collect();
    let Snapshot { uni, shortcuts, all, log, closed: _ } = snapshot;
    let settings = Settings::from_config(&cfg, &uni, hulls)?;
    // The worker gets the wormholes again each 5 minutes, and wakes the window after each refresh.
    let wake = ctx.clone();
    let pairs = report.pairs.clone();
    let mut setup = Setup::new(base, report, types, cfg.nexum.clone(), &paths);
    setup.last = all.clone();
    setup.metro = cfg.eve_metro.gated(sso_ready);
    let refresher = Refresher::start(setup, router_core::wormhole::now, move || wake.request_repaint());
    let mut session = Session::new(uni, settings, cfg, paths.config.clone(), shortcuts);
    session.wormhole_data = all;
    session.bridges = pairs;
    session.log = log;
    session.refresher = Some(refresher);
    for line in startup_lines {
        session.notify(Level::Info, line);
    }
    // A tracker event or a login result repaints the window.
    session.pilots = Pilots::open(&paths.config, keyring(), client_id(), Arc::new(move || ctx.request_repaint()));
    // The command line can give the start, the midpoints and the destination.
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
    /// The jump bridges, one pair for each bridge, in the order of the names.
    pub bridges: Vec<Pair>,
    /// The wormholes that the pilot closed, from the last refresh. The worker holds them for the
    /// session, so they never reach the config file.
    pub closed: Vec<ClosedHole>,
    /// The rows of the Log window: the fetches since the start, newest last.
    pub log: Vec<LogEntry>,
    /// The refresh worker. `None` in the tests, and after the worker stops.
    refresher: Option<Refresher>,
    /// The time of the last manual refresh.
    refreshed_at: Option<Instant>,
    /// The SDE update messages of the startup load.
    /// The messages for the pilot. The window moves them to the toasts at the next frame.
    pub notices: Vec<Notice>,
    /// The reason for the last "Jump bridges off" notice, so a notice shows only when it changes.
    bridges_reason: Option<String>,
    /// The fetch warning of the last notice, so a notice shows only when it changes.
    fetch_warnings: Vec<OverlayWarning>,
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
    pub status: Status,
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
            bridges: Vec::new(),
            closed: Vec::new(),
            log: Vec::new(),
            refresher: None,
            refreshed_at: None,
            notices: Vec::new(),
            bridges_reason: None,
            fetch_warnings: Vec::new(),
            waypoints: Vec::new(),
            routes: Vec::new(),
            selected: 0,
            selected_step: None,
            hubs: Vec::new(),
            route_time: None,
            now: router_core::wormhole::now(),
            clock: router_core::wormhole::now,
            status: Status::Ready,
            apply_at: None,
            map_names: Vec::new(),
            pilots: Pilots::offline(active_path(&cfg_path)),
        };
        session.recompute();
        session.fetch_warnings = session.shortcuts.warnings.clone();
        for warning in session.fetch_warnings.clone() {
            session.notify(Level::Warning, warning.to_string());
        }
        session
    }

    /// Tell the pilot something. An identical notice that waits already is not added again.
    pub fn notify(&mut self, level: Level, text: impl Into<String>) {
        let notice = Notice { level, text: text.into() };
        if !self.notices.contains(&notice) {
            self.notices.push(notice);
        }
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

    /// Add the system to the avoid list, or take it off if it is in the list. Search again and save.
    pub fn toggle_avoid_system(&mut self, node: NodeIndex) {
        if self.settings.avoid.has_system(node) {
            self.settings.avoid.remove_system(node);
        } else {
            self.settings.avoid.add_system(node);
        }
        self.avoid_changed();
    }

    /// Add the region to the avoid list, or take it off if it is in the list. Search again and save.
    pub fn toggle_avoid_region(&mut self, region: &str) {
        if self.settings.avoid.has_region(region) {
            self.settings.avoid.remove_region(region);
        } else {
            self.settings.avoid.add_region(region);
        }
        self.avoid_changed();
    }

    /// Set the "never" switch of a system on the avoid list. Search again and save.
    pub fn set_avoid_never_system(&mut self, node: NodeIndex, never: bool) {
        self.settings.avoid.set_system_never(node, never);
        self.avoid_changed();
    }

    /// Set the "never" switch of a region on the avoid list. Search again and save.
    pub fn set_avoid_never_region(&mut self, region: &str, never: bool) {
        self.settings.avoid.set_region_never(region, never);
        self.avoid_changed();
    }

    /// Empty the avoid list. Search again and save.
    pub fn clear_avoid(&mut self) {
        self.settings.avoid.clear();
        self.avoid_changed();
    }

    /// Search again and save after a change of the avoid list. The status stays quiet: the pilot
    /// just made the change, and the route table shows the result.
    fn avoid_changed(&mut self) {
        self.recompute();
        self.save();
    }

    /// Leave a wormhole out of the routes until the app stops. The worker builds the map again,
    /// and the next snapshot searches again. Nothing is saved.
    pub fn close_link(&mut self, hole: ClosedHole) {
        match &self.refresher {
            Some(refresher) => refresher.close(hole),
            None => self.notify(Level::Error, REFRESH_STOPPED),
        }
    }

    /// Put a closed wormhole back into the routes.
    pub fn reopen_link(&mut self, hole: &ClosedHole) {
        let Some(refresher) = &self.refresher else {
            self.notify(Level::Error, REFRESH_STOPPED);
            return;
        };
        refresher.reopen(hole.clone());
        // The list shows the change at once. The next snapshot confirms it.
        if let Some(i) = self.closed.iter().position(|c| c == hole) {
            self.closed.remove(i);
        }
    }

    /// Add the bridges of a pasted SMT list to the list, or replace the list with them. Save the
    /// list, and give the map to the refresh worker. Return the lines that did not give a bridge.
    pub fn import_bridges(&mut self, text: &str, replace: bool) -> Vec<String> {
        let report = overlay::parse_bridges(&self.uni, text);
        let mut problems: Vec<String> = report.unknown.iter().map(|name| format!("Unknown system \"{name}\"")).collect();
        problems.extend(report.same.iter().map(|name| format!("{name} cannot bridge to itself")));
        let before = if replace { Vec::new() } else { self.bridges.clone() };
        let mut pairs = before.clone();
        pairs.extend(report.pairs.iter().filter(|p| !before.contains(p)));
        let added = pairs.len() - before.len();
        self.set_bridges(pairs);
        let s = if added == 1 { "" } else { "s" };
        // The dialog shows each problem line, so the toast shows only a clean import.
        if problems.is_empty() {
            self.notify(Level::Info, format!("Added {added} jump bridge{s}"));
        }
        problems
    }

    /// Take the bridge at row `i` off the list.
    pub fn remove_bridge(&mut self, i: usize) {
        if i < self.bridges.len() {
            let mut pairs = self.bridges.clone();
            pairs.remove(i);
            self.set_bridges(pairs);
        }
    }

    /// Empty the list of bridges.
    pub fn clear_bridges(&mut self) {
        self.set_bridges(Vec::new());
    }

    /// Keep `pairs` as the bridges: write the bridge file, and tell the worker, which builds the new map.
    /// Without a worker, the new bridges load at the next start.
    fn set_bridges(&mut self, mut pairs: Vec<Pair>) {
        pairs.sort_by_cached_key(|&(a, b)| (self.uni.name(a).to_lowercase(), self.uni.name(b).to_lowercase()));
        let path = self.cfg_path.with_file_name(router_core::config::BRIDGES_FILE);
        if let Err(e) = overlay::save_bridges(&path, &self.uni, &pairs) {
            self.notify(Level::Error, e.to_string());
            return;
        }
        self.bridges = pairs;
        match &self.refresher {
            Some(refresher) => {
                refresher.bridges(OverlayReport { bridges: self.bridges.len() * 2, pairs: self.bridges.clone(), ..Default::default() });
            }
            None => self.notify(Level::Info, "Jump bridges saved. Restart to load them."),
        }
    }

    /// Find the routes and the favourite distances again.
    pub fn recompute(&mut self) {
        if let Some(reason) = self.plan() {
            self.notify(Level::Warning, format!("Jump bridges off: {reason}"));
        }
    }

    /// Find the routes again, as `recompute` does. Return the reason when the jump bridges turned off
    /// with this search, so the caller can fold it into its own notice.
    pub fn plan(&mut self) -> Option<String> {
        self.routes.clear();
        self.selected = 0;
        self.selected_step = None;
        self.status = Status::Ready;
        let reason = self.settings.rules.blocked_reason();
        let turned_off = if reason != self.bridges_reason { reason.clone() } else { None };
        self.bridges_reason = reason;
        self.search();
        turned_off
    }

    /// Find the favourite distances and the routes for the waypoints.
    fn search(&mut self) {
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
                    let order = settings.order(&router, nodes)?;
                    router.routes(&order, settings.top)
                })
            },
        );
        self.route_time = Some(started.elapsed());
        self.hubs = hubs;
        match routes {
            None => {}
            Some(Ok(routes)) => self.routes = routes,
            Some(Err(e)) => self.notify(Level::Error, self.settings.explain(&e)),
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
        // An entry that left the list without a reopen retired: its wormhole left the map.
        let retired = self.closed.iter().any(|c| !snap.closed.contains(c));
        self.closed = snap.closed;
        if retired {
            self.notify(Level::Info, "A closed wormhole left the map.");
        }
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
                match route_change(&old_uni, path, &self.uni, kept.is_some()) {
                    Some(RouteChange::Collapsed) => self.notify(Level::Warning, "A wormhole on your route collapsed."),
                    Some(RouteChange::Faster) => self.notify(Level::Info, "A faster route has become available."),
                    None => {}
                }
            }
        }
        // A fetch problem gives a notice, as at startup. The same problem on the next refresh
        // gives no second notice. A problem that goes and comes back gives a new one.
        let warnings = self.shortcuts.warnings.clone();
        let new: Vec<String> = warnings.iter().filter(|w| !self.fetch_warnings.contains(w)).map(ToString::to_string).collect();
        for text in new {
            self.notify(Level::Warning, text);
        }
        self.fetch_warnings = warnings;
    }

    /// Take the newest map from the refresh worker, if it sent one. Call this each frame.
    /// A stopped worker gives a toast one time, and the map stays.
    pub fn poll_refresh(&mut self) {
        let Some(refresher) = &self.refresher else { return };
        match refresher.try_recv() {
            Ok(snap) => self.apply_snapshot(snap),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.refresher = None;
                self.notify(Level::Error, REFRESH_STOPPED);
                let row = LogEntry {
                    time: router_core::wormhole::now(),
                    op: LogOp::Refresh,
                    kind: LogEntryKind::WorkerStopped,
                    message: "the worker stopped".into(),
                };
                log::push(&mut self.log, [row]);
            }
        }
    }

    /// Ask the worker to refresh the wormholes now, also when a cache is fresh. Two calls in
    /// `REFRESH_GAP` send one message.
    pub fn refresh_now(&mut self, now: Instant) {
        let Some(refresher) = &self.refresher else {
            self.notify(Level::Error, REFRESH_STOPPED);
            return;
        };
        if self.refreshed_at.is_some_and(|at| now.saturating_duration_since(at) < REFRESH_GAP) {
            return;
        }
        self.refreshed_at = Some(now);
        refresher.refresh();
        self.status = Status::Refreshing;
    }

    /// Add a waypoint at the end. The same system two times in a row is not a waypoint.
    pub fn add_waypoint(&mut self, node: NodeIndex) {
        if self.waypoints.last() == Some(&node) {
            self.notify(Level::Info, format!("{} is already the last waypoint", self.uni.name(node)));
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
                Err(e) => problems.push(e.to_string()),
            }
        }
        if added > 0 {
            self.recompute();
        }
        let s = if added == 1 { "" } else { "s" };
        // The dialog shows each problem line, so the toast gives only the count.
        let mut text = format!("Added {added} system{s}");
        if !problems.is_empty() {
            text = format!("{text}, {} not found", problems.len());
        }
        self.notify(if problems.is_empty() { Level::Info } else { Level::Warning }, text);
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
            self.notify(Level::Info, format!("{} is already a favourite", self.uni.name(node)));
            return false;
        }
        self.settings.favourites.push(node);
        self.recompute();
        self.save();
        true
    }

    /// Save the config. A save that works gives no message: the pilot just made the change.
    /// Only an error gives a notice.
    pub fn save(&mut self) {
        if let Err(e) = self.write_config() {
            self.notify(Level::Error, e.to_string());
        }
    }

    /// Save the config after a change to a Nexum value. The refresh worker loads the new map at once.
    pub fn nexum_saved(&mut self) {
        match self.write_config() {
            Ok(()) => match &self.refresher {
                Some(refresher) => {
                    refresher.nexum(self.cfg.nexum.clone());
                    self.notify(Level::Info, refresh::NEXUM_LOADING);
                }
                // No worker: a test, or a worker that stopped.
                None => self.notify(Level::Info, "Nexum settings saved. Restart to load the new map."),
            },
            Err(e) => self.notify(Level::Error, e.to_string()),
        }
    }

    /// Save the config after a change to an EVE Metro value. The refresh worker loads the map at
    /// once. Without an EVE client ID the source stays off, and the worker gets the settings off.
    pub fn eve_metro_saved(&mut self) {
        if let Err(e) = self.write_config() {
            self.notify(Level::Error, e.to_string());
            return;
        }
        let sso_ready = self.pilots.shows_characters();
        let Some(refresher) = &self.refresher else {
            // No worker: a test, or a worker that stopped.
            self.notify(Level::Info, "EVE Metro settings saved. Restart to load the map.");
            return;
        };
        refresher.eve_metro(self.cfg.eve_metro.gated(sso_ready));
        if !sso_ready {
            self.notify(Level::Warning, evemetro::NEEDS_CLIENT_ID);
        } else if self.cfg.eve_metro.enabled {
            self.notify(Level::Info, refresh::EVE_METRO_LOADING);
        }
    }

    /// The active faction theme.
    pub fn faction_theme(&self) -> theme::FactionTheme {
        self.cfg.theme.unwrap_or_default()
    }

    /// Set and apply a faction theme.
    pub fn set_faction_theme(&mut self, theme: theme::FactionTheme, ctx: &egui::Context) {
        self.cfg.theme = Some(theme);
        theme::apply(ctx, theme, self.cfg.compact);
        self.save();
    }

    /// Whether compact UI mode is active.
    pub fn compact_mode(&self) -> bool {
        self.cfg.compact
    }

    /// Set and apply compact layout mode.
    pub fn set_compact_mode(&mut self, compact: bool, ctx: &egui::Context) {
        self.cfg.compact = compact;
        theme::apply(ctx, self.faction_theme(), compact);
        self.save();
    }

    /// Set the position of the routing overlay for a place, and save it. `None` resets the position.
    #[allow(dead_code, reason = "The Overlay settings page calls this in the next step.")]
    pub fn set_overlay_position(&mut self, place: router_core::config::Place, pos: Option<router_core::config::Pos>) {
        self.cfg.routing_overlay.set_position(place, pos);
        self.save();
    }

    /// Apply the theme and compact mode from config to the egui context.
    pub fn apply_theme(&self, ctx: &egui::Context) {
        theme::apply(ctx, self.faction_theme(), self.cfg.compact);
    }

    /// Copy the settings to the config, and write the config file.
    fn write_config(&mut self) -> Result<(), FileError> {
        self.settings.store(&self.uni, &mut self.cfg);
        self.cfg.save(&self.cfg_path)
    }
}

impl eframe::App for RouterApp {
    /// eframe calls this before each `ui`, and also while the main window is minimized. The overlay
    /// reads the focused window here, so it works with the router window out of sight.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let State::Ready(session, view) = &mut self.state {
            view.overlay.logic(ctx, session);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll(ui.ctx());
        // The refresh worker sends a new map each 5 minutes.
        if let State::Ready(session, _) = &mut self.state {
            session.poll_refresh();
        }
        // The window has no OS frame: the app draws the title bar and the border, on every screen.
        crate::chrome::square_window(frame);
        let detail = match &self.state {
            State::Ready(session, _) => session.uni.build.map(|build| format!("SDE {build}")).unwrap_or_default(),
            _ => String::new(),
        };
        crate::chrome::title_bar(ui, "EVE Router", &detail);
        match &mut self.state {
            State::Loading(loading) => crate::view::splash(ui, None, loading.step.as_ref()),
            State::Failed(e) => crate::view::splash(ui, Some(e), None),
            State::Ready(session, view) => {
                view.show(ui, session);
                view.overlay.show(ui.ctx(), session);
            }
        }
        crate::chrome::drag_through_modal(ui.ctx());
        crate::chrome::paint_border(ui.ctx(), theme::accent(ui));
        crate::chrome::resize_edges(ui.ctx());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::config::ApiKey;
    use router_core::labels::{jumps_label, route_extras};
    use router_core::log::LogEntry;
    use router_core::refresh::{Control, EVE_METRO_LOADING, NEXUM_LOADING};
    use router_core::sources::FetchError;
    use router_core::test_support::{FIXTURE_TIME, fake_refresher, hole, overlay_universe, settings, snapshot};
    use router_core::wormhole::{SourceId, Wormhole};

    /// A session that writes its config to a new temporary directory. Keep the directory until the test ends.
    fn session() -> (Session, tempfile::TempDir) {
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default(), &Default::default());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eve-router.json");
        let mut s = Session::new(Arc::new(uni), settings, Config::default(), path, shortcuts);
        s.clock = || FIXTURE_TIME;
        (s, tmp)
    }

    /// The level and the text of the newest notice.
    fn last(s: &Session) -> Option<(Level, &str)> {
        s.notices.last().map(|n| (n.level, n.text.as_str()))
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
        let (mut s, _tmp) = session();
        s.uni = snapshot(Vec::new()).uni;
        s.add_list("Jita, Amarr");
        assert!(s.routes.len() >= 2, "{:?}", s.status);
        s.selected = 1;
        s.selected_step = Some(3);
        let nodes = s.routes[1].path.nodes.clone();
        s.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(s.selected_route().unwrap().path.nodes, nodes);
        assert_eq!(s.selected_step, Some(3));
    }

    #[test]
    fn a_refresh_selects_the_new_index_of_the_kept_path() {
        let (mut s, _tmp) = session();
        s.uni = snapshot(Vec::new()).uni;
        s.add_list("Jita, Amarr");
        s.selected_step = Some(2);
        let nodes = s.routes[0].path.nodes.clone();
        // The new wormhole route is first, so the kept path moves down.
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!(s.routes[0].wormholes, 1, "{:?}", s.status);
        assert_eq!(s.selected, 1);
        assert_eq!(s.selected_route().unwrap().path.nodes, nodes);
        assert_eq!(s.selected_step, Some(2));
    }

    /// Review focus: a stargate and a wormhole between the same two systems.
    #[test]
    fn a_refresh_keeps_the_selected_parallel_route() {
        let perimeter = || Wormhole { sig_a: Some("ABC-123".into()), ..hole(JITA, PERIMETER) };
        let (mut s, _tmp) = session();
        s.uni = snapshot(vec![perimeter()]).uni;
        s.add_list("Jita, Perimeter");
        assert_eq!((s.routes[0].wormholes, s.routes[1].wormholes), (0, 1), "{:?}", s.status);
        s.selected = 1;
        s.apply_snapshot(snapshot(vec![perimeter()]));
        assert_eq!(s.selected, 1);
        assert_eq!(s.selected_route().unwrap().wormholes, 1);
    }

    #[test]
    fn a_closed_wormhole_gives_a_toast() {
        let (mut s, _tmp) = session();
        s.settings.top = 1;
        s.uni = snapshot(vec![jita_amarr()]).uni;
        s.add_list("Jita, Amarr");
        assert_eq!(s.routes[0].wormholes, 1, "{:?}", s.status);
        s.selected_step = Some(1);
        s.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(s.routes[0].wormholes, 0);
        assert_eq!((s.selected, s.selected_step), (0, None));
        assert_eq!(last(&s), Some((Level::Warning, "A wormhole on your route collapsed.")));
    }

    #[test]
    fn a_new_first_route_gives_a_toast() {
        let (mut s, _tmp) = session();
        s.settings.top = 1;
        s.uni = snapshot(Vec::new()).uni;
        s.add_list("Jita, Amarr");
        assert_eq!(s.routes[0].wormholes, 0);
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!(s.routes[0].wormholes, 1);
        assert_eq!(last(&s), Some((Level::Info, "A faster route has become available.")));
    }

    /// A closed entry for the wormhole `jita_amarr`, as a snapshot returns it.
    fn closed_jita_amarr() -> ClosedHole {
        ClosedHole { seen: Some(1), ..ClosedHole::of(&jita_amarr()) }
    }

    #[test]
    fn closing_a_link_asks_the_worker_and_saves_nothing() {
        let (refresher, _snapshots, control) = fake_refresher();
        let (mut s, tmp) = session();
        s.refresher = Some(refresher);
        let hole = ClosedHole::of(&jita_amarr());
        s.close_link(hole.clone());
        let Ok(Control::Close(sent)) = control.try_recv() else { panic!("no close message") };
        assert_eq!(sent, hole);
        assert!(!tmp.path().join("config.toml").exists(), "a closed link is not saved");
        assert!(tmp.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn a_snapshot_with_a_closed_hole_searches_again_without_it() {
        let (mut s, _tmp) = session();
        s.settings.top = 1;
        s.uni = snapshot(vec![jita_amarr()]).uni;
        s.add_list("Jita, Amarr");
        assert_eq!(s.routes[0].wormholes, 1, "{:?}", s.status);
        let mut closed = snapshot(Vec::new());
        closed.closed = vec![closed_jita_amarr()];
        s.apply_snapshot(closed);
        assert_eq!(s.routes[0].wormholes, 0);
        assert_eq!(s.closed, [closed_jita_amarr()]);
        assert_eq!(last(&s), Some((Level::Warning, "A wormhole on your route collapsed.")));
    }

    #[test]
    fn reopening_a_link_asks_the_worker_and_empties_the_list_at_once() {
        let (refresher, _snapshots, control) = fake_refresher();
        let (mut s, _tmp) = session();
        s.refresher = Some(refresher);
        s.closed = vec![closed_jita_amarr()];
        s.reopen_link(&closed_jita_amarr());
        assert!(matches!(control.try_recv(), Ok(Control::Reopen(h)) if h == closed_jita_amarr()));
        assert!(s.closed.is_empty());
        // The snapshot that answers the message does not give a retire notice.
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert!(s.notices.is_empty(), "{:?}", s.notices);
    }

    #[test]
    fn a_retired_entry_gives_a_notice() {
        let (mut s, _tmp) = session();
        let mut snap = snapshot(Vec::new());
        snap.closed = vec![closed_jita_amarr()];
        s.apply_snapshot(snap);
        s.apply_snapshot(snapshot(Vec::new()));
        assert!(s.closed.is_empty());
        assert_eq!(last(&s), Some((Level::Info, "A closed wormhole left the map.")));
    }

    #[test]
    fn closing_a_link_without_a_worker_gives_an_error() {
        let (mut s, _tmp) = session();
        s.close_link(ClosedHole::of(&jita_amarr()));
        assert_eq!(last(&s), Some((Level::Error, REFRESH_STOPPED)));
        s.closed = vec![closed_jita_amarr()];
        s.reopen_link(&closed_jita_amarr());
        assert_eq!(s.closed.len(), 1);
    }

    #[test]
    fn a_refresh_with_no_waypoints_only_swaps_the_map() {
        let (mut s, _tmp) = session();
        s.status = Status::Refreshing;
        s.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!((s.shortcuts.wormholes, s.wormhole_data.len()), (1, 1));
        assert!(s.routes.is_empty());
        assert_eq!(s.status, Status::Refreshing);
    }

    #[test]
    fn the_poll_applies_a_snapshot_and_reports_a_stopped_worker() {
        let (refresher, snapshots, _control) = fake_refresher();
        let (mut s, _tmp) = session();
        s.refresher = Some(refresher);
        snapshots.send(snapshot(vec![jita_amarr()])).unwrap();
        s.poll_refresh();
        assert_eq!(s.shortcuts.wormholes, 1);
        drop(snapshots);
        s.poll_refresh();
        assert_eq!(last(&s), Some((Level::Error, REFRESH_STOPPED)));
        assert!(s.refresher.is_none());
    }

    #[test]
    fn a_refresh_adds_its_rows_to_the_log() {
        let (mut s, _tmp) = session();
        let row =
            |time| LogEntry { time, op: LogOp::NexumFetch, kind: LogEntryKind::Loaded, message: "Loaded 1 wormhole connection".into() };
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
        let (refresher, snapshots, _control) = fake_refresher();
        let (mut s, _tmp) = session();
        s.refresher = Some(refresher);
        drop(snapshots);
        s.poll_refresh();
        assert_eq!(s.log.len(), 1);
        assert_eq!(
            (s.log[0].op, s.log[0].kind, s.log[0].message.as_str()),
            (LogOp::Refresh, LogEntryKind::WorkerStopped, "the worker stopped")
        );
    }

    #[test]
    fn refresh_now_asks_the_worker_and_waits_between_two_calls() {
        let (refresher, _snapshots, control) = fake_refresher();
        let (mut s, _tmp) = session();
        s.refresher = Some(refresher);
        let start = Instant::now();
        s.refresh_now(start);
        assert!(matches!(control.try_recv(), Ok(Control::Refresh)));
        assert_eq!(s.status, Status::Refreshing);
        // A second call soon after sends nothing.
        s.refresh_now(start + Duration::from_secs(5));
        assert!(control.try_recv().is_err());
        assert_eq!(s.status, Status::Refreshing);
        // After the gap, the call sends a message again.
        s.refresh_now(start + REFRESH_GAP);
        assert!(matches!(control.try_recv(), Ok(Control::Refresh)));
    }

    #[test]
    fn an_import_keeps_one_pair_for_a_bridge_in_both_directions_and_tells_the_worker() {
        let (mut s, tmp) = session();
        let (refresher, _snapshots, control) = fake_refresher();
        s.refresher = Some(refresher);
        let problems = s.import_bridges("0 Jita --> Perimeter\n0 Perimeter --> Jita\n0 Jita --> Jita\n0 Jita --> Nowhere\n", true);
        assert_eq!(problems, ["Unknown system \"Nowhere\"", "Jita cannot bridge to itself"]);
        assert_eq!(s.bridges.len(), 1);
        let Ok(Control::Bridges(report)) = control.try_recv() else { panic!("no bridge message") };
        assert_eq!((report.pairs, report.bridges), (s.bridges.clone(), 2));
        // The file holds both directions.
        let text = std::fs::read_to_string(tmp.path().join("ansiblex.txt")).unwrap();
        assert_eq!(text.lines().filter(|l| l.contains("-->")).count(), 2);
        // A second import of the same bridge adds nothing. Replace drops the old list.
        assert!(s.import_bridges("0 Perimeter --> Jita", false).is_empty());
        assert_eq!(s.bridges.len(), 1);
        s.import_bridges("0 Jita --> Amarr", true);
        assert_eq!(s.bridges.len(), 1);
        assert_eq!((s.uni.name(s.bridges[0].0), s.uni.name(s.bridges[0].1)), ("Amarr", "Jita"));
        s.remove_bridge(0);
        assert!(s.bridges.is_empty());
    }

    #[test]
    fn an_identical_waiting_notice_is_not_added_again() {
        let (mut s, _tmp) = session();
        s.notify(Level::Info, "same");
        s.notify(Level::Info, "same");
        s.notify(Level::Error, "same");
        assert_eq!(s.notices.len(), 2, "the level is part of the notice");
    }

    #[test]
    fn the_bridge_reason_gives_a_notice_only_when_it_changes() {
        let (mut s, _tmp) = session();
        let _ = std::fs::remove_file(&s.cfg_path);
        s.notices.clear();
        s.settings.rules.capital = None;
        s.recompute();
        assert_eq!(last(&s), Some((Level::Warning, "Jump bridges off: Set a capital to use jump bridges")));
        // The same reason on the next search gives no second notice.
        s.notices.clear();
        s.recompute();
        assert!(s.notices.is_empty(), "{:?}", s.notices);
        // A capital clears the reason. Losing it again gives a new notice.
        s.settings.rules.capital = s.uni.exact("JK-Q77");
        s.recompute();
        assert!(s.notices.is_empty());
        s.settings.rules.capital = None;
        s.recompute();
        assert_eq!(s.notices.len(), 1);
    }

    #[test]
    fn a_fetch_warning_gives_a_notice_once_per_problem() {
        let (mut s, _tmp) = session();
        s.notices.clear();
        let snapshot = |offline: bool| {
            let mut snap = snapshot(Vec::new());
            if offline {
                let error = FetchError::Offline("down".into());
                snap.shortcuts.warnings = vec![OverlayWarning::Fetch { source: SourceId::Nexum, error, cached_at: None }];
            }
            snap
        };
        s.apply_snapshot(snapshot(true));
        s.apply_snapshot(snapshot(true));
        assert_eq!(s.notices.len(), 1, "the same problem shows one time");
        // The window shows the notice. A problem that goes and comes back shows again.
        s.notices.clear();
        s.apply_snapshot(snapshot(false));
        s.apply_snapshot(snapshot(true));
        assert_eq!(s.notices.len(), 1);
    }

    #[test]
    fn a_second_warning_gives_only_its_own_notice() {
        let (mut s, _tmp) = session();
        s.notices.clear();
        let offline = |source| OverlayWarning::Fetch { source, error: FetchError::Offline("down".into()), cached_at: None };
        let snapshot = |warnings: Vec<OverlayWarning>| {
            let mut snap = snapshot(Vec::new());
            snap.shortcuts.warnings = warnings;
            snap
        };
        s.apply_snapshot(snapshot(vec![offline(SourceId::Nexum)]));
        s.notices.clear();
        s.apply_snapshot(snapshot(vec![offline(SourceId::Nexum), offline(SourceId::EveScout)]));
        assert_eq!(s.notices.len(), 1);
        assert_eq!(s.notices[0].text, "EVE-Scout offline");
    }

    #[test]
    fn refresh_now_without_a_worker_says_so() {
        let (mut s, _tmp) = session();
        s.refresh_now(Instant::now());
        assert_eq!(last(&s), Some((Level::Error, REFRESH_STOPPED)));
    }

    #[test]
    fn a_nexum_change_asks_the_worker_for_the_new_map() {
        let (refresher, _snapshots, control) = fake_refresher();
        let (mut s, _tmp) = session();
        s.refresher = Some(refresher);
        s.cfg.nexum.url = Some("https://nexum.example".into());
        s.nexum_saved();
        assert_eq!(last(&s), Some((Level::Info, NEXUM_LOADING)));
        let Ok(Control::Nexum(sent)) = control.try_recv() else { panic!("no Nexum message") };
        assert_eq!(sent.url.as_deref(), Some("https://nexum.example"));
    }

    fn metro_settings() -> router_core::config::EveMetroConfig {
        router_core::config::EveMetroConfig { enabled: true, key: Some(ApiKey("metro_test_key".into())), character: Some(42) }
    }

    #[test]
    fn an_eve_metro_change_asks_the_worker_for_the_map() {
        let (refresher, _snapshots, control) = fake_refresher();
        let (mut s, tmp) = session();
        s.refresher = Some(refresher);
        s.pilots = Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[]);
        s.cfg.eve_metro = metro_settings();
        s.eve_metro_saved();
        assert_eq!(last(&s), Some((Level::Info, EVE_METRO_LOADING)));
        let Ok(Control::EveMetro(sent)) = control.try_recv() else { panic!("no EVE Metro message") };
        assert_eq!(sent, metro_settings());
        // The settings are in the file.
        assert_eq!(Config::load(&s.cfg_path).unwrap().eve_metro, metro_settings());
    }

    #[test]
    fn eve_metro_stays_off_without_a_client_id() {
        let (refresher, _snapshots, control) = fake_refresher();
        let (mut s, _tmp) = session();
        s.refresher = Some(refresher);
        s.cfg.eve_metro = metro_settings();
        s.eve_metro_saved();
        assert_eq!(last(&s), Some((Level::Warning, router_core::sources::evemetro::NEEDS_CLIENT_ID)));
        // The worker gets the settings off. The file keeps the settings of the user.
        let Ok(Control::EveMetro(sent)) = control.try_recv() else { panic!("no EVE Metro message") };
        assert_eq!(sent.complete(), None);
        assert_eq!(Config::load(&s.cfg_path).unwrap().eve_metro, metro_settings());
    }

    #[test]
    fn eve_metro_settings_save_without_a_worker() {
        let (mut s, _tmp) = session();
        s.cfg.eve_metro = metro_settings();
        s.eve_metro_saved();
        assert_eq!(last(&s), Some((Level::Info, "EVE Metro settings saved. Restart to load the map.")));
    }

    #[test]
    fn avoiding_a_system_or_region_searches_again_and_saves() {
        let (mut s, _tmp) = session();
        let _ = std::fs::remove_file(&s.cfg_path);
        s.add_list("Jita, Dodixie");
        let first = s.routes[0].path.nodes.clone();
        let ends = [s.uni.system(first[0]).region.clone(), s.uni.system(*first.last().unwrap()).region.clone()];
        let middle = *first.iter().find(|&&n| !ends.contains(&s.uni.system(n).region)).unwrap();
        let (name, region) = (s.uni.name(middle).to_string(), s.uni.system(middle).region.clone());

        s.toggle_avoid_system(middle);
        assert!(!s.routes[0].path.nodes.contains(&middle));
        assert!(std::fs::read_to_string(&s.cfg_path).unwrap().contains(&name));
        s.toggle_avoid_system(middle);
        assert_eq!((s.settings.avoid.len(), s.routes[0].path.nodes.clone()), (0, first));

        s.toggle_avoid_region(&region);
        assert!(s.routes[0].path.nodes.iter().all(|&n| s.uni.system(n).region != region));
        // The "never" switch keeps the region out of every route, and the file keeps the switch.
        s.set_avoid_never_region(&region, true);
        assert!(s.routes.iter().all(|r| r.path.nodes.iter().all(|&n| s.uni.system(n).region != region)));
        assert!(std::fs::read_to_string(&s.cfg_path).unwrap().contains("\"never\": true"));
        s.set_avoid_never_region(&region, false);
        s.clear_avoid();
        assert!(s.settings.avoid.is_empty());
        assert!(s.routes[0].path.nodes.contains(&middle));
    }

    #[test]
    fn a_never_destination_says_the_avoid_list_is_in_the_way() {
        let (mut s, _tmp) = session();
        let _ = std::fs::remove_file(&s.cfg_path);
        s.add_list("Jita, Perimeter");
        let perimeter = *s.waypoints.last().unwrap();
        s.toggle_avoid_system(perimeter);
        assert!(!s.routes.is_empty(), "prefer still gives a route");
        s.set_avoid_never_system(perimeter, true);
        assert!(s.routes.is_empty());
        assert_eq!(last(&s), Some((Level::Error, "No route from Jita to Perimeter. Your avoid list might be in the way.")));
    }

    #[test]
    fn a_dragged_value_applies_once_after_the_quiet_time() {
        let (mut s, _tmp) = session();
        let _ = std::fs::remove_file(&s.cfg_path);
        s.add_list("Jita, Amarr");
        s.notices.clear();
        let start = Instant::now();
        s.settings.costs.cap_weight = 1.0;
        s.apply_later(start);
        // A second change restarts the quiet time.
        s.settings.costs.cap_weight = 2.0;
        s.apply_later(start + Duration::from_millis(300));
        assert_eq!(s.apply_due(start + Duration::from_millis(600)), Some(APPLY_DELAY - Duration::from_millis(300)));
        assert!(s.notices.is_empty(), "no search yet");
        assert!(!s.cfg_path.exists(), "no write yet");
        assert_eq!(s.apply_due(start + APPLY_DELAY + Duration::from_millis(300)), None);
        assert!(s.notices.is_empty(), "a save that works is quiet");
        assert!(std::fs::read_to_string(&s.cfg_path).unwrap().contains("2.0"));
        // Nothing waits now.
        s.notices.clear();
        assert_eq!(s.apply_due(start + Duration::from_secs(10)), None);
        assert!(s.notices.is_empty());
    }

    #[test]
    fn closing_a_window_applies_a_waiting_change_at_once() {
        let (mut s, _tmp) = session();
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

    #[test]
    fn replace_with_no_good_name_clears_the_routes() {
        let (mut s, _tmp) = session();
        s.add_list("Jita, Amarr");
        assert!(!s.routes.is_empty(), "{:?}", s.status);
        let problems = s.replace_list("Nowhere");
        assert_eq!(problems.len(), 1);
        assert!(s.waypoints.is_empty() && s.routes.is_empty());
    }

    #[test]
    fn a_lost_load_thread_shows_an_error() {
        let (tx, rx) = mpsc::channel::<LoadMsg>();
        drop(tx);
        let mut app = RouterApp { state: State::Loading(Loading { rx, step: None }) };
        app.poll(&egui::Context::default());
        let State::Failed(text) = &app.state else { panic!("the state is not Failed") };
        assert_eq!(text, LOAD_STOPPED);
    }

    #[test]
    fn the_splash_keeps_the_newest_step() {
        let (tx, rx) = mpsc::channel::<LoadMsg>();
        let mut app = RouterApp { state: State::Loading(Loading { rx, step: None }) };
        tx.send(LoadMsg::Progress(startup::Progress::CheckingSde)).unwrap();
        tx.send(LoadMsg::Progress(startup::Progress::DownloadingSde { done: 5, total: 10 })).unwrap();
        app.poll(&egui::Context::default());
        let State::Loading(loading) = &app.state else { panic!("the load ended") };
        assert_eq!(loading.step, Some(startup::Progress::DownloadingSde { done: 5, total: 10 }));
    }

    #[test]
    fn waypoints_give_the_routes() {
        let (mut s, _tmp) = session();
        let jita = s.uni.exact("Jita").unwrap();
        let ualx = s.uni.exact("UALX-3").unwrap();
        s.add_waypoint(jita);
        assert!(s.routes.is_empty());
        assert_eq!(s.hubs.len(), 0);
        s.set_start(ualx);
        assert_eq!(names(&s), ["UALX-3", "Jita"]);
        let lines: Vec<String> = s.routes.iter().map(|r| format!("{}{}", jumps_label(r.jumps), route_extras(r))).collect();
        assert_eq!(lines.len(), 3, "{:?}", s.status);
        // The list shows three different routes. A route that only detours the first one is not in it.
        assert_eq!(lines, ["30 jumps", "31 jumps", "41 jumps"]);
    }

    #[test]
    fn waypoint_operations() {
        let (mut s, _tmp) = session();
        for name in ["Jita", "Amarr", "Dodixie"] {
            let node = s.uni.exact(name).unwrap();
            s.add_waypoint(node);
        }
        // The same system two times in a row is not added.
        let dodixie = s.uni.exact("Dodixie").unwrap();
        s.add_waypoint(dodixie);
        assert_eq!(names(&s), ["Jita", "Amarr", "Dodixie"]);
        assert_eq!(last(&s), Some((Level::Info, "Dodixie is already the last waypoint")));
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
        let (mut s, _tmp) = session();
        let problems = s.add_list("UALX-3\r\nUALX-3\nNowhere\n\njita\n");
        assert_eq!(names(&s), ["UALX-3", "Jita"]);
        assert_eq!(problems, ["Unknown system \"Nowhere\""]);
        assert_eq!(s.routes.len(), 3);
        assert!(last(&s).is_some_and(|(level, text)| level == Level::Warning && text.contains("Added 2 systems")), "{:?}", last(&s));
        // A name that matches more than one system is not added.
        let problems = s.add_list("Ama, Amarr");
        assert_eq!(names(&s), ["UALX-3", "Jita", "Amarr"]);
        assert!(problems[0].contains("matches more than one system"), "{problems:?}");
    }

    /// A list of Jove Observatory systems, as a user pastes it: one name for each line.
    const JOVE: &str = "0P-U0Q\n16AM-3\n3L3N-X\n9-980U\nBW-WJ2\nF-ZBO0\nFE-6YQ\nH-HWQR\nJI1-SY\nKW-OAM\nMS1-KJ\nNZW-ZO\nPEK-8Z\nT-AKQZ\nWB-AYY\nY-EQ0C\nY-ORBJ\nZD1-Z2\nZO-P5K\n";

    #[test]
    fn jove_list_with_line_breaks_and_commas() {
        let (mut s, _tmp) = session();
        assert_eq!(s.add_list(JOVE), Vec::<String>::new(), "{:?}", s.status);
        assert_eq!(s.waypoints.len(), 19);
        assert!(!s.routes.is_empty(), "{:?}", s.status);
        let (mut csv, _tmp_csv) = session();
        csv.add_list(&JOVE.trim().replace('\n', ", "));
        assert_eq!(csv.waypoints, s.waypoints);
        // 17 midpoints: "Optimize order" changes their order and keeps the start and the destination.
        s.settings.optimize = true;
        s.recompute();
        assert!(!s.routes.is_empty(), "{:?}", s.status);
        let route = &s.routes[0];
        assert_eq!(route.path.nodes[0], s.waypoints[0]);
        assert_eq!(*route.path.nodes.last().unwrap(), s.waypoints[18]);
    }

    #[test]
    fn favourites_save_the_config() {
        let (mut s, _tmp) = session();
        let _ = std::fs::remove_file(&s.cfg_path);
        let rens = s.uni.exact("Rens").unwrap();
        assert!(s.add_favourite(rens));
        assert!(!s.add_favourite(rens));
        let saved = Config::load(&s.cfg_path).unwrap();
        assert_eq!(saved.favourites, Some(vec!["Rens".to_string()]));
        assert_eq!(saved.capital.as_deref(), Some("JK-Q77"));
        std::fs::remove_file(&s.cfg_path).unwrap();
    }

    #[test]
    fn the_overlay_position_is_saved_and_reset() {
        use router_core::config::{Place, Pos};
        let (mut s, _tmp) = session();
        let _ = std::fs::remove_file(&s.cfg_path);
        let here = Pos { x: 120, y: -30 };
        s.set_overlay_position(Place::Character(7), Some(here));
        s.set_overlay_position(Place::Global, Some(Pos { x: 5, y: 6 }));
        let saved = Config::load(&s.cfg_path).unwrap().routing_overlay;
        assert_eq!(saved.position_of(Place::Character(7)), Some(here));
        assert_eq!(saved.position_of(Place::Global), Some(Pos { x: 5, y: 6 }));
        s.set_overlay_position(Place::Character(7), None);
        let saved = Config::load(&s.cfg_path).unwrap().routing_overlay;
        assert_eq!(saved.position_of(Place::Character(7)), None);
        assert_eq!(saved.position_of(Place::Global), Some(Pos { x: 5, y: 6 }));
        std::fs::remove_file(&s.cfg_path).unwrap();
    }
}
