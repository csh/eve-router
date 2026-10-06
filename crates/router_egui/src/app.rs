//! The window state, the startup load and the route search.

use crate::theme;
use crate::view::View;
use petgraph::graph::NodeIndex;
use router_core::config::{self, Config};
use router_core::esi::active::active_path;
use router_core::esi::pilots::Pilots;
use router_core::labels::Shortcuts;
use router_core::route::{Route, Stop};
use router_core::settings::{Settings, split_systems};
use router_core::sources::nexum::MapInfo;
use router_core::startup;
use router_core::universe::Universe;
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
    let loaded = startup::finish(pending, &sde_dir, None, |outcome| startup_lines.extend(outcome.message(&sde_dir)))?;
    let shortcuts = Shortcuts::new(&loaded.uni, &loaded.report, &loaded.wh, &loaded.scout);
    let settings = Settings::from_config(&cfg, &loaded.uni)?;
    let mut session = Session::new(loaded.uni, settings, cfg, cfg_path.clone(), shortcuts);
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
    pub uni: Universe,
    pub settings: Settings,
    pub cfg: Config,
    cfg_path: PathBuf,
    pub shortcuts: Shortcuts,
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
    /// The last Nexum map list, for the map name in the settings.
    pub map_names: Vec<MapInfo>,
    /// The logins, the tracking and the active route.
    pub pilots: Pilots,
}

impl Session {
    pub fn new(uni: Universe, settings: Settings, cfg: Config, cfg_path: PathBuf, shortcuts: Shortcuts) -> Self {
        let mut session = Session {
            uni,
            settings,
            cfg,
            cfg_path: cfg_path.clone(),
            shortcuts,
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

    /// Find the routes and the favourite distances again.
    pub fn recompute(&mut self) {
        self.routes.clear();
        self.selected = 0;
        self.selected_step = None;
        self.status.clear();
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

    /// Save the config after a change to a Nexum value. A Nexum change applies at the next start.
    pub fn nexum_saved(&mut self) {
        self.status = match self.write_config() {
            Ok(()) => "Nexum settings saved. Restart to load the new map.".into(),
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
    use router_core::labels::{jumps_label, route_extras};
    use router_core::test_support::{FIXTURE_TIME, overlay_universe, settings};

    fn session(name: &str) -> Session {
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        let path = std::env::temp_dir().join(format!("eve-router-egui-test-{name}.json"));
        let mut s = Session::new(uni, settings, Config::default(), path, shortcuts);
        s.clock = || FIXTURE_TIME;
        s
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
        assert_eq!(lines[0], "30 jumps");
        assert_eq!(lines[1], "30 jumps (2 wormholes)");
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
