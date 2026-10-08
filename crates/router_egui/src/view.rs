//! The layout of the window. It copies the regions of the TUI: the route input (here the route
//! planner), the route list, the route table, the sidebar and the status line.

use crate::app::Session;
use crate::pilots_view::{PilotsUi, avatar_row};
use crate::search::{Pick, SearchBox};
use crate::settings_window::{self, Popup, SettingsForm};
use crate::theme::{self, panel};
use egui::{Align, Button, Color32, Frame, Key, Label, Layout, Margin, Modifiers, RichText, ScrollArea, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use petgraph::graph::NodeIndex;
use router_core::esi::pilots::pilots_by_step;
use router_core::labels::{jumps_label, link_label, on_off, pilot_label, route_extras, route_text};
use router_core::route::{Mode, Stop};
use router_core::sources::FetchError;
use router_core::sources::nexum::MapInfo;
use router_core::universe::display_sec;
use router_core::wormhole::{SourceId, age_text};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// How long the status bar shows a status text.
const STATUS_TIME: Duration = Duration::from_secs(4);

/// The age of the status text. The status bar clears a text after `STATUS_TIME`.
#[derive(Default)]
struct StatusTimer {
    /// The text, and the time when it first showed.
    shown: Option<(String, Instant)>,
}

impl StatusTimer {
    /// Look at the current text at `now`. Return `None` if the text must clear now, else the
    /// time until it must clear. An empty text, or a new text, gives a full `STATUS_TIME`.
    fn update(&mut self, text: &str, now: Instant) -> Option<Duration> {
        if text.is_empty() {
            self.shown = None;
            return Some(STATUS_TIME);
        }
        match &self.shown {
            Some((shown, at)) if shown == text => {
                let left = STATUS_TIME.saturating_sub(now - *at);
                if left.is_zero() {
                    self.shown = None;
                    return None;
                }
                Some(left)
            }
            _ => {
                self.shown = Some((text.to_string(), now));
                Some(STATUS_TIME)
            }
        }
    }
}

/// The UI state that is not part of the route: the search, the popups and the settings window.
pub struct View {
    search: SearchBox,
    pub popup: Option<Popup>,
    pub settings: Option<SettingsForm>,
    /// True while the Log window is open.
    log_open: bool,
    /// The Nexum map list fetch, while it runs.
    pub maps: Option<Receiver<Result<Vec<MapInfo>, FetchError>>>,
    status_timer: StatusTimer,
    /// The login, the route start, the active route and the avatars.
    pilots: PilotsUi,
}

/// A change of the avoid list, from the context menu of a route step.
enum Toggle {
    System(NodeIndex),
    Region(String),
}

/// A change to the waypoints, from a click in the route planner.
enum Edit {
    SetStart(usize),
    MoveUp(usize),
    MoveDown(usize),
    Remove(usize),
}

impl View {
    pub fn new(_session: &Session) -> Self {
        View {
            search: SearchBox::new("system-search").with_lists(),
            popup: None,
            settings: None,
            log_open: false,
            maps: None,
            status_timer: StatusTimer::default(),
            pilots: PilotsUi::default(),
        }
    }

    pub fn show(&mut self, ui: &mut Ui, s: &mut Session) {
        self.poll_maps(s);
        self.pilots.update(ui.ctx(), s);
        // A dragged value in the settings waits for a quiet time, then the app searches and saves.
        if let Some(left) = s.apply_due(Instant::now()) {
            ui.ctx().request_repaint_after(left);
        }
        // The status text and the startup lines clear after `STATUS_TIME`.
        let text = std::iter::once(&s.status).chain(&s.startup_lines).filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
        match self.status_timer.update(&text, Instant::now()) {
            None => {
                s.status.clear();
                s.startup_lines.clear();
            }
            Some(_) if text.is_empty() => {}
            Some(left) => ui.ctx().request_repaint_after(left),
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) {
            s.refresh_now(Instant::now());
        }
        let modal_open = self.popup.is_some() || self.settings.is_some();
        if !modal_open {
            if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
                self.search.focus(ui);
            }
            // The arrow keys select a route while no text field has the focus. While a route is
            // active, the route list is hidden.
            if ui.memory(|m| m.focused().is_none()) && !s.routes.is_empty() && s.pilots.active.is_none() {
                let (up, down) = ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
                if up && s.selected > 0 {
                    s.selected -= 1;
                    s.selected_step = None;
                }
                if down && s.selected + 1 < s.routes.len() {
                    s.selected += 1;
                    s.selected_step = None;
                }
            }
        }

        let bar = Frame::new().fill(theme::HEADER).inner_margin(Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, theme::LINE));
        egui::Panel::top("top-bar").frame(bar).show(ui, |ui| self.top_bar(ui, s));
        egui::Panel::bottom("status-bar").frame(bar).show(ui, |ui| status_bar(ui, s, &mut self.log_open));
        let side = Frame::new().fill(theme::BG).inner_margin(Margin { left: 0, right: 10, top: 10, bottom: 10 });
        egui::Panel::right("sidebar").resizable(false).exact_size(280.0).frame(side).show(ui, |ui| sidebar(ui, s, &mut self.pilots));
        let central = Frame::new().fill(theme::BG).inner_margin(Margin::same(10));
        egui::CentralPanel::default().frame(central).show(ui, |ui| {
            // The active route hides the planner and the route list.
            if s.pilots.active.is_some() {
                self.pilots.active_view(ui, s);
                return;
            }
            self.planner(ui, s);
            ui.add_space(8.0);
            let routes_height = (ui.available_height() * 0.3).clamp(90.0, 220.0);
            ui.allocate_ui(vec2(ui.available_width(), routes_height), |ui| route_list(ui, s));
            ui.add_space(8.0);
            route_table(ui, s, &mut self.pilots);
        });

        settings_window::show(ui, self, s);
        self.pilots.windows(ui, s);
        crate::log_window::show(ui.ctx(), &mut self.log_open, s);
    }

    /// Take the result of the Nexum map list fetch, if the thread sent it.
    fn poll_maps(&mut self, s: &mut Session) {
        let Some(rx) = &self.maps else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            // The thread stopped with no result: it panicked.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.maps = None;
                self.popup = Some(Popup::Message("The map list fetch stopped with an internal error".into()));
                return;
            }
        };
        self.maps = None;
        self.popup = Some(match result {
            Ok(maps) if maps.is_empty() => Popup::Message("The key has access to no maps".into()),
            Ok(maps) => {
                s.map_names = maps.clone();
                Popup::Maps(maps)
            }
            Err(FetchError::Offline(detail)) => Popup::Message(format!("Nexum offline: {detail}")),
            Err(e) => Popup::Message(e.status(SourceId::Nexum, None)),
        });
    }

    fn top_bar(&mut self, ui: &mut Ui, s: &mut Session) {
        ui.horizontal(|ui| {
            let title = RichText::new("EVE ROUTER").family(theme::bold()).size(16.0).color(theme::ACCENT).extra_letter_spacing(3.0);
            ui.label(title);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("⚙ Settings").clicked() {
                    self.settings = Some(SettingsForm::new(s));
                }
                let avoid = match s.settings.avoid.len() {
                    0 => "Avoid".to_string(),
                    n => format!("Avoid ({n})"),
                };
                let locked = s.pilots.active.is_some();
                let button = ui.add_enabled(!locked, Button::new(avoid)).on_hover_text("Systems and regions that routes avoid");
                if button.clicked() {
                    self.popup = Some(Popup::Avoid { confirm: false, search: SearchBox::new("avoid-search"), region: String::new() });
                }
                button.on_disabled_hover_text(settings_window::LOCKED);
                self.pilots.characters_button(ui, s);
                ui.separator();
                // While a route is active, the controls that change the route are off.
                let locked = s.pilots.active.is_some();
                if locked {
                    ui.disable();
                }

                // The number of routes.
                if ui.add_enabled(true, Button::new("+")).clicked() {
                    s.settings.top += 1;
                    s.recompute();
                    s.save_quietly();
                }
                ui.label(RichText::new(s.settings.top.to_string()).color(Color32::WHITE));
                if ui.add_enabled(s.settings.top > 1, Button::new("−")).clicked() {
                    s.settings.top -= 1;
                    s.recompute();
                    s.save_quietly();
                }
                ui.label(theme::header_text("Routes"));
                ui.separator();

                let blocked = s.settings.rules.blocked_reason();
                let bridges = match &blocked {
                    Some(_) => "off (no capital)",
                    None => on_off(s.settings.bridges),
                };
                let response = ui.add(Button::selectable(s.settings.bridges && blocked.is_none(), format!("Jump bridges: {bridges}")));
                if response.clicked() {
                    s.settings.bridges = !s.settings.bridges;
                    s.recompute();
                }
                if let Some(reason) = blocked {
                    response.on_hover_text(reason);
                }
                if ui.add(Button::selectable(s.settings.wormholes, format!("Wormholes: {}", on_off(s.settings.wormholes)))).clicked() {
                    s.settings.wormholes = !s.settings.wormholes;
                    s.recompute();
                }
                ui.separator();

                if ui
                    .checkbox(&mut s.settings.optimize, "Optimize order")
                    .on_hover_text("Visit each system once, in the cheapest order")
                    .changed()
                {
                    s.recompute();
                    s.save_quietly();
                }
                let mut mode = s.settings.mode;
                egui::ComboBox::from_id_salt("mode").selected_text(mode.title()).width(150.0).show_ui(ui, |ui| {
                    for m in Mode::ALL {
                        ui.selectable_value(&mut mode, m, m.title()).on_hover_text(m.description());
                    }
                });
                if mode != s.settings.mode {
                    s.settings.mode = mode;
                    s.recompute();
                    s.save_quietly();
                }
                ui.label(theme::header_text("Mode"));
            });
        });
    }

    /// The search box and the waypoint list. They take the place of the route input of the TUI.
    fn planner(&mut self, ui: &mut Ui, s: &mut Session) {
        let info = match s.waypoints.len() {
            0 => String::new(),
            1 => "1 waypoint".into(),
            n => format!("{n} waypoints"),
        };
        panel(ui, "Route planner", &info, false, |ui| {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 260.0).max(200.0);
                let menu = [Pick::AddWaypoint, Pick::SetStart, Pick::AddFavourite];
                let picked = self.search.show(ui, &s.uni, width, "Search system…  (Ctrl+F)", &menu);
                if let Some((pick, node)) = picked {
                    apply_pick(s, pick, node);
                }
                if let Some(list) = self.search.take_list() {
                    s.add_list(&list);
                }
                if ui.button("+ Add waypoint").clicked() {
                    match self.search.take_highlighted() {
                        Some(node) => s.add_waypoint(node),
                        None => self.search.focus(ui),
                    }
                }
                let hint = "Add many systems at once: one name for each line, or names separated by commas";
                if ui.button("Paste list…").on_hover_text(hint).clicked() {
                    self.popup = Some(Popup::List { text: String::new(), problems: Vec::new() });
                }
            });
            ui.horizontal(|ui| {
                ui.label(theme::header_text("Pilot"));
                let hint = "Plan for the ship of a character, or for a hull that you pick";
                if ui.button(pilot_label(&s.settings, &s.pilots)).on_hover_text(hint).clicked() {
                    self.popup = Some(Popup::Pilot { filter: String::new() });
                }
            });
            ui.add_space(6.0);
            if s.waypoints.is_empty() {
                ui.label(
                    RichText::new(
                        "Search a system and push Enter to add it as a waypoint. To add many systems, paste a list into the search box.",
                    )
                    .color(theme::TEXT_DIM),
                );
                return;
            }
            let mut edit = None;
            ScrollArea::vertical().max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("waypoints").num_columns(6).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
                    let last = s.waypoints.len() - 1;
                    for (i, &node) in s.waypoints.iter().enumerate() {
                        let sys = s.uni.system(node);
                        let stop = s.stop(i);
                        let color = if matches!(stop, Stop::Midpoint(_)) { theme::TEXT_DIM } else { theme::ACCENT };
                        ui.label(RichText::new(format!("{:>2}", i + 1)).color(theme::TEXT_DIM));
                        ui.label(RichText::new(stop.label().to_uppercase()).color(color).size(11.0).extra_letter_spacing(1.0));
                        let name =
                            ui.add(Label::new(RichText::new(&sys.name).family(theme::bold()).color(Color32::WHITE)).sense(Sense::click()));
                        ui.label(RichText::new(format!("{:.1}", display_sec(sys.security))).color(theme::sec_color(sys.security)));
                        ui.label(RichText::new(&sys.region).color(theme::TEXT_DIM));
                        ui.horizontal(|ui| {
                            if ui.add_enabled(i > 0, Button::new("⏶").small()).on_hover_text("Move up").clicked() {
                                edit = Some(Edit::MoveUp(i));
                            }
                            if ui.add_enabled(i < last, Button::new("⏷").small()).on_hover_text("Move down").clicked() {
                                edit = Some(Edit::MoveDown(i));
                            }
                            if ui.add(Button::new("🗙").small()).on_hover_text("Remove").clicked() {
                                edit = Some(Edit::Remove(i));
                            }
                        });
                        name.context_menu(|ui| {
                            for (label, action, enabled) in [
                                ("Set as start", Edit::SetStart(i), i > 0),
                                ("Move up", Edit::MoveUp(i), i > 0),
                                ("Move down", Edit::MoveDown(i), i < last),
                                ("Remove", Edit::Remove(i), true),
                            ] {
                                if ui.add_enabled(enabled, Button::new(label)).clicked() {
                                    edit = Some(action);
                                    ui.close();
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(s.waypoints.len() > 1, Button::new("Reverse")).clicked() {
                    s.reverse();
                }
                if ui.button("Clear route").clicked() {
                    s.clear();
                }
            });
            match edit {
                Some(Edit::SetStart(i)) => s.set_start(s.waypoints[i]),
                Some(Edit::MoveUp(i)) => s.move_up(i),
                Some(Edit::MoveDown(i)) => s.move_down(i),
                Some(Edit::Remove(i)) => s.remove_waypoint(i),
                None => {}
            }
        });
    }
}

/// Do what a pick in the main search box asks.
fn apply_pick(s: &mut Session, pick: Pick, node: NodeIndex) {
    match pick {
        Pick::Default | Pick::AddWaypoint => s.add_waypoint(node),
        Pick::SetStart => s.set_start(node),
        Pick::AddFavourite => {
            s.add_favourite(node);
        }
    }
}

fn route_list(ui: &mut Ui, s: &mut Session) {
    let title = format!("Routes ({})", s.routes.len());
    // The time of the last search, with the favourite search.
    let time = match s.route_time {
        Some(t) if !s.routes.is_empty() => format!("{:.1} ms", t.as_secs_f64() * 1000.0),
        _ => String::new(),
    };
    panel(ui, &title, &time, true, |ui| {
        if s.routes.is_empty() {
            let text = if s.waypoints.len() < 2 { "Add two or more waypoints to find a route." } else { "No route." };
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
            return;
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (i, route) in s.routes.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                let selected = i == s.selected;
                if selected {
                    ui.painter().rect_filled(rect, 0.0, theme::ROW_FILL);
                    theme::selection_bar(ui, rect);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 0.0, theme::ACCENT_DIM);
                }
                let color = if selected { Color32::WHITE } else { theme::TEXT };
                let text = format!("#{}  {}{}", i + 1, jumps_label(route.jumps), route_extras(route));
                let font = egui::TextStyle::Body.resolve(ui.style());
                ui.painter().text(rect.left_center() + vec2(12.0, 0.0), egui::Align2::LEFT_CENTER, text, font, color);
                if response.clicked() {
                    s.selected = i;
                    s.selected_step = None;
                }
            }
        });
    });
}

fn route_table(ui: &mut Ui, s: &mut Session, pilots: &mut PilotsUi) {
    let Some(route) = s.selected_route() else {
        panel(ui, "Route", "", true, |_| {});
        return;
    };
    let title = format!("Route #{}", s.selected + 1);
    let info = format!("{}{}", jumps_label(route.jumps), route_extras(route));
    let mut clicked = None;
    let mut toggle = None;
    let mut copy = false;
    let characters = s.pilots.characters();
    // The Pilots column shows only when a character is logged in.
    let show_pilots = !characters.is_empty();
    let mut start = false;
    let header = |ui: &mut Ui| {
        let has_pilot = characters.iter().any(|c| !c.live.expired);
        let text = if has_pilot { format!("Start route #{}", s.selected + 1) } else { "Log in to start".into() };
        let button = Button::new(RichText::new(text).size(11.0).color(Color32::WHITE)).fill(theme::ACCENT.gamma_multiply(0.35)).small();
        start = ui.add(button).on_hover_text("Send the waypoints of this route to a character in the game").clicked();
        ui.add_space(6.0);
        let button = Button::new(RichText::new("Copy route").size(11.0)).small();
        copy = ui.add(button).on_hover_text("Copy the route as text, in the format of --print").clicked();
        ui.add_space(6.0);
        ui.label(RichText::new(&info).color(theme::TEXT_DIM).size(11.0));
    };
    theme::panel_with(ui, &title, header, true, |ui| {
        let uni = &s.uni;
        let header = |ui: &mut Ui, text: &str| _ = ui.label(theme::header_text(text));
        let mut table = TableBuilder::new(ui)
            .id_salt("route-table")
            .striped(false)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(36.0))
            .column(Column::exact(110.0))
            .column(Column::initial(150.0).at_least(90.0).resizable(true));
        if show_pilots {
            table = table.column(Column::initial(190.0).at_least(70.0).resizable(true).clip(true));
        }
        table
            .column(Column::exact(72.0))
            .column(Column::initial(170.0).at_least(90.0).resizable(true))
            .column(Column::remainder().at_least(120.0))
            .auto_shrink(false)
            .header(20.0, |mut row| {
                let pilots_header = show_pilots.then_some("Pilots");
                for text in ["#", "Stop", "System"].into_iter().chain(pilots_header).chain(["Security", "Region", "Via"]) {
                    row.col(|ui| header(ui, text));
                }
            })
            .body(|body| {
                let systems: Vec<u32> = route.path.nodes.iter().map(|&n| uni.system(n).id).collect();
                let by_step = pilots_by_step(&systems, &characters, None);
                body.rows(22.0, route.path.nodes.len(), |mut row| {
                    let step = row.index();
                    let sys = uni.system(route.path.nodes[step]);
                    let via = match step.checked_sub(1).map(|i| route.path.edges[i]) {
                        // Oxanium and the egui fallback fonts have no right arrow glyph. Oxanium has "»".
                        Some(e) => link_label(uni, &s.settings.rules, e, s.now).replace('\u{2192}', "»"),
                        None => String::new(),
                    };
                    let via_color = match via.as_str() {
                        v if v.starts_with("Wormhole") => theme::WORMHOLE,
                        v if v.starts_with("Ansiblex") => theme::BRIDGE,
                        _ => theme::TEXT_DIM,
                    };
                    let stop = route.stop_at(step);
                    row.set_selected(s.selected_step == Some(step));
                    // The start, the midpoints and the destination stand out from the other steps.
                    let name = match stop {
                        Some(_) => RichText::new(&sys.name).family(theme::bold()).color(Color32::WHITE),
                        None => RichText::new(&sys.name).color(theme::TEXT),
                    };
                    row.col(|ui| _ = ui.label(RichText::new(step.to_string()).color(theme::TEXT_DIM)));
                    row.col(|ui| {
                        if let Some(stop) = stop {
                            ui.label(RichText::new(stop.label().to_uppercase()).color(theme::ACCENT).size(11.0).extra_letter_spacing(1.0));
                        }
                    });
                    row.col(|ui| _ = ui.add(Label::new(name).truncate()));
                    if show_pilots {
                        row.col(|ui| avatar_row(ui, &mut pilots.portraits, &by_step[step]));
                    }
                    row.col(|ui| {
                        _ = ui.label(RichText::new(format!("{:.1}", display_sec(sys.security))).color(theme::sec_color(sys.security)))
                    });
                    row.col(|ui| _ = ui.add(Label::new(RichText::new(&sys.region).color(theme::TEXT_DIM)).truncate()));
                    row.col(|ui| _ = ui.add(Label::new(RichText::new(&via).color(via_color)).truncate()).on_hover_text(&via));
                    let node = route.path.nodes[step];
                    row.response().context_menu(|ui| {
                        let avoid = &s.settings.avoid;
                        let system =
                            if avoid.has_system(node) { format!("Stop avoiding {}", sys.name) } else { format!("Avoid {}", sys.name) };
                        if ui.button(system).clicked() {
                            toggle = Some(Toggle::System(node));
                            ui.close();
                        }
                        let region = if avoid.has_region(&sys.region) {
                            format!("Stop avoiding {}", sys.region)
                        } else {
                            format!("Avoid {}", sys.region)
                        };
                        if ui.button(region).clicked() {
                            toggle = Some(Toggle::Region(sys.region.clone()));
                            ui.close();
                        }
                    });
                    if row.response().clicked() {
                        clicked = Some(step);
                    }
                });
            });
    });
    match toggle {
        Some(Toggle::System(node)) => s.toggle_avoid_system(node),
        Some(Toggle::Region(region)) => s.toggle_avoid_region(&region),
        None => {}
    }
    if clicked.is_some() {
        s.selected_step = clicked;
    }
    if start {
        pilots.begin_start(s, s.selected);
    }
    if copy && let Some(route) = s.selected_route() {
        ui.ctx().copy_text(route_text(&s.uni, &s.settings.rules, s.selected, route, s.now));
        s.status = format!("Route #{} copied to the clipboard", s.selected + 1);
    }
}

fn sidebar(ui: &mut Ui, s: &mut Session, pilots: &mut PilotsUi) {
    // The "Shortcuts" box shows only when an overlay loaded a connection.
    let sc = &s.shortcuts;
    if sc.wormholes + sc.bridges > 0 {
        let settings = &s.settings;
        let bridges_on = settings.bridges && settings.rules.blocked_reason().is_none();
        panel(ui, "Shortcuts", "", false, |ui| {
            egui::Grid::new("shortcuts").num_columns(2).spacing(vec2(8.0, 2.0)).min_col_width(60.0).show(ui, |ui| {
                // A kind that is off for routing shows in gray.
                let row = |ui: &mut Ui, label: &str, count: usize, on: bool| {
                    let color = if on { theme::TEXT } else { theme::TEXT_DIM };
                    ui.label(RichText::new(label).color(color));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(count.to_string()).color(color)));
                    ui.end_row();
                };
                row(ui, "Wormholes", sc.wormholes, settings.wormholes);
                for (label, count, on) in [("  Thera", sc.thera, settings.hubs.thera), ("  Turnur", sc.turnur, settings.hubs.turnur)] {
                    if count > 0 {
                        row(ui, label, count, settings.wormholes && on);
                    }
                }
                row(ui, "Jump bridges", sc.bridges, bridges_on);
                if sc.skipped > 0 {
                    row(ui, "Skipped", sc.skipped, false);
                }
            });
        });
        ui.add_space(8.0);
    }

    let info = s.waypoints.first().map_or(String::new(), |&n| format!("from {}", s.uni.name(n)));
    let mut add = None;
    // With pilots, the Pilots panel goes below, so this panel does not fill the sidebar.
    let fill = s.pilots.characters().is_empty();
    panel(ui, "Shortest route", &info, fill, |ui| {
        if s.settings.favourites.is_empty() {
            ui.label(RichText::new("No favourites. Add one in the settings, or right-click a search result.").color(theme::TEXT_DIM));
            return;
        }
        if s.waypoints.is_empty() {
            ui.label(RichText::new("Add a waypoint to see the jumps to each favourite.").color(theme::TEXT_DIM));
            ui.add_space(6.0);
        }
        for &fav in &s.settings.favourites {
            let jumps = s.hubs.iter().find(|(n, _)| *n == fav).map(|&(_, j)| j);
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(rect, 0.0, theme::ACCENT_DIM);
                theme::selection_bar(ui, rect);
            }
            let font = egui::TextStyle::Body.resolve(ui.style());
            let sys = s.uni.system(fav);
            let p = ui.painter();
            p.text(rect.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, &sys.name, font.clone(), theme::TEXT);
            let jumps = match jumps {
                Some(Some(j)) => j.to_string(),
                Some(None) => "-".into(),
                None => String::new(),
            };
            p.text(rect.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, jumps, font, theme::ACCENT);
            if response.on_hover_text("Click to add as a waypoint").clicked() {
                add = Some(fav);
            }
        }
    });
    if let Some(node) = add {
        s.add_waypoint(node);
    }
    if !fill {
        ui.add_space(8.0);
        pilots.sidebar_panel(ui, s);
    }
}

fn status_bar(ui: &mut Ui, s: &mut Session, log_open: &mut bool) {
    // The sync status goes first, at the right. The status text gets the space that is left,
    // and a text that is too long ends in "…". The full text shows on hover.
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // The Log button shows in the warning color while the newest row is a failure.
        let failed = s.log.last().is_some_and(|e| !e.ok);
        let log_text = RichText::new("Log").small().color(if failed { theme::WARN } else { theme::TEXT });
        if ui.add(Button::new(log_text).small()).on_hover_text("Show the fetch log in a window").clicked() {
            *log_open = true;
        }
        if ui.add(Button::new(RichText::new("Refresh").small()).small()).on_hover_text("Fetch the wormholes now (F5)").clicked() {
            s.refresh_now(Instant::now());
        }
        ui.add_space(6.0);
        sync_status(ui, s);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            // The refresh note stays: the status timer clears only the status and the startup lines.
            let parts: Vec<&str> = std::iter::once(&s.status)
                .chain(&s.startup_lines)
                .chain(std::iter::once(&s.note))
                .map(String::as_str)
                .filter(|t| !t.is_empty())
                .collect();
            let (text, color) = match parts.is_empty() {
                true => ("Ready".to_string(), theme::TEXT_DIM),
                false => (parts.join(" · "), theme::WARN),
            };
            ui.add(Label::new(RichText::new(&text).color(color)).truncate()).on_hover_text(&text);
        });
    });
}

/// The wormhole sources and the age of their data, for example "Nexum 2 min ago" after a dot.
/// The dot is green for data from the last 15 minutes, else amber. A source with no data is gray.
fn sync_status(ui: &mut Ui, s: &Session) {
    const FRESH_SECS: u64 = 15 * 60;
    let now = router_core::wormhole::now();
    // The ages change with time, so the bar draws again each minute.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(60));
    // The layout is right to left: EVE-Scout goes first, so Nexum shows on its left.
    for source in [SourceId::EveScout, SourceId::Nexum] {
        let fetched = s.shortcuts.sources.iter().find(|(id, _)| *id == source).map(|&(_, at)| at);
        let (dot, text) = match fetched {
            Some(at) if now.saturating_sub(at) <= FRESH_SECS => (theme::OK, age_text(at, now)),
            Some(at) => (theme::WARN, age_text(at, now)),
            None if source == SourceId::Nexum && s.cfg.nexum.complete().is_none() => (theme::TEXT_DIM, "not set".into()),
            None => (theme::TEXT_DIM, "no data".into()),
        };
        ui.label(RichText::new(text).color(theme::TEXT_DIM).small());
        ui.label(RichText::new(source.label()).color(theme::TEXT).small());
        // A painted dot: the fonts have no circle glyph.
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, dot);
        ui.add_space(10.0);
    }
}

/// The loading screen, or the load error.
pub fn splash(ui: &mut Ui, error: Option<&str>) {
    let frame = Frame::new().fill(theme::BG);
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 80.0).max(0.0));
            ui.allocate_ui(vec2(420.0, 160.0), |ui| {
                panel(ui, "EVE Router", "", false, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        match error {
                            None => {
                                ui.add(egui::Spinner::new().size(28.0).color(theme::ACCENT));
                                ui.add_space(8.0);
                                ui.label(RichText::new("Loading the map and the wormhole data…").color(theme::TEXT));
                            }
                            Some(e) => {
                                ui.label(RichText::new("The load failed").color(theme::ERROR).family(theme::bold()));
                                ui.add_space(4.0);
                                ui.label(RichText::new(e).color(theme::TEXT));
                            }
                        }
                        ui.add_space(8.0);
                    });
                });
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_clears_after_status_time() {
        let mut timer = StatusTimer::default();
        let start = Instant::now();
        assert_eq!(timer.update("Config saved!", start), Some(STATUS_TIME));
        assert_eq!(timer.update("Config saved!", start + Duration::from_secs(1)), Some(Duration::from_secs(3)));
        // A new text starts a new time.
        assert_eq!(timer.update("Added 2 systems", start + Duration::from_secs(3)), Some(STATUS_TIME));
        assert_eq!(timer.update("Added 2 systems", start + Duration::from_secs(6)), Some(Duration::from_secs(1)));
        assert_eq!(timer.update("Added 2 systems", start + Duration::from_secs(7)), None);
        // After the clear, the same text again shows for a full time.
        assert_eq!(timer.update("", start + Duration::from_secs(7)), Some(STATUS_TIME));
        assert_eq!(timer.update("Added 2 systems", start + Duration::from_secs(8)), Some(STATUS_TIME));
    }
}
