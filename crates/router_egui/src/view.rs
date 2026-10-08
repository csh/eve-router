//! The layout of the window. It copies the regions of the TUI: the route input (here the route
//! planner), the route list, the route table, the sidebar and the status line.

use crate::app::Session;
use crate::pilots_view::PilotsUi;
use crate::search::{Pick, SearchBox};
use crate::settings_window::{self, Popup, SettingsForm};
use crate::strip;
use crate::theme::{self, panel};
use egui::{Align, Button, Color32, Frame, Key, Label, Layout, Margin, Modifiers, RichText, ScrollArea, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use petgraph::graph::NodeIndex;
use router_core::labels::{jumps_label, link_label, on_off, pilot_label, route_summary, route_text};
use router_core::route::{Mode, Stop};
use router_core::sources::FetchError;
use router_core::sources::nexum::MapInfo;
use router_core::universe::display_sec;
use router_core::wormhole::{SourceId, age_text};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// The height of a row of the route list: the summary line and the strip, and the space below.
const ROUTE_ROW: f32 = 44.0;
const ROUTE_PITCH: f32 = ROUTE_ROW + 4.0;
/// The height that a panel adds around its content: the header strip, the margins and the lines.
const PANEL_CHROME: f32 = 46.0;
/// The most rows that the route list shows before it scrolls.
const LIST_ROWS: usize = 3;
/// The least height that the route table keeps, in pixels. The planner and the list give way.
const TABLE_MIN: f32 = 200.0;
/// The height of the planner with the waypoint list closed.
const PLANNER_CLOSED: f32 = 110.0;
/// The height of the planner with the waypoint list open, without the list.
const PLANNER_CHROME: f32 = 140.0;
/// The least height of the waypoint list, when it is open.
const GRID_MIN: f32 = 52.0;
/// The space between two panels.
const GAP: f32 = 8.0;

/// The height of the route list panel for `routes` routes. `spare` is the height that is left
/// when the planner and the route table have their share. The list shows at least one row and a
/// half, so a scroll bar is a sign that more routes follow.
fn route_list_height(routes: usize, spare: f32) -> f32 {
    let full = routes.clamp(1, LIST_ROWS) as f32 * ROUTE_PITCH + PANEL_CHROME;
    spare.clamp(full.min(ROUTE_PITCH * 1.5 + PANEL_CHROME), full)
}

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
    /// True if the pilot opened the waypoint list, false if the pilot closed it. With `None`, the
    /// list is open until the first route shows, so the route gets the room.
    waypoints_open: Option<bool>,
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
            waypoints_open: None,
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
            let available = ui.available_height();
            let open = self.waypoints_open.unwrap_or(s.routes.is_empty());
            let planner = if open { PLANNER_CHROME + GRID_MIN } else { PLANNER_CLOSED };
            let list_height = route_list_height(s.routes.len(), available - TABLE_MIN - planner - 2.0 * GAP);
            let grid_max = (available - list_height - TABLE_MIN - PLANNER_CHROME - 2.0 * GAP).clamp(GRID_MIN, 200.0);
            self.planner(ui, s, open, grid_max);
            ui.add_space(8.0);
            ui.allocate_ui(vec2(ui.available_width(), list_height), |ui| route_list(ui, s));
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
    fn planner(&mut self, ui: &mut Ui, s: &mut Session, open: bool, grid_max: f32) {
        let info = match s.waypoints.len() {
            0 => String::new(),
            1 => "1 waypoint".into(),
            n => format!("{n} waypoints"),
        };
        panel(ui, "Route planner", &info, false, |ui| {
            // The buttons stay at the right edge. The search box takes the space that is left.
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let hint = "Plan for the ship of a character, or for a hull that you pick";
                    if ui.button(pilot_label(&s.settings, &s.pilots)).on_hover_text(hint).clicked() {
                        self.popup = Some(Popup::Pilot { filter: String::new() });
                    }
                    ui.label(theme::header_text("Pilot"));
                    let hint = "Add many systems at once: one name for each line, or names separated by commas";
                    if ui.button("Paste list…").on_hover_text(hint).clicked() {
                        self.popup = Some(Popup::List { text: String::new(), problems: Vec::new() });
                    }
                    if ui.button("+ Add waypoint").clicked() {
                        match self.search.take_highlighted() {
                            Some(node) => s.add_waypoint(node),
                            None => self.search.focus(ui),
                        }
                    }
                    let width = (ui.available_width() - ui.spacing().item_spacing.x).max(120.0);
                    let menu = [Pick::AddWaypoint, Pick::SetStart, Pick::AddFavourite];
                    let picked = self.search.show(ui, &s.uni, width, "Search system…  (Ctrl+F)", &menu);
                    if let Some((pick, node)) = picked {
                        apply_pick(s, pick, node);
                    }
                    if let Some(list) = self.search.take_list() {
                        s.add_list(&list);
                    }
                });
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
            let arrow = if open { "⏷" } else { "▶" };
            let chain = s.waypoints.iter().map(|&n| s.uni.name(n)).collect::<Vec<_>>().join(" » ");
            ui.horizontal(|ui| {
                let hint = if open { "Hide the waypoint list" } else { "Show the waypoint list, to move or remove a waypoint" };
                if ui.add(Button::new(arrow).small()).on_hover_text(hint).clicked() {
                    self.waypoints_open = Some(!open);
                }
                if !open {
                    ui.add(Label::new(RichText::new(&chain).color(theme::TEXT)).truncate()).on_hover_text(&chain);
                }
            });
            if !open {
                return;
            }
            ScrollArea::vertical().max_height(grid_max).auto_shrink([false, true]).show(ui, |ui| {
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Clear route").clicked() {
                        s.clear();
                    }
                    if ui.add_enabled(s.waypoints.len() > 1, Button::new("Reverse")).clicked() {
                        s.reverse();
                    }
                });
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
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROUTE_ROW), Sense::click());
                let selected = i == s.selected;
                if selected {
                    ui.painter().rect_filled(rect, 0.0, theme::ROW_FILL);
                    theme::selection_bar(ui, rect);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 0.0, theme::ACCENT_DIM);
                }
                let painter = ui.painter_at(rect);
                let font = egui::TextStyle::Body.resolve(ui.style());
                let left = rect.left() + 12.0;
                // Line 1: the number, then the summary in one font and one color. The jumps of each leg
                // are at the right, in the same font.
                let top = rect.top() + 4.0;
                let text_color = theme::TEXT_SOFT;
                let number = painter.layout_no_wrap(format!("#{}  ", i + 1), font.clone(), theme::TEXT_DIM);
                let summary = painter.layout_no_wrap(route_summary(route), font.clone(), text_color);
                let summary_left = left + number.size().x;
                let summary_text = route_summary(route);
                painter.galley(egui::pos2(left, top), number, theme::TEXT_DIM);
                painter.galley(egui::pos2(summary_left, top), summary, text_color);
                let legs: Vec<usize> = route.legs().iter().map(|&(a, b)| b - a).collect();
                if legs.len() > 1 {
                    let text = legs.iter().map(usize::to_string).collect::<Vec<_>>().join(" + ");
                    let galley = painter.layout_no_wrap(text, font, theme::TEXT_DIM);
                    let at = egui::pos2(rect.right() - galley.size().x - 10.0, top);
                    painter.galley(at, galley, theme::TEXT_DIM);
                }
                // Line 2: the strip.
                let line_y = rect.top() + 22.0;
                let strip_rect = egui::Rect::from_min_max(
                    egui::pos2(left, line_y),
                    egui::pos2((rect.right() - 10.0).max(left + 20.0), rect.bottom() - 4.0),
                );
                strip::paint(&painter, &s.uni, route, strip_rect);
                let names = |k: usize| s.uni.name(route.path.nodes[k]);
                let mut tip = vec![format!("#{}  {summary_text}", i + 1)];
                if legs.len() > 1 {
                    tip.extend(route.legs().iter().map(|&(a, b)| format!("{} » {}: {}", names(a), names(b), jumps_label(b - a))));
                }
                let tip = tip.join("\n");
                if response.on_hover_text(tip).clicked() {
                    s.selected = i;
                    s.selected_step = None;
                }
            }
        });
    });
}

/// A row of the route table: a step, or the heading of a leg.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableItem {
    Leg(usize),
    Step(usize),
}

/// The rows of the table. A route with two or more legs gets a heading before each leg. The stop
/// that ends a leg stays in that leg, and the next leg starts with the step after it.
fn table_items(route: &router_core::route::Route) -> Vec<TableItem> {
    let steps = route.path.nodes.len();
    let legs = route.legs();
    let mut items = Vec::new();
    if legs.len() > 1 {
        for (k, &(from, to)) in legs.iter().enumerate() {
            items.push(TableItem::Leg(k));
            let first = if k == 0 { from } else { from + 1 };
            items.extend((first..=to).map(TableItem::Step));
        }
    }
    // One leg, or stops that do not cover the steps: the plain list.
    let covered = items.iter().filter(|i| matches!(i, TableItem::Step(_))).count();
    if covered != steps {
        items = (0..steps).map(TableItem::Step).collect();
    }
    items
}

fn route_table(ui: &mut Ui, s: &mut Session, pilots: &mut PilotsUi) {
    let Some(route) = s.selected_route() else {
        panel(ui, "Route", "", true, |_| {});
        return;
    };
    let title = format!("Route #{}", s.selected + 1);
    let info = route_summary(route);
    let selected_step = s.selected_step;
    let mut clicked = None;
    let mut toggle = None;
    let mut copy = false;
    let characters = s.pilots.characters();
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
        let table_clip =
            egui::Rect::from_x_y_ranges(ui.min_rect().left()..=ui.available_rect_before_wrap().right(), ui.clip_rect().y_range());
        let header = |ui: &mut Ui, text: &str| _ = ui.label(theme::header_text(text));
        let mut table = TableBuilder::new(ui)
            .id_salt("route-table")
            .striped(false)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(36.0))
            .column(Column::exact(110.0))
            .column(Column::initial(150.0).at_least(90.0).resizable(true).clip(true));
        // The table has no Pilots column: the active route shows where the pilots are.
        table = table
            .column(Column::exact(72.0))
            .column(Column::initial(170.0).at_least(90.0).resizable(true).clip(true))
            .column(Column::remainder().at_least(120.0).clip(true))
            .auto_shrink(false);
        table
            .header(20.0, |mut row| {
                for text in ["#", "Stop", "System", "Security", "Region", "Via"] {
                    row.col(|ui| header(ui, text));
                }
            })
            .body(|body| {
                let items = table_items(route);
                let heights: Vec<f32> = items.iter().map(|item| if matches!(item, TableItem::Leg(_)) { 28.0 } else { 22.0 }).collect();
                body.heterogeneous_rows(heights.into_iter(), |mut row| {
                    let step = match items[row.index()] {
                        TableItem::Step(step) => step,
                        TableItem::Leg(k) => {
                            // A heading for the leg: where it starts, where it ends and how many jumps it has.
                            let (from, to) = route.legs()[k];
                            let text = format!(
                                "{} » {}  ·  {}",
                                uni.name(route.path.nodes[from]),
                                uni.name(route.path.nodes[to]),
                                jumps_label(to - from)
                            );
                            // A table cell clips its text. The heading is wider than the first column, so
                            // it draws on the layer of the cell with the clip of the whole table row.
                            row.col(|ui| {
                                let cell = ui.max_rect();
                                let clip = egui::Rect::from_x_y_ranges(table_clip.x_range(), ui.clip_rect().y_range());
                                let painter = ui.ctx().layer_painter(ui.layer_id()).with_clip_rect(clip);
                                if k > 0 {
                                    painter.hline(table_clip.x_range(), cell.top() + 1.0, egui::Stroke::new(1.0, theme::LINE));
                                }
                                let font = egui::FontId::proportional(11.0);
                                let at = egui::pos2(cell.left(), cell.center().y + 3.0);
                                painter.text(at, egui::Align2::LEFT_CENTER, text.to_uppercase(), font, theme::TEXT_DIM);
                            });
                            return;
                        }
                    };
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
                    row.set_selected(selected_step == Some(step));
                    // The start, the midpoints and the destination stand out from the other steps.
                    // A step that the pilot would rather avoid, and the route enters it anyway, gets an amber tag.
                    let avoided = step > 0 && stop.is_none() && s.settings.avoid.covers(uni, route.path.nodes[step]);
                    let name = match (stop, avoided) {
                        (Some(_), _) => RichText::new(&sys.name).family(theme::bold()).color(Color32::WHITE),
                        (None, true) => RichText::new(&sys.name).color(theme::WARN),
                        (None, false) => RichText::new(&sys.name).color(theme::TEXT),
                    };
                    row.col(|ui| _ = ui.label(RichText::new(step.to_string()).color(theme::TEXT_DIM)));
                    row.col(|ui| {
                        if let Some(stop) = stop {
                            ui.label(RichText::new(stop.label().to_uppercase()).color(theme::ACCENT).size(11.0).extra_letter_spacing(1.0));
                        } else if avoided {
                            ui.label(RichText::new("AVOID").color(theme::WARN).size(11.0).extra_letter_spacing(1.0));
                        }
                    });
                    row.col(|ui| {
                        let label = ui.add(Label::new(name).truncate());
                        if avoided {
                            label.on_hover_text("You'd rather skip this one, but there was no better way through");
                        }
                    });
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

    fn route_with_stops(steps: usize, stops: Vec<usize>) -> router_core::route::Route {
        use router_core::route::{Path, Route};
        let nodes = (0..steps).map(NodeIndex::new).collect();
        let path = Path { nodes, edges: Vec::new(), cost: 0 };
        Route {
            path,
            jumps: steps - 1,
            wormholes: 0,
            bridges: 0,
            bridge_tj: None,
            bridge_cap_pct: None,
            unknown_sigs: 0,
            avoided: 0,
            stops,
        }
    }

    #[test]
    fn the_table_groups_the_steps_by_leg() {
        use TableItem::{Leg, Step};
        // Two legs: the stop in the middle ends the first leg.
        let items = table_items(&route_with_stops(5, vec![0, 2, 4]));
        assert_eq!(items, [Leg(0), Step(0), Step(1), Step(2), Leg(1), Step(3), Step(4)]);
        // One leg: no heading.
        let items = table_items(&route_with_stops(3, vec![0, 2]));
        assert_eq!(items, [Step(0), Step(1), Step(2)]);
        // Stops that do not reach the last step: the plain list.
        let items = table_items(&route_with_stops(5, vec![0, 2, 3]));
        assert_eq!(items, [Step(0), Step(1), Step(2), Step(3), Step(4)]);
    }

    #[test]
    fn the_route_list_gives_way_to_the_table() {
        // Room for all rows of a short list.
        assert_eq!(route_list_height(2, 500.0), 2.0 * ROUTE_PITCH + PANEL_CHROME);
        // A long list stops at the rows that fit without a scroll.
        assert_eq!(route_list_height(9, 900.0), LIST_ROWS as f32 * ROUTE_PITCH + PANEL_CHROME);
        // Little room: one row and a half, so the table keeps its height.
        assert_eq!(route_list_height(5, 40.0), 1.5 * ROUTE_PITCH + PANEL_CHROME);
        // No routes: the empty text fits.
        assert_eq!(route_list_height(0, 500.0), ROUTE_PITCH + PANEL_CHROME);
    }

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
