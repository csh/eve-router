//! The TUI state and the key handling.

use crate::ansiblex::{HullClass, table};
use crate::config::Config;
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
}

/// The rows of the settings page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsRow {
    Capital,
    MaxCap,
    Favourite(usize),
    AddFavourite,
}

pub enum Popup {
    /// Row i is `Mode::ALL[i]`.
    Mode(ListState),
    /// The rows are `hull_rows(filter)`.
    Hull { filter: String, state: ListState },
    Prompt { kind: PromptKind, text: String },
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
            Popup::Prompt { kind, mut text } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Enter => self.apply_prompt(kind, text),
                KeyCode::Backspace => {
                    text.pop();
                    Some(Popup::Prompt { kind, text })
                }
                KeyCode::Tab if kind != PromptKind::MaxCap => {
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

    fn save(&mut self) {
        let rules = &self.settings.rules;
        self.cfg.capital = rules.capital.map(|n| self.uni.name(n).to_string());
        self.cfg.hull = rules.hull.map(|h| h.name.clone());
        self.cfg.max_cap_tj = rules.max_cap;
        self.cfg.mode = Some(self.settings.mode);
        self.cfg.top = Some(self.settings.top);
        self.cfg.favourites = Some(self.settings.favourites.iter().map(|&n| self.uni.name(n).to_string()).collect());
        self.status = match self.cfg.save(&self.cfg_path) {
            Ok(()) => format!("Saved {}", self.cfg_path.display()),
            Err(e) => e,
        };
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
