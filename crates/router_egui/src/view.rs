//! The layout of the window: the route planner, the route list, the route table, the sidebar and
//! the status line.

use crate::app::Session;
use crate::notice::{Level, Toaster};
use crate::pilots_view::PilotsUi;
use crate::search::{Pick, SearchBox};
use crate::settings_window::{self, Page, Popup, SettingsForm};
use crate::strip;
use crate::theme::{self, panel};
use crate::widgets::{AppButton, UiExt, icon_label};
use egui::{Align, Button, Color32, Frame, Key, Label, Layout, Margin, Modifiers, RichText, ScrollArea, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor_icons::icons;
use petgraph::graph::NodeIndex;
use router_core::labels::{jumps_label, link_label, on_off, pilot_label, route_summary, route_tsv};
use router_core::route::{Mode, Stop};
use router_core::sources::FetchError;
use router_core::sources::nexum::MapInfo;
use router_core::universe::display_sec;
use router_core::wormhole::{SourceId, age_text};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// The height of a row of the route list: the summary line and the strip, and the space below.
pub fn route_row_height(compact: bool) -> f32 {
    if compact { 36.0 } else { 44.0 }
}

pub fn route_pitch(compact: bool) -> f32 {
    route_row_height(compact) + if compact { 2.0 } else { 4.0 }
}

/// The height that a panel adds around its content: the header strip, the margins and the lines.
pub fn panel_chrome(compact: bool) -> f32 {
    if compact { 30.0 } else { 46.0 }
}

/// The most rows that the route list shows before it scrolls.
const LIST_ROWS: usize = 3;
/// The least height that the route table keeps, in pixels. The planner and the list give way.
const TABLE_MIN: f32 = 200.0;
/// The height of the planner with the waypoint list closed.
fn planner_closed_height(compact: bool) -> f32 {
    if compact { 84.0 } else { 110.0 }
}
/// The height of the planner with the waypoint list open, without the list.
fn planner_chrome_height(compact: bool) -> f32 {
    if compact { 108.0 } else { 140.0 }
}
/// The least height of the waypoint list, when it is open.
const GRID_MIN: f32 = 52.0;
/// The space between two panels.
const GAP: f32 = 8.0;
/// The width of the sidebar.
const SIDEBAR: f32 = 280.0;

/// The height of the route list panel for `routes` routes. `spare` is the height that is left
/// when the planner and the route table have their share. The list shows at least one row and a
/// half, so a scroll bar is a sign that more routes follow.
fn route_list_height(routes: usize, spare: f32, compact: bool) -> f32 {
    let pitch = route_pitch(compact);
    let chrome = panel_chrome(compact);
    let full = routes.clamp(1, LIST_ROWS) as f32 * pitch + chrome;
    spare.clamp(full.min(pitch * 1.5 + chrome), full)
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
    /// The page that the Settings window showed last. It lasts for the session only.
    pub settings_page: Page,
    /// True while the Log window is open.
    log_open: bool,
    /// The Nexum map list fetch, while it runs.
    pub maps: Option<Receiver<Result<Vec<MapInfo>, FetchError>>>,
    status_timer: StatusTimer,
    toaster: Toaster,
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
            settings_page: Page::default(),
            log_open: false,
            maps: None,
            status_timer: StatusTimer::default(),
            toaster: Toaster::default(),
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
        // The status text clears after `STATUS_TIME`. A notice shows as a toast.
        match self.status_timer.update(&s.status, Instant::now()) {
            None => s.status.clear(),
            Some(_) if s.status.is_empty() => {}
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

        let bar =
            Frame::new().fill(theme::bar_fill(ui)).inner_margin(Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, theme::header_fill(ui)));
        egui::Panel::top("top-bar").frame(bar).show(ui, |ui| self.top_bar(ui, s));
        egui::Panel::bottom("status-bar").frame(bar).show(ui, |ui| status_bar(ui, s, &mut self.log_open));
        let side = Frame::new().fill(theme::BG).inner_margin(Margin { left: 0, right: 10, top: 10, bottom: 10 });
        egui::Panel::right("sidebar").show_separator_line(false).resizable(false).exact_size(SIDEBAR).frame(side).show(ui, |ui| {
            sidebar(ui, s, &mut self.pilots);
        });
        let central = Frame::new().fill(theme::BG).inner_margin(Margin::same(10));
        egui::CentralPanel::default().frame(central).show(ui, |ui| {
            // The active route hides the planner and the route list.
            if s.pilots.active.is_some() {
                self.pilots.active_view(ui, s);
                return;
            }
            self.plan_column(ui, s);
            ui.add_space(8.0);
            route_table(ui, s, &mut self.pilots);
        });

        settings_window::show(ui, self, s);
        self.pilots.windows(ui, s);
        crate::log_window::show(ui.ctx(), &mut self.log_open, s);
        self.toaster.show(ui, &mut s.notices);
    }

    /// The planner and the route list. They leave room for the route table under them.
    fn plan_column(&mut self, ui: &mut Ui, s: &mut Session) {
        let compact = theme::is_compact(ui);
        let available = ui.available_height();
        let open = self.waypoints_open.unwrap_or(s.routes.is_empty());
        let planner_chrome = planner_chrome_height(compact);
        let planner = if open { planner_chrome + GRID_MIN } else { planner_closed_height(compact) };
        let table = TABLE_MIN + GAP;
        let list_height = route_list_height(s.routes.len(), available - table - planner - GAP, compact);
        let grid_max = (available - list_height - table - planner_chrome - GAP).clamp(GRID_MIN, 200.0);
        self.planner(ui, s, open, grid_max);
        ui.add_space(if compact { 4.0 } else { 8.0 });
        ui.allocate_ui(vec2(ui.available_width(), list_height), |ui| route_list(ui, s));
    }

    /// Take the result of the Nexum map list fetch, if the thread sent it. The Wormholes page shows
    /// the list or the error.
    fn poll_maps(&mut self, s: &mut Session) {
        let Some(rx) = &self.maps else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            // The thread stopped with no result: it panicked.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(FetchError::Offline("the map list fetch stopped".into())),
        };
        self.maps = None;
        let error = match result {
            Ok(maps) => {
                s.map_names = maps;
                None
            }
            Err(FetchError::Offline(detail)) => Some(format!("Nexum offline: {detail}")),
            Err(e) => Some(e.status(SourceId::Nexum, None)),
        };
        if let Some(form) = &mut self.settings {
            form.set_maps_error(error);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, s: &mut Session) {
        let locked = s.pilots.active.is_some();
        ui.horizontal(|ui| {
            // The route options start at the left edge. While a route is active, they are hidden.
            if !locked {
                ui.label(theme::header_text("Mode"));
                mode_combo(ui, s, 150.0);
                optimize_box(ui, s);
                ui.separator();
                wormholes_toggle(ui, s);
                bridges_toggle(ui, s);
                ui.separator();
                ui.label(theme::header_text("Show up to"));
                count_stepper(ui, s);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The right-to-left layout draws the Settings button first, so it sits at the right edge.
                if ui.app_button(icon_label(ui, icons::GEAR.fill(), "Settings")).clicked() {
                    self.settings = Some(SettingsForm::new(s, self.settings_page));
                }
                let avoid = match s.settings.avoid.len() {
                    0 => "Avoid".to_string(),
                    n => format!("Avoid ({n})"),
                };
                let button = ui.add_enabled(!locked, AppButton::new(avoid)).on_hover_text("Systems and regions that routes avoid");
                if button.clicked() {
                    self.popup = Some(Popup::Avoid { confirm: false, search: SearchBox::new("avoid-search"), region: String::new() });
                }
                button.on_disabled_hover_text(settings_window::LOCKED);
                self.pilots.characters_button(ui, s);
            });
        });
    }

    /// The Pilot, Paste list and Add waypoint buttons, for a right-to-left layout: the first button
    /// that the code draws sits at the right edge.
    fn planner_buttons(&mut self, ui: &mut Ui, s: &mut Session) {
        if ui.add(AppButton::new("+ Add waypoint").primary()).clicked() {
            match self.search.take_highlighted() {
                Some(node) => s.add_waypoint(node),
                None => self.search.focus(ui),
            }
        }
        let hint = "Add many systems at once: one name for each line, or names separated by commas";
        if ui.app_button("Paste list…").on_hover_text(hint).clicked() {
            self.popup = Some(Popup::List { text: String::new(), problems: Vec::new() });
        }
        let hint = "Plan for the ship of a character, or for a hull that you pick";
        let pilot = pilot_label(&s.settings, &s.pilots);
        if ui.app_button(pilot).on_hover_text(hint).clicked() {
            self.popup = Some(Popup::Pilot { filter: String::new() });
        }
        ui.label(theme::header_text("Pilot"));
    }

    fn planner_search(&mut self, ui: &mut Ui, s: &mut Session, width: f32) {
        let menu = [Pick::AddWaypoint, Pick::SetStart, Pick::AddFavourite];
        let picked = self.search.show(ui, &s.uni, width, "Search system…  (Ctrl+F)", &menu);
        if let Some((pick, node)) = picked {
            apply_pick(s, pick, node);
        }
        if let Some(list) = self.search.take_list() {
            s.add_list(&list);
        }
    }

    /// The search box and the waypoint list. They set the waypoints of the route.
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
                    self.planner_buttons(ui, s);
                    let width = (ui.available_width() - ui.spacing().item_spacing.x).max(120.0);
                    self.planner_search(ui, s, width);
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
            let arrow = if open { icons::CARET_DOWN } else { icons::CARET_RIGHT }.as_str();
            let names: Vec<&str> = s.waypoints.iter().map(|&n| s.uni.name(n)).collect();
            let chain = names.join(" » ");
            let summary = waypoint_summary(&names);
            ui.horizontal(|ui| {
                let hint = if open { "Hide the waypoint list" } else { "Show the waypoint list, to move or remove a waypoint" };
                if ui.add(Button::new(arrow).small()).on_hover_text(hint).clicked() {
                    self.waypoints_open = Some(!open);
                }
                if !open {
                    let label = ui.add(Label::new(RichText::new(&summary).color(theme::TEXT)).truncate().sense(Sense::hover()));
                    // A short summary leaves out the midpoints, so the hover text lists them all.
                    if summary != chain {
                        label.on_hover_text(&chain);
                    }
                }
            });
            if !open {
                return;
            }
            ScrollArea::both().max_height(grid_max).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("waypoints").num_columns(6).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
                    let last = s.waypoints.len() - 1;
                    for (i, &node) in s.waypoints.iter().enumerate() {
                        let sys = s.uni.system(node);
                        let stop = s.stop(i);
                        let color = if matches!(stop, Stop::Midpoint(_)) { theme::TEXT_DIM } else { theme::accent(ui) };
                        ui.label(RichText::new(format!("{:>2}", i + 1)).color(theme::TEXT_DIM));
                        ui.label(RichText::new(stop.label().to_uppercase()).color(color).size(11.0).extra_letter_spacing(1.0));
                        let name =
                            ui.add(Label::new(RichText::new(&sys.name).family(theme::bold()).color(Color32::WHITE)).sense(Sense::click()));
                        ui.label(RichText::new(format!("{:.1}", display_sec(sys.security))).color(theme::sec_color(sys.security)));
                        ui.label(RichText::new(&sys.region).color(theme::TEXT_DIM));
                        ui.horizontal(|ui| {
                            if ui.add_enabled(i > 0, Button::new(icons::CARET_UP.as_str()).small()).on_hover_text("Move up").clicked() {
                                edit = Some(Edit::MoveUp(i));
                            }
                            if ui
                                .add_enabled(i < last, Button::new(icons::CARET_DOWN.as_str()).small())
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                edit = Some(Edit::MoveDown(i));
                            }
                            if ui.add(Button::new(icons::X.as_str()).small()).on_hover_text("Remove").clicked() {
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
                                if ui.add_enabled(enabled, AppButton::new(label)).clicked() {
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
                    if ui.app_button("Clear route").clicked() {
                        s.clear();
                    }
                    if ui.add_enabled(s.waypoints.len() > 1, AppButton::new("Reverse")).clicked() {
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

/// The "- n +" stepper for the number of routes. In a right-to-left layout it draws backwards, so
/// the "+" comes first.
fn count_stepper(ui: &mut Ui, s: &mut Session) {
    let rtl = ui.layout().prefer_right_to_left();
    let minus = |ui: &mut Ui, s: &mut Session| {
        if ui.add_enabled(s.settings.top > 1, AppButton::new("−")).clicked() {
            s.settings.top -= 1;
            s.recompute();
            s.save_quietly();
        }
    };
    let plus = |ui: &mut Ui, s: &mut Session| {
        if ui.app_button("+").clicked() {
            s.settings.top += 1;
            s.recompute();
            s.save_quietly();
        }
    };
    if rtl {
        plus(ui, s);
        ui.label(RichText::new(s.settings.top.to_string()).color(Color32::WHITE));
        minus(ui, s);
    } else {
        minus(ui, s);
        ui.label(RichText::new(s.settings.top.to_string()).color(Color32::WHITE));
        plus(ui, s);
    }
}

fn bridges_toggle(ui: &mut Ui, s: &mut Session) {
    let blocked = s.settings.rules.blocked_reason();
    let bridges = match &blocked {
        Some(_) => "off (no capital)",
        None => on_off(s.settings.bridges),
    };
    let response = ui.add(AppButton::new(format!("Jump bridges: {bridges}")).selected(s.settings.bridges && blocked.is_none()));
    if response.clicked() {
        s.settings.bridges = !s.settings.bridges;
        s.recompute();
    }
    if let Some(reason) = blocked {
        response.on_hover_text(reason);
    }
}

fn wormholes_toggle(ui: &mut Ui, s: &mut Session) {
    if ui.add(AppButton::new(format!("Wormholes: {}", on_off(s.settings.wormholes))).selected(s.settings.wormholes)).clicked() {
        s.settings.wormholes = !s.settings.wormholes;
        s.recompute();
    }
}

fn optimize_box(ui: &mut Ui, s: &mut Session) {
    if ui.app_checkbox(&mut s.settings.optimize, "Optimize order").on_hover_text("Visit each system once, in the cheapest order").changed()
    {
        s.recompute();
        s.save_quietly();
    }
}

fn mode_combo(ui: &mut Ui, s: &mut Session, width: f32) {
    let mut mode = s.settings.mode;
    egui::ComboBox::from_id_salt("mode").selected_text(mode.title()).width(width).show_ui(ui, |ui| {
        for m in Mode::ALL {
            ui.selectable_value(&mut mode, m, m.title()).on_hover_text(m.description());
        }
    });
    if mode != s.settings.mode {
        s.settings.mode = mode;
        s.recompute();
        s.save_quietly();
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
    let compact = theme::is_compact(ui);
    let row_height = route_row_height(compact);
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
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::click());
                let selected = i == s.selected;
                if selected {
                    ui.painter().rect_filled(rect, 0.0, theme::ROW_FILL);
                    theme::selection_bar(ui, rect);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 0.0, theme::accent(ui).linear_multiply(0.18));
                }
                let painter = ui.painter_at(rect);
                let font = egui::TextStyle::Body.resolve(ui.style());
                let left = rect.left() + (if compact { 8.0 } else { 12.0 });
                // Line 1: the number, then the summary in one font and one color.
                let top = rect.top() + (if compact { 2.0 } else { 4.0 });
                let text_color = theme::TEXT_SOFT;
                let number = painter.layout_no_wrap(format!("#{}  ", i + 1), font.clone(), theme::TEXT_DIM);
                let legs = route.legs();
                let summary_left = left + number.size().x;
                // The summary ends in "…" before it reaches the right edge. The tooltip has the whole text.
                let summary_text = route_summary(route);
                let summary = fit_text(&painter, &summary_text, font.clone(), text_color, rect.right() - 10.0 - summary_left);
                painter.galley(egui::pos2(left, top), number, theme::TEXT_DIM);
                painter.galley(egui::pos2(summary_left, top), summary, text_color);
                // Line 2: the strip.
                let line_y = rect.top() + (if compact { 18.0 } else { 22.0 });
                let strip_rect = egui::Rect::from_min_max(
                    egui::pos2(left, line_y),
                    egui::pos2((rect.right() - 10.0).max(left + 20.0), rect.bottom() - (if compact { 2.0 } else { 4.0 })),
                );
                strip::paint(&painter, &s.uni, route, strip_rect);
                let names = |k: usize| s.uni.name(route.path.nodes[k]);
                let mut tip = Vec::new();
                if legs.len() > 1 {
                    tip.extend(legs.iter().map(|&(a, b)| format!("{} » {}: {}", names(a), names(b), jumps_label(b - a))));
                }
                // The row shows the summary already, so the tooltip is only for the jumps of each leg.
                let response = if legs.len() > 1 { response.on_hover_text(tip.join("\n")) } else { response };
                if response.clicked() {
                    s.selected = i;
                    s.selected_step = None;
                }
            }
        });
    });
}

/// A text on one line that ends in "…" when it is wider than `width`.
fn fit_text(painter: &egui::Painter, text: &str, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(40.0));
    painter.layout_job(job)
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
        // Without a client ID the router cannot log in, so the button goes, as the Characters one does.
        if s.pilots.shows_characters() {
            let has_pilot = characters.iter().any(|c| !c.live.expired);
            let text = if has_pilot { format!("Start route #{}", s.selected + 1) } else { "Log in to start".to_string() };
            let button =
                Button::new(RichText::new(text).size(11.0).color(Color32::WHITE)).fill(theme::accent(ui).gamma_multiply(0.35)).small();
            start = ui.add(button).on_hover_text("Send the waypoints of this route to a character in the game").clicked();
            ui.add_space(6.0);
        }
        let button = Button::new(RichText::new("Copy route").size(11.0)).small();
        copy = ui.add(button).on_hover_text("Copy the route as tab-separated text, for a Slack table").clicked();
        ui.add_space(6.0);
        ui.label(RichText::new(&info).color(theme::TEXT_DIM).size(11.0));
    };
    theme::panel_with(ui, &title, header, true, |ui| {
        let uni = &s.uni;
        let header = |ui: &mut Ui, text: &str| _ = ui.label(theme::header_text(text));
        // The table has no Pilots column: the active route shows where the pilots are.
        let table = TableBuilder::new(ui)
            .id_salt("route-table")
            .striped(false)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(36.0))
            .column(Column::exact(110.0))
            .column(Column::initial(150.0).at_least(90.0).resizable(true).clip(true))
            .column(Column::exact(72.0))
            .column(Column::initial(170.0).at_least(90.0).resizable(true).clip(true))
            .column(Column::remainder().at_least(120.0).clip(true))
            .auto_shrink(false);
        table
            .header(20.0, |mut row| {
                for text in ["#", "Role", "System", "Security", "Region", "Via"] {
                    row.col(|ui| {
                        header(ui, text);
                        // The line under the header runs over every cell, as in the client.
                        let y = ui.max_rect().bottom();
                        ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, theme::LINE));
                    });
                }
            })
            .body(|body| {
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
                            ui.label(
                                RichText::new(stop.label().to_uppercase()).color(theme::accent(ui)).size(11.0).extra_letter_spacing(1.0),
                            );
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
                        _ = ui.label(RichText::new(format!("{:.1}", display_sec(sys.security))).color(theme::sec_color(sys.security)));
                    });
                    row.col(|ui| _ = ui.add(Label::new(RichText::new(&sys.region).color(theme::TEXT_DIM)).truncate()));
                    row.col(|ui| _ = ui.add(Label::new(RichText::new(&via).color(via_color)).truncate()));
                    let node = route.path.nodes[step];
                    row.response().context_menu(|ui| {
                        let avoid = &s.settings.avoid;
                        let system =
                            if avoid.has_system(node) { format!("Stop avoiding {}", sys.name) } else { format!("Avoid {}", sys.name) };
                        if ui.app_button(system).clicked() {
                            toggle = Some(Toggle::System(node));
                            ui.close();
                        }
                        let region = if avoid.has_region(&sys.region) {
                            format!("Stop avoiding {}", sys.region)
                        } else {
                            format!("Avoid {}", sys.region)
                        };
                        if ui.app_button(region).clicked() {
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
        ui.ctx().copy_text(route_tsv(&s.uni, &s.settings.rules, route, s.now));
        s.notify(Level::Success, format!("Route #{} copied to the clipboard", s.selected + 1));
    }
}

/// The panels of the sidebar: the data that the overlays loaded, the favourites and the pilots.
fn sidebar(ui: &mut Ui, s: &mut Session, pilots: &mut PilotsUi) {
    let has_pilots = !s.pilots.characters().is_empty();
    if connections_panel(ui, s) {
        ui.add_space(8.0);
    }
    // With pilots, the Pilots panel goes below, so this panel does not fill the sidebar.
    favourites_panel(ui, s, !has_pilots);
    if has_pilots {
        ui.add_space(8.0);
        pilots.sidebar_panel(ui, s);
    }
}

/// The sync state of a wormhole source: the dot color and the text, for example "2 min ago".
/// The dot is green for data from the last 15 minutes, else amber. A source with no data is gray.
fn source_state(s: &Session, source: SourceId) -> (Color32, String) {
    const FRESH_SECS: u64 = 15 * 60;
    let now = router_core::wormhole::now();
    let fetched = s.shortcuts.sources.iter().find(|(id, _)| *id == source).map(|&(_, at)| at);
    match fetched {
        Some(at) if now.saturating_sub(at) <= FRESH_SECS => (theme::OK, age_text(at, now)),
        Some(at) => (theme::WARN, age_text(at, now)),
        None if source == SourceId::Nexum && s.cfg.nexum.complete().is_none() => (theme::TEXT_DIM, "not set".into()),
        None => (theme::TEXT_DIM, "no data".into()),
    }
}

/// The wormholes and the jump bridges that the overlays loaded. It shows only when an overlay
/// loaded a connection. Returns true if it drew the panel.
fn connections_panel(ui: &mut Ui, s: &Session) -> bool {
    let sc = &s.shortcuts;
    if sc.wormholes + sc.bridges == 0 {
        return false;
    }
    let settings = &s.settings;
    let bridges_on = settings.bridges && settings.rules.blocked_reason().is_none();
    panel(ui, "Connections loaded", "", false, |ui| {
        egui::Grid::new("shortcuts").num_columns(2).spacing(vec2(8.0, 2.0)).min_col_width(60.0).show(ui, |ui| {
            // A kind that is off for routing shows in gray.
            let row = |ui: &mut Ui, label: &str, count: usize, on: bool| {
                let color = if on { theme::TEXT } else { theme::TEXT_DIM };
                ui.label(RichText::new(label).color(color));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(count.to_string()).color(color)));
                ui.end_row();
            };
            for (label, count, on) in
                [("Nexum", sc.nexum, true), ("Thera", sc.thera, settings.hubs.thera), ("Turnur", sc.turnur, settings.hubs.turnur)]
            {
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
    true
}

/// The favourites, with the jumps from the first waypoint. `fill` makes the panel as high as the
/// space that is left.
/// The collapsed list of waypoints: the whole chain when it is short, else the start and the
/// destination with the count of the midpoints between them.
fn waypoint_summary(names: &[&str]) -> String {
    const MAX_SHOWN: usize = 4;
    match names {
        [first, .., last] if names.len() > MAX_SHOWN => {
            let midpoints = names.len() - 2;
            format!("{first} » {last} · {midpoints} midpoints")
        }
        _ => names.join(" » "),
    }
}

fn favourites_panel(ui: &mut Ui, s: &mut Session, fill: bool) {
    let info = s.waypoints.first().map_or(String::new(), |&n| format!("jumps from {}", s.uni.name(n)));
    let mut add = None;
    panel(ui, "Favourites", &info, fill, |ui| {
        if s.settings.favourites.is_empty() {
            ui.label(RichText::new("No favourites. Add one in the settings, or right-click a search result.").color(theme::TEXT_DIM));
            return;
        }
        if s.waypoints.is_empty() {
            ui.label(
                RichText::new("Click a favourite to add it as a waypoint. The jumps to each one show once you have a waypoint.")
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(6.0);
        }
        for &fav in &s.settings.favourites {
            let jumps = s.hubs.iter().find(|(n, _)| *n == fav).map(|&(_, j)| j);
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
            let accent = theme::accent(ui);
            // A row has a faint fill at rest, so it reads as a button before the pointer comes.
            // Under the pointer the fill is brighter and the row gets the bar of a selected row.
            let hovered = response.hovered();
            ui.painter().rect_filled(rect, 0.0, accent.gamma_multiply(if hovered { 0.13 } else { 0.045 }));
            if hovered {
                theme::selection_bar(ui, rect);
            }
            let font = egui::TextStyle::Body.resolve(ui.style());
            let sys = s.uni.system(fav);
            // The star is the filled icon, so it reads as a mark, not as an outline.
            let star = egui::WidgetText::from(icons::STAR.fill()).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            );
            let p = ui.painter();
            p.galley(rect.left_center() + vec2(10.0, -star.size().y / 2.0), star, accent.gamma_multiply(0.7));
            p.text(rect.left_center() + vec2(30.0, 0.0), egui::Align2::LEFT_CENTER, &sys.name, font.clone(), theme::TEXT);
            // The plus at the right edge tells that a click adds the system. It is dim at rest.
            let plus = if hovered { accent } else { accent.gamma_multiply(0.5) };
            p.text(rect.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, icons::PLUS.as_str(), font.clone(), plus);
            let jumps = match jumps {
                Some(Some(j)) => j.to_string(),
                Some(None) => "-".into(),
                None => String::new(),
            };
            p.text(rect.right_center() - vec2(30.0, 0.0), egui::Align2::RIGHT_CENTER, jumps, font, accent);
            if response.on_hover_text("Click to add as a waypoint").clicked() {
                add = Some(fav);
            }
            ui.add_space(2.0);
        }
    });
    if let Some(node) = add {
        s.add_waypoint(node);
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
            let parts: Vec<&str> =
                std::iter::once(&s.status).chain(std::iter::once(&s.note)).map(String::as_str).filter(|t| !t.is_empty()).collect();
            let (text, color) = match parts.is_empty() {
                true => ("Ready".to_string(), theme::TEXT_DIM),
                false => (parts.join(" · "), theme::WARN),
            };
            ui.add(Label::new(RichText::new(&text).color(color)).truncate());
        });
    });
}

/// The wormhole sources and the age of their data, for example "Nexum 2 min ago" after a dot.
/// The dot is green for data from the last 15 minutes, else amber. A source with no data is gray.
fn sync_status(ui: &mut Ui, s: &Session) {
    // The ages change with time, so the bar draws again each minute.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(60));
    // The layout is right to left: EVE-Scout goes first, so Nexum shows on its left.
    for source in [SourceId::EveScout, SourceId::Nexum] {
        let (dot, text) = source_state(s, source);
        ui.label(RichText::new(&text).color(theme::TEXT_DIM).small());
        ui.label(RichText::new(source.label()).color(theme::TEXT).small());
        // A painted dot: the fonts have no circle glyph.
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, dot);
        ui.add_space(10.0);
    }
}

/// The loading screen with the current step, or the load error. A download shows a progress bar.
pub fn splash(ui: &mut Ui, error: Option<&str>, step: Option<&router_core::startup::Progress>) {
    let frame = Frame::new().fill(theme::BG);
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 80.0).max(0.0));
            ui.allocate_ui(vec2(420.0, 190.0), |ui| {
                panel(ui, "EVE Router", "", false, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        match error {
                            None => {
                                ui.add(egui::Spinner::new().size(28.0).color(theme::accent(ui)));
                                ui.add_space(8.0);
                                let text = step.map_or_else(|| "Starting…".to_string(), router_core::startup::Progress::text);
                                ui.label(RichText::new(text).color(theme::TEXT));
                                if let Some(fraction) = step.and_then(router_core::startup::Progress::fraction) {
                                    ui.add_space(6.0);
                                    ui.add(egui::ProgressBar::new(fraction).fill(theme::accent(ui)).desired_width(300.0));
                                }
                            }
                            Some(e) => {
                                ui.add_space(4.0);
                                ui.label(RichText::new(icons::WARNING_CIRCLE.as_str()).size(30.0).color(theme::ERROR_SOFT));
                                ui.add_space(10.0);
                                ui.label(
                                    RichText::new("Error loading")
                                        .family(theme::bold())
                                        .size(18.0)
                                        .extra_letter_spacing(0.5)
                                        .color(Color32::WHITE),
                                );
                                ui.add_space(8.0);
                                let message = RichText::new(e).color(theme::TEXT_SOFT).line_height(Some(20.0));
                                ui.add(Label::new(message).wrap());
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
    fn a_short_chain_shows_every_waypoint() {
        assert_eq!(waypoint_summary(&["Jita"]), "Jita");
        assert_eq!(waypoint_summary(&["Jita", "Amarr"]), "Jita » Amarr");
        assert_eq!(waypoint_summary(&["A", "B", "C", "D"]), "A » B » C » D");
    }

    #[test]
    fn a_long_chain_shows_the_ends_and_counts_the_midpoints() {
        assert_eq!(waypoint_summary(&["A", "B", "C", "D", "E"]), "A » E · 3 midpoints");
        let names = vec!["x"; 21];
        assert_eq!(waypoint_summary(&names), "x » x · 19 midpoints");
    }

    #[test]
    fn the_splash_draws_every_step_and_the_error() {
        use router_core::startup::Progress;
        let steps = [None, Some(Progress::CheckingSde), Some(Progress::DownloadingSde { done: 5, total: 10 }), Some(Progress::LoadingMap)];
        // The app registers its fonts and theme first: the splash uses the bold font.
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let draw = |error: Option<&str>, step: Option<&Progress>| {
            // The frame output holds texture changes. A test with no renderer drops them.
            let mut output = ctx.run_ui(Default::default(), |ui| splash(ui, error, step));
            output.textures_delta.clear();
        };
        for step in &steps {
            draw(None, step.as_ref());
        }
        draw(Some("Cannot read the config"), None);
    }

    #[test]
    fn the_route_list_gives_way_to_the_table() {
        // Room for all rows of a short list.
        assert_eq!(route_list_height(2, 500.0, false), 2.0 * route_pitch(false) + panel_chrome(false));
        // A long list stops at the rows that fit without a scroll.
        assert_eq!(route_list_height(9, 900.0, false), LIST_ROWS as f32 * route_pitch(false) + panel_chrome(false));
        // Little room: one row and a half, so the table keeps its height.
        assert_eq!(route_list_height(5, 40.0, false), 1.5 * route_pitch(false) + panel_chrome(false));
        // No routes: the empty text fits.
        assert_eq!(route_list_height(0, 500.0, false), route_pitch(false) + panel_chrome(false));

        // In compact mode:
        assert_eq!(route_list_height(2, 500.0, true), 2.0 * route_pitch(true) + panel_chrome(true));
        assert_eq!(route_list_height(9, 900.0, true), LIST_ROWS as f32 * route_pitch(true) + panel_chrome(true));
        assert_eq!(route_list_height(5, 40.0, true), 1.5 * route_pitch(true) + panel_chrome(true));
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
