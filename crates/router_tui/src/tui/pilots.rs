//! The keys of the EVE login, the route start and the active route.

use super::app::{App, Popup, SettingsRow};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;
use router_core::esi::active::ActiveRoute;
use router_core::route::Route;

/// The status text for a key that could change the route.
pub const LOCKED: &str = "Locked while route is active — press x to stop";

/// True for a key that changes the route search. These keys do nothing while a route is active,
/// so the app and the in-game waypoints stay the same.
pub fn route_locked(code: KeyCode) -> bool {
    matches!(code, KeyCode::Char('i' | '/' | 'm' | 'w' | 'j' | 'h' | '+' | '=' | '-') | KeyCode::Enter)
}

/// True for a settings row that changes the route search. Only the favourites stay editable
/// while a route is active.
pub fn route_setting(row: SettingsRow) -> bool {
    !matches!(row, SettingsRow::Favourite(_) | SettingsRow::AddFavourite)
}

/// The confirm step of a route start.
pub struct Confirm {
    pub planned: ActiveRoute,
    /// The route from the current system of the pilot, when the pilot is not on the route.
    pub from_here: Option<ActiveRoute>,
    /// The name of the current system of the pilot, when the pilot is not on the route.
    pub here: Option<String>,
    pub online: Option<bool>,
}

impl App<'_> {
    /// The name of a system, or its ID if the map does not have it.
    pub fn system_name(&self, id: u32) -> String {
        self.uni.by_id.get(&id).map_or_else(|| id.to_string(), |&n| self.uni.name(n).to_string())
    }

    pub fn pilot_name(&self, id: u64) -> String {
        self.pilots.characters().into_iter().find(|c| c.id == id).map_or_else(|| id.to_string(), |c| c.name)
    }

    /// Read the tracker and the login. The run loop calls this after each key and each 250 ms.
    pub fn tick(&mut self) {
        self.pilots.update();
        let notices = std::mem::take(&mut self.pilots.notices);
        if let Some(last) = notices.last() {
            self.status.clone_from(last);
        }
        // The login is done: close its popup, and continue the route start.
        if matches!(self.popup, Some(Popup::Login { .. })) && self.pilots.login.is_none() {
            self.popup = None;
            if let Some(route) = self.start_after_login.take() {
                self.start_route_at(route);
            }
        }
        // Scroll the table to a new step.
        let progress = self.pilots.active.as_ref().map(|a| a.progress);
        if progress != self.last_progress {
            self.detail.select(progress);
            self.last_progress = progress;
        }
    }

    /// Ask "Resume?" when the last run left an active route.
    pub fn on_open(&mut self) {
        if self.pilots.resume.is_some() {
            self.popup = Some(Popup::Resume);
        }
    }

    pub fn open_characters(&mut self) {
        if self.pilots.accounts.is_none() && self.pilots.keyring_error.is_some() {
            self.popup = Some(Popup::SessionOnly);
            return;
        }
        self.popup = Some(Popup::Characters(ListState::default().with_selected(Some(0))));
    }

    fn begin_login(&mut self) -> Option<Popup> {
        if self.pilots.accounts.is_none() && self.pilots.keyring_error.is_some() {
            return Some(Popup::SessionOnly);
        }
        match self.pilots.start_login() {
            Ok(()) => Some(Popup::Login { paste: String::new() }),
            Err(e) => Some(Popup::Message(e)),
        }
    }

    /// `g`: start the selected route.
    pub fn start_route(&mut self) {
        match self.selected.selected().filter(|&i| i < self.routes.len()) {
            Some(index) => self.start_route_at(index),
            None => self.status = "Select a route first".into(),
        }
    }

    fn start_route_at(&mut self, index: usize) {
        let ids: Vec<u64> = self.pilots.characters().into_iter().filter(|c| !c.live.expired).map(|c| c.id).collect();
        self.popup = match ids.len() {
            0 => {
                self.start_after_login = Some(index);
                self.begin_login()
            }
            1 => self.confirm(index, ids[0]),
            _ => {
                let row = self.pilots.last_used().and_then(|id| ids.iter().position(|&i| i == id)).unwrap_or(0);
                Some(Popup::Pick { route: index, ids, state: ListState::default().with_selected(Some(row)) })
            }
        };
    }

    /// The route from `from` through the given stops, with the current settings.
    fn route_from(&self, from: u32, stops: &[u32]) -> Result<Route, String> {
        let node = |id: &u32| self.uni.by_id.get(id).copied().ok_or_else(|| format!("System {id} is not on the map"));
        let nodes = std::iter::once(&from).chain(stops).map(node).collect::<Result<Vec<_>, _>>()?;
        let mut routes = self.settings.router(self.uni, router_core::wormhole::now()).routes(&nodes, 1)?;
        if routes.is_empty() { Err("No route.".into()) } else { Ok(routes.remove(0)) }
    }

    fn confirm(&mut self, index: usize, id: u64) -> Option<Popup> {
        let route = &self.routes[index];
        let name = self.pilot_name(id);
        let now = router_core::wormhole::now();
        let planned = ActiveRoute::new(self.uni, &self.settings.rules, route, index + 1, id, &name, now);
        let live = self.pilots.live.get(&id).cloned().unwrap_or_default();
        let mut confirm = Confirm { planned, from_here: None, here: None, online: live.online };
        // The pilot is not on the route: offer the route from the current system.
        if let Some(system) = live.system.filter(|s| !confirm.planned.steps.iter().any(|step| step.system == *s)) {
            let stops: Vec<u32> = confirm.planned.stops.iter().skip(1).map(|&i| confirm.planned.steps[i].system).collect();
            confirm.here = Some(self.system_name(system));
            match self.route_from(system, &stops) {
                Ok(r) => confirm.from_here = Some(ActiveRoute::new(self.uni, &self.settings.rules, &r, index + 1, id, &name, now)),
                Err(e) => self.status = e,
            }
        }
        Some(Popup::Confirm(Box::new(confirm)))
    }

    /// The keys while the first send of a route runs or failed.
    pub fn on_sending_key(&mut self, key: KeyEvent) {
        let failed = self.pilots.send.as_ref().is_some_and(|s| s.failed.is_some());
        match key.code {
            KeyCode::Char('r') if failed => self.pilots.retry_send(),
            KeyCode::Esc if failed => self.pilots.cancel_send(),
            _ => {}
        }
    }

    /// The keys of the active route view.
    pub fn on_active_key(&mut self, key: KeyEvent) {
        let Some(active) = &self.pilots.active else { return };
        let last = active.jumps();
        let arrived = active.arrived();
        let off_route = active.is_off_route();
        let expired = self.pilots.live.get(&active.character).is_some_and(|l| l.expired);
        let send_failed = self.pilots.send.as_ref().is_some_and(|s| s.failed.is_some());
        let stops = active.stops.clone();
        let current = self.detail.selected().unwrap_or(0);
        let page = self.detail_page.max(1);
        let target = match key.code {
            KeyCode::Char('x') => {
                self.popup = Some(Popup::StopRoute);
                return;
            }
            KeyCode::Char('q') => {
                self.popup = Some(Popup::Quit);
                return;
            }
            KeyCode::Char('c') => {
                self.open_characters();
                return;
            }
            KeyCode::Char('s') => {
                self.settings_page = Some(ListState::default().with_selected(Some(0)));
                return;
            }
            KeyCode::Char('r') if send_failed => {
                self.pilots.retry_send();
                return;
            }
            KeyCode::Char('r') if off_route => {
                self.popup = self.reroute();
                return;
            }
            KeyCode::Char('l') if expired => {
                self.popup = self.begin_login();
                return;
            }
            KeyCode::Enter if arrived => {
                self.pilots.stop_route();
                self.status = "Route done.".into();
                return;
            }
            // Esc does not quit while a route is active.
            KeyCode::Esc => return,
            code if route_locked(code) => {
                self.status = LOCKED.into();
                return;
            }
            KeyCode::Up => current.saturating_sub(1),
            KeyCode::Down => current + 1,
            KeyCode::PageUp => current.saturating_sub(page),
            KeyCode::PageDown => current + page,
            KeyCode::Home => 0,
            KeyCode::End => last,
            KeyCode::Char('n') => stops.iter().copied().find(|&s| s > current).unwrap_or(current),
            KeyCode::Char('p') => stops.iter().rev().copied().find(|&s| s < current).unwrap_or(current),
            _ => return,
        };
        self.detail.select(Some(target.min(last)));
    }

    /// "Re-route from here": the route from the current system through the stops ahead.
    fn reroute(&mut self) -> Option<Popup> {
        let active = self.pilots.active.as_ref()?;
        let Some(system) = self.pilots.live.get(&active.character).and_then(|l| l.system) else {
            self.status = "The location of the pilot is not known yet".into();
            return None;
        };
        let stops: Vec<u32> = active.stops.iter().filter(|&&i| i > active.progress).map(|&i| active.steps[i].system).collect();
        let (number, id, name) = (active.number, active.character, active.character_name.clone());
        match self.route_from(system, &stops) {
            Ok(route) => {
                let now = router_core::wormhole::now();
                Some(Popup::Reroute(Box::new(ActiveRoute::new(self.uni, &self.settings.rules, &route, number, id, &name, now))))
            }
            Err(e) => {
                self.status = e;
                None
            }
        }
    }

    pub fn on_pilot_popup_key(&mut self, popup: Popup, key: KeyEvent) -> Option<Popup> {
        let yes = matches!(key.code, KeyCode::Char('y') | KeyCode::Enter);
        let no = matches!(key.code, KeyCode::Char('n') | KeyCode::Esc);
        match popup {
            Popup::Characters(mut state) => {
                let rows = self.pilots.characters();
                let row = state.selected().and_then(|i| rows.get(i));
                match key.code {
                    KeyCode::Esc | KeyCode::Char('c') => None,
                    KeyCode::Up => {
                        state.select_previous();
                        Some(Popup::Characters(state))
                    }
                    KeyCode::Down => {
                        if state.selected().is_some_and(|i| i + 1 < rows.len()) {
                            state.select_next();
                        }
                        Some(Popup::Characters(state))
                    }
                    KeyCode::Char('a') => self.begin_login(),
                    KeyCode::Enter if row.is_some_and(|r| r.live.expired || r.needs_reauth) => self.begin_login(),
                    KeyCode::Char('d') | KeyCode::Delete => match row {
                        Some(r) => Some(Popup::RemovePilot(r.id)),
                        None => Some(Popup::Characters(state)),
                    },
                    _ => Some(Popup::Characters(state)),
                }
            }
            Popup::SessionOnly if yes => {
                self.pilots.use_session_only();
                Some(Popup::Characters(ListState::default().with_selected(Some(0))))
            }
            Popup::SessionOnly if no => None,
            Popup::Login { mut paste } => match key.code {
                KeyCode::Esc => {
                    self.pilots.cancel_login();
                    self.start_after_login = None;
                    None
                }
                KeyCode::Enter => {
                    self.pilots.paste(&paste);
                    Some(Popup::Login { paste })
                }
                KeyCode::Backspace => {
                    paste.pop();
                    Some(Popup::Login { paste })
                }
                KeyCode::Char(c) => {
                    paste.push(c);
                    Some(Popup::Login { paste })
                }
                _ => Some(Popup::Login { paste }),
            },
            Popup::Pick { route, ids, mut state } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Up => {
                    state.select_previous();
                    Some(Popup::Pick { route, ids, state })
                }
                KeyCode::Down => {
                    if state.selected().is_some_and(|i| i + 1 < ids.len()) {
                        state.select_next();
                    }
                    Some(Popup::Pick { route, ids, state })
                }
                KeyCode::Enter => match state.selected().and_then(|i| ids.get(i)) {
                    Some(&id) => self.confirm(route, id),
                    None => None,
                },
                _ => Some(Popup::Pick { route, ids, state }),
            },
            Popup::Confirm(confirm) => match key.code {
                KeyCode::Esc => None,
                // Enter takes the default: the route from the current system, if there is one.
                KeyCode::Enter => {
                    let Confirm { planned, from_here, .. } = *confirm;
                    self.pilots.start_route(from_here.unwrap_or(planned));
                    None
                }
                KeyCode::Char('a') if confirm.from_here.is_some() => {
                    self.pilots.start_route(confirm.planned);
                    None
                }
                _ => Some(Popup::Confirm(confirm)),
            },
            Popup::RemovePilot(id) if yes => {
                self.pilots.remove(id);
                Some(Popup::Characters(ListState::default().with_selected(Some(0))))
            }
            Popup::RemovePilot(_) if no => Some(Popup::Characters(ListState::default().with_selected(Some(0)))),
            Popup::StopRoute if yes => {
                self.pilots.stop_route();
                self.status = "Route stopped. The in-game waypoints stay.".into();
                None
            }
            Popup::Quit if yes => {
                self.quit = true;
                None
            }
            Popup::StopRoute | Popup::Quit if no => None,
            Popup::Resume if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) => {
                self.pilots.resume_route();
                None
            }
            Popup::Resume if matches!(key.code, KeyCode::Char('n') | KeyCode::Char('d')) => {
                self.pilots.discard_resume();
                None
            }
            Popup::Reroute(route) if yes => {
                self.pilots.replace_route(*route);
                None
            }
            Popup::Reroute(_) if no => None,
            other => Some(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::tests::app;
    use ratatui::crossterm::event::KeyModifiers;
    use router_core::config::Config;
    use router_core::esi::active::{Hop, Step};

    fn active(systems: &[u32]) -> ActiveRoute {
        let steps = systems
            .iter()
            .enumerate()
            .map(|(i, &system)| Step { system, hop: if i == 0 { Hop::Start } else { Hop::Gate }, via: String::new() })
            .collect();
        ActiveRoute::from_steps(steps, vec![0, systems.len() - 1], 1, 7, "Alice")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn route_keys_are_locked_while_a_route_is_active() {
        let mut app = app("lock", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        let (mode, wormholes, bridges, top, hull) =
            (app.settings.mode, app.settings.wormholes, app.settings.bridges, app.settings.top, app.settings.rules.hull);
        app.pilots.active = Some(active(&[30000142, 30002187]));
        for code in ['i', '/', 'm', 'w', 'j', 'h', '+', '=', '-'].map(KeyCode::Char).into_iter().chain([KeyCode::Enter]) {
            press(&mut app, code);
            assert!(app.popup.is_none(), "{code:?} opened a popup");
            assert_eq!(app.status, LOCKED, "{code:?}");
            app.status.clear();
        }
        assert_eq!(app.settings.mode, mode);
        assert_eq!(app.settings.wormholes, wormholes);
        assert_eq!(app.settings.bridges, bridges);
        assert_eq!(app.settings.top, top);
        assert!(app.settings.rules.hull.is_none() && hull.is_none());
        assert_eq!(app.input, "Jita > Amarr");
    }

    #[test]
    fn esc_does_not_quit_and_q_asks() {
        let mut app = app("quit", Config::default());
        app.pilots.active = Some(active(&[30000142, 30002187]));
        press(&mut app, KeyCode::Esc);
        assert!(!app.quit && app.popup.is_none());
        press(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.popup, Some(Popup::Quit)));
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.quit && app.popup.is_none());
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.quit);
    }

    #[test]
    fn routing_settings_are_locked_and_favourites_are_not() {
        let mut app = app("lock-settings", Config::default());
        app.pilots.active = Some(active(&[30000142, 30002187]));
        press(&mut app, KeyCode::Char('s'));
        assert!(app.settings_page.is_some());
        // Row 0 is the alliance capital.
        press(&mut app, KeyCode::Enter);
        assert!(app.popup.is_none());
        assert_eq!(app.status, LOCKED);
        // `a` adds a favourite.
        press(&mut app, KeyCode::Char('a'));
        assert!(matches!(app.popup, Some(Popup::Prompt { .. })));
    }

    #[test]
    fn stop_asks_first() {
        let mut app = app("stop", Config::default());
        app.pilots.active = Some(active(&[30000142, 30002187]));
        press(&mut app, KeyCode::Char('x'));
        assert!(matches!(app.popup, Some(Popup::StopRoute)));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.pilots.active.is_none());
        assert_eq!(app.status, "Route stopped. The in-game waypoints stay.");
    }

    #[test]
    fn start_without_client_id_gives_the_reason() {
        let mut app = app("start", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        app.focus = crate::tui::app::Focus::Routes;
        press(&mut app, KeyCode::Char('g'));
        let Some(Popup::Message(text)) = &app.popup else { panic!("no message") };
        assert!(text.contains("EVE_ROUTER_CLIENT_ID"), "{text}");
    }

    #[test]
    fn arrived_route_is_done_with_enter() {
        let mut app = app("arrived", Config::default());
        let mut route = active(&[30000142, 30002187]);
        route.progress = 1;
        app.pilots.active = Some(route);
        press(&mut app, KeyCode::Enter);
        assert!(app.pilots.active.is_none());
    }
}
