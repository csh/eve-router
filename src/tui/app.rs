//! The TUI state and the key handling.

use crate::ansiblex::{HullClass, table};
use crate::config::{ApiKey, Config};
use crate::sources::{self, nexum::{self, MapInfo}};
use super::Shortcuts;
use crate::route::{Mode, Route};
use crate::universe::Universe;
use crate::{Settings, resolve_all, split_systems};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::{ListState, TableState};
use petgraph::graph::NodeIndex;
use std::path::PathBuf;
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
    /// Row i is `Mode::ALL[i]`.
    Mode(ListState),
    /// The rows are `hull_rows(filter)`.
    Hull { filter: String, state: ListState },
    Prompt { kind: PromptKind, text: String },
    /// The Nexum map list. Row i is `maps[i]`.
    Maps { maps: Vec<MapInfo>, state: ListState },
    /// "Loading maps…", while the map list fetch runs.
    Loading,
    /// A message. Any key closes it.
    Message(String),
}

pub struct App<'a> {
    pub uni: &'a Universe,
    pub settings: Settings,
    cfg: Config,
    cfg_path: PathBuf,
    pub input: String,
    pub focus: Focus,
    pub popup: Option<Popup>,
    pub routes: Vec<Route>,
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
    pub quit: bool,
    /// True after Enter on the map row. The run loop draws, then calls `load_maps`.
    pub load_maps_pending: bool,
    /// The last map list, for the map name on the settings page.
    pub map_names: Vec<MapInfo>,
}

impl<'a> App<'a> {
    pub fn new(
        uni: &'a Universe,
        settings: Settings,
        cfg: Config,
        cfg_path: PathBuf,
        input: String,
        shortcuts: Shortcuts,
    ) -> Self {
        let mut app = App {
            uni,
            settings,
            cfg,
            cfg_path,
            focus: if input.is_empty() { Focus::Input } else { Focus::Routes },
            input,
            popup: None,
            routes: Vec::new(),
            selected: ListState::default(),
            detail: TableState::default(),
            detail_page: 10,
            settings_page: None,
            route_time: None,
            now: crate::wormhole::now(),
            hub_origin: None,
            hubs: Vec::new(),
            status: String::new(),
            shortcuts,
            quit: false,
            load_maps_pending: false,
            map_names: Vec::new(),
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

    /// Find the routes and the hub distances again.
    pub fn recompute(&mut self) {
        self.routes.clear();
        self.selected.select(None);
        self.detail = TableState::default();
        self.status.clear();
        if let Some(reason) = self.settings.rules.blocked_reason() {
            self.status = format!("Jump bridges off: {reason}");
        }
        let names = split_systems(&self.input);
        if names.is_empty() {
            self.hubs.clear();
            self.hub_origin = None;
            return;
        }
        let nodes = match resolve_all(self.uni, &names) {
            Ok(nodes) => nodes,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let started = Instant::now();
        self.now = crate::wormhole::now();
        let router = self.settings.router(self.uni, self.now);
        self.hub_origin = Some(self.uni.name(nodes[0]).to_string());
        // The sidebar search and the route search are independent, so they run at the same time.
        let (hubs, routes) = rayon::join(
            || router.jumps_to(nodes[0], &self.settings.favourites),
            || (nodes.len() >= 2).then(|| router.routes(&nodes, self.settings.top)),
        );
        self.route_time = Some(started.elapsed());
        self.hubs = hubs;
        match routes {
            None => {}
            Some(Ok(routes)) => {
                self.routes = routes;
                self.selected.select(Some(0));
            }
            Some(Err(e)) => self.status = e,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
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
        let common = names.iter().fold(first.len(), |len, n| {
            first.chars().zip(n.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count().min(len)
        });
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
                let current = self.settings.rules.hull;
                let same = |row: &Option<HullClass>| match (row, current) {
                    (Some(a), Some(b)) => std::ptr::eq(*a, b),
                    (None, None) => true,
                    _ => false,
                };
                let row = hull_rows("").iter().position(same).unwrap_or(0);
                let state = ListState::default().with_selected(Some(row));
                self.popup = Some(Popup::Hull { filter: String::new(), state });
            }
            KeyCode::Char('s') => self.settings_page = Some(ListState::default().with_selected(Some(0))),
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
            (KeyCode::Up, _) if !shift => state.select_previous(),
            (KeyCode::Down, _) if !shift && index + 1 < rows.len() => state.select_next(),
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
                self.save();
                self.recompute();
            }
            (KeyCode::Enter, SettingsRow::Turnur) => {
                self.settings.hubs.turnur = !self.settings.hubs.turnur;
                self.save();
                self.recompute();
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
                    if state.selected().is_some_and(|i| i + 1 < Mode::ALL.len()) {
                        state.select_next();
                    }
                    Some(Popup::Mode(state))
                }
                KeyCode::Enter => {
                    self.settings.mode = Mode::ALL[state.selected().unwrap_or(0)];
                    self.recompute();
                    None
                }
                _ => Some(Popup::Mode(state)),
            },
            Popup::Hull { mut filter, mut state } => {
                let rows = hull_rows(&filter);
                match key.code {
                    KeyCode::Esc => return None,
                    KeyCode::Up => state.select_previous(),
                    KeyCode::Down if state.selected().is_some_and(|i| i + 1 < rows.len()) => state.select_next(),
                    KeyCode::Enter => {
                        if let Some(&hull) = state.selected().and_then(|i| rows.get(i)) {
                            self.settings.rules.hull = hull;
                            self.recompute();
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
                        let first_match = !filter.is_empty() && hull_rows(&filter).len() > 1;
                        state.select(Some(usize::from(first_match)));
                    }
                    _ => {}
                }
                Some(Popup::Hull { filter, state })
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
            PromptKind::Capital => match resolve_all(self.uni, &[value.to_string()]) {
                Ok(nodes) => self.settings.rules.capital = Some(nodes[0]),
                Err(e) => {
                    self.status = e;
                    return Some(Popup::Prompt { kind, text });
                }
            },
            PromptKind::Favourite if value.is_empty() => return None,
            PromptKind::Favourite => match resolve_all(self.uni, &[value.to_string()]) {
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
            PromptKind::NexumUrl if value.is_empty() => {
                self.cfg.nexum.url = None;
                self.nexum_saved();
                return None;
            }
            PromptKind::NexumUrl if value.starts_with("https://") || value.starts_with("http://") => {
                self.cfg.nexum.url = Some(value.trim_end_matches('/').to_string());
                self.nexum_saved();
                return None;
            }
            PromptKind::NexumUrl => {
                self.status = format!("\"{value}\" is not a URL. Give a URL that starts with https://.");
                return Some(Popup::Prompt { kind, text });
            }
            PromptKind::NexumKey => {
                self.cfg.nexum.key = (!value.is_empty()).then(|| ApiKey(value.to_string()));
                self.nexum_saved();
                return None;
            }
            PromptKind::MaxCap if value.is_empty() => self.settings.rules.max_cap = None,
            PromptKind::MaxCap => match value.parse::<f32>() {
                Ok(tj) if (0.0..=table().gate_capacitor_tj).contains(&tj) => self.settings.rules.max_cap = Some(tj),
                _ => {
                    let max = table().gate_capacitor_tj;
                    self.status = format!("\"{value}\" is not a TJ value. Give a number from 0 to {max}, or leave it empty for no limit.");
                    return Some(Popup::Prompt { kind, text });
                }
            },
        }
        self.recompute();
        None
    }

    /// Save the config after a change to a Nexum row. A Nexum change applies at the next start.
    fn nexum_saved(&mut self) {
        self.status = match self.write_config() {
            Ok(()) => "Nexum settings saved. Restart to load the new map.".into(),
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
            Err(e) => Popup::Message(e.status(crate::wormhole::SourceId::Nexum, None)),
        });
    }

    /// The value text of a Nexum row on the settings page.
    pub fn nexum_value(&self, row: SettingsRow) -> String {
        let n = &self.cfg.nexum;
        match row {
            SettingsRow::NexumUrl => n.url.clone().unwrap_or_else(|| "none".into()),
            SettingsRow::NexumKey => n.key.as_ref().map_or("none".into(), ApiKey::masked),
            SettingsRow::NexumMap => match &n.map_id {
                None => "none".into(),
                Some(id) => self.map_names.iter().find(|m| &m.id == id).map_or(id.clone(), |m| m.name.clone()),
            },
            _ => String::new(),
        }
    }

    /// Save the config, and show the result on the status line.
    fn save(&mut self) {
        self.status = match self.write_config() {
            Ok(()) => format!("Saved {}", self.cfg_path.display()),
            Err(e) => e,
        };
    }

    /// Copy the settings to the config, and write the config file.
    fn write_config(&mut self) -> Result<(), String> {
        let rules = &self.settings.rules;
        self.cfg.capital = rules.capital.map(|n| self.uni.name(n).to_string());
        self.cfg.hull = rules.hull.map(|h| h.name.clone());
        self.cfg.max_cap_tj = rules.max_cap;
        self.cfg.mode = Some(self.settings.mode);
        self.cfg.top = Some(self.settings.top);
        self.cfg.eve_scout = self.settings.hubs;
        self.cfg.favourites = Some(self.settings.favourites.iter().map(|&n| self.uni.name(n).to_string()).collect());
        self.cfg.save(&self.cfg_path)
    }
}

/// The hull picker rows: "none", then each ship whose name or group contains `filter`.
pub fn hull_rows(filter: &str) -> Vec<Option<HullClass>> {
    let filter = filter.to_lowercase();
    std::iter::once(None)
        .chain(
            table()
                .ships
                .iter()
                .filter(|h| h.name.to_lowercase().contains(&filter) || h.group.to_lowercase().contains(&filter))
                .map(Some),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansiblex::BridgeRules;
    use crate::config::ApiKey;
    use crate::route::Mode;
    use crate::sources::test_server::serve;
    use crate::universe::tests::universe;

    fn app(name: &str, cfg: Config) -> App<'static> {
        let settings = Settings {
            mode: Mode::Shortest,
            top: 1,
            wormholes: true,
            hubs: Default::default(),
            bridges: false,
            rules: BridgeRules::default(),
            favourites: Vec::new(),
        };
        let path = std::env::temp_dir().join(format!("eve-router-test-{name}.json"));
        let shortcuts = Shortcuts::new(universe(), &Default::default(), &Default::default(), &Default::default());
        App::new(universe(), settings, cfg, path, String::new(), shortcuts)
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
