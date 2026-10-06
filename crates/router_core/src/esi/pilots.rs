//! `Pilots`: the state that the TUI and the GUI share. It holds the logins, the token store,
//! the tracker, the live data of each pilot and the active route. The front ends draw this
//! state and call its methods. They hold no SSO, ESI or tracking logic.
//!
//! The front end calls `update` often (each frame, or each 250 ms). `update` reads the tracker
//! events and the login result, and gives the status lines in `notices`.

use super::active::{ActiveRoute, Observation, active_path};
use super::client::{Esi, Location, Ship};
use super::sso::{Login, LoginError, Sso, Tokens};
use super::store::Accounts;
use super::tracker::{Command, Event, Intervals, Live, Tracker};
use super::{Character, client_id};
use crate::ansiblex::{HullClass, hull_by_type, hull_rows, same_class, same_hull};
use crate::route::Route;
use crate::settings::{HullSource, Settings};
use crate::universe::Universe;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

/// Runs after a background event, for example to repaint the window.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The live data of one pilot. A value is `None` until the first poll.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PilotState {
    pub system: Option<u32>,
    pub ship: Option<Ship>,
    pub online: Option<bool>,
    /// SSO refused the refresh token. The pilot must log in again.
    pub expired: bool,
}

/// One row of the character list.
#[derive(Clone, Debug, PartialEq)]
pub struct PilotView {
    pub id: u64,
    pub name: String,
    pub live: PilotState,
    /// A scope from `SCOPES` is missing. The pilot must log in again.
    pub needs_reauth: bool,
    pub active: bool,
}

/// A login in progress.
pub struct LoginFlow {
    pub login: Login,
    exchange: Option<Receiver<Result<Tokens, LoginError>>>,
    /// The last error. The user can paste again, or start a new login.
    pub error: Option<String>,
}

impl LoginFlow {
    /// True while the code exchange runs.
    pub fn busy(&self) -> bool {
        self.exchange.is_some()
    }
}

/// A waypoint send in progress, or one that failed.
#[derive(Clone, Debug, PartialEq)]
pub struct SendState {
    pub character: u64,
    pub systems: Vec<u32>,
    pub done: usize,
    /// The error text, after a failure. `retry_send` sends all waypoints again.
    pub failed: Option<String>,
}

/// A row of the Pilot picker.
pub enum PilotRow {
    /// Follow the ship of this character.
    Pilot(PilotView),
    /// A manual hull. `None` is the "none" row.
    Hull(Option<HullClass>),
}

impl PilotRow {
    /// True if the row is the hull source of `settings`.
    pub fn is_current(&self, settings: &Settings) -> bool {
        match (self, settings.hull_source) {
            (PilotRow::Pilot(v), HullSource::Pilot(id)) => v.id == id,
            (PilotRow::Hull(h), HullSource::Manual) => same_hull(*h, settings.rules.hull),
            _ => false,
        }
    }
}

/// The pilots at each step of a route, for the Pilots column. A route can visit a system more
/// than one time, but a pilot shows at one step only: the progress step for the pilot of
/// `active`, else the first visit of the system of the pilot.
pub fn pilots_by_step(systems: &[u32], pilots: &[PilotView], active: Option<&ActiveRoute>) -> Vec<Vec<PilotView>> {
    let mut rows = vec![Vec::new(); systems.len()];
    for pilot in pilots {
        let Some(system) = pilot.live.system else { continue };
        let progress = active.filter(|a| a.character == pilot.id).map(|a| a.progress);
        let step = progress.filter(|&i| systems.get(i) == Some(&system)).or_else(|| systems.iter().position(|&s| s == system));
        if let Some(step) = step {
            rows[step].push(pilot.clone());
        }
    }
    rows
}

/// What `Pilots::sync_hull` changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HullSync {
    None,
    /// The followed pilot flies a different hull.
    Hull,
    /// The followed character is not stored: the source is manual now, and the hull stays.
    Source,
}

pub struct Pilots {
    /// `None` until the user chooses "Session only", when the keyring is missing.
    pub accounts: Option<Accounts>,
    /// Why the keyring is not available, for the "Session only" prompt.
    pub keyring_error: Option<String>,
    client_id: Option<String>,
    tracker: Option<Tracker>,
    pub live: HashMap<u64, PilotState>,
    pub login: Option<LoginFlow>,
    /// The route that is in the game. The UI hides the planner while it is `Some`.
    pub active: Option<ActiveRoute>,
    /// A route whose first send runs or failed. It becomes `active` after a full send.
    pub pending: Option<ActiveRoute>,
    pub send: Option<SendState>,
    /// An active route from the last run. The UI asks "Resume" or "Discard".
    pub resume: Option<ActiveRoute>,
    /// True while ESI limits the router.
    pub limited: bool,
    /// Status lines for the UI. The UI takes them with `std::mem::take`.
    pub notices: Vec<String>,
    /// The hull changed during an active route. The routes in the planner are for the old hull.
    replan: bool,
    active_path: PathBuf,
    wake: Wake,
}

impl Pilots {
    /// Open the token store and `characters.json`, start the tracker, and read the active route
    /// of the last run.
    pub fn open(cfg_path: &Path, wake: Wake) -> Pilots {
        let (accounts, keyring_error) = match Accounts::open(cfg_path) {
            Ok(accounts) => (Some(accounts), None),
            Err(e) => (None, Some(e)),
        };
        let client_id = client_id();
        let tracker = client_id.as_deref().map(|id| {
            let api = Live { sso: Sso::new(id), esi: Esi::default() };
            let wake = wake.clone();
            Tracker::start(api, Intervals::default(), move || wake())
        });
        let mut pilots = Self::with_parts(accounts, client_id, tracker, active_path(cfg_path), wake);
        pilots.keyring_error = keyring_error;
        pilots.resume = ActiveRoute::load(&pilots.active_path);
        pilots
    }

    /// No token store, no client ID and no tracker. The UI shows the planner only. The tests of
    /// the front ends use it, and the TUI uses it until `open` is done.
    pub fn offline(active_path: PathBuf) -> Pilots {
        Self::with_parts(None, None, None, active_path, Arc::new(|| {}))
    }

    fn with_parts(
        accounts: Option<Accounts>,
        client_id: Option<String>,
        tracker: Option<Tracker>,
        active_path: PathBuf,
        wake: Wake,
    ) -> Pilots {
        let mut pilots = Pilots {
            accounts,
            keyring_error: None,
            client_id,
            tracker,
            live: HashMap::new(),
            login: None,
            active: None,
            pending: None,
            send: None,
            resume: None,
            limited: false,
            notices: Vec::new(),
            replan: false,
            active_path,
            wake,
        };
        pilots.track_all();
        pilots
    }

    /// Track each stored character that has a refresh token.
    fn track_all(&mut self) {
        let Some(accounts) = &self.accounts else { return };
        let mut problems = Vec::new();
        for entry in &accounts.characters {
            self.live.entry(entry.id).or_default();
            match accounts.refresh_token(entry.id) {
                Ok(Some(refresh)) => {
                    if let Some(tracker) = &self.tracker {
                        tracker.send(Command::Add { id: entry.id, refresh });
                    }
                }
                Ok(None) => self.live.entry(entry.id).or_default().expired = true,
                Err(e) => problems.push(format!("{}: {e}", entry.name)),
            }
        }
        self.notices.extend(problems);
    }

    /// Without a keyring: keep the tokens in memory until the app closes.
    pub fn use_session_only(&mut self) {
        self.accounts = Some(Accounts::session_only());
    }

    /// The text for a disabled "Add character", if the router cannot log in.
    pub fn login_blocked(&self) -> Option<String> {
        if self.client_id.is_none() {
            return Some(format!("No EVE client ID in this build. Set {}.", super::CLIENT_ID_VAR));
        }
        self.accounts.is_none().then(|| self.keyring_error.clone().unwrap_or_else(|| "No token store.".into()))
    }

    /// True if the UI shows the characters: the router can log in, or a character is stored.
    pub fn shows_characters(&self) -> bool {
        self.client_id.is_some() || self.accounts.as_ref().is_some_and(|a| !a.characters.is_empty())
    }

    /// Start a login, and open the URL in the browser. The UI also shows the URL for a copy.
    pub fn start_login(&mut self) -> Result<(), String> {
        if let Some(reason) = self.login_blocked() {
            return Err(reason);
        }
        let sso = Sso::new(self.client_id.as_deref().expect("client ID"));
        let login = sso.start_login();
        // A failure to open the browser is not an error: the URL is on the screen.
        let _ = webbrowser::open(&login.url);
        self.login = Some(LoginFlow { login, exchange: None, error: None });
        Ok(())
    }

    pub fn cancel_login(&mut self) {
        self.login = None;
    }

    /// Read the code from a pasted URL, and start the exchange.
    pub fn paste(&mut self, text: &str) {
        let Some(flow) = &mut self.login else { return };
        match flow.login.paste(text) {
            Ok(code) => self.exchange(code),
            Err(e) => flow.error = Some(e.to_string()),
        }
    }

    fn exchange(&mut self, code: oauth2::AuthorizationCode) {
        let (Some(flow), Some(client_id)) = (&mut self.login, &self.client_id) else { return };
        if flow.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (verifier, client_id, wake) = (flow.login.verifier(), client_id.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Sso::new(&client_id).exchange(code, verifier));
            wake();
        });
        flow.exchange = Some(rx);
        flow.error = None;
    }

    /// Log out a character: revoke the token at SSO (on a thread), and delete it.
    pub fn remove(&mut self, id: u64) {
        if self.active.as_ref().is_some_and(|a| a.character == id) {
            self.stop_route();
        }
        if let Some(tracker) = &self.tracker {
            tracker.send(Command::Remove(id));
        }
        self.live.remove(&id);
        let Some(accounts) = &mut self.accounts else { return };
        if let (Ok(Some(refresh)), Some(client_id)) = (accounts.refresh_token(id), self.client_id.clone()) {
            std::thread::spawn(move || {
                let _ = Sso::new(&client_id).revoke(&refresh);
            });
        }
        if let Err(e) = accounts.remove(id) {
            self.notices.push(e);
        }
    }

    /// The character list: the active pilot first, then by name.
    pub fn characters(&self) -> Vec<PilotView> {
        let Some(accounts) = &self.accounts else { return Vec::new() };
        let active = self.active.as_ref().map(|a| a.character);
        let mut rows: Vec<PilotView> = accounts
            .characters
            .iter()
            .map(|e| {
                let character = e.character();
                PilotView {
                    id: e.id,
                    name: e.name.clone(),
                    live: self.live.get(&e.id).cloned().unwrap_or_default(),
                    needs_reauth: !character.missing_scopes().is_empty(),
                    active: active == Some(e.id),
                }
            })
            .collect();
        rows.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        rows
    }

    /// Set the hull from the live ship of the pilot in `settings.hull_source`.
    /// Without a keyring (no token store, or "Session only"), the source stays: the keyring can
    /// come back at the next start.
    /// A hull change during an active route gives `Hull`, and one more `Hull` when the route ends.
    pub fn sync_hull(&mut self, settings: &mut Settings) -> HullSync {
        let HullSource::Pilot(id) = settings.hull_source else { return HullSync::None };
        let Some(accounts) = &self.accounts else { return HullSync::None };
        if !accounts.characters.iter().any(|c| c.id == id) {
            // A "Session only" store starts empty. The character comes back with the keyring.
            if accounts.is_session_only() {
                return HullSync::None;
            }
            settings.hull_source = HullSource::Manual;
            return HullSync::Source;
        }
        let ship_type = self.live.get(&id).and_then(|l| l.ship.as_ref()).map(|s| s.ship_type_id);
        if settings.follow(ship_type) {
            self.replan |= self.active.is_some();
            return HullSync::Hull;
        }
        if self.replan && self.active.is_none() {
            self.replan = false;
            return HullSync::Hull;
        }
        HullSync::None
    }

    /// The status text when each logged-in pilot is offline.
    pub const NO_SENDER: &str = "No pilot is online. Waypoints need the game client running.";

    /// The pilots that can take a route: a valid login, and not offline. An unknown online state stays.
    pub fn senders(&self) -> Vec<PilotView> {
        self.characters().into_iter().filter(|c| !c.live.expired && c.live.online != Some(false)).collect()
    }

    /// The Pilot picker rows: the characters whose name contains `filter`, then `hull_rows(filter)`.
    pub fn pilot_rows(&self, filter: &str) -> Vec<PilotRow> {
        let lower = filter.to_lowercase();
        let pilots = self.characters().into_iter().filter(|c| c.name.to_lowercase().contains(&lower)).map(PilotRow::Pilot);
        pilots.chain(hull_rows(filter).into_iter().map(PilotRow::Hull)).collect()
    }

    /// The character of the last route start, for the preselection in the picker.
    pub fn last_used(&self) -> Option<u64> {
        self.accounts.as_ref()?.last_used()
    }

    /// Send the first segment of a route. The route becomes active after a full send.
    pub fn start_route(&mut self, route: ActiveRoute) {
        let systems = route.first_waypoints();
        let id = route.character;
        if let Some(accounts) = &mut self.accounts
            && let Err(e) = accounts.mark_used(id, crate::wormhole::now())
        {
            self.notices.push(e);
        }
        self.pending = Some(route);
        self.send_waypoints(id, systems);
    }

    fn send_waypoints(&mut self, id: u64, systems: Vec<u32>) {
        self.send = Some(SendState { character: id, systems: systems.clone(), done: 0, failed: None });
        match &self.tracker {
            Some(tracker) => tracker.send(Command::Send { id, systems }),
            None => self.send.as_mut().expect("send").failed = Some("No EVE client ID in this build.".into()),
        }
    }

    /// Send all waypoints of the failed send again. The first one clears the in-game route.
    pub fn retry_send(&mut self) {
        if let Some(SendState { character, systems, failed: Some(_), .. }) = self.send.clone() {
            self.send_waypoints(character, systems);
        }
    }

    /// Give up a failed first send. The route does not become active.
    pub fn cancel_send(&mut self) {
        self.send = None;
        self.pending = None;
    }

    /// Replace the active route, for example "Re-route from here". The new route goes to the game.
    pub fn replace_route(&mut self, route: ActiveRoute) {
        self.active = None;
        self.start_route(route);
    }

    /// Stop the tracking of the route. The in-game waypoints stay: ESI cannot clear them.
    pub fn stop_route(&mut self) {
        self.active = None;
        self.pending = None;
        self.send = None;
        if let Some(tracker) = &self.tracker {
            tracker.send(Command::SetActive(None));
        }
        if let Err(e) = ActiveRoute::clear(&self.active_path) {
            self.notices.push(e);
        }
    }

    /// Continue the route of the last run. Its waypoints are still in the game.
    pub fn resume_route(&mut self) {
        let Some(route) = self.resume.take() else { return };
        self.activate(route);
    }

    pub fn discard_resume(&mut self) {
        self.resume = None;
        if let Err(e) = ActiveRoute::clear(&self.active_path) {
            self.notices.push(e);
        }
    }

    fn activate(&mut self, route: ActiveRoute) {
        if let Some(tracker) = &self.tracker {
            tracker.send(Command::SetActive(Some(route.character)));
        }
        // The pilot can already be further along.
        let mut route = route;
        if let Some(system) = self.live.get(&route.character).and_then(|l| l.system) {
            route.observe(&Location { solar_system_id: system, station_id: None, structure_id: None });
        }
        self.save(&route);
        self.active = Some(route);
    }

    fn save(&mut self, route: &ActiveRoute) {
        if let Err(e) = route.save(&self.active_path) {
            self.notices.push(e);
        }
    }

    /// Read the login result and the tracker events. True if anything changed.
    pub fn update(&mut self) -> bool {
        let mut changed = self.update_login();
        let events: Vec<Event> = match &self.tracker {
            Some(tracker) => tracker.events.try_iter().collect(),
            None => Vec::new(),
        };
        changed |= !events.is_empty();
        for event in events {
            self.event(event);
        }
        changed
    }

    fn update_login(&mut self) -> bool {
        let Some(flow) = &mut self.login else { return false };
        if let Some(rx) = &flow.exchange {
            let Ok(result) = rx.try_recv() else { return false };
            flow.exchange = None;
            match result {
                Ok(tokens) => self.logged_in(tokens),
                Err(e) => flow.error = Some(e.to_string()),
            }
            return true;
        }
        match flow.login.poll() {
            Some(Ok(code)) => self.exchange(code),
            Some(Err(e)) => flow.error = Some(e.to_string()),
            None => return false,
        }
        true
    }

    fn logged_in(&mut self, tokens: Tokens) {
        self.login = None;
        let Character { id, name, .. } = tokens.character.clone();
        let Some(accounts) = &mut self.accounts else { return };
        if let Err(e) = accounts.store(&tokens) {
            self.notices.push(e);
            return;
        }
        let live = self.live.entry(id).or_default();
        live.expired = false;
        if let Some(tracker) = &self.tracker {
            tracker.send(Command::Add { id, refresh: tokens.refresh });
            if self.active.as_ref().is_some_and(|a| a.character == id) {
                tracker.send(Command::SetActive(Some(id)));
            }
        }
        self.notices.push(format!("Logged in as {name}"));
    }

    fn name(&self, id: u64) -> String {
        let name = self.accounts.as_ref().and_then(|a| a.characters.iter().find(|e| e.id == id)).map(|e| e.name.clone());
        name.unwrap_or_else(|| id.to_string())
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Location { id, system } => {
                self.live.entry(id).or_default().system = Some(system);
                self.observe(id, system);
            }
            Event::Ship { id, ship } => self.live.entry(id).or_default().ship = Some(ship),
            Event::Online { id, online } => self.live.entry(id).or_default().online = Some(online),
            Event::Refreshed(tokens) => {
                if let Some(accounts) = &mut self.accounts
                    && let Err(e) = accounts.store(&tokens)
                {
                    self.notices.push(e);
                }
            }
            Event::LoginExpired(id) => {
                self.live.entry(id).or_default().expired = true;
                let name = self.name(id);
                self.notices.push(format!("Tracking paused — {name}'s login expired. Log in again."));
            }
            Event::Sending { done, .. } => {
                if let Some(send) = &mut self.send {
                    send.done = done;
                }
            }
            Event::Sent { .. } => {
                self.send = None;
                if let Some(route) = self.pending.take() {
                    self.activate(route);
                }
            }
            Event::SendFailed { done, total, error, .. } => {
                if let Some(send) = &mut self.send {
                    send.done = done;
                    send.failed = Some(format!("Sent {done} of {total}. The game now has a partial route. {error}"));
                }
            }
            Event::Limited(limited) => self.limited = limited,
            Event::Error { text, .. } => self.notices.push(text),
        }
    }

    /// Feed a location of the active pilot to the route.
    fn observe(&mut self, id: u64, system: u32) {
        let Some(route) = self.active.as_mut().filter(|r| r.character == id) else { return };
        let observation = route.observe(&Location { solar_system_id: system, station_id: None, structure_id: None });
        let route = route.clone();
        match observation {
            Observation::Same => return,
            Observation::NextSegment { waypoints, .. } => self.send_waypoints(id, waypoints),
            Observation::Arrived | Observation::Progress(_) | Observation::OffRoute { .. } => {}
        }
        self.save(&route);
    }

    /// The scopes that a character lacks.
    pub fn missing_scopes(&self, id: u64) -> Vec<&'static str> {
        let Some(entry) = self.accounts.as_ref().and_then(|a| a.characters.iter().find(|e| e.id == id)) else { return Vec::new() };
        Character { id, name: String::new(), scopes: entry.scopes.clone() }.missing_scopes()
    }
}

/// The route from the system `from` through `stops` (system IDs), with the current settings.
pub fn route_from(uni: &Universe, settings: &Settings, from: u32, stops: &[u32], now: u64) -> Result<Route, String> {
    let node = |id: &u32| uni.by_id.get(id).copied().ok_or_else(|| format!("System {id} is not on the map"));
    let nodes = std::iter::once(&from).chain(stops).map(node).collect::<Result<Vec<_>, _>>()?;
    let mut routes = settings.router(uni, now).routes(&nodes, 1)?;
    if routes.is_empty() { Err("No route.".into()) } else { Ok(routes.remove(0)) }
}

/// The confirm step of a route start.
pub struct StartPlan {
    /// The route as planned.
    pub planned: ActiveRoute,
    /// The route from the current system of the pilot, when the pilot is not on the route.
    /// The UI offers it as the default.
    pub from_here: Option<ActiveRoute>,
    /// The current system of the pilot, when the pilot is not on the route.
    pub here: Option<u32>,
    pub online: Option<bool>,
    /// Why `from_here` is missing, when the search failed.
    pub error: Option<String>,
    /// The hull of the pilot, when it does not suit the planned hull. The UI offers "Plan for" it.
    pub flown: Option<HullClass>,
}

impl StartPlan {
    /// `number` is the route number in the list, from 1.
    pub fn new(uni: &Universe, settings: &Settings, route: &Route, number: usize, pilot: &PilotView, now: u64) -> StartPlan {
        let planned = ActiveRoute::new(uni, &settings.rules, route, number, pilot.id, &pilot.name, now);
        let mut plan = StartPlan { planned, from_here: None, here: None, online: pilot.live.online, error: None, flown: None };
        let flown = pilot.live.ship.as_ref().and_then(|s| hull_by_type(s.ship_type_id));
        plan.flown = flown.filter(|&f| !settings.rules.hull.is_some_and(|planned| same_class(planned, f)));
        let Some(system) = pilot.live.system.filter(|s| !plan.planned.steps.iter().any(|step| step.system == *s)) else { return plan };
        plan.here = Some(system);
        let stops: Vec<u32> = plan.planned.stops.iter().skip(1).map(|&i| plan.planned.steps[i].system).collect();
        match route_from(uni, settings, system, &stops, now) {
            Ok(r) => plan.from_here = Some(ActiveRoute::new(uni, &settings.rules, &r, number, pilot.id, &pilot.name, now)),
            Err(e) => plan.error = Some(e),
        }
        plan
    }

    /// The route that the default action sends.
    pub fn default_route(&self) -> &ActiveRoute {
        self.from_here.as_ref().unwrap_or(&self.planned)
    }
}

impl Pilots {
    /// "Re-route from here": the route from the current system of the active pilot through the
    /// stops ahead. The UI shows its jump count, and sends it with `replace_route` on confirm.
    pub fn reroute(&self, uni: &Universe, settings: &Settings, now: u64) -> Result<ActiveRoute, String> {
        let active = self.active.as_ref().ok_or("No active route.")?;
        let system = self.live.get(&active.character).and_then(|l| l.system).ok_or("The location of the pilot is not known yet.")?;
        let stops: Vec<u32> = active.stops.iter().filter(|&&i| i > active.progress).map(|&i| active.steps[i].system).collect();
        let route = route_from(uni, settings, system, &stops, now)?;
        Ok(ActiveRoute::new(uni, &settings.rules, &route, active.number, active.character, &active.character_name, now))
    }
}

/// `Pilots` with a test `Api` and short poll intervals, for the tests of the front ends.
#[cfg(any(test, feature = "test-support"))]
impl Pilots {
    pub fn for_tests(accounts: Accounts, api: impl super::tracker::Api, active_path: PathBuf) -> Pilots {
        // Short intervals, so a test sees a change in milliseconds.
        let ms = std::time::Duration::from_millis;
        let intervals = Intervals { active_location: ms(20), location: ms(50), ship: ms(1000), online: ms(1000) };
        let tracker = Tracker::start(api, intervals, || {});
        Self::with_parts(Some(accounts), Some("test-client".into()), Some(tracker), active_path, Arc::new(|| {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esi::active::{Hop, Step};
    use crate::esi::store::MemoryStore;
    use crate::esi::tracker::fake::Fake;
    use oauth2::{AccessToken, RefreshToken, Scope};
    use std::time::{Duration, Instant};

    fn tokens(id: u64, name: &str) -> Tokens {
        Tokens {
            character: Character { id, name: name.into(), scopes: crate::esi::SCOPES.iter().map(|s| Scope::new(s.to_string())).collect() },
            access: AccessToken::new("a".into()),
            refresh: RefreshToken::new("r".into()),
            expires_at: 0,
        }
    }

    fn pilots(name: &str, fake: &Fake) -> Pilots {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let mut accounts = Accounts::with_store(None, Box::new(MemoryStore::default())).unwrap();
        accounts.store(&tokens(1, "Alice")).unwrap();
        Pilots::for_tests(accounts, fake.clone(), dir.join("active-route.json"))
    }

    fn route(systems: &[u32], manual: Option<u32>) -> ActiveRoute {
        let steps = systems
            .iter()
            .enumerate()
            .map(|(i, &system)| Step {
                system,
                hop: if i == 0 {
                    Hop::Start
                } else if Some(system) == manual {
                    Hop::Wormhole
                } else {
                    Hop::Gate
                },
                via: String::new(),
                sig: None,
            })
            .collect();
        ActiveRoute::from_steps(steps, vec![0, systems.len() - 1], 1, 1, "Alice")
    }

    /// Call `update` until `done` is true.
    fn until(p: &mut Pilots, mut done: impl FnMut(&Pilots) -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            p.update();
            if done(p) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timeout");
    }

    #[test]
    fn characters_show_with_a_client_id_or_a_character() {
        let dir = std::env::temp_dir().join("eve-router-test-pilots-show");
        // No client ID and no token store: nothing to show.
        assert!(!Pilots::offline(dir.join("active-route.json")).shows_characters());
        // `for_tests` sets a client ID.
        let fake = Fake::default();
        assert!(pilots("eve-router-test-pilots-show-2", &fake).shows_characters());
    }

    #[test]
    fn sync_hull_follows_the_pilot() {
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let fake = Fake::default();
        let mut p = pilots("eve-router-test-pilots-sync", &fake);
        let mut s = settings(&uni, Some("black-ops"));
        // Before the first ship poll, nothing changes.
        s.hull_source = HullSource::Pilot(1);
        assert_eq!(p.sync_hull(&mut s), HullSync::None);
        until(&mut p, |p| p.live.get(&1).is_some_and(|l| l.ship.is_some()));
        assert_eq!(p.sync_hull(&mut s), HullSync::Hull);
        assert_eq!(s.rules.hull.unwrap().name, "Capsule");
        assert_eq!(p.sync_hull(&mut s), HullSync::None);
    }

    #[test]
    fn sync_hull_replans_when_the_active_route_ends() {
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let fake = Fake::default();
        let mut p = pilots("eve-router-test-pilots-sync-active", &fake);
        until(&mut p, |p| p.live.get(&1).is_some_and(|l| l.ship.is_some()));
        let mut s = settings(&uni, Some("Sin"));
        s.hull_source = HullSource::Pilot(1);
        // The ship changes during a route: the route stays.
        p.active = Some(route(&[10, 20], None));
        assert_eq!(p.sync_hull(&mut s), HullSync::Hull);
        assert_eq!(p.sync_hull(&mut s), HullSync::None);
        // The route ends: the routes in the planner are for the old hull, so plan again one time.
        p.active = None;
        assert_eq!(p.sync_hull(&mut s), HullSync::Hull);
        assert_eq!(p.sync_hull(&mut s), HullSync::None);
    }

    #[test]
    fn sync_hull_drops_a_removed_pilot() {
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let dir = std::env::temp_dir().join("eve-router-test-pilots-sync-removed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A store with a list file, as with a keyring.
        let mut accounts = Accounts::with_store(Some(dir.join("characters.json")), Box::new(MemoryStore::default())).unwrap();
        accounts.store(&tokens(1, "Alice")).unwrap();
        let mut p = Pilots::for_tests(accounts, Fake::default(), dir.join("active-route.json"));
        let mut s = settings(&uni, Some("Sin"));
        // A character that is not stored: the source becomes manual, and the hull stays.
        s.hull_source = HullSource::Pilot(99);
        assert_eq!(p.sync_hull(&mut s), HullSync::Source);
        assert_eq!(s.hull_source, HullSource::Manual);
        assert_eq!(s.rules.hull.unwrap().name, "Sin");
    }

    #[test]
    fn sync_hull_keeps_the_pilot_without_a_token_store() {
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let mut p = Pilots::offline(std::env::temp_dir().join("eve-router-test-pilots-sync-off").join("active-route.json"));
        let mut s = settings(&uni, Some("Sin"));
        s.hull_source = HullSource::Pilot(1);
        assert_eq!(p.sync_hull(&mut s), HullSync::None);
        assert_eq!(s.hull_source, HullSource::Pilot(1));
    }

    #[test]
    fn sync_hull_keeps_the_pilot_with_a_session_only_store() {
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let mut p = Pilots::offline(std::env::temp_dir().join("eve-router-test-pilots-sync-session").join("active-route.json"));
        p.use_session_only();
        let mut s = settings(&uni, Some("Sin"));
        s.hull_source = HullSource::Pilot(1);
        assert_eq!(p.sync_hull(&mut s), HullSync::None);
        assert_eq!(s.hull_source, HullSource::Pilot(1));
    }

    #[test]
    fn pilot_rows_list_the_characters_then_the_hulls() {
        let fake = Fake::default();
        let p = pilots("eve-router-test-pilots-rows", &fake);
        let rows = p.pilot_rows("");
        assert!(matches!(&rows[0], PilotRow::Pilot(v) if v.name == "Alice"));
        assert!(matches!(rows[1], PilotRow::Hull(None)));
        assert!(matches!(rows[2], PilotRow::Hull(Some(_))));
        // The filter applies to the names and the hulls.
        let rows = p.pilot_rows("ali");
        assert!(matches!(&rows[0], PilotRow::Pilot(_)));
        assert!(rows.iter().skip(1).all(|r| matches!(r, PilotRow::Hull(_))));
        let rows = p.pilot_rows("rorqual");
        assert!(matches!(rows[0], PilotRow::Hull(None)));
        assert!(matches!(rows[1], PilotRow::Hull(Some(h)) if h.name == "Rorqual"));
    }

    #[test]
    fn pilot_row_marks_the_current_source() {
        use crate::ansiblex::find_hull;
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let fake = Fake::default();
        let p = pilots("eve-router-test-pilots-current", &fake);
        let mut s = settings(&uni, Some("Sin"));
        let alice = p.pilot_rows("").into_iter().next().unwrap();
        let sin = PilotRow::Hull(find_hull("Sin"));
        assert!(sin.is_current(&s) && !alice.is_current(&s));
        // With a pilot source, the hull row of the same hull is not current.
        s.hull_source = HullSource::Pilot(1);
        assert!(alice.is_current(&s) && !sin.is_current(&s));
        assert!(!PilotRow::Hull(None).is_current(&s));
    }

    #[test]
    fn start_route_becomes_active_after_the_send() {
        let fake = Fake::default();
        fake.0.lock().unwrap().system = 10;
        let mut p = pilots("eve-router-test-pilots-start", &fake);
        until(&mut p, |p| p.live.get(&1).and_then(|l| l.system) == Some(10));
        p.start_route(route(&[10, 20, 30], None));
        assert!(p.active.is_none() && p.pending.is_some());
        until(&mut p, |p| p.active.is_some());
        assert!(p.send.is_none());
        assert_eq!(fake.0.lock().unwrap().waypoints, [(20, true), (30, false)]);
        assert!(p.characters()[0].active);
        assert!(ActiveRoute::load(&p.active_path).is_some());
        p.stop_route();
        assert!(ActiveRoute::load(&p.active_path).is_none());
    }

    #[test]
    fn failed_send_keeps_the_route_pending() {
        let fake = Fake::default();
        fake.0.lock().unwrap().fail_waypoint_at = Some(1);
        let mut p = pilots("eve-router-test-pilots-fail", &fake);
        p.start_route(route(&[10, 20, 30], None));
        until(&mut p, |p| p.send.as_ref().is_some_and(|s| s.failed.is_some()));
        assert!(p.active.is_none());
        assert!(p.send.as_ref().unwrap().failed.as_ref().unwrap().starts_with("Sent 1 of 2."));
        // Retry sends all waypoints again, with a clear first.
        fake.0.lock().unwrap().fail_waypoint_at = None;
        p.retry_send();
        until(&mut p, |p| p.active.is_some());
        assert_eq!(fake.0.lock().unwrap().waypoints, [(20, true), (20, true), (30, false)]);
    }

    #[test]
    fn manual_hop_sends_the_next_segment() {
        let fake = Fake::default();
        fake.0.lock().unwrap().system = 10;
        let mut p = pilots("eve-router-test-pilots-segment", &fake);
        p.start_route(route(&[10, 20, 30, 40], Some(30)));
        until(&mut p, |p| p.active.is_some());
        assert_eq!(fake.0.lock().unwrap().waypoints, [(20, true)]);
        // The pilot takes the bridge into 30.
        fake.0.lock().unwrap().system = 30;
        until(&mut p, |p| p.active.as_ref().is_some_and(|a| a.progress == 2) && p.send.is_none());
        assert_eq!(fake.0.lock().unwrap().waypoints, [(20, true), (40, true)]);
    }

    #[test]
    fn expired_login_gives_a_notice() {
        let fake = Fake::default();
        fake.0.lock().unwrap().refresh_rejected = true;
        let mut p = pilots("eve-router-test-pilots-expired", &fake);
        until(&mut p, |p| p.live.get(&1).is_some_and(|l| l.expired));
        assert_eq!(p.notices, ["Tracking paused — Alice's login expired. Log in again."]);
    }

    #[test]
    fn resume_and_discard() {
        let fake = Fake::default();
        let mut p = pilots("eve-router-test-pilots-resume", &fake);
        let r = route(&[10, 20], None);
        r.save(&p.active_path).unwrap();
        p.resume = ActiveRoute::load(&p.active_path);
        p.resume_route();
        assert_eq!(p.active.as_ref().map(|a| a.steps.len()), Some(2));
        p.stop_route();
        r.save(&p.active_path).unwrap();
        p.resume = ActiveRoute::load(&p.active_path);
        p.discard_resume();
        assert!(p.resume.is_none() && ActiveRoute::load(&p.active_path).is_none());
    }

    #[test]
    fn start_plan_offers_the_route_from_here() {
        use crate::test_support::{FIXTURE_TIME, overlay_universe, settings};
        let uni = overlay_universe();
        let s = settings(&uni, None);
        let nodes = [uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()];
        let route = &s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap()[0];
        let rens = uni.system(uni.exact("Rens").unwrap()).id;
        let mut pilot = PilotView { id: 7, name: "Alice".into(), live: PilotState::default(), needs_reauth: false, active: false };
        // No location yet: the planned route only.
        let plan = StartPlan::new(&uni, &s, route, 1, &pilot, FIXTURE_TIME);
        assert!(plan.from_here.is_none() && plan.here.is_none());
        // The pilot is in Rens, not on the route.
        pilot.live.system = Some(rens);
        let plan = StartPlan::new(&uni, &s, route, 1, &pilot, FIXTURE_TIME);
        assert_eq!(plan.here, Some(rens));
        let from_here = plan.from_here.as_ref().unwrap();
        assert_eq!(from_here.steps[0].system, rens);
        assert_eq!(from_here.steps.last().map(|s| s.system), plan.planned.steps.last().map(|s| s.system));
        assert_eq!(plan.default_route(), from_here);
    }

    #[test]
    fn start_plan_compares_the_flown_hull() {
        use crate::ansiblex::find_hull;
        use crate::test_support::{FIXTURE_TIME, overlay_universe, settings};
        let uni = overlay_universe();
        let s = settings(&uni, Some("black-ops"));
        let nodes = [uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()];
        let route = &s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap()[0];
        let mut pilot = PilotView { id: 7, name: "Alice".into(), live: PilotState::default(), needs_reauth: false, active: false };
        let flies = |pilot: &mut PilotView, name: &str| {
            let type_id = find_hull(name).unwrap().type_id.unwrap();
            pilot.live.ship = Some(Ship { ship_type_id: type_id, ship_item_id: 1, ship_name: String::new() });
        };
        // The ship is not known: no check.
        assert!(StartPlan::new(&uni, &s, route, 1, &pilot, FIXTURE_TIME).flown.is_none());
        // A Sin is in the planned group.
        flies(&mut pilot, "Sin");
        assert!(StartPlan::new(&uni, &s, route, 1, &pilot, FIXTURE_TIME).flown.is_none());
        // A Rorqual is not.
        flies(&mut pilot, "Rorqual");
        assert_eq!(StartPlan::new(&uni, &s, route, 1, &pilot, FIXTURE_TIME).flown.unwrap().name, "Rorqual");
    }

    #[test]
    fn senders_leave_out_offline_pilots() {
        let fake = Fake::default();
        let mut p = pilots("eve-router-test-pilots-senders", &fake);
        // The online state is not known yet: the pilot stays in the list.
        assert_eq!(p.senders().len(), 1);
        until(&mut p, |p| p.live.get(&1).is_some_and(|l| l.online == Some(true)));
        assert_eq!(p.senders().len(), 1);
        p.live.get_mut(&1).unwrap().online = Some(false);
        assert!(p.senders().is_empty());
        p.live.get_mut(&1).unwrap().expired = true;
        p.live.get_mut(&1).unwrap().online = Some(true);
        assert!(p.senders().is_empty());
    }

    #[test]
    fn a_pilot_shows_at_one_step_only() {
        let pilot = |id: u64, system: u32| PilotView {
            id,
            name: format!("P{id}"),
            live: PilotState { system: Some(system), ..PilotState::default() },
            needs_reauth: false,
            active: false,
        };
        // A round trip: system 1 is the start and the destination.
        let systems = [1, 2, 3, 1];
        let pilots = [pilot(7, 1), pilot(8, 3), pilot(9, 99)];
        let ids = |rows: &[Vec<PilotView>]| rows.iter().map(|r| r.iter().map(|p| p.id).collect()).collect::<Vec<Vec<u64>>>();
        // In the planner, a pilot shows at the first visit of its system.
        assert_eq!(ids(&pilots_by_step(&systems, &pilots, None)), [vec![7], vec![], vec![8], vec![]]);
        // The active pilot shows at the progress step. The other pilots stay at the first visit.
        let mut active = route(&systems, None);
        active.character = 7;
        active.progress = 3;
        assert_eq!(ids(&pilots_by_step(&systems, &pilots, Some(&active))), [vec![], vec![], vec![8], vec![7]]);
    }

    #[test]
    fn remove_stops_the_route_of_that_pilot() {
        let fake = Fake::default();
        let mut p = pilots("eve-router-test-pilots-remove", &fake);
        p.active = Some(route(&[10, 20], None));
        p.remove(1);
        assert!(p.active.is_none());
        assert!(p.characters().is_empty());
    }
}
