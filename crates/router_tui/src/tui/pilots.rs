//! The keys of the EVE login, the route start and the active route.

use super::app::{App, Popup, SettingsRow};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;
use router_core::esi::pilots::{HullSync, Pilots, StartPlan};
use router_core::settings::HullSource;

/// The status text for a key that could change the route.
pub const LOCKED: &str = "Locked while route is active — press x to stop";

/// The status text after a ship change closes a route choice.
pub const ROUTES_CHANGED: &str = "The ship changed, so the routes changed. Start the route again.";

/// The status text after a wormhole refresh closes a route choice.
pub const WORMHOLES_CHANGED: &str = "The wormholes changed, so the routes changed. Start the route again.";

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

impl App {
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
        match self.pilots.sync_hull(&mut self.settings) {
            HullSync::None => {}
            HullSync::Source => self.save_quietly(),
            // The active route stays as it is. The pilot chooses when to re-route.
            HullSync::Hull if self.pilots.active.is_some() => {
                let kind = self.settings.rules.hull.map_or("an unknown ship".into(), |h| h.name.clone());
                self.status = format!("Ship changed to {kind}. The routes update when this route ends.");
                self.save_quietly();
            }
            HullSync::Hull => {
                let forgot = self.forget_route_choice();
                self.recompute();
                self.save_quietly();
                if forgot {
                    self.status = ROUTES_CHANGED.into();
                }
            }
        }
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

    /// Forget a choice that holds a route index: the Pick popup and the start after the login.
    /// After a recompute, the index can point to a different route. True if a choice went away.
    pub(super) fn forget_route_choice(&mut self) -> bool {
        let pick = matches!(self.popup, Some(Popup::Pick { .. }));
        if pick {
            self.popup = None;
        }
        self.start_after_login.take().is_some() || pick
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
        let ids: Vec<u64> = self.pilots.senders().into_iter().map(|c| c.id).collect();
        self.popup = match ids.len() {
            // A waypoint does nothing without the game client, so an offline pilot is not in the list.
            0 if self.pilots.characters().iter().any(|c| !c.live.expired) => {
                self.status = Pilots::NO_SENDER.into();
                None
            }
            0 => {
                // Start the route after the login only when a login starts.
                let popup = self.begin_login();
                if matches!(popup, Some(Popup::Login { .. })) {
                    self.start_after_login = Some(index);
                }
                popup
            }
            1 => self.confirm(index, ids[0]),
            _ => {
                let row = self.pilots.last_used().and_then(|id| ids.iter().position(|&i| i == id)).unwrap_or(0);
                Some(Popup::Pick { route: index, ids, state: ListState::default().with_selected(Some(row)) })
            }
        };
    }

    fn confirm(&mut self, index: usize, id: u64) -> Option<Popup> {
        let pilot = self.pilots.characters().into_iter().find(|c| c.id == id)?;
        let route = self.routes.get(index)?;
        let plan = StartPlan::new(&self.uni, &self.settings, route, index + 1, &pilot, router_core::wormhole::now());
        if let Some(error) = &plan.error {
            self.status.clone_from(error);
        }
        Some(Popup::Confirm(Box::new(plan)))
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

    /// "Re-route from here": the new route goes into a confirm popup.
    fn reroute(&mut self) -> Option<Popup> {
        match self.pilots.reroute(&self.uni, &self.settings, router_core::wormhole::now()) {
            Ok(route) => Some(Popup::Reroute(Box::new(route))),
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
                    let StartPlan { planned, from_here, .. } = *confirm;
                    self.pilots.start_route(from_here.unwrap_or(planned));
                    None
                }
                // The route list changes, so the pilot starts the route again from the new list.
                KeyCode::Char('p') if confirm.flown.is_some() => {
                    self.settings.hull_source = HullSource::Pilot(confirm.planned.character);
                    self.pilots.sync_hull(&mut self.settings);
                    self.recompute();
                    self.save_quietly();
                    let kind = confirm.flown.map_or_else(String::new, |h| h.name.clone());
                    self.status = format!("Routes planned for the {kind}. Start the route again.");
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
    use router_core::esi::active::{ActiveRoute, Hop, Step};

    fn active(systems: &[u32]) -> ActiveRoute {
        let steps = systems
            .iter()
            .enumerate()
            .map(|(i, &system)| Step { system, hop: if i == 0 { Hop::Start } else { Hop::Gate }, via: String::new(), sig: None })
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
    fn pick_with_a_stale_route_index_does_nothing() {
        let mut app = app("stale-pick", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        app.pilots.add_test_pilot(7, "Alice", 30000142);
        // Route 5 is not in the list: a recompute made the list shorter.
        app.popup = Some(Popup::Pick { route: 5, ids: vec![7], state: ListState::default().with_selected(Some(0)) });
        press(&mut app, KeyCode::Enter);
        assert!(app.popup.is_none());
    }

    #[test]
    fn a_failed_login_start_forgets_the_route() {
        let mut app = app("failed-login-start", Config::default());
        app.input = "Jita > Amarr".into();
        app.recompute();
        app.focus = crate::tui::app::Focus::Routes;
        press(&mut app, KeyCode::Char('g'));
        assert!(matches!(app.popup, Some(Popup::Message(_))));
        assert!(app.start_after_login.is_none());
    }

    #[test]
    fn a_route_change_forgets_the_pick() {
        let mut app = app("forget-pick", Config::default());
        app.popup = Some(Popup::Pick { route: 0, ids: vec![7], state: ListState::default() });
        app.start_after_login = Some(0);
        assert!(app.forget_route_choice());
        assert!(app.popup.is_none() && app.start_after_login.is_none());
        assert!(!app.forget_route_choice());
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
