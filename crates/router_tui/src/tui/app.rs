//! The TUI state and the key handling.

use super::pilots::{LOCKED, WORMHOLES_CHANGED, route_setting};
use petgraph::graph::NodeIndex;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::{ListState, TableState};
use router_core::config::{self, ApiKey, Config, RunOverrides};
use router_core::esi::active::{ActiveRoute, active_path};
use router_core::esi::pilots::{PilotRow, Pilots, StartPlan};
use router_core::labels::Shortcuts;
use router_core::refresh::{self, Refresher, RouteKey, Snapshot, kept_route, route_note};
use router_core::route::{Mode, Route};
use router_core::settings::{HullSource, Settings, parse_max_cap, resolve_all, split_systems};
use router_core::sources::{
    self,
    nexum::{self, MapInfo},
};
use router_core::universe::Universe;
use router_core::wormhole::SourceData;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

#[derive(PartialEq, Eq)]
pub enum Focus {
    Input,
    Routes,
    /// The step table of the selected route.
    Detail,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Capital,
    MaxCap,
    Favourite,
    NexumUrl,
    NexumKey,
}

/// The rows of the settings page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsRow {
    Capital,
    MaxCap,
    Favourite(usize),
    AddFavourite,
    NexumUrl,
    NexumKey,
    NexumMap,
    /// The EVE-Scout hub switches.
    Thera,
    Turnur,
}

pub enum Popup {
    /// Row i is `Mode::ALL[i]`. The last row is "optimize order".
    Mode(ListState),
    /// The Pilot picker. The rows are `Pilots::pilot_rows(filter)`.
    Pilot {
        filter: String,
        state: ListState,
    },
    Prompt {
        kind: PromptKind,
        text: String,
    },
    /// The Nexum map list. Row i is `maps[i]`.
    Maps {
        maps: Vec<MapInfo>,
        state: ListState,
    },
    /// "Loading maps…", while the map list fetch runs.
    Loading,
    /// A message. Any key closes it.
    Message(String),
    /// The character list. Row i is `pilots.characters()[i]`.
    Characters(ListState),
    /// "Session only?", when the system has no keyring.
    SessionOnly,
    /// The login: the URL, and a field for the redirected URL.
    Login {
        paste: String,
    },
    /// The character for a route start. Row i is `ids[i]`.
    Pick {
        route: usize,
        ids: Vec<u64>,
        state: ListState,
    },
    /// "Send route #n to <name>?"
    Confirm(Box<StartPlan>),
    RemovePilot(u64),
    StopRoute,
    Quit,
    /// "Resume route #n?" at the start, for the active route of the last run.
    Resume,
    /// The new route from the current system, after "Re-route from here".
    Reroute(Box<ActiveRoute>),
}

pub struct App {
    pub uni: Arc<Universe>,
    pub settings: Settings,
    cfg: Config,
    /// The file values that the CLI flags replace for this run. `write_config` puts them back.
    pub overrides: RunOverrides,
    cfg_path: PathBuf,
    pub input: String,
    pub focus: Focus,
    pub popup: Option<Popup>,
    pub routes: Vec<Route>,
    /// The systems of the last search. A wormhole refresh searches again with them, not with
    /// the text in the input box.
    pub searched: Vec<NodeIndex>,
    pub selected: ListState,
    /// The selected step in the step table.
    pub detail: TableState,
    /// The number of step rows that the step table shows. The draw code sets it.
    pub detail_page: usize,
    /// The settings page. `Some` while it is open. The state selects a row.
    pub settings_page: Option<ListState>,
    /// The time of the last route search, with the sidebar search.
    pub route_time: Option<Duration>,
    /// The time of the last route search, in Unix seconds. The labels use it.
    pub now: u64,
    pub hub_origin: Option<String>,
    pub hubs: Vec<(NodeIndex, Option<u64>)>,
    pub status: String,
    pub shortcuts: Shortcuts,
    /// The wormhole data of each source, from the startup or the last refresh.
    pub wormhole_data: Vec<SourceData>,
    /// The refresh worker. `None` in the tests, and after the worker stops.
    pub refresher: Option<Refresher>,
    pub quit: bool,
    /// True after Enter on the map row. The run loop draws, then calls `load_maps`.
    pub load_maps_pending: bool,
    /// The last map list, for the map name on the settings page.
    pub map_names: Vec<MapInfo>,
    /// The logins, the tracking and the active route.
    pub pilots: Pilots,
    /// The route to start after the current login.
    pub start_after_login: Option<usize>,
    /// The progress of the active route at the last draw, to scroll the table to a new step.
    pub last_progress: Option<usize>,
}

impl App {
    pub fn new(uni: Arc<Universe>, settings: Settings, cfg: Config, cfg_path: PathBuf, input: String, shortcuts: Shortcuts) -> Self {
        let mut app = App {
            uni,
            settings,
            cfg,
            overrides: RunOverrides::default(),
            cfg_path: cfg_path.clone(),
            focus: if input.is_empty() { Focus::Input } else { Focus::Routes },
            input,
            popup: None,
            routes: Vec::new(),
            searched: Vec::new(),
            selected: ListState::default(),
            detail: TableState::default(),
            detail_page: 10,
            settings_page: None,
            route_time: None,
            now: router_core::wormhole::now(),
            hub_origin: None,
            hubs: Vec::new(),
            status: String::new(),
            shortcuts,
            wormhole_data: Vec::new(),
            refresher: None,
            quit: false,
            load_maps_pending: false,
            map_names: Vec::new(),
            pilots: Pilots::offline(active_path(&cfg_path)),
            start_after_login: None,
            last_progress: None,
        };
        app.recompute();
        if let Some(warning) = &app.shortcuts.warning {
            app.status = if app.status.is_empty() { warning.clone() } else { format!("{} · {warning}", app.status) };
        }
        app
    }

    pub fn selected_route(&self) -> Option<&Route> {
        self.selected.selected().and_then(|i| self.routes.get(i))
    }

    /// Find the routes and the hub distances again, for the systems in the input box.
    pub fn recompute(&mut self) {
        self.reset_routes();
        let names = split_systems(&self.input);
        if names.is_empty() {
            self.searched.clear();
            self.hubs.clear();
            self.hub_origin = None;
            return;
        }
        match resolve_all(&self.uni, &names) {
            Ok(nodes) => {
                self.searched = nodes;
                self.search();
            }
            Err(e) => {
                self.searched.clear();
                self.status = e;
            }
        }
    }

    /// Clear the routes, the selection and the status. The status gets the jump bridge problem, if any.
    fn reset_routes(&mut self) {
        self.routes.clear();
        self.selected.select(None);
        self.detail = TableState::default();
        // The table of an active route also uses `detail`. The next tick marks the step again.
        self.last_progress = None;
        self.status.clear();
        if let Some(reason) = self.settings.rules.blocked_reason() {
            self.status = format!("Jump bridges off: {reason}");
        }
    }

    /// Find the routes and the hub distances for `searched`, which has one system or more.
    fn search(&mut self) {
        let started = Instant::now();
        self.now = router_core::wormhole::now();
        let nodes = &self.searched;
        let router = self.settings.router(&self.uni, self.now);
        self.hub_origin = Some(self.uni.name(nodes[0]).to_string());
        // The sidebar search and the route search are independent, so they run at the same time.
        let (hubs, routes) = rayon::join(
            || router.jumps_to(nodes[0], &self.settings.favourites),
            || {
                (nodes.len() >= 2).then(|| {
                    let (order, changed) = self.settings.order(&router, nodes)?;
                    router.routes(&order, self.settings.top).map(|routes| (routes, changed))
                })
            },
        );
        self.route_time = Some(started.elapsed());
        self.hubs = hubs;
        match routes {
            None => {}
            Some(Ok((routes, changed))) => {
                self.routes = routes;
                self.selected.select(Some(0));
                if let Some(text) = changed {
                    self.append_status(&text);
                }
            }
            Some(Err(e)) => self.status = e,
        }
    }

    /// Add a text to the status line, after a " · ".
    fn append_status(&mut self, text: &str) {
        self.status = if self.status.is_empty() { text.to_string() } else { format!("{} · {text}", self.status) };
    }

    /// Swap in a new map from the refresh worker. Search again with the systems of the last
    /// search. Keep the selected route and step if `kept_route` finds the route. Else select
    /// the first route. With no search, only swap the map.
    pub fn apply_snapshot(&mut self, snap: Snapshot) {
        let old_uni = std::mem::replace(&mut self.uni, snap.uni);
        self.shortcuts = snap.shortcuts;
        self.wormhole_data = snap.all;
        // The map loaded, so the text "Loading the new map" is not true any more.
        if self.status.contains(refresh::NEXUM_LOADING) {
            self.status = self.status.split(" · ").filter(|part| *part != refresh::NEXUM_LOADING).collect::<Vec<_>>().join(" · ");
        }
        if !self.searched.is_empty() {
            let old = self.selected_route().map(|r| r.path.clone());
            let old_keys: Vec<_> = self.routes.iter().map(|r| RouteKey::new(&old_uni, &r.path)).collect();
            let old_detail = self.detail;
            self.reset_routes();
            self.search();
            let kept = old.as_ref().and_then(|path| kept_route(&self.routes, &self.uni, &old_uni, path));
            if let Some(i) = kept {
                self.selected.select(Some(i));
                self.detail = old_detail;
            }
            // A route number of the old list can point to another route now.
            let same = self.routes.iter().map(|r| RouteKey::new(&self.uni, &r.path)).eq(old_keys);
            let forgot = !same && self.forget_route_choice();
            let note = old.and_then(|path| route_note(&old_uni, &path, &self.uni, kept.is_some()));
            for text in note.into_iter().chain(forgot.then(|| WORMHOLES_CHANGED.to_string())) {
                self.append_status(&text);
            }
        }
        // The status line shows a fetch problem, as at startup, but only one time.
        if let Some(warning) = self.shortcuts.warning.clone()
            && !self.status.contains(&warning)
        {
            self.append_status(&warning);
        }
    }

    /// Take the newest map from the refresh worker, if it sent one. The run loop calls this
    /// each 250 ms. A stopped worker gives a status note one time, and the map stays.
    pub fn poll_refresh(&mut self) {
        let Some(refresher) = &self.refresher else { return };
        match refresher.try_recv() {
            Ok(snap) => self.apply_snapshot(snap),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.refresher = None;
                self.append_status(refresh::STOPPED);
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        // A first send of a route runs or failed. Only its keys work.
        if self.pilots.pending.is_some() {
            self.on_sending_key(key);
            return;
        }
        if let Some(popup) = self.popup.take() {
            self.popup = self.on_popup_key(popup, key);
            return;
        }
        if self.settings_page.is_some() {
            self.on_settings_key(key);
            return;
        }
        if self.pilots.active.is_some() {
            self.on_active_key(key);
            return;
        }
        match self.focus {
            Focus::Input => self.on_input_key(key),
            Focus::Routes => self.on_routes_key(key),
            Focus::Detail => self.on_detail_key(key),
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.recompute();
                self.focus = Focus::Routes;
            }
            KeyCode::Esc => self.focus = Focus::Routes,
            KeyCode::Tab => self.complete_input(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    /// Complete the last system name in the input.
    fn complete_input(&mut self) {
        let start = self.input.rfind(['>', ',']).map_or(0, |i| i + 1);
        let partial = self.input[start..].trim().to_string();
        if partial.is_empty() {
            return;
        }
        match self.complete(&partial) {
            Some(name) => {
                self.input.truncate(start);
                if start > 0 {
                    self.input.push(' ');
                }
                self.input += &name;
            }
            None => self.status = format!("No system starts with \"{partial}\""),
        }
    }

    /// The full name for a prefix, or the longest common prefix if more than one name matches.
    fn complete(&mut self, partial: &str) -> Option<String> {
        let matches = self.uni.complete(partial);
        let names: Vec<&str> = matches.iter().map(|&n| self.uni.name(n)).collect();
        let first = *names.first()?;
        if names.len() > 1 {
            self.status = format!("{} matches: {}", names.len(), names.iter().take(8).copied().collect::<Vec<_>>().join(", "));
        }
        let common = names
            .iter()
            .fold(first.len(), |len, n| first.chars().zip(n.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count().min(len));
        Some(first.chars().take(common).collect())
    }

    fn on_routes_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('i') | KeyCode::Char('/') => self.focus = Focus::Input,
            KeyCode::Enter if self.selected_route().is_some() => {
                self.focus = Focus::Detail;
                if self.detail.selected().is_none() {
                    self.detail.select(Some(0));
                }
            }
            KeyCode::Up => {
                self.selected.select_previous();
                self.detail = TableState::default();
            }
            KeyCode::Down => {
                if self.selected.selected().is_some_and(|i| i + 1 < self.routes.len()) {
                    self.selected.select_next();
                    self.detail = TableState::default();
                }
            }
            KeyCode::Char('m') => {
                let current = Mode::ALL.iter().position(|&m| m == self.settings.mode);
                self.popup = Some(Popup::Mode(ListState::default().with_selected(current)));
            }
            KeyCode::Char('w') => {
                self.settings.wormholes = !self.settings.wormholes;
                self.recompute();
            }
            KeyCode::Char('j') => {
                self.settings.bridges = !self.settings.bridges;
                self.recompute();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.settings.top += 1;
                self.recompute();
            }
            KeyCode::Char('-') if self.settings.top > 1 => {
                self.settings.top -= 1;
                self.recompute();
            }
            KeyCode::Char('h') => {
                let row = self.pilots.pilot_rows("").iter().position(|r| r.is_current(&self.settings)).unwrap_or(0);
                let state = ListState::default().with_selected(Some(row));
                self.popup = Some(Popup::Pilot { filter: String::new(), state });
            }
            KeyCode::Char('s') => self.settings_page = Some(ListState::default().with_selected(Some(0))),
            KeyCode::Char('g') => self.start_route(),
            KeyCode::Char('c') => self.open_characters(),
            _ => {}
        }
    }

    fn on_detail_key(&mut self, key: KeyEvent) {
        let Some(route) = self.selected_route() else {
            self.focus = Focus::Routes;
            return;
        };
        let last = route.jumps;
        let current = self.detail.selected().unwrap_or(0);
        let page = self.detail_page.max(1);
        let target = match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.focus = Focus::Routes;
                return;
            }
            KeyCode::Char('q') => {
                self.quit = true;
                return;
            }
            KeyCode::Char('g') => {
                self.start_route();
                return;
            }
            KeyCode::Up => current.saturating_sub(1),
            KeyCode::Down => current + 1,
            KeyCode::PageUp => current.saturating_sub(page),
            KeyCode::PageDown => current + page,
            KeyCode::Home => 0,
            KeyCode::End => last,
            // n: the next stop. p: the previous stop.
            KeyCode::Char('n') => route.stops.iter().copied().find(|&s| s > current).unwrap_or(current),
            KeyCode::Char('p') => route.stops.iter().rev().copied().find(|&s| s < current).unwrap_or(current),
            _ => return,
        };
        self.detail.select(Some(target.min(last)));
    }

    pub fn settings_rows(&self) -> Vec<SettingsRow> {
        let mut rows = vec![SettingsRow::Capital, SettingsRow::MaxCap];
        rows.extend((0..self.settings.favourites.len()).map(SettingsRow::Favourite));
        rows.push(SettingsRow::AddFavourite);
        rows.extend([SettingsRow::NexumUrl, SettingsRow::NexumKey, SettingsRow::NexumMap, SettingsRow::Thera, SettingsRow::Turnur]);
        rows
    }

    fn on_settings_key(&mut self, key: KeyEvent) {
        let rows = self.settings_rows();
        let Some(state) = self.settings_page.as_mut() else {
            return;
        };
        let index = state.selected().unwrap_or(0).min(rows.len() - 1);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // While a route is active, only the favourites can change.
        if self.pilots.active.is_some() && key.code == KeyCode::Enter && route_setting(rows[index]) {
            self.status = LOCKED.into();
            return;
        }
        let favs = &mut self.settings.favourites;
        match (key.code, rows[index]) {
            (KeyCode::Esc | KeyCode::Char('s') | KeyCode::Char('q'), _) => {
                self.settings_page = None;
                self.save();
                return;
            }
            // Shift+Up and Shift+Down move a favourite.
            (KeyCode::Up, SettingsRow::Favourite(i)) if shift && i > 0 => {
                favs.swap(i, i - 1);
                state.select_previous();
            }
            (KeyCode::Down, SettingsRow::Favourite(i)) if shift && i + 1 < favs.len() => {
                favs.swap(i, i + 1);
                state.select_next();
            }
            (KeyCode::Up, _) if !shift => {
                state.select_previous();
                return;
            }
            (KeyCode::Down, _) if !shift && index + 1 < rows.len() => {
                state.select_next();
                return;
            }
            (KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('d'), SettingsRow::Favourite(i)) => {
                favs.remove(i);
            }
            (KeyCode::Enter, SettingsRow::Capital) => {
                let text = self.settings.rules.capital.map(|n| self.uni.name(n).to_string()).unwrap_or_default();
                self.popup = Some(Popup::Prompt { kind: PromptKind::Capital, text });
                return;
            }
            (KeyCode::Enter, SettingsRow::MaxCap) => {
                let text = self.settings.rules.max_cap.map(|m| m.to_string()).unwrap_or_default();
                self.popup = Some(Popup::Prompt { kind: PromptKind::MaxCap, text });
                return;
            }
            (KeyCode::Enter, SettingsRow::NexumUrl) => {
                let text = self.cfg.nexum.url.clone().unwrap_or_default();
                self.popup = Some(Popup::Prompt { kind: PromptKind::NexumUrl, text });
                return;
            }
            (KeyCode::Enter, SettingsRow::NexumKey) => {
                // The prompt starts empty, so the key never shows on the screen.
                self.popup = Some(Popup::Prompt { kind: PromptKind::NexumKey, text: String::new() });
                return;
            }
            (KeyCode::Enter, SettingsRow::Thera) => {
                self.settings.hubs.thera = !self.settings.hubs.thera;
                self.recompute();
                self.save();
                return;
            }
            (KeyCode::Enter, SettingsRow::Turnur) => {
                self.settings.hubs.turnur = !self.settings.hubs.turnur;
                self.recompute();
                self.save();
                return;
            }
            (KeyCode::Enter, SettingsRow::NexumMap) => {
                if self.cfg.nexum.url.is_none() || self.cfg.nexum.key.is_none() {
                    self.status = "Set the Nexum URL and key first".into();
                } else {
                    self.popup = Some(Popup::Loading);
                    self.load_maps_pending = true;
                }
                return;
            }
            (KeyCode::Enter | KeyCode::Char('a'), SettingsRow::AddFavourite) | (KeyCode::Char('a'), _) => {
                self.popup = Some(Popup::Prompt { kind: PromptKind::Favourite, text: String::new() });
                return;
            }
            _ => return,
        }
        self.recompute();
    }

    /// Handle a key in a popup. Return the popup if it stays open.
    fn on_popup_key(&mut self, popup: Popup, key: KeyEvent) -> Option<Popup> {
        match popup {
            Popup::Mode(mut state) => match key.code {
                KeyCode::Esc => None,
                KeyCode::Up => {
                    state.select_previous();
                    Some(Popup::Mode(state))
                }
                KeyCode::Down => {
                    if state.selected().is_some_and(|i| i < Mode::ALL.len()) {
                        state.select_next();
                    }
                    Some(Popup::Mode(state))
                }
                // The "optimize order" row toggles, and the popup stays open.
                KeyCode::Enter if state.selected() == Some(Mode::ALL.len()) => {
                    self.settings.optimize = !self.settings.optimize;
                    self.recompute();
                    Some(Popup::Mode(state))
                }
                KeyCode::Enter => {
                    self.settings.mode = Mode::ALL[state.selected().unwrap_or(0)];
                    self.recompute();
                    None
                }
                _ => Some(Popup::Mode(state)),
            },
            Popup::Pilot { mut filter, mut state } => {
                let rows = self.pilots.pilot_rows(&filter);
                match key.code {
                    KeyCode::Esc => return None,
                    KeyCode::Up => state.select_previous(),
                    KeyCode::Down if state.selected().is_some_and(|i| i + 1 < rows.len()) => state.select_next(),
                    KeyCode::Enter => {
                        if let Some(row) = state.selected().and_then(|i| rows.get(i)) {
                            self.pick_pilot(row);
                        }
                        return None;
                    }
                    KeyCode::Backspace | KeyCode::Char(_) => {
                        if let KeyCode::Char(c) = key.code {
                            filter.push(c);
                        } else {
                            filter.pop();
                        }
                        // Select the first match, not the "none" row.
                        let rows = self.pilots.pilot_rows(&filter);
                        let first = if filter.is_empty() { None } else { rows.iter().position(|r| !matches!(r, PilotRow::Hull(None))) };
                        state.select(Some(first.unwrap_or(0)));
                    }
                    _ => {}
                }
                Some(Popup::Pilot { filter, state })
            }
            Popup::Maps { maps, mut state } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Up => {
                    state.select_previous();
                    Some(Popup::Maps { maps, state })
                }
                KeyCode::Down => {
                    if state.selected().is_some_and(|i| i + 1 < maps.len()) {
                        state.select_next();
                    }
                    Some(Popup::Maps { maps, state })
                }
                KeyCode::Enter => {
                    if let Some(map) = state.selected().and_then(|i| maps.get(i)) {
                        self.cfg.nexum.map_id = Some(map.id.clone());
                        self.nexum_saved();
                    }
                    None
                }
                _ => Some(Popup::Maps { maps, state }),
            },
            Popup::Loading => Some(Popup::Loading),
            Popup::Message(_) => None,
            Popup::Characters(_)
            | Popup::SessionOnly
            | Popup::Login { .. }
            | Popup::Pick { .. }
            | Popup::Confirm(_)
            | Popup::RemovePilot(_)
            | Popup::StopRoute
            | Popup::Quit
            | Popup::Resume
            | Popup::Reroute(_) => self.on_pilot_popup_key(popup, key),
            Popup::Prompt { kind, mut text } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Enter => self.apply_prompt(kind, text),
                KeyCode::Backspace => {
                    text.pop();
                    Some(Popup::Prompt { kind, text })
                }
                KeyCode::Tab if matches!(kind, PromptKind::Capital | PromptKind::Favourite) => {
                    if let Some(name) = self.complete(&text) {
                        text = name;
                    }
                    Some(Popup::Prompt { kind, text })
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    Some(Popup::Prompt { kind, text })
                }
                _ => Some(Popup::Prompt { kind, text }),
            },
        }
    }

    /// Apply a prompt value. On an error, keep the prompt open and show the error.
    fn apply_prompt(&mut self, kind: PromptKind, text: String) -> Option<Popup> {
        let value = text.trim();
        match kind {
            PromptKind::Capital if value.is_empty() => self.settings.rules.capital = None,
            PromptKind::Capital => match resolve_all(&self.uni, &[value.to_string()]) {
                Ok(nodes) => self.settings.rules.capital = Some(nodes[0]),
                Err(e) => {
                    self.status = e;
                    return Some(Popup::Prompt { kind, text });
                }
            },
            PromptKind::Favourite if value.is_empty() => return None,
            PromptKind::Favourite => match resolve_all(&self.uni, &[value.to_string()]) {
                Ok(nodes) if self.settings.favourites.contains(&nodes[0]) => {
                    self.status = format!("{} is already a favourite", self.uni.name(nodes[0]));
                    return Some(Popup::Prompt { kind, text });
                }
                Ok(nodes) => {
                    self.settings.favourites.push(nodes[0]);
                    // Select the new favourite, one row above "Add favourite".
                    if let Some(state) = &mut self.settings_page {
                        state.select(Some(1 + self.settings.favourites.len()));
                    }
                }
                Err(e) => {
                    self.status = e;
                    return Some(Popup::Prompt { kind, text });
                }
            },
            PromptKind::NexumUrl => match config::parse_nexum_url(value) {
                Ok(url) => {
                    self.cfg.nexum.url = url;
                    self.nexum_saved();
                    return None;
                }
                Err(e) => {
                    self.status = e;
                    return Some(Popup::Prompt { kind, text });
                }
            },
            PromptKind::NexumKey => {
                self.cfg.nexum.key = (!value.is_empty()).then(|| ApiKey(value.to_string()));
                self.nexum_saved();
                return None;
            }
            PromptKind::MaxCap => match parse_max_cap(value) {
                Ok(max_cap) => self.settings.rules.max_cap = max_cap,
                Err(e) => {
                    self.status = e;
                    return Some(Popup::Prompt { kind, text });
                }
            },
        }
        self.recompute();
        None
    }

    /// Save the config after a change to a Nexum row. The refresh worker loads the new map at once.
    fn nexum_saved(&mut self) {
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

    /// Fetch the map list and open it. This blocks for up to 5 seconds, so the run loop draws
    /// "Loading maps…" before it calls this function.
    pub fn load_maps(&mut self) {
        self.load_maps_pending = false;
        self.popup = Some(match nexum::fetch_maps(&self.cfg.nexum, sources::TIMEOUT) {
            Ok(maps) if maps.is_empty() => Popup::Message("The key has access to no maps".into()),
            Ok(maps) => {
                let current = maps.iter().position(|m| Some(&m.id) == self.cfg.nexum.map_id.as_ref());
                self.map_names = maps.clone();
                Popup::Maps { maps, state: ListState::default().with_selected(Some(current.unwrap_or(0))) }
            }
            Err(sources::FetchError::Offline(detail)) => Popup::Message(format!("Nexum offline: {detail}")),
            Err(e) => Popup::Message(e.status(router_core::wormhole::SourceId::Nexum, None)),
        });
    }

    /// The value text of a Nexum row on the settings page.
    pub fn nexum_value(&self, row: SettingsRow) -> String {
        let n = &self.cfg.nexum;
        match row {
            SettingsRow::NexumUrl => n.url.clone().unwrap_or_else(|| "none".into()),
            SettingsRow::NexumKey => n.key.as_ref().map_or("none".into(), ApiKey::masked),
            SettingsRow::NexumMap => nexum::map_label(n, &self.map_names, &self.wormhole_data),
            _ => String::new(),
        }
    }

    /// Save the config, and show the result on the status line.
    fn save(&mut self) {
        self.status = match self.write_config() {
            Ok(()) => "Config saved!".into(),
            Err(e) => e,
        };
    }

    /// Apply a row of the Pilot picker: follow a character, or set a manual hull.
    fn pick_pilot(&mut self, row: &PilotRow) {
        match row {
            PilotRow::Pilot(v) => {
                self.settings.hull_source = HullSource::Pilot(v.id);
                self.pilots.sync_hull(&mut self.settings);
            }
            PilotRow::Hull(hull) => {
                self.settings.hull_source = HullSource::Manual;
                self.settings.rules.hull = *hull;
            }
        }
        self.recompute();
        self.save_quietly();
    }

    /// Save the config. Show only an error on the status line.
    pub(super) fn save_quietly(&mut self) {
        if let Err(e) = self.write_config() {
            self.status = e;
        }
    }

    /// Copy the settings to the config, and write the config file. A CLI flag value does not go
    /// into the file, unless the user changed that field.
    fn write_config(&mut self) -> Result<(), String> {
        self.settings.store(&self.uni, &mut self.cfg);
        let mut file = self.cfg.clone();
        self.overrides.restore(&mut file);
        file.save(&self.cfg_path)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use router_core::ansiblex::BridgeRules;
    use router_core::config::ApiKey;
    use router_core::config::NexumConfig;
    use router_core::config::RunOverrides;
    use router_core::refresh::{Control, NEXUM_LOADING, Refresher, STOPPED};
    use router_core::route::Mode;
    use router_core::test_support::{hole, serve, shared_universe, snapshot, universe};
    use router_core::wormhole::{SourceId, Wormhole};

    const JITA: u32 = 30000142;
    const AMARR: u32 = 30002187;
    const PERIMETER: u32 = 30000144;

    /// A wormhole from Jita to Amarr, with the signature ABC at Jita. It gives a 1-jump route.
    fn jita_amarr() -> Wormhole {
        Wormhole { sig_a: Some("ABC-123".into()), ..hole(JITA, AMARR) }
    }

    #[test]
    fn a_refresh_keeps_the_same_path_and_step() {
        let mut app = app("refresh-keep", Config::default());
        app.settings.top = 3;
        app.input = "Jita > Amarr".into();
        app.recompute();
        assert!(app.routes.len() >= 2, "{}", app.status);
        app.selected.select(Some(1));
        app.detail.select(Some(3));
        let nodes = app.selected_route().unwrap().path.nodes.clone();
        app.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(app.selected_route().unwrap().path.nodes, nodes);
        assert_eq!(app.detail.selected(), Some(3));
        assert!(!app.status.contains("Wormhole"), "{}", app.status);
    }

    #[test]
    fn a_closed_wormhole_gives_a_note() {
        let mut app = app("refresh-closed", Config::default());
        app.uni = snapshot(vec![jita_amarr()]).uni;
        app.input = "Jita > Amarr".into();
        app.recompute();
        assert_eq!(app.routes[0].wormholes, 1, "{}", app.status);
        app.detail.select(Some(1));
        app.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(app.routes[0].wormholes, 0);
        assert_eq!((app.selected.selected(), app.detail.selected()), (Some(0), None));
        // The note does not replace the jump bridge text. The two join with " · ".
        assert!(app.status.starts_with("Jump bridges off: "), "{}", app.status);
        assert!(app.status.ends_with(" · Wormhole ABC closed: the route changed"), "{}", app.status);
    }

    #[test]
    fn a_new_first_route_gives_a_note() {
        let mut app = app("refresh-new", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        assert_eq!(app.routes[0].wormholes, 0);
        app.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!(app.routes[0].wormholes, 1);
        assert!(app.status.ends_with(" · Wormholes updated: a new route is first"), "{}", app.status);
    }

    #[test]
    fn a_refresh_ignores_half_typed_input() {
        let mut app = app("refresh-typing", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        // "Ama" matches more than one system. The refresh uses the systems of the last search.
        app.input = "Jita > Ama".into();
        app.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(app.input, "Jita > Ama");
        assert_eq!(app.routes.len(), 1);
        assert!(!app.status.contains("more than one system"), "{}", app.status);
    }

    #[test]
    fn a_refresh_with_no_search_only_swaps_the_map() {
        let mut app = app("refresh-none", Config::default());
        app.status = "keep".into();
        let snap = snapshot(vec![jita_amarr()]);
        let uni = Arc::clone(&snap.uni);
        app.apply_snapshot(snap);
        assert!(Arc::ptr_eq(&app.uni, &uni));
        assert_eq!((app.shortcuts.wormholes, app.wormhole_data.len()), (1, 1));
        assert!(app.routes.is_empty());
        assert_eq!(app.status, "keep");
    }

    /// Review focus: the new Nexum map arrives, with no search.
    #[test]
    fn a_refresh_clears_the_loading_text() {
        let mut app = app("refresh-loading", Config::default());
        app.status = format!("keep · {NEXUM_LOADING}");
        app.apply_snapshot(snapshot(Vec::new()));
        assert_eq!(app.status, "keep");
        app.status = NEXUM_LOADING.into();
        let mut snap = snapshot(Vec::new());
        snap.shortcuts.warning = Some("Nexum offline".into());
        app.apply_snapshot(snap);
        assert_eq!(app.status, "Nexum offline");
    }

    /// Review focus: the same fetch warning at each refresh, with no search.
    #[test]
    fn a_fetch_warning_shows_one_time() {
        let mut app = app("refresh-warning", Config::default());
        app.status = "keep".into();
        for _ in 0..2 {
            let mut snap = snapshot(Vec::new());
            snap.shortcuts.warning = Some("EVE-Scout offline".into());
            app.apply_snapshot(snap);
        }
        assert_eq!(app.status, "keep · EVE-Scout offline");
    }

    /// Review focus: a refresh while the character picker of a route start is open.
    #[test]
    fn a_refresh_forgets_the_pick() {
        let mut app = app("refresh-pick", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        app.popup = Some(Popup::Pick { route: 0, ids: vec![7], state: ListState::default() });
        app.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert!(app.popup.is_none());
        assert!(app.status.ends_with(WORMHOLES_CHANGED), "{}", app.status);
    }

    #[test]
    fn a_refresh_with_the_same_routes_keeps_the_pick() {
        let mut app = app("refresh-pick-same", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        app.popup = Some(Popup::Pick { route: 0, ids: vec![7], state: ListState::default() });
        app.start_after_login = Some(0);
        app.apply_snapshot(snapshot(Vec::new()));
        assert!(matches!(app.popup, Some(Popup::Pick { .. })));
        assert_eq!(app.start_after_login, Some(0));
        assert!(!app.status.contains(WORMHOLES_CHANGED), "{}", app.status);
    }

    /// Review focus: a stargate and a wormhole between the same two systems swap places.
    #[test]
    fn swapped_parallel_routes_forget_the_pick() {
        let perimeter = || Wormhole { sig_a: Some("ABC-123".into()), ..hole(JITA, PERIMETER) };
        let mut app = app("refresh-parallel", Config::default());
        app.settings.top = 2;
        app.uni = snapshot(vec![perimeter()]).uni;
        app.input = "Jita > Perimeter".into();
        app.recompute();
        assert_eq!(app.routes.iter().map(|r| r.wormholes).collect::<Vec<_>>(), [0, 1], "{}", app.status);
        // The old list has the wormhole route first. The new search puts the stargate route first.
        app.routes.reverse();
        app.popup = Some(Popup::Pick { route: 0, ids: vec![7], state: ListState::default() });
        app.apply_snapshot(snapshot(vec![perimeter()]));
        assert!(app.popup.is_none());
        assert!(app.status.ends_with(WORMHOLES_CHANGED), "{}", app.status);
        // The selection stays on the wormhole route.
        assert_eq!(app.selected.selected(), Some(1));
    }

    #[test]
    fn a_refresh_selects_the_new_index_of_the_kept_path() {
        let mut app = app("refresh-moved", Config::default());
        app.settings.top = 3;
        app.input = "Jita > Amarr".into();
        app.recompute();
        app.selected.select(Some(0));
        app.detail.select(Some(2));
        let nodes = app.selected_route().unwrap().path.nodes.clone();
        // The new wormhole route is first, so the kept path moves down.
        app.apply_snapshot(snapshot(vec![jita_amarr()]));
        assert_eq!(app.routes[0].wormholes, 1, "{}", app.status);
        assert_eq!(app.selected.selected(), Some(1));
        assert_eq!(app.selected_route().unwrap().path.nodes, nodes);
        assert_eq!(app.detail.selected(), Some(2));
    }

    #[test]
    fn the_poll_applies_a_snapshot_and_reports_a_stopped_worker() {
        let (refresher, snapshots, _control) = Refresher::fake();
        let mut app = app("poll-refresh", Config::default());
        app.refresher = Some(refresher);
        app.poll_refresh();
        assert_eq!(app.shortcuts.wormholes, 0);
        snapshots.send(snapshot(vec![jita_amarr()])).unwrap();
        app.poll_refresh();
        assert_eq!(app.shortcuts.wormholes, 1);
        drop(snapshots);
        app.status.clear();
        app.poll_refresh();
        assert_eq!(app.status, STOPPED);
        assert!(app.refresher.is_none());
        // The note shows one time.
        app.status.clear();
        app.poll_refresh();
        assert_eq!(app.status, "");
    }

    #[test]
    fn a_nexum_change_asks_the_worker_for_the_new_map() {
        let (refresher, _snapshots, control) = Refresher::fake();
        let mut app = app("nexum-refresh", Config::default());
        app.refresher = Some(refresher);
        open_row(&mut app, SettingsRow::NexumUrl);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        type_text(&mut app, "https://nexum.example");
        assert_eq!(app.status, NEXUM_LOADING);
        let Ok(Control::Nexum(sent)) = control.try_recv() else { panic!("no Nexum message") };
        assert_eq!(sent.url.as_deref(), Some("https://nexum.example"));
    }

    #[test]
    fn map_row_uses_the_name_in_the_wormhole_data() {
        let cfg = Config {
            nexum: NexumConfig { url: Some("https://nexum.example".into()), key: None, map_id: Some("m1".into()) },
            ..Default::default()
        };
        let mut app = app("map-name-data", cfg);
        assert_eq!(app.nexum_value(SettingsRow::NexumMap), "m1");
        let origin = nexum::map_url(&app.cfg.nexum);
        app.wormhole_data =
            vec![SourceData { source: SourceId::Nexum, fetched_at: 0, origin, name: Some("Home".into()), holes: Vec::new() }];
        assert_eq!(app.nexum_value(SettingsRow::NexumMap), "Home");
    }

    pub(crate) fn app(name: &str, cfg: Config) -> App {
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 1,
            wormholes: true,
            hubs: Default::default(),
            bridges: false,
            rules: BridgeRules::default(),
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
            costs: Default::default(),
            favourites: Vec::new(),
        };
        let path = std::env::temp_dir().join(format!("eve-router-test-{name}.json"));
        let shortcuts = Shortcuts::new(universe(), &Default::default(), &Default::default(), &Default::default());
        App::new(shared_universe(), settings, cfg, path, String::new(), shortcuts)
    }

    #[test]
    fn settings_arrows_keep_the_routes_and_the_status() {
        let mut app = app("settings-arrows", Config::default());
        app.settings.top = 3;
        app.input = "Jita > Amarr".into();
        app.recompute();
        assert!(app.routes.len() >= 2, "{}", app.status);
        app.selected.select(Some(1));
        app.status = "keep".into();
        open_row(&mut app, SettingsRow::Capital);
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(app.status, "keep");
        assert_eq!(app.selected.selected(), Some(1));
    }

    #[test]
    fn hub_switch_keeps_the_saved_status() {
        let mut app = app("settings-thera", Config::default());
        let thera = app.settings.hubs.thera;
        open_row(&mut app, SettingsRow::Thera);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.settings.hubs.thera, !thera);
        assert_eq!(app.status, "Config saved!");
    }

    #[test]
    fn a_save_keeps_the_file_value_of_a_cli_flag() {
        let file = Config { top: Some(3), ..Config::default() };
        let run = Config { top: Some(5), ..Config::default() };
        let mut app = app("cli-flags", run.clone());
        app.settings.top = 5;
        app.overrides = RunOverrides::new(file, run);
        app.save();
        let path = std::env::temp_dir().join("eve-router-test-cli-flags.json");
        assert_eq!(Config::load(&path).unwrap().top, Some(3));
    }

    fn open_row(app: &mut App, row: SettingsRow) {
        let index = app.settings_rows().iter().position(|&r| r == row).unwrap();
        app.settings_page = Some(ListState::default().with_selected(Some(index)));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
    }

    #[test]
    fn edit_nexum_url_and_key() {
        let mut app = app("edit-url-key", Config::default());
        open_row(&mut app, SettingsRow::NexumUrl);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        type_text(&mut app, "ftp://x");
        // A URL without http:// or https:// keeps the prompt open.
        assert!(matches!(app.popup, Some(Popup::Prompt { kind: PromptKind::NexumUrl, .. })));
        app.popup = None;
        app.on_key(KeyEvent::from(KeyCode::Enter));
        type_text(&mut app, "https://nexum.example/");
        assert_eq!(app.cfg.nexum.url.as_deref(), Some("https://nexum.example"));
        assert!(app.status.contains("Restart to load the new map"), "{}", app.status);

        open_row(&mut app, SettingsRow::NexumKey);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        type_text(&mut app, "nxm_secret_key");
        assert_eq!(app.cfg.nexum.key, Some(ApiKey("nxm_secret_key".into())));
        assert_eq!(app.nexum_value(SettingsRow::NexumKey), "nxm_…key");
        assert_eq!(app.nexum_value(SettingsRow::NexumMap), "none");
    }

    #[test]
    fn pick_a_map_from_the_list() {
        let body = r#"{"maps":[{"id":"m1","name":"Home"},{"id":"m2","name":"Scanning"}]}"#;
        let (url, _) = serve("200 OK", body, std::time::Duration::ZERO);
        let mut cfg = Config::default();
        cfg.nexum.url = Some(url);
        cfg.nexum.key = Some(ApiKey("nxm_test_key".into()));
        let mut app = app("pick-map", cfg);
        open_row(&mut app, SettingsRow::NexumMap);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        // The run loop draws "Loading maps…", then calls load_maps.
        assert!(app.load_maps_pending);
        assert!(matches!(app.popup, Some(Popup::Loading)));
        app.load_maps();
        assert!(!app.load_maps_pending);
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.cfg.nexum.map_id.as_deref(), Some("m2"));
        assert_eq!(app.nexum_value(SettingsRow::NexumMap), "Scanning");
        assert!(app.status.contains("Restart to load the new map"), "{}", app.status);
    }

    #[test]
    fn map_list_error_shows_in_a_popup() {
        let (url, _) = serve("401 Unauthorized", "{}", std::time::Duration::ZERO);
        let mut cfg = Config::default();
        cfg.nexum.url = Some(url);
        cfg.nexum.key = Some(ApiKey("nxm_bad_key".into()));
        let mut app = app("map-error", cfg);
        open_row(&mut app, SettingsRow::NexumMap);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app.load_maps();
        assert!(matches!(&app.popup, Some(Popup::Message(m)) if m == "Nexum key rejected"));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.popup.is_none());
    }

    #[test]
    fn map_row_needs_url_and_key() {
        let mut app = app("map-needs-key", Config::default());
        open_row(&mut app, SettingsRow::NexumMap);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(!app.load_maps_pending);
        assert_eq!(app.status, "Set the Nexum URL and key first");
    }
}
