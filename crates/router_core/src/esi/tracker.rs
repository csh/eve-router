//! The tracker: one thread that keeps the access tokens fresh, polls each character, and sends
//! the waypoints of a route. The front ends send `Command`s and read `Event`s. Neither front end
//! calls ESI or SSO itself.
//!
//! Poll intervals (`Intervals::default`):
//! - location: 5 s for the active pilot, 30 s for the others,
//! - ship: 30 s,
//! - online: 60 s.
//!
//! A poll never comes before the `Expires` time of the last response. When the ESI error budget
//! falls below `SLOW_BELOW`, each interval is 4 times longer, and `Event::Limited(true)` goes out.

use super::client::{Esi, EsiError, Fetched, Location, Online, Ship};
use super::sso::{LoginError, Sso, Tokens};
use oauth2::{AccessToken, RefreshToken};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Below this error budget, the tracker slows down.
pub const SLOW_BELOW: u32 = 20;
/// The intervals are this many times longer while the tracker is slow.
const SLOW_FACTOR: u32 = 4;
/// Refresh an access token this many seconds before it expires.
const REFRESH_MARGIN: u64 = 60;
/// The wait after a network error or a 420 with no reset time.
const RETRY: Duration = Duration::from_secs(30);

/// The SSO and ESI calls of the tracker. `Live` uses the real services. The tests use a fake.
pub trait Api: Send + 'static {
    fn refresh(&self, token: &RefreshToken) -> Result<Tokens, LoginError>;
    fn location(&self, token: &AccessToken, id: u64) -> Result<Fetched<Location>, EsiError>;
    fn ship(&self, token: &AccessToken, id: u64) -> Result<Fetched<Ship>, EsiError>;
    fn online(&self, token: &AccessToken, id: u64) -> Result<Fetched<Online>, EsiError>;
    fn add_waypoint(&self, token: &AccessToken, system: u32, clear: bool) -> Result<(), EsiError>;
}

pub struct Live {
    pub sso: Sso,
    pub esi: Esi,
}

impl Api for Live {
    fn refresh(&self, token: &RefreshToken) -> Result<Tokens, LoginError> {
        self.sso.refresh(token)
    }
    fn location(&self, token: &AccessToken, id: u64) -> Result<Fetched<Location>, EsiError> {
        self.esi.location(token, id)
    }
    fn ship(&self, token: &AccessToken, id: u64) -> Result<Fetched<Ship>, EsiError> {
        self.esi.ship(token, id)
    }
    fn online(&self, token: &AccessToken, id: u64) -> Result<Fetched<Online>, EsiError> {
        self.esi.online(token, id)
    }
    fn add_waypoint(&self, token: &AccessToken, system: u32, clear: bool) -> Result<(), EsiError> {
        self.esi.add_waypoint(token, system, clear)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Intervals {
    pub active_location: Duration,
    pub location: Duration,
    pub ship: Duration,
    pub online: Duration,
}

impl Default for Intervals {
    fn default() -> Self {
        Intervals {
            active_location: Duration::from_secs(5),
            location: Duration::from_secs(30),
            ship: Duration::from_secs(30),
            online: Duration::from_secs(60),
        }
    }
}

pub enum Command {
    /// Track a character. A second `Add` replaces the refresh token, for example after a new login.
    Add { id: u64, refresh: RefreshToken },
    /// Stop the tracking of a character.
    Remove(u64),
    /// The pilot of the active route. It gets the short location interval.
    SetActive(Option<u64>),
    /// Send the waypoints, in order. The first one clears the in-game route.
    Send { id: u64, systems: Vec<u32> },
}

#[derive(Debug)]
pub enum Event {
    /// The current system of a character. It goes out at the first poll and at each change.
    Location {
        id: u64,
        system: u32,
    },
    Ship {
        id: u64,
        ship: Ship,
    },
    Online {
        id: u64,
        online: bool,
    },
    /// New tokens. Store the refresh token with `Accounts::store`, because SSO can rotate it.
    Refreshed(Tokens),
    /// SSO refused the refresh token. The character must log in again. Tracking pauses.
    LoginExpired(u64),
    Sending {
        id: u64,
        done: usize,
        total: usize,
    },
    Sent {
        id: u64,
        total: usize,
    },
    /// The send stopped. `done` waypoints are in the game.
    SendFailed {
        id: u64,
        done: usize,
        total: usize,
        error: String,
    },
    /// True while ESI limits the router, or while the error budget is low.
    Limited(bool),
    /// A problem that does not stop the tracking, for the status line.
    Error {
        id: u64,
        text: String,
    },
}

/// The handle of the tracker thread. Drop it to stop the thread.
pub struct Tracker {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Tracker {
    /// Start the thread. `wake` runs after each event, for example to repaint the window.
    pub fn start(api: impl Api, intervals: Intervals, wake: impl Fn() + Send + 'static) -> Tracker {
        let (tx, commands) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        std::thread::spawn(move || Worker::new(api, intervals, events_tx, wake).run(commands));
        Tracker { tx, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
}

struct Pilot {
    refresh: RefreshToken,
    access: Option<(AccessToken, u64)>,
    /// No polls until a new `Add`, after SSO refused the refresh token.
    paused: bool,
    next_location: Instant,
    next_ship: Instant,
    next_online: Instant,
    system: Option<u32>,
    ship: Option<Ship>,
    online: Option<bool>,
}

struct Worker<A, W> {
    api: A,
    intervals: Intervals,
    events: Sender<Event>,
    wake: W,
    pilots: HashMap<u64, Pilot>,
    active: Option<u64>,
    /// No calls before this time, after a 420 or 429.
    blocked_until: Option<Instant>,
    slow: bool,
}

/// The result of one poll: the event to send (if any), the expiry and the error budget.
type Polled = (Option<Event>, Option<SystemTime>, Option<u32>);

enum Kind {
    Location,
    Ship,
    Online,
}

impl<A: Api, W: Fn()> Worker<A, W> {
    fn new(api: A, intervals: Intervals, events: Sender<Event>, wake: W) -> Self {
        Worker { api, intervals, events, wake, pilots: HashMap::new(), active: None, blocked_until: None, slow: false }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        (self.wake)();
    }

    fn run(mut self, commands: Receiver<Command>) {
        loop {
            let wait = self.next_due().map_or(Duration::from_secs(1), |due| due.saturating_duration_since(Instant::now()));
            match commands.recv_timeout(wait) {
                Ok(command) => self.command(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.poll(Instant::now());
        }
    }

    /// The earliest time at which a poll is due.
    fn next_due(&self) -> Option<Instant> {
        let polls = self.pilots.values().filter(|p| !p.paused).flat_map(|p| [p.next_location, p.next_ship, p.next_online]);
        let due = polls.min()?;
        Some(self.blocked_until.map_or(due, |b| due.max(b)))
    }

    fn command(&mut self, command: Command) {
        let now = Instant::now();
        match command {
            Command::Add { id, refresh } => {
                let pilot = Pilot {
                    refresh,
                    access: None,
                    paused: false,
                    next_location: now,
                    next_ship: now,
                    next_online: now,
                    system: None,
                    ship: None,
                    online: None,
                };
                self.pilots.insert(id, pilot);
            }
            Command::Remove(id) => {
                self.pilots.remove(&id);
            }
            Command::SetActive(id) => {
                self.active = id;
                // The new active pilot gets a location at once.
                if let Some(pilot) = id.and_then(|id| self.pilots.get_mut(&id)) {
                    pilot.next_location = now;
                }
            }
            Command::Send { id, systems } => self.send_route(id, &systems),
        }
    }

    /// Scale an interval while the tracker is slow.
    fn interval(&self, base: Duration) -> Duration {
        if self.slow { base * SLOW_FACTOR } else { base }
    }

    fn poll(&mut self, now: Instant) {
        if self.blocked_until.is_some_and(|b| now < b) {
            return;
        }
        self.blocked_until = None;
        let ids: Vec<u64> = self.pilots.keys().copied().collect();
        for id in ids {
            for kind in [Kind::Location, Kind::Ship, Kind::Online] {
                let Some(pilot) = self.pilots.get(&id) else { break };
                let due = match kind {
                    Kind::Location => pilot.next_location,
                    Kind::Ship => pilot.next_ship,
                    Kind::Online => pilot.next_online,
                };
                if pilot.paused || now < due {
                    continue;
                }
                self.poll_one(id, kind, now);
                if self.blocked_until.is_some() {
                    return;
                }
            }
        }
    }

    fn poll_one(&mut self, id: u64, kind: Kind, now: Instant) {
        let Some(token) = self.access_token(id) else { return };
        let base = match kind {
            Kind::Location if self.active == Some(id) => self.intervals.active_location,
            Kind::Location => self.intervals.location,
            Kind::Ship => self.intervals.ship,
            Kind::Online => self.intervals.online,
        };
        let interval = self.interval(base);
        let result: Result<Polled, EsiError> = match kind {
            Kind::Location => self.api.location(&token, id).map(|f| {
                let pilot = self.pilots.get_mut(&id).expect("pilot");
                let changed = pilot.system != Some(f.value.solar_system_id);
                pilot.system = Some(f.value.solar_system_id);
                (changed.then_some(Event::Location { id, system: f.value.solar_system_id }), f.expires, f.errors_left)
            }),
            Kind::Ship => self.api.ship(&token, id).map(|f| {
                let pilot = self.pilots.get_mut(&id).expect("pilot");
                let changed = pilot.ship.as_ref() != Some(&f.value);
                pilot.ship = Some(f.value.clone());
                (changed.then_some(Event::Ship { id, ship: f.value }), f.expires, f.errors_left)
            }),
            Kind::Online => self.api.online(&token, id).map(|f| {
                let pilot = self.pilots.get_mut(&id).expect("pilot");
                let changed = pilot.online != Some(f.value.online);
                pilot.online = Some(f.value.online);
                (changed.then_some(Event::Online { id, online: f.value.online }), f.expires, f.errors_left)
            }),
        };
        let next = match result {
            Ok((event, expires, errors_left)) => {
                if let Some(event) = event {
                    self.emit(event);
                }
                self.budget(errors_left);
                // Not before ESI has new data.
                let fresh = expires.and_then(|e| e.duration_since(SystemTime::now()).ok()).map_or(now, |d| now + d);
                (now + interval).max(fresh)
            }
            Err(e) => self.esi_error(id, e, now) + interval,
        };
        let Some(pilot) = self.pilots.get_mut(&id) else { return };
        match kind {
            Kind::Location => pilot.next_location = next,
            Kind::Ship => pilot.next_ship = next,
            Kind::Online => pilot.next_online = next,
        }
    }

    /// Handle an ESI error. The result is the time from which the next interval counts.
    fn esi_error(&mut self, id: u64, error: EsiError, now: Instant) -> Instant {
        match error {
            // The token can be revoked or expired early. The next poll refreshes it.
            EsiError::Auth => {
                if let Some(pilot) = self.pilots.get_mut(&id) {
                    pilot.access = None;
                }
                now
            }
            EsiError::Limited { retry_after } => {
                let until = now + retry_after.unwrap_or(RETRY);
                self.blocked_until = Some(until);
                if !self.slow {
                    self.slow = true;
                    self.emit(Event::Limited(true));
                }
                until
            }
            EsiError::Offline(_) | EsiError::Status(_) => {
                self.emit(Event::Error { id, text: error.to_string() });
                now + RETRY
            }
        }
    }

    /// Track the error budget. Slow down below `SLOW_BELOW`, and go back to normal above it.
    fn budget(&mut self, errors_left: Option<u32>) {
        let Some(left) = errors_left else { return };
        let slow = left < SLOW_BELOW;
        if slow != self.slow {
            self.slow = slow;
            self.emit(Event::Limited(slow));
        }
    }

    /// A valid access token, after a refresh if necessary. `None` if the refresh failed.
    fn access_token(&mut self, id: u64) -> Option<AccessToken> {
        let now = unix_now();
        let pilot = self.pilots.get(&id)?;
        if let Some((token, expires_at)) = &pilot.access
            && now + REFRESH_MARGIN < *expires_at
        {
            return Some(token.clone());
        }
        match self.api.refresh(&pilot.refresh) {
            Ok(tokens) => {
                let pilot = self.pilots.get_mut(&id)?;
                pilot.refresh = tokens.refresh.clone();
                pilot.access = Some((tokens.access.clone(), tokens.expires_at));
                let token = tokens.access.clone();
                self.emit(Event::Refreshed(tokens));
                Some(token)
            }
            Err(LoginError::Rejected(_) | LoginError::BadToken(_)) => {
                if let Some(pilot) = self.pilots.get_mut(&id) {
                    pilot.paused = true;
                    pilot.access = None;
                }
                self.emit(Event::LoginExpired(id));
                None
            }
            Err(e) => {
                // SSO is offline. Try again after `RETRY`.
                let retry = Instant::now() + RETRY;
                if let Some(pilot) = self.pilots.get_mut(&id) {
                    pilot.next_location = pilot.next_location.max(retry);
                    pilot.next_ship = pilot.next_ship.max(retry);
                    pilot.next_online = pilot.next_online.max(retry);
                }
                self.emit(Event::Error { id, text: e.to_string() });
                None
            }
        }
    }

    /// Send all waypoints in order. Stop at the first failure. An auth failure gets one
    /// refresh and one retry.
    fn send_route(&mut self, id: u64, systems: &[u32]) {
        let total = systems.len();
        let fail = |error: String, done: usize| Event::SendFailed { id, done, total, error };
        if !self.pilots.contains_key(&id) {
            self.emit(fail("The character is not logged in.".into(), 0));
            return;
        }
        for (done, &system) in systems.iter().enumerate() {
            self.emit(Event::Sending { id, done, total });
            let mut retried = false;
            loop {
                let Some(token) = self.access_token(id) else {
                    self.emit(fail("The login of the character expired.".into(), done));
                    return;
                };
                match self.api.add_waypoint(&token, system, done == 0) {
                    Ok(()) => break,
                    Err(EsiError::Auth) if !retried => {
                        retried = true;
                        if let Some(pilot) = self.pilots.get_mut(&id) {
                            pilot.access = None;
                        }
                    }
                    Err(e) => {
                        self.emit(fail(e.to_string(), done));
                        return;
                    }
                }
            }
        }
        self.emit(Event::Sent { id, total });
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A fake `Api` for the tests of the tracker and of `Pilots`.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use crate::esi::Character;
    use std::sync::{Arc, Mutex};

    /// A fake API. Each call goes into `calls`. The location comes from `system`.
    #[derive(Clone, Default)]
    pub(crate) struct Fake(pub Arc<Mutex<FakeState>>);

    #[derive(Default)]
    pub(crate) struct FakeState {
        pub calls: Vec<String>,
        pub system: u32,
        pub refresh_rejected: bool,
        pub errors_left: Option<u32>,
        pub limited: bool,
        /// The waypoint call fails at this index.
        pub fail_waypoint_at: Option<usize>,
        pub waypoints: Vec<(u32, bool)>,
    }

    pub(crate) fn fetched<T>(value: T, errors_left: Option<u32>) -> Fetched<T> {
        Fetched { value, expires: None, errors_left }
    }

    impl Api for Fake {
        fn refresh(&self, token: &RefreshToken) -> Result<Tokens, LoginError> {
            let mut s = self.0.lock().unwrap();
            s.calls.push(format!("refresh {}", token.secret()));
            if s.refresh_rejected {
                return Err(LoginError::Rejected("invalid_grant".into()));
            }
            Ok(Tokens {
                character: Character { id: 1, name: "Alice".into(), scopes: vec![] },
                access: AccessToken::new("a".into()),
                refresh: RefreshToken::new(format!("{}+", token.secret())),
                expires_at: unix_now() + 1200,
            })
        }
        fn location(&self, _: &AccessToken, id: u64) -> Result<Fetched<Location>, EsiError> {
            let mut s = self.0.lock().unwrap();
            s.calls.push(format!("location {id}"));
            if s.limited {
                return Err(EsiError::Limited { retry_after: Some(Duration::from_secs(60)) });
            }
            Ok(fetched(Location { solar_system_id: s.system, station_id: None, structure_id: None }, s.errors_left))
        }
        fn ship(&self, _: &AccessToken, id: u64) -> Result<Fetched<Ship>, EsiError> {
            self.0.lock().unwrap().calls.push(format!("ship {id}"));
            Ok(fetched(Ship { ship_type_id: 670, ship_item_id: 1, ship_name: "Pod".into() }, None))
        }
        fn online(&self, _: &AccessToken, id: u64) -> Result<Fetched<Online>, EsiError> {
            self.0.lock().unwrap().calls.push(format!("online {id}"));
            Ok(fetched(Online { online: true }, None))
        }
        fn add_waypoint(&self, _: &AccessToken, system: u32, clear: bool) -> Result<(), EsiError> {
            let mut s = self.0.lock().unwrap();
            if s.fail_waypoint_at == Some(s.waypoints.len()) {
                return Err(EsiError::Offline("down".into()));
            }
            s.waypoints.push((system, clear));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    fn fast() -> Intervals {
        let ms = Duration::from_millis;
        Intervals { active_location: ms(20), location: ms(200), ship: ms(10_000), online: ms(10_000) }
    }

    /// Wait for the first event that `pick` accepts. Other events are dropped.
    fn wait_for<T>(tracker: &Tracker, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Ok(event) = tracker.events.recv_timeout(Duration::from_millis(50))
                && let Some(value) = pick(event)
            {
                return value;
            }
        }
        panic!("no matching event");
    }

    fn start(fake: &Fake) -> Tracker {
        let tracker = Tracker::start(fake.clone(), fast(), || {});
        tracker.send(Command::Add { id: 1, refresh: RefreshToken::new("r".into()) });
        tracker
    }

    #[test]
    fn first_poll_refreshes_and_reports_everything() {
        let fake = Fake::default();
        fake.0.lock().unwrap().system = 30000142;
        let tracker = start(&fake);
        let tokens = wait_for(&tracker, |e| if let Event::Refreshed(t) = e { Some(t) } else { None });
        assert_eq!(tokens.refresh.secret(), "r+");
        assert_eq!(wait_for(&tracker, |e| if let Event::Location { system, .. } = e { Some(system) } else { None }), 30000142);
        wait_for(&tracker, |e| matches!(e, Event::Ship { .. }).then_some(()));
        wait_for(&tracker, |e| matches!(e, Event::Online { online: true, .. }).then_some(()));
    }

    #[test]
    fn location_event_only_on_change() {
        let fake = Fake::default();
        fake.0.lock().unwrap().system = 1;
        let tracker = start(&fake);
        tracker.send(Command::SetActive(Some(1)));
        wait_for(&tracker, |e| matches!(e, Event::Location { system: 1, .. }).then_some(()));
        std::thread::sleep(Duration::from_millis(100));
        fake.0.lock().unwrap().system = 2;
        // Polls between the two changes give no event.
        let mut seen = vec![];
        wait_for(&tracker, |e| match e {
            Event::Location { system, .. } => {
                seen.push(system);
                (system == 2).then_some(())
            }
            _ => None,
        });
        assert_eq!(seen, [2]);
        let polls = fake.0.lock().unwrap().calls.iter().filter(|c| *c == "location 1").count();
        assert!(polls >= 3, "{polls} polls");
    }

    #[test]
    fn rejected_refresh_pauses_the_pilot() {
        let fake = Fake::default();
        fake.0.lock().unwrap().refresh_rejected = true;
        let tracker = start(&fake);
        assert_eq!(wait_for(&tracker, |e| if let Event::LoginExpired(id) = e { Some(id) } else { None }), 1);
        std::thread::sleep(Duration::from_millis(100));
        let calls = fake.0.lock().unwrap().calls.clone();
        assert_eq!(calls, ["refresh r"], "no polls after the refusal");
    }

    #[test]
    fn send_all_waypoints_in_order() {
        let fake = Fake::default();
        let tracker = start(&fake);
        tracker.send(Command::Send { id: 1, systems: vec![10, 20, 30] });
        assert_eq!(wait_for(&tracker, |e| if let Event::Sent { total, .. } = e { Some(total) } else { None }), 3);
        assert_eq!(fake.0.lock().unwrap().waypoints, [(10, true), (20, false), (30, false)]);
    }

    #[test]
    fn send_stops_at_the_first_failure() {
        let fake = Fake::default();
        fake.0.lock().unwrap().fail_waypoint_at = Some(1);
        let tracker = start(&fake);
        tracker.send(Command::Send { id: 1, systems: vec![10, 20, 30] });
        let (done, total) = wait_for(&tracker, |e| if let Event::SendFailed { done, total, .. } = e { Some((done, total)) } else { None });
        assert_eq!((done, total), (1, 3));
        assert_eq!(fake.0.lock().unwrap().waypoints, [(10, true)]);
    }

    #[test]
    fn send_to_an_unknown_character_fails() {
        let tracker = Tracker::start(Fake::default(), fast(), || {});
        tracker.send(Command::Send { id: 9, systems: vec![10] });
        assert_eq!(wait_for(&tracker, |e| if let Event::SendFailed { done, .. } = e { Some(done) } else { None }), 0);
    }

    #[test]
    fn low_error_budget_slows_the_tracker() {
        let fake = Fake::default();
        fake.0.lock().unwrap().errors_left = Some(5);
        let tracker = start(&fake);
        wait_for(&tracker, |e| matches!(e, Event::Limited(true)).then_some(()));
        fake.0.lock().unwrap().errors_left = Some(90);
        wait_for(&tracker, |e| matches!(e, Event::Limited(false)).then_some(()));
    }

    #[test]
    fn error_limit_blocks_all_calls() {
        let fake = Fake::default();
        fake.0.lock().unwrap().limited = true;
        let tracker = start(&fake);
        tracker.send(Command::SetActive(Some(1)));
        wait_for(&tracker, |e| matches!(e, Event::Limited(true)).then_some(()));
        std::thread::sleep(Duration::from_millis(150));
        let locations = fake.0.lock().unwrap().calls.iter().filter(|c| c.starts_with("location")).count();
        assert_eq!(locations, 1, "no call while blocked");
    }
}
