//! The layout of the window: the route planner, the route list, the route table, the sidebar and
//! the status line.

use crate::app::{Session, Status};
use crate::notice::{Level, Toaster};
use crate::pilots_view::PilotsUi;
use crate::search::{Pick, SearchBox};
use crate::settings_window::{self, Page, Popup, SettingsForm};
use crate::setup_wizard::{Flow, Wizard};
use crate::strip;
use crate::theme::{self, panel};
#[cfg(test)]
use crate::welcome::Step;
use crate::welcome::{Answers, Preset, Welcome, WelcomeFlow};
use crate::widgets::{self, AppButton, UiExt, icon_label};
use egui::{Align, Button, Color32, Frame, Key, Label, Layout, Margin, Modifiers, RichText, ScrollArea, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor_icons::icons;
use petgraph::graph::NodeIndex;
use router_core::labels::{LinkView, jumps_label, on_off, pilot_label, route_summary, route_tsv};
use router_core::route::{Mode, Stop};
use router_core::settings::MAX_TOP;
use router_core::sources::FetchError;
use router_core::sources::nexum::MapInfo;
use router_core::universe::{Link, display_sec};
use router_core::wormhole::{ClosedHole, SourceId, age_text};
use router_core::wormhole_space::Stance;
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
    if compact { 34.0 } else { 50.0 }
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
/// The margin above and below the page of the setup wizard.
const WIZARD_MARGIN: i8 = 16;
/// The margin at the left and at the right of the page and of the footer of the setup wizard.
const WIZARD_SIDE: i8 = 32;

/// The height of the route list panel for `routes` routes. `spare` is the height that is left
/// when the planner and the route table have their share. Up to `LIST_ROWS` routes always fit
/// without a scroll bar. With more routes, the list shows at least one row and a half, so a
/// scroll bar is a sign that more routes follow.
fn route_list_height(routes: usize, spare: f32, compact: bool) -> f32 {
    let pitch = route_pitch(compact);
    let chrome = panel_chrome(compact);
    let full = routes.clamp(1, LIST_ROWS) as f32 * pitch + chrome;
    if routes <= LIST_ROWS {
        return full;
    }
    spare.clamp(full.min(pitch * 1.5 + chrome), full)
}

/// The time in seconds after a change between the welcome screen and the wizard, or between two
/// pages of the wizard, in which the window ignores all clicks. The footers of the screens and of
/// the pages are at the same place, so the second click of a double-click would hit a button of
/// the new screen or page without this guard.
const SWITCH_CLICK_GUARD: f64 = theme::CLICK_GUARD;

/// How long the status bar shows a status text.
const STATUS_TIME: Duration = Duration::from_secs(4);

/// The age of the status. The status bar clears a status after `STATUS_TIME`.
#[derive(Default)]
struct StatusTimer {
    /// The status, and the time when it first showed.
    shown: Option<(Status, Instant)>,
}

impl StatusTimer {
    /// Look at the current status at `now`. Return `None` if the status must clear now, else the
    /// time until it must clear. `Ready`, or a new status, gives a full `STATUS_TIME`.
    fn update(&mut self, status: Status, now: Instant) -> Option<Duration> {
        if status == Status::Ready {
            self.shown = None;
            return Some(STATUS_TIME);
        }
        match &self.shown {
            Some((shown, at)) if *shown == status => {
                let left = STATUS_TIME.saturating_sub(now - *at);
                if left.is_zero() {
                    self.shown = None;
                    return None;
                }
                Some(left)
            }
            _ => {
                self.shown = Some((status, now));
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
    pub pilots: PilotsUi,
    /// True if the pilot opened the waypoint list, false if the pilot closed it. With `None`, the
    /// list is open until the first route shows, so the route gets the room.
    waypoints_open: Option<bool>,
    /// The routing overlay. It lives here, because the settings window sets its position.
    pub overlay: crate::overlay::Overlay,
    /// The welcome screen. It is `Some` from the start of a setup until Finish. While the wizard is
    /// `None`, the window shows it in place of the route view. It keeps its step while the wizard runs,
    /// so Back on the first wizard page returns to that step.
    pub welcome: Option<Welcome>,
    /// The setup wizard. While it is `Some`, the window shows the wizard in place of the route view
    /// and of the welcome screen.
    pub wizard: Option<Wizard>,
    /// The egui time of the last change between the welcome screen and the wizard, or between two
    /// pages of the wizard. Until `SWITCH_CLICK_GUARD` seconds after this time, the window ignores
    /// all clicks.
    switched_at: f64,
}

/// A change of the avoid list, or a closed wormhole, from the context menu of a route step.
enum Toggle {
    System(NodeIndex),
    Region(String),
    Close(ClosedHole),
}

/// A change to the waypoints, from a click in the route planner.
enum Edit {
    SetStart(usize),
    MoveUp(usize),
    MoveDown(usize),
    Remove(usize),
}

impl View {
    /// The view at the start. The welcome screen shows first when the start asks for it
    /// (`Session::setup_due`). The wizard starts when the pilot is done with the welcome screen.
    pub fn new(session: &Session) -> Self {
        View {
            welcome: session.setup_due.then(Welcome::new),
            wizard: None,
            switched_at: f64::NEG_INFINITY,
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
            overlay: Default::default(),
        }
    }

    pub fn show(&mut self, ui: &mut Ui, s: &mut Session) {
        if self.wizard.is_some() {
            self.show_wizard(ui, s);
            self.switch_guard(ui);
            return;
        }
        if self.welcome.is_some() {
            self.show_welcome(ui, s);
            self.switch_guard(ui);
            return;
        }
        self.poll_maps(s);
        self.pilots.update(ui.ctx(), s);
        // A dragged value in the settings waits for a quiet time, then the app searches and saves.
        if let Some(left) = s.apply_due(Instant::now()) {
            ui.ctx().request_repaint_after(left);
        }
        // The status clears after `STATUS_TIME`. A notice shows as a toast.
        match self.status_timer.update(s.status, Instant::now()) {
            None => s.status = Status::Ready,
            Some(_) if s.status == Status::Ready => {}
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

    /// The welcome screen in place of the route view: the status note of a dry run, the pinned
    /// footer of step 2 and the body. It shows no top bar, no sidebar and no Settings window, and it
    /// runs no `apply_due`, so nothing is saved. A `Start` from the body or the footer builds the
    /// wizard. The welcome screen stays in `self.welcome` until Finish.
    fn show_welcome(&mut self, ui: &mut Ui, s: &mut Session) {
        // The same rules as the wizard: no Settings window and no old Remove question.
        self.settings = None;
        self.pilots.clear_remove();
        self.pilots.poll_login(ui.ctx(), s);
        let Some(welcome) = &mut self.welcome else { return };
        // Both steps draw the footer panel, so the body has the same room. Step 1 draws it invisible.
        // The panel must come before the central panel.
        let footer = Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(WIZARD_SIDE, theme::FOOTER_MARGIN));
        let footer_flow = egui::Panel::bottom("welcome-footer").frame(footer).show(ui, |ui| welcome.footer(ui)).inner;
        let central = Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(WIZARD_SIDE, WIZARD_MARGIN));
        let body_flow = egui::CentralPanel::default().frame(central).show(ui, |ui| welcome.body(ui, s)).inner;
        self.toaster.show(ui, &mut s.notices);
        let flow = if footer_flow == WelcomeFlow::Open { body_flow } else { footer_flow };
        if let WelcomeFlow::Start(preset, answers) = flow {
            self.start_wizard(s, preset, answers, ui.input(|i| i.time));
        }
    }

    /// Build the wizard from a preset and its answers. This drops any earlier wizard and its edits.
    /// The welcome screen stays. `now` is the egui time of the change, and the click guard starts
    /// at this time.
    fn start_wizard(&mut self, s: &Session, preset: Preset, answers: Answers, now: f64) {
        self.wizard = Some(Wizard::with_preset(s, preset, answers));
        self.switched_at = now;
    }

    /// Draw the click guard after a change between the welcome screen and the wizard, or between
    /// two pages of the wizard. Call it after all panels and windows of the frame.
    ///
    /// The guard is an invisible area on the foreground layer that covers the window. While the
    /// guard time runs, the area takes all clicks, so the widgets below get none. Then it lets the
    /// clicks go through. egui finds the widget under the pointer from the layers of the frame
    /// before. Thus the area of the change frame takes the click of the next frame.
    ///
    /// The area shows on each frame of the setup, also while it lets the clicks go through. egui
    /// sizes a new area in its first frame, and an area that sizes takes no click in the next frame.
    /// Thus the area must exist before the first change.
    fn switch_guard(&self, ui: &Ui) {
        let ctx = ui.ctx();
        let left = SWITCH_CLICK_GUARD - (ui.input(|i| i.time) - self.switched_at);
        let on = left > 0.0;
        let sense = if on { Sense::click_and_drag() } else { Sense::hover() };
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("switch-guard"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(on)
            .sense(sense)
            .fade_in(false)
            .show(ctx, |ui| _ = ui.allocate_rect(screen, sense));
        if on {
            ctx.request_repaint_after(Duration::from_secs_f64(left));
        }
    }

    /// The setup wizard in place of the route view: the dry-run note, the pinned footer, the steps
    /// and the page. It shows no top bar, no sidebar and no Settings window, and it runs no
    /// `apply_due`, so nothing is saved before Finish. The Characters window shows for the login,
    /// with no Remove.
    fn show_wizard(&mut self, ui: &mut Ui, s: &mut Session) {
        // A Settings window would keep its old state after Finish. Only the top bar opens it, and the wizard does not draw that bar.
        self.settings = None;
        // A "Remove <name>?" question from before the wizard must not come back after Finish.
        self.pilots.clear_remove();
        self.pilots.poll_login(ui.ctx(), s);
        // The status clears after `STATUS_TIME`, as in the route view. The wizard shows no status,
        // but a status that stays would show again for a full time after Finish.
        match self.status_timer.update(s.status, Instant::now()) {
            None => s.status = Status::Ready,
            Some(_) if s.status == Status::Ready => {}
            Some(left) => ui.ctx().request_repaint_after(left),
        }
        let Some(wizard) = &mut self.wizard else { return };
        let footer = Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(WIZARD_SIDE, theme::FOOTER_MARGIN));
        let page = wizard.page();
        let flow = egui::Panel::bottom("wizard-footer").frame(footer).show(ui, |ui| wizard.footer(ui)).inner;
        // Next, Back and Skip of two pages are at the same place, so a page change starts the click guard too.
        if flow == Flow::Open && wizard.page() != page {
            self.switched_at = ui.input(|i| i.time);
        }
        let central = Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(WIZARD_SIDE, WIZARD_MARGIN));
        egui::CentralPanel::default().frame(central).show(ui, |ui| {
            wizard.steps(ui);
            ui.add_space(theme::STEPS_GAP);
            wizard.body(ui, s, &mut self.pilots);
        });
        self.pilots.characters_window(ui, s, false);
        crate::log_window::show(ui.ctx(), &mut self.log_open, s);
        self.toaster.show(ui, &mut s.notices);
        // The page of this frame is drawn. The next frame shows the welcome screen or the route view.
        match flow {
            Flow::Open => {}
            // The welcome screen is still `Some`. It shows again at the step it had. The theme
            // preview of the wizard goes back to the saved theme, as after Finish.
            Flow::Welcome => {
                self.wizard = None;
                self.switched_at = ui.input(|i| i.time);
                s.apply_theme(ui.ctx());
            }
            Flow::Done => {
                if let Some(wizard) = self.wizard.take() {
                    wizard.finish(s, ui.ctx());
                }
                self.welcome = None;
            }
        }
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

    /// Take the result of the Nexum map list fetch, if the thread sent it. The Nexum page shows
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
                filaments_toggle(ui, s);
                ui.separator();
                ui.label(theme::header_text("Show up to"));
                count_stepper(ui, s);
                ui.label(theme::header_text("routes"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The right-to-left layout draws the Settings button first, so it sits at the right edge.
                if ui.app_button(icon_label(ui, icons::GEAR.fill(), "Settings")).clicked() {
                    self.settings = Some(SettingsForm::new(s, self.settings_page));
                }
                let avoid = match s.settings.avoid.len() + s.closed.len() {
                    0 => "Avoid".to_string(),
                    n => format!("Avoid ({n})"),
                };
                let tip = "Systems, regions and wormholes that routes avoid";
                let button = ui.add_enabled(!locked, AppButton::new(avoid)).on_hover_text(tip);
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
                        ui.label(RichText::new(stop.to_string().to_uppercase()).color(color).size(11.0).extra_letter_spacing(1.0));
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
            s.save();
        }
    };
    let plus = |ui: &mut Ui, s: &mut Session| {
        if ui.add_enabled(s.settings.top < MAX_TOP, AppButton::new("+")).clicked() {
            s.settings.top += 1;
            s.recompute();
            s.save();
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
    // With no capital or no bridge, the toggle has no use, so it is hidden.
    if s.settings.rules.capital.is_none() || s.bridges.is_empty() {
        return;
    }
    let blocked = s.settings.rules.blocked_reason();
    let bridges = match &blocked {
        Some(_) => "off (not allowed)",
        None => on_off(s.settings.bridges),
    };
    // A hull that cannot use bridges makes the button off, and the tooltip gives the reason.
    let button = AppButton::new(format!("Jump bridges: {bridges}")).selected(s.settings.bridges && blocked.is_none());
    let response = ui.add_enabled(blocked.is_none(), button);
    if response.clicked() {
        s.settings.bridges = !s.settings.bridges;
        s.recompute();
        s.save();
    }
    if let Some(reason) = blocked {
        response.on_disabled_hover_text(reason);
    }
}

fn wormholes_toggle(ui: &mut Ui, s: &mut Session) {
    if ui.add(AppButton::new(format!("Wormholes: {}", on_off(s.settings.wormholes))).selected(s.settings.wormholes)).clicked() {
        s.settings.wormholes = !s.settings.wormholes;
        s.recompute();
        s.save();
    }
}

fn filaments_toggle(ui: &mut Ui, s: &mut Session) {
    // A filament jump ends in Pochven, so a blocked Pochven turns the toggle off, like a hull that cannot bridge.
    // A click turns the filaments on or off. The button shows Avoid, which the Engine page sets, and a click then turns off.
    let blocked = s.settings.pochven_blocked();
    let state = if blocked { "off (not allowed)" } else { s.settings.filaments.state() };
    let button = AppButton::new(format!("Filaments: {state}")).selected(s.settings.filaments != Stance::Block && !blocked);
    let tip = "Allow routes to use a filament into Pochven. A click turns it on or off. The Engine page can also set Avoid, which uses a filament only when a trip has no other way.";
    let response = ui.add_enabled(!blocked, button);
    if response.clicked() {
        s.settings.filaments = s.settings.filaments.toggled();
        s.recompute();
        s.save();
    }
    if blocked {
        response.on_disabled_hover_text(settings_window::POCHVEN_OFF);
    } else {
        response.on_hover_text(tip);
    }
}

fn optimize_box(ui: &mut Ui, s: &mut Session) {
    if ui.app_checkbox(&mut s.settings.optimize, "Optimize order").on_hover_text("Visit each system once, in the cheapest order").changed()
    {
        s.recompute();
        s.save();
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
        s.save();
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
            let tip = if has_pilot { "Send this route to a character ingame." } else { "Log in to send this route to a character ingame." };
            start = ui.add(button).on_hover_text(tip).clicked();
            ui.add_space(6.0);
        }
        let button = Button::new(RichText::new("Copy route").size(11.0)).small();
        copy = ui.add(button).on_hover_text("Copy the route as tab-separated text, for a Slack table").clicked();
        ui.add_space(6.0);
        ui.label(RichText::new(&info).color(theme::TEXT_DIM).size(11.0));
    };
    theme::panel_with(ui, &title, header, true, |ui| {
        let uni = &s.uni;
        // A selectable label takes the right click for itself, so the row menu would not open
        // over a text. The table has no text to select.
        ui.style_mut().interaction.selectable_labels = false;
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
                    let link = step.checked_sub(1).map(|i| LinkView::of(uni, &s.settings.rules, route.path.edges[i]));
                    // Oxanium and the egui fallback fonts have no right arrow glyph. Oxanium has "»".
                    let via = link.as_ref().map(|l| l.label(s.now).replace('\u{2192}', "»")).unwrap_or_default();
                    let via_color = link.as_ref().map_or(theme::TEXT_DIM, |l| theme::link_color(l.hop()));
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
                                RichText::new(stop.to_string().to_uppercase())
                                    .color(theme::accent(ui))
                                    .size(11.0)
                                    .extra_letter_spacing(1.0),
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
                    widgets::context_menu(&row.response(), |ui| {
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
                        // The link into this step, if it is a wormhole.
                        let hole = step.checked_sub(1).and_then(|i| match &uni.graph[route.path.edges[i]] {
                            Link::Wormhole(w) => Some(ClosedHole::of(w).with_start(uni.system(route.path.nodes[i]).id)),
                            _ => None,
                        });
                        if let Some(hole) = hole {
                            ui.separator();
                            let tip = "Ignore this connection until the next app restart. Use it when the wormhole has collapsed.";
                            if ui.app_button("Mark wormhole closed").on_hover_text(tip).clicked() {
                                toggle = Some(Toggle::Close(hole));
                                ui.close();
                            }
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
        Some(Toggle::Close(hole)) => s.close_link(hole),
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
/// A source that the server rate limits is amber, also when its cache is young.
fn source_state(s: &Session, source: SourceId) -> (Color32, String) {
    const FRESH_SECS: u64 = 15 * 60;
    let now = router_core::wormhole::now();
    let fetched = s.shortcuts.sources.iter().find(|(id, _)| *id == source).map(|&(_, at)| at);
    let limited = s.shortcuts.warnings.iter().any(
        |w| matches!(w, router_core::sources::OverlayWarning::Fetch { source: id, error: FetchError::RateLimit, .. } if *id == source),
    );
    if limited {
        let text = fetched.map_or_else(|| "rate limited".into(), |at| format!("rate limited, {}", age_text(at, now)));
        return (theme::WARN, text);
    }
    match fetched {
        Some(at) if now.saturating_sub(at) <= FRESH_SECS => (theme::OK, age_text(at, now)),
        Some(at) => (theme::WARN, age_text(at, now)),
        None if source == SourceId::Nexum && s.cfg.nexum.complete().is_none() => (theme::TEXT_DIM, "not set".into()),
        None if source == SourceId::EveMetro && !metro_on(s) => (theme::TEXT_DIM, "not set".into()),
        None => (theme::TEXT_DIM, "no data".into()),
    }
}

/// True if the router reads EVE Metro: the source is on, the key and the character are set,
/// and the app has an EVE client ID.
fn metro_on(s: &Session) -> bool {
    s.cfg.eve_metro.gated(s.pilots.shows_characters()).complete().is_some()
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
            for (label, count, on) in [
                ("Nexum", sc.nexum, true),
                ("EVE Metro", sc.metro, true),
                ("Thera", sc.thera, settings.hubs.thera != Stance::Block),
                ("Turnur", sc.turnur, settings.hubs.turnur != Stance::Block),
            ] {
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
        let failed = s.log.last().is_some_and(|e| !e.kind.is_ok());
        let log_text = RichText::new("Log").small().color(if failed { theme::WARN } else { theme::TEXT });
        if ui
            .add(Button::new(log_text).small())
            .on_hover_text(if failed { "Show the fetch log. The last fetch failed." } else { "Show the fetch log in a window" })
            .clicked()
        {
            *log_open = true;
        }
        if ui.add(Button::new(RichText::new("Refresh").small()).small()).on_hover_text("Fetch the wormholes now (F5)").clicked() {
            s.refresh_now(Instant::now());
        }
        ui.add_space(6.0);
        sync_status(ui, s);
        // A dry run saves nothing for the whole process, so its note stays in place of the status.
        let text = if s.is_dry_run() {
            RichText::new(crate::app::DRY_RUN_NOTE).color(theme::WARN)
        } else {
            RichText::new(s.status.text()).color(theme::TEXT_DIM)
        };
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(Label::new(text).truncate());
        });
    });
}

/// The wormhole sources and the age of their data, for example "Nexum 2 min ago" after a dot.
/// The dot is green for data from the last 15 minutes, else amber. A source with no data is gray.
/// A rate limited source is amber.
fn sync_status(ui: &mut Ui, s: &Session) {
    // The ages change with time, so the bar draws again each minute.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(60));
    // The layout is right to left: EVE-Scout goes first, so Nexum shows on its left. EVE Metro
    // shows only when the router reads it, or when it has data.
    let metro_shows = metro_on(s) || s.shortcuts.sources.iter().any(|(id, _)| *id == SourceId::EveMetro);
    let sources = [SourceId::EveScout, SourceId::Nexum, SourceId::EveMetro];
    for source in sources.into_iter().filter(|&source| source != SourceId::EveMetro || metro_shows) {
        let (dot, text) = source_state(s, source);
        ui.label(RichText::new(&text).color(theme::TEXT_DIM).small());
        ui.label(RichText::new(source.label()).color(theme::TEXT).small());
        // A painted dot: the fonts have no circle glyph.
        let (rect, response) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, dot);
        response.on_hover_text("Green: data from the last 15 minutes. Amber: older data, or the server limits the rate. Gray: no data.");
        ui.add_space(10.0);
    }
}

/// The loading screen with the current step, or the load error. A download shows a progress bar.
/// With `update` set, it asks to install that version instead of the step. The result is the
/// answer: `Some(true)` installs, `Some(false)` skips.
pub fn splash(
    ui: &mut Ui,
    error: Option<&str>,
    step: Option<&crate::app::Step>,
    update: Option<&crate::app_update::Available>,
) -> Option<bool> {
    let mut answer = None;
    let frame = Frame::new().fill(theme::BG);
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 80.0).max(0.0));
            ui.allocate_ui(vec2(420.0, 190.0), |ui| {
                panel(ui, "EVE Router", "", false, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        match (error, update) {
                            (None, Some(update)) => {
                                ui.label(
                                    RichText::new("A new version is available").family(theme::bold()).size(16.0).color(Color32::WHITE),
                                );
                                ui.add_space(4.0);
                                ui.label(RichText::new(format!("Current version: {}", env!("CARGO_PKG_VERSION"))).color(theme::TEXT_DIM));
                                ui.label(RichText::new(format!("Available: {}", update.version)).color(theme::TEXT));
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    // Center the two buttons: the row takes only its own width.
                                    ui.add_space(((ui.available_width() - 250.0) / 2.0).max(0.0));
                                    if ui.add(AppButton::new("Install and restart").primary()).clicked() {
                                        answer = Some(true);
                                    }
                                    if ui.app_button("Skip this time").clicked() {
                                        answer = Some(false);
                                    }
                                });
                            }
                            (None, None) => {
                                ui.add(egui::Spinner::new().size(28.0).color(theme::accent(ui)));
                                ui.add_space(8.0);
                                let text = step.map_or_else(|| "Starting…".to_string(), crate::app::Step::text);
                                ui.label(RichText::new(text).color(theme::TEXT));
                                if let Some(fraction) = step.and_then(crate::app::Step::fraction) {
                                    ui.add_space(6.0);
                                    ui.add(egui::ProgressBar::new(fraction).fill(theme::accent(ui)).desired_width(300.0));
                                }
                            }
                            (Some(e), _) => {
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
    answer
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
    fn a_rate_limited_source_gets_an_amber_dot_also_with_a_young_cache() {
        use router_core::config::Config;
        use router_core::labels::Shortcuts;
        use router_core::sources::OverlayWarning;
        use router_core::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default(), &Default::default());
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Session::new(std::sync::Arc::new(uni), settings, Config::default(), tmp.path().join("eve-router.json"), shortcuts);
        let now = router_core::wormhole::now();
        s.shortcuts.sources = vec![(SourceId::EveMetro, now), (SourceId::EveScout, now)];
        assert_eq!(source_state(&s, SourceId::EveMetro).0, theme::OK);
        let limited = |source, error| OverlayWarning::Fetch { source, error, cached_at: Some(now) };
        s.shortcuts.warnings =
            vec![limited(SourceId::EveMetro, FetchError::RateLimit), limited(SourceId::EveScout, FetchError::Offline("down".into()))];
        // The limited source is amber with a young cache. Another failure keeps the age rule.
        assert_eq!(source_state(&s, SourceId::EveMetro), (theme::WARN, "rate limited, just now".to_string()));
        assert_eq!(source_state(&s, SourceId::EveScout).0, theme::OK);
        // With no data, the text still says why.
        s.shortcuts.sources.clear();
        assert_eq!(source_state(&s, SourceId::EveMetro), (theme::WARN, "rate limited".to_string()));
    }

    #[test]
    fn the_splash_draws_every_step_and_the_error() {
        use crate::app::Step;
        use crate::app_update::Progress as Update;
        use router_core::startup::Progress;
        let steps = [
            None,
            Some(Step::Update(Update::Checking)),
            Some(Step::Update(Update::Downloading { done: 5, total: 10 })),
            Some(Step::Load(Progress::CheckingSde)),
            Some(Step::Load(Progress::DownloadingSde { done: 5, total: 10 })),
            Some(Step::Load(Progress::LoadingMap)),
        ];
        // The app registers its fonts and theme first: the splash uses the bold font.
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let draw = |error: Option<&str>, step: Option<&Step>, update: Option<&crate::app_update::Available>| {
            // The frame output holds texture changes. A test with no renderer drops them.
            let mut output = ctx.run_ui(Default::default(), |ui| _ = splash(ui, error, step, update));
            output.textures_delta.clear();
        };
        for step in &steps {
            draw(None, step.as_ref(), None);
        }
        draw(Some("Cannot read the config"), None, None);
        let update = crate::app_update::Available { tag: "v9.9.9".into(), version: "9.9.9".into(), notes: None };
        draw(None, Some(&Step::Update(Update::Checking)), Some(&update));
    }

    #[test]
    fn the_route_list_gives_way_to_the_table() {
        // Room for all rows of a short list.
        assert_eq!(route_list_height(2, 500.0, false), 2.0 * route_pitch(false) + panel_chrome(false));
        // A long list stops at the rows that fit without a scroll.
        assert_eq!(route_list_height(9, 900.0, false), LIST_ROWS as f32 * route_pitch(false) + panel_chrome(false));
        // Three routes or fewer: all rows show, even with little room.
        assert_eq!(route_list_height(3, 40.0, false), 3.0 * route_pitch(false) + panel_chrome(false));
        // Little room and more than three routes: one row and a half, so the table keeps its height.
        assert_eq!(route_list_height(5, 40.0, false), 1.5 * route_pitch(false) + panel_chrome(false));
        // No routes: the empty text fits.
        assert_eq!(route_list_height(0, 500.0, false), route_pitch(false) + panel_chrome(false));

        // In compact mode:
        assert_eq!(route_list_height(2, 500.0, true), 2.0 * route_pitch(true) + panel_chrome(true));
        assert_eq!(route_list_height(9, 900.0, true), LIST_ROWS as f32 * route_pitch(true) + panel_chrome(true));
        assert_eq!(route_list_height(5, 40.0, true), 1.5 * route_pitch(true) + panel_chrome(true));
    }

    /// The texts of a frame, with the position and the size of each.
    type Texts = Vec<(String, egui::Pos2, egui::Vec2)>;

    /// A session for the wizard tests: a config in a temporary directory, an EVE client ID with
    /// the characters Alice and Bo, and an EVE Metro key. Thus the wizard has all four pages.
    fn wizard_session(setup_due: bool) -> (Session, tempfile::TempDir) {
        use router_core::config::{ApiKey, Config};
        use router_core::esi::pilots::Pilots;
        use router_core::labels::Shortcuts;
        use router_core::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default(), &Default::default());
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Session::new(std::sync::Arc::new(uni), settings, Config::default(), tmp.path().join("eve-router.json"), shortcuts);
        s.pilots = Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[(1, "Alice Ander"), (2, "Bo Berg")]);
        s.cfg.eve_metro.key = Some(ApiKey("metro_test_key".into()));
        s.setup_due = setup_due;
        (s, tmp)
    }

    /// The view of the session, with the wizard started as the welcome screen starts it: the pilot
    /// picks "I'll configure things myself, thanks." (`Preset::Custom`), and the wizard has all the
    /// pages that the session allows. The portraits of Alice and Bo need no download, so no test needs the network.
    /// The change has no time, so no click guard starts, and a test can click the wizard at once.
    fn wizard_view(s: &Session) -> View {
        let mut view = View::new(s);
        view.pilots.portraits = crate::pilots_view::Portraits::without_download(&[1, 2]);
        let WelcomeFlow::Start(preset, answers) = view.welcome.as_mut().expect("the start asks for a setup").choose(Preset::Custom) else {
            panic!("Custom starts the wizard at once");
        };
        view.start_wizard(s, preset, answers, f64::NEG_INFINITY);
        view
    }

    /// One frame of `view.show` in a window of `size`, with these input events.
    fn show_output(ctx: &egui::Context, view: &mut View, s: &mut Session, size: egui::Vec2, events: Vec<egui::Event>) -> egui::FullOutput {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| view.show(ui, s));
        // The frame output holds texture changes. A test with no renderer drops them.
        output.textures_delta.clear();
        output
    }

    /// One frame of `view.show` in the smallest window, with these input events.
    fn show_frame(ctx: &egui::Context, view: &mut View, s: &mut Session, events: Vec<egui::Event>) -> Texts {
        crate::test_ui::drawn_texts(&show_output(ctx, view, s, crate::setup_wizard::SMALLEST_WINDOW, events))
    }

    /// The drawn text that is exactly `text`.
    fn find<'a>(drawn: &'a Texts, text: &str) -> Option<&'a (String, egui::Pos2, egui::Vec2)> {
        drawn.iter().find(|(t, ..)| t == text)
    }

    /// The bottom edge of the drawn text `text`.
    fn bottom_of(drawn: &Texts, text: &str) -> f32 {
        let (_, pos, size) = find(drawn, text).unwrap_or_else(|| panic!("\"{text}\" is drawn"));
        pos.y + size.y
    }

    /// A click on the center of the drawn text `text`: the pointer moves there and pushes the
    /// button in one frame, and releases it in the next frame.
    fn click(ctx: &egui::Context, view: &mut View, s: &mut Session, text: &str) {
        let drawn = show_frame(ctx, view, s, vec![]);
        let (_, pos, size) = find(&drawn, text).unwrap_or_else(|| panic!("\"{text}\" is drawn"));
        let at = *pos + *size / 2.0;
        let button =
            |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        show_frame(ctx, view, s, vec![egui::Event::PointerMoved(at), button(true)]);
        show_frame(ctx, view, s, vec![button(false)]);
    }

    #[test]
    fn the_welcome_shows_first_when_the_start_asks_for_it() {
        let (mut s, _tmp) = wizard_session(true);
        let mut view = View::new(&s);
        assert!(view.welcome.is_some() && view.wizard.is_none());
        let mut run = Run::new();
        let drawn = run.texts(&mut view, &mut s);
        for text in ["EVE Router", "Where do you live?", "I live in highsec.", "I'll configure things myself, thanks."] {
            assert!(find(&drawn, text).is_some(), "\"{text}\" is drawn");
        }
        assert!(find(&drawn, "Essentials").is_none(), "no wizard page");
        let (s, _tmp) = wizard_session(false);
        let view = View::new(&s);
        assert!(view.welcome.is_none() && view.wizard.is_none());
    }

    #[test]
    fn the_wizard_fits_the_smallest_window() {
        use crate::setup_wizard::{FRAME_ROOM, SMALLEST_WINDOW, WizardPage};
        let (mut s, _tmp) = wizard_session(true);
        let mut view = wizard_view(&s);
        let ctx = crate::test_ui::ctx();
        // The last texts of the body on each page.
        let pages = [
            (WizardPage::Essentials, vec!["Turnur", "+ Add character"]),
            (WizardPage::Alliance, vec!["Import"]),
            (WizardPage::Nexum, vec!["Skip connections with no known signatures"]),
            (WizardPage::EveMetro, vec!["Bo Berg"]),
        ];
        for (index, (page, last_texts)) in pages.iter().enumerate() {
            assert_eq!(view.wizard.as_ref().unwrap().page(), *page);
            // The first frame lays the page out.
            show_frame(&ctx, &mut view, &mut s, vec![]);
            let drawn = show_frame(&ctx, &mut view, &mut s, vec![]);
            let next = if index + 1 == pages.len() { "Finish" } else { "Next" };
            // The footer sits at the bottom of the screen. The page ends at the footer, less the margin of the page.
            let footer = egui::PanelState::load(&ctx, egui::Id::new("wizard-footer")).expect("the footer is drawn").outer_rect;
            let page_end = footer.top() - f32::from(WIZARD_MARGIN);
            for button in ["Back", "Skip", next] {
                let bottom = bottom_of(&drawn, button);
                assert!(bottom <= SMALLEST_WINDOW.y && bottom <= footer.bottom(), "{page:?}: \"{button}\" is on the screen");
            }
            let back_top = find(&drawn, "Back").unwrap().1.y;
            for text in last_texts {
                let bottom = bottom_of(&drawn, text);
                assert!(bottom < back_top && bottom <= page_end, "{page:?}: \"{text}\" ends at {bottom}, the page ends at {page_end}");
            }
            // The room of the frame: the steps row puts the page lower than the top of the screen,
            // and the footer takes the bottom. The page tests in setup_wizard.rs use FRAME_ROOM.
            if *page == WizardPage::Alliance {
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SMALLEST_WINDOW);
                let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
                let mut output = ctx.run_ui(input, |ui| view.wizard.as_mut().unwrap().body(ui, &mut s, &mut view.pilots));
                output.textures_delta.clear();
                let offset = bottom_of(&drawn, "Import") - bottom_of(&crate::test_ui::drawn_texts(&output), "Import");
                let room = offset + (SMALLEST_WINDOW.y - page_end);
                assert!(room <= FRAME_ROOM, "the frame takes {room} points: {offset} above the page, the page ends at {page_end}");
            }
            view.wizard.as_mut().unwrap().next();
        }
    }

    /// The page of the wizard, drawn in a window of `size` after three frames. The cards of the
    /// Essentials page read their natural heights in one frame and use them in the next frame.
    fn settled_wizard(size: egui::Vec2) -> (egui::FullOutput, egui::Rect) {
        let (mut s, _tmp) = wizard_session(true);
        let mut view = wizard_view(&s);
        let ctx = crate::test_ui::ctx();
        let mut last = None;
        for _ in 0..3 {
            last = Some(show_output(&ctx, &mut view, &mut s, size, vec![]));
        }
        let footer = egui::PanelState::load(&ctx, egui::Id::new("wizard-footer")).expect("the footer is drawn").outer_rect;
        (last.expect("three frames ran"), footer)
    }

    /// The cards of the Essentials page, in the order of drawing: Characters, Updates, Theme and Features.
    fn essentials_cards(output: &egui::FullOutput) -> [egui::Rect; 4] {
        let cards = crate::test_ui::card_rects(output);
        cards.try_into().unwrap_or_else(|cards: Vec<egui::Rect>| panic!("Characters, Updates, Theme and Features: {}", cards.len()))
    }

    #[test]
    fn the_first_page_is_one_centered_column() {
        use crate::setup_wizard::SMALLEST_WINDOW;
        let mut heights = Vec::new();
        for size in [SMALLEST_WINDOW, egui::vec2(SMALLEST_WINDOW.x, 1000.0)] {
            let (output, footer) = settled_wizard(size);
            let [characters, updates, theme, features] = essentials_cards(&output);
            // The column is 800 points wide and centered. Characters and Features have its width.
            for card in [characters, features] {
                assert!((card.width() - 800.0).abs() <= 1.0, "a card is {} points wide", card.width());
                assert!((card.center().x - size.x / 2.0).abs() <= 1.0, "the column is centered at {}", card.center().x);
            }
            // Updates and Theme share one row of the column, with a gap of 16 between them.
            assert!((updates.left() - characters.left()).abs() <= 1.0 && (theme.right() - characters.right()).abs() <= 1.0);
            assert!((theme.left() - updates.right() - 16.0).abs() <= 1.0, "the gap of the row is {}", theme.left() - updates.right());
            assert!((updates.top() - theme.top()).abs() <= 1.0, "the two cards of the row start together");
            assert!((updates.bottom() - theme.bottom()).abs() <= 1.0, "the two cards of the row end together");
            // The cards of the column have the gap of 16, and the column ends above the footer.
            assert!((updates.top() - characters.bottom() - 16.0).abs() <= 1.0);
            assert!((features.top() - updates.bottom() - 16.0).abs() <= 1.0);
            assert!(features.bottom() < footer.top(), "the cards are above the footer");
            heights.push([characters.height(), updates.height(), features.height()]);
        }
        // A taller window gives no card more height: the cards keep their own size.
        for (small, tall) in heights[0].iter().zip(heights[1]) {
            assert!((small - tall).abs() <= 1.0, "a card changed its height from {small} to {tall}");
        }
    }

    #[test]
    fn the_footer_keeps_a_margin_of_32() {
        use crate::setup_wizard::SMALLEST_WINDOW;
        for size in [SMALLEST_WINDOW, egui::vec2(1500.0, 800.0)] {
            let (output, footer) = settled_wizard(size);
            // The footer buttons keep the margin: Back at the left, the primary button at the right.
            let buttons: Vec<egui::Rect> = crate::test_ui::drawn_shapes(&output)
                .into_iter()
                .filter_map(|shape| if let egui::epaint::Shape::Rect(r) = shape { Some(r.rect) } else { None })
                .filter(|rect| rect.top() >= footer.top() && rect.bottom() <= footer.bottom() && rect.width() < 200.0)
                .collect();
            let left = buttons.iter().map(|r| r.left()).fold(f32::MAX, f32::min);
            let right_edge = buttons.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
            assert!((left - 32.0).abs() <= 1.0, "the footer starts at {left}");
            assert!((right_edge - (size.x - 32.0)).abs() <= 1.0, "the footer ends at {right_edge}");
        }
    }

    /// Review focus: no write before Finish, also when the followed pilot flies a new ship.
    #[test]
    fn a_wizard_frame_writes_no_file() {
        use router_core::esi::client::Ship;
        use router_core::esi::pilots::PilotState;
        use router_core::settings::HullSource;
        let (mut s, tmp) = wizard_session(true);
        s.settings.hull_source = HullSource::Pilot(1);
        let ship = Ship { ship_type_id: 1, ship_item_id: 1, ship_name: "Test".into() };
        s.pilots.live.insert(1, PilotState { ship: Some(ship), ..Default::default() });
        let mut view = wizard_view(&s);
        let ctx = crate::test_ui::ctx();
        for _ in 0..5 {
            show_frame(&ctx, &mut view, &mut s, vec![]);
        }
        assert!(tmp.path().read_dir().unwrap().next().is_none(), "no file on the disk");
        assert!(view.wizard.is_some());
        // Finish writes the config one time.
        for _ in 0..3 {
            view.wizard.as_mut().unwrap().next();
        }
        click(&ctx, &mut view, &mut s, "Finish");
        assert!(view.wizard.is_none(), "Finish ends the wizard");
        assert!(tmp.path().join("eve-router.json").exists());
    }

    #[test]
    fn the_wizard_draws_no_status_bar_in_a_dry_run() {
        let (mut s, _tmp) = wizard_session(true);
        s.set_dry_run();
        let mut view = wizard_view(&s);
        let drawn = show_frame(&crate::test_ui::ctx(), &mut view, &mut s, vec![]);
        assert!(find(&drawn, crate::app::DRY_RUN_NOTE).is_none());
        assert!(find(&drawn, Status::Ready.text()).is_none());
    }

    /// Review focus: a Settings window would keep its old state after Finish, so it never opens
    /// or draws while the wizard runs.
    #[test]
    fn the_wizard_shows_no_settings_window() {
        let (mut s, tmp) = wizard_session(true);
        let mut view = wizard_view(&s);
        // A form from some other path. The wizard drops it, and draws no window.
        view.settings = Some(SettingsForm::new(&s, Page::default()));
        let ctx = crate::test_ui::ctx();
        let keys = [(Key::F, Modifiers::COMMAND), (Key::F5, Modifiers::NONE), (Key::Escape, Modifiers::NONE)];
        let events = keys
            .iter()
            .map(|&(key, modifiers)| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers })
            .collect();
        for events in [vec![], events, vec![]] {
            let drawn = show_frame(&ctx, &mut view, &mut s, events);
            assert!(find(&drawn, "SETTINGS").is_none(), "the Settings window is drawn");
            assert!(view.settings.is_none());
        }
        // The top bar with its Settings button is not drawn. The Essentials page holds no text with that word.
        let drawn = show_frame(&ctx, &mut view, &mut s, vec![]);
        assert!(!drawn.iter().any(|(t, ..)| t.contains("Settings")), "the Settings button is drawn");
        assert!(tmp.path().read_dir().unwrap().next().is_none(), "no file on the disk");
    }

    /// Review focus: a "Remove <name>?" question from before the wizard must not come back after Finish.
    #[test]
    fn the_wizard_closes_an_open_remove_question() {
        let (mut s, _tmp) = wizard_session(true);
        let mut view = wizard_view(&s);
        view.pilots.characters_open = true;
        view.pilots.ask_remove(1);
        let ctx = crate::test_ui::ctx();
        // A window sizes itself in the first frame and draws nothing, so the test reads the second frame.
        show_frame(&ctx, &mut view, &mut s, vec![]);
        let drawn = show_frame(&ctx, &mut view, &mut s, vec![]);
        assert!(find(&drawn, "Alice Ander").is_some(), "the Characters window is open");
        assert!(!drawn.iter().any(|(t, ..)| t.starts_with("Remove")), "no Remove while the wizard runs");
        // After the wizard, the window offers Remove again, and the old question is gone.
        view.wizard.take().unwrap().finish(&mut s, &ctx);
        view.welcome = None;
        show_frame(&ctx, &mut view, &mut s, vec![]);
        let drawn = show_frame(&ctx, &mut view, &mut s, vec![]);
        assert!(find(&drawn, "Remove").is_some(), "the Characters window offers Remove");
        assert!(!drawn.iter().any(|(t, ..)| t.starts_with("Remove Alice Ander?")));
    }

    /// A run of frames in the smallest window with a clock. Each frame moves the clock by 0.02 s,
    /// so the guard time of the welcome screen (0.4 s) has a meaning. `wait` moves the clock more.
    struct Run {
        ctx: egui::Context,
        time: f64,
    }

    impl Run {
        /// A new context and a clock at zero.
        fn new() -> Run {
            Run { ctx: crate::test_ui::ctx(), time: 0.0 }
        }

        /// One frame of `view.show` with these input events.
        fn output(&mut self, view: &mut View, s: &mut Session, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.02;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::setup_wizard::SMALLEST_WINDOW);
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(self.time), events, ..Default::default() };
            let mut output = self.ctx.run_ui(input, |ui| view.show(ui, s));
            output.textures_delta.clear();
            output
        }

        /// The texts of one frame without input.
        fn texts(&mut self, view: &mut View, s: &mut Session) -> Texts {
            crate::test_ui::drawn_texts(&self.output(view, s, vec![]))
        }

        /// Two frames without input. The second one shows the settled layout.
        fn settled(&mut self, view: &mut View, s: &mut Session) -> Texts {
            self.texts(view, s);
            self.texts(view, s)
        }

        /// Let `secs` seconds pass.
        fn wait(&mut self, secs: f64) {
            self.time += secs;
        }

        /// Click the `nth` drawn text `text`, counted from the top and from the left: the pointer
        /// moves, pushes the button and releases it in three frames.
        fn click_nth(&mut self, view: &mut View, s: &mut Session, text: &str, nth: usize) {
            let drawn = self.texts(view, s);
            let mut found: Vec<_> = drawn.iter().filter(|(t, ..)| t == text).collect();
            found.sort_by(|a, b| (a.1.y, a.1.x).partial_cmp(&(b.1.y, b.1.x)).unwrap());
            let (_, pos, size) = **found.get(nth).unwrap_or_else(|| panic!("no \"{text}\" number {nth} is drawn"));
            let at = pos + size / 2.0;
            let button =
                |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.output(view, s, vec![egui::Event::PointerMoved(at)]);
            self.output(view, s, vec![button(true)]);
            self.output(view, s, vec![button(false)]);
        }

        /// Click the first drawn text `text`.
        fn click(&mut self, view: &mut View, s: &mut Session, text: &str) {
            self.click_nth(view, s, text, 0);
        }

        /// The center of the first text `text` in a frame without input.
        fn center(&mut self, view: &mut View, s: &mut Session, text: &str) -> egui::Pos2 {
            let drawn = self.texts(view, s);
            let (_, pos, size) = find(&drawn, text).unwrap_or_else(|| panic!("\"{text}\" is drawn"));
            *pos + *size / 2.0
        }

        /// A click at `at` in two frames: the pointer moves there and pushes the button in the
        /// first frame, and releases it in the second frame. Two calls make a double-click.
        fn tap(&mut self, view: &mut View, s: &mut Session, at: egui::Pos2) {
            let button =
                |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.output(view, s, vec![egui::Event::PointerMoved(at), button(true)]);
            self.output(view, s, vec![button(false)]);
        }

        /// The accent color of the window style.
        fn accent(&self) -> Color32 {
            self.ctx.global_style().visuals.hyperlink_color
        }

        /// The outer rect of the panel `id`, if a frame of this run drew it.
        fn panel(&self, id: &str) -> Option<egui::Rect> {
            egui::PanelState::load(&self.ctx, egui::Id::new(id)).map(|state| state.outer_rect)
        }
    }

    /// The pages of the running wizard.
    fn wizard_pages(view: &View) -> Vec<crate::setup_wizard::WizardPage> {
        view.wizard.as_ref().expect("the wizard runs").pages().to_vec()
    }

    #[test]
    fn a_preset_click_starts_the_wizard_with_its_pages() {
        use crate::setup_wizard::WizardPage;
        let (mut s, _tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in highsec.");
        assert!(view.wizard.is_none(), "the preset click only moves to step 2");
        assert_eq!(view.welcome.as_ref().unwrap().step(), Step::Travel);
        // Wait past the guard time of the welcome screen.
        run.wait(0.5);
        run.click(&mut view, &mut s, "Continue");
        assert_eq!(wizard_pages(&view), [WizardPage::Essentials]);
        assert_eq!(view.wizard.as_ref().unwrap().features(), Preset::Highsec.answers().features());
        let drawn = run.settled(&mut view, &mut s);
        assert!(find(&drawn, "Pochven").is_some() && find(&drawn, "Where do you live?").is_none(), "the wizard page shows");
        assert!(view.welcome.is_some(), "the welcome screen waits behind the wizard");
    }

    #[test]
    fn a_new_continue_builds_the_wizard_again() {
        use crate::setup_wizard::WizardPage::{Alliance, Essentials, EveMetro, Nexum};
        let (mut s, _tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in lowsec.");
        run.wait(0.5);
        run.click(&mut view, &mut s, "Continue");
        assert_eq!(wizard_pages(&view), [Essentials, Nexum]);
        // Edit a value on Essentials: Filaments starts on in lowsec, and the edit turns it off. The cards of
        // the page need two frames to settle. The wait is for the click guard of the change.
        run.wait(0.5);
        run.settled(&mut view, &mut s);
        run.click(&mut view, &mut s, "Filaments");
        assert!(!view.wizard.as_ref().unwrap().features().filaments, "the edit is made");
        // Back to the welcome screen, back to step 1 and the nullsec preset.
        run.click(&mut view, &mut s, "Back");
        assert!(view.wizard.is_none());
        run.wait(0.5);
        run.click(&mut view, &mut s, "Back");
        assert_eq!(view.welcome.as_ref().unwrap().step(), Step::Preset);
        run.wait(0.5);
        run.click(&mut view, &mut s, "I live in nullsec.");
        run.wait(0.5);
        run.click_nth(&mut view, &mut s, "Yes", 2);
        run.click(&mut view, &mut s, "Continue");
        assert_eq!(wizard_pages(&view), [Essentials, Alliance, Nexum, EveMetro], "the pages follow the new answers");
        assert_eq!(view.wizard.as_ref().unwrap().features(), Preset::Nullsec.answers().features());
        assert!(view.wizard.as_ref().unwrap().features().filaments, "the old edit is gone");
    }

    #[test]
    fn back_from_essentials_returns_to_the_right_welcome_step() {
        let (mut s, _tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in highsec.");
        run.wait(0.5);
        // Change the Wormholes answer from No to Yes.
        run.click_nth(&mut view, &mut s, "Yes", 0);
        let changed = Answers { wormholes: crate::welcome::Comfort::Yes, ..Preset::Highsec.answers() };
        assert_eq!(view.welcome.as_ref().unwrap().answers(), changed);
        run.click(&mut view, &mut s, "Continue");
        assert!(view.wizard.is_some());
        run.wait(0.5);
        run.click(&mut view, &mut s, "Back");
        assert!(view.wizard.is_none(), "Back on Essentials ends the wizard");
        let welcome = view.welcome.as_ref().unwrap();
        assert_eq!((welcome.step(), welcome.answers(), welcome.preset()), (Step::Travel, changed, Preset::Highsec));
        let drawn = run.settled(&mut view, &mut s);
        assert!(find(&drawn, "Are you comfortable travelling through Pochven?").is_some(), "step 2 shows again");
        // After "configure myself", there is no step 2 to return to.
        let (mut s, _tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I'll configure things myself, thanks.");
        assert!(view.wizard.is_some());
        run.wait(0.5);
        run.click(&mut view, &mut s, "Back");
        assert!(view.wizard.is_none());
        assert_eq!(view.welcome.as_ref().unwrap().step(), Step::Preset);
        let drawn = run.settled(&mut view, &mut s);
        assert!(find(&drawn, "Where do you live?").is_some(), "step 1 shows again");
    }

    #[test]
    fn finish_ends_both_screens() {
        let (mut s, tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I'll configure things myself, thanks.");
        assert!(view.wizard.is_some() && view.welcome.is_some());
        for _ in 0..3 {
            view.wizard.as_mut().unwrap().next();
        }
        run.wait(0.5);
        run.click(&mut view, &mut s, "Finish");
        assert!(view.wizard.is_none() && view.welcome.is_none(), "Finish ends the wizard and the welcome screen");
        assert!(tmp.path().join("eve-router.json").exists(), "Finish writes the config");
    }

    /// Review focus: the footers of the welcome screen and the wizard are at the same place. On the
    /// highsec preset the wizard has one page, so the button under Continue ends the setup.
    #[test]
    fn a_double_click_on_continue_does_not_end_the_setup() {
        use crate::setup_wizard::WizardPage;
        let (mut s, tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in highsec.");
        run.wait(0.5);
        let at = run.center(&mut view, &mut s, "Continue");
        run.tap(&mut view, &mut s, at);
        assert_eq!(wizard_pages(&view), [WizardPage::Essentials], "Continue starts the wizard");
        // The frame of the change puts the guard on top, so the click of the next frame hits it.
        assert_eq!(run.ctx.layer_id_at(at).map(|layer| layer.order), Some(egui::Order::Foreground), "the guard is on top");
        // The second click of the double-click, in the next frame and again 0.1 s later.
        run.tap(&mut view, &mut s, at);
        run.wait(0.1);
        run.tap(&mut view, &mut s, at);
        run.settled(&mut view, &mut s);
        assert!(view.welcome.is_some(), "the setup did not end");
        assert_eq!(view.wizard.as_ref().map(|w| w.page()), Some(WizardPage::Essentials), "the wizard stays on its first page");
        assert!(tmp.path().read_dir().unwrap().next().is_none(), "no file on the disk");
        // After the guard time, a click at the same place counts. The guard asks for a frame at
        // the end of its time, and this frame lifts it.
        run.wait(0.6);
        run.texts(&mut view, &mut s);
        run.tap(&mut view, &mut s, at);
        assert!(view.wizard.is_none() && view.welcome.is_none(), "the click ends the setup");
        assert!(tmp.path().join("eve-router.json").exists(), "Finish writes the config");
    }

    /// The view of a lowsec setup without the subscription answer, on the Essentials page, after
    /// the guard time of the change. The wizard has two pages: Essentials and Nexum.
    fn lowsec_on_essentials(run: &mut Run, s: &mut Session) -> View {
        use crate::setup_wizard::WizardPage::{Essentials, Nexum};
        let mut view = View::new(s);
        run.click(&mut view, s, "I live in lowsec.");
        run.wait(0.5);
        run.click(&mut view, s, "Continue");
        assert_eq!(wizard_pages(&view), [Essentials, Nexum], "the Metro gates are off");
        run.wait(0.5);
        run.settled(&mut view, s);
        view
    }

    /// Review focus: Next and Finish are at the same place. A double-click on `button` on Essentials
    /// must not press Finish on the Nexum page. After the guard time, a click there finishes.
    fn assert_a_double_click_stays_on_nexum(button: &str) {
        use crate::setup_wizard::WizardPage;
        let (mut s, tmp) = wizard_session(true);
        // No EVE Metro key, so the lowsec wizard has no EVE Metro page.
        s.cfg.eve_metro.key = None;
        let mut run = Run::new();
        let mut view = lowsec_on_essentials(&mut run, &mut s);
        let at = run.center(&mut view, &mut s, button);
        run.tap(&mut view, &mut s, at);
        assert_eq!(view.wizard.as_ref().map(|w| w.page()), Some(WizardPage::Nexum), "{button} moves to the Nexum page");
        // The second click of the double-click, in the next frame and again 0.1 s later.
        run.tap(&mut view, &mut s, at);
        run.wait(0.1);
        run.tap(&mut view, &mut s, at);
        run.settled(&mut view, &mut s);
        assert_eq!(view.wizard.as_ref().map(|w| w.page()), Some(WizardPage::Nexum), "{button}: the wizard stays on the Nexum page");
        assert!(tmp.path().read_dir().unwrap().next().is_none(), "{button}: no file on the disk");
        // After the guard time and the frame that the guard asks for, Finish at the same place counts.
        run.wait(0.6);
        let finish = run.center(&mut view, &mut s, "Finish");
        run.tap(&mut view, &mut s, finish);
        assert!(view.wizard.is_none() && view.welcome.is_none(), "{button}: Finish ends the setup");
        assert!(tmp.path().join("eve-router.json").exists(), "{button}: Finish writes the config");
    }

    #[test]
    fn a_double_click_on_next_does_not_end_the_setup() {
        assert_a_double_click_stays_on_nexum("Next");
    }

    #[test]
    fn a_double_click_on_skip_does_not_end_the_setup() {
        assert_a_double_click_stays_on_nexum("Skip");
    }

    /// Review focus: Back is at the same place on each page. A double-click on Back on page 2
    /// goes to page 1 only and does not reach the welcome screen.
    #[test]
    fn a_double_click_on_back_on_page_2_stays_in_the_wizard() {
        use crate::setup_wizard::WizardPage;
        let (mut s, _tmp) = wizard_session(true);
        s.cfg.eve_metro.key = None;
        let mut run = Run::new();
        let mut view = lowsec_on_essentials(&mut run, &mut s);
        run.click(&mut view, &mut s, "Next");
        run.wait(0.5);
        let at = run.center(&mut view, &mut s, "Back");
        run.tap(&mut view, &mut s, at);
        assert_eq!(view.wizard.as_ref().map(|w| w.page()), Some(WizardPage::Essentials), "Back moves to page 1");
        run.tap(&mut view, &mut s, at);
        run.wait(0.1);
        run.tap(&mut view, &mut s, at);
        run.settled(&mut view, &mut s);
        assert_eq!(view.wizard.as_ref().map(|w| w.page()), Some(WizardPage::Essentials), "the wizard stays on page 1");
    }

    /// Review focus: without a client ID the Essentials page is one column in the middle, below the
    /// Custom button.
    #[test]
    fn a_double_click_on_custom_changes_no_feature() {
        let (mut s, tmp) = wizard_session(true);
        s.pilots = router_core::esi::pilots::Pilots::offline(tmp.path().join("active-route.json"));
        let mut view = View::new(&s);
        let mut run = Run::new();
        let at = run.center(&mut view, &mut s, "I'll configure things myself, thanks.");
        run.tap(&mut view, &mut s, at);
        let start = view.wizard.as_ref().expect("Custom starts the wizard").features();
        run.settled(&mut view, &mut s);
        run.wait(0.04);
        run.tap(&mut view, &mut s, at);
        // The guard took the second click. In the smallest window no switch is at this point, so
        // the check of the switches alone cannot see a click that goes through.
        assert_eq!(run.ctx.layer_id_at(at).map(|layer| layer.order), Some(egui::Order::Foreground), "the guard is on top");
        run.settled(&mut view, &mut s);
        assert_eq!(view.wizard.as_ref().expect("the wizard runs").features(), start, "no switch changed");
    }

    /// Review focus: the Back buttons of the wizard and of the welcome screen are at the same place.
    #[test]
    fn a_double_click_on_the_wizard_back_stays_on_step_2() {
        let (mut s, _tmp) = wizard_session(true);
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in highsec.");
        run.wait(0.5);
        run.click(&mut view, &mut s, "Continue");
        run.wait(0.5);
        let at = run.center(&mut view, &mut s, "Back");
        run.tap(&mut view, &mut s, at);
        assert!(view.wizard.is_none(), "Back on Essentials ends the wizard");
        // A frame between the two clicks, as a pointer move makes in the app. Thus egui finds the
        // welcome Back under the pointer.
        run.texts(&mut view, &mut s);
        run.wait(0.04);
        run.tap(&mut view, &mut s, at);
        assert_eq!(view.welcome.as_ref().unwrap().step(), Step::Travel, "the second click does nothing");
        // After the guard time and the frame that the guard asks for, Back on step 2 goes to step 1.
        run.wait(0.5);
        run.texts(&mut view, &mut s);
        run.tap(&mut view, &mut s, at);
        assert_eq!(view.welcome.as_ref().unwrap().step(), Step::Preset);
    }

    /// Review focus: Back to the welcome screen drops the theme preview, as Finish does.
    #[test]
    fn back_to_the_welcome_screen_drops_the_theme_preview() {
        use crate::theme::{FactionColors, FactionTheme};
        let (mut s, _tmp) = wizard_session(true);
        let saved = s.faction_theme().accent();
        assert_ne!(saved, FactionTheme::Gallente.accent());
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in highsec.");
        run.wait(0.5);
        run.click(&mut view, &mut s, "Continue");
        run.wait(0.5);
        run.settled(&mut view, &mut s);
        run.click(&mut view, &mut s, "Gallente");
        run.settled(&mut view, &mut s);
        assert_eq!(run.accent(), FactionTheme::Gallente.accent(), "the wizard shows the preview");
        run.wait(0.5);
        run.click(&mut view, &mut s, "Back");
        assert!(view.wizard.is_none());
        run.settled(&mut view, &mut s);
        assert_eq!(run.accent(), saved, "the welcome screen shows the saved theme");
        // A new wizard starts from the saved theme, and the window shows it.
        run.wait(0.5);
        run.click(&mut view, &mut s, "Continue");
        run.settled(&mut view, &mut s);
        assert_eq!(run.accent(), saved, "the new wizard shows the saved theme");
    }

    #[test]
    fn without_a_client_id_the_pages_skip_eve_metro_after_a_preset() {
        use crate::setup_wizard::WizardPage::{Alliance, Essentials, Nexum};
        let (mut s, tmp) = wizard_session(true);
        s.pilots = router_core::esi::pilots::Pilots::offline(tmp.path().join("active-route.json"));
        let mut view = View::new(&s);
        let mut run = Run::new();
        run.click(&mut view, &mut s, "I live in lowsec.");
        run.wait(0.5);
        let drawn = run.settled(&mut view, &mut s);
        assert!(
            find(&drawn, "Do you have an EVE Metro subscription?").is_none(),
            "the subscription question has no use without a client ID"
        );
        run.click(&mut view, &mut s, "Continue");
        assert_eq!(wizard_pages(&view), [Essentials, Nexum]);
        run.wait(0.5);
        run.click(&mut view, &mut s, "Back");
        run.wait(0.5);
        run.click(&mut view, &mut s, "Back");
        run.wait(0.5);
        run.click(&mut view, &mut s, "I'll configure things myself, thanks.");
        assert_eq!(wizard_pages(&view), [Essentials, Alliance, Nexum]);
    }

    /// Assert that a settled frame of the setup shows no status dots, no dry-run note and no bar.
    /// `welcome` is true on the welcome screen, where "Nexum" is not the name of a wizard page either.
    fn assert_no_status_dots(run: &mut Run, view: &mut View, s: &mut Session, welcome: bool, what: &str) {
        let drawn = run.settled(view, s);
        let mut absent = vec!["Refresh", "Log", "EVE-Scout", Status::Ready.text()];
        if welcome {
            absent.push("Nexum");
        }
        for text in absent {
            assert!(find(&drawn, text).is_none(), "{what}: \"{text}\" is drawn");
        }
        assert!(run.panel("status-bar").is_none(), "{what}: the bar is drawn");
        assert!(find(&drawn, crate::app::DRY_RUN_NOTE).is_none(), "{what}: the note is drawn");
    }

    #[test]
    fn setup_shows_no_status_dots() {
        for dry_run in [false, true] {
            let (mut s, _tmp) = wizard_session(true);
            if dry_run {
                s.set_dry_run();
            }
            let mut view = View::new(&s);
            let mut run = Run::new();
            assert_no_status_dots(&mut run, &mut view, &mut s, true, "step 1");
            // Lowsec shows the subscription question too.
            run.click(&mut view, &mut s, "I live in lowsec.");
            run.wait(0.5);
            assert_no_status_dots(&mut run, &mut view, &mut s, true, "step 2");
            let (mut s, _tmp) = wizard_session(true);
            if dry_run {
                s.set_dry_run();
            }
            let mut view = wizard_view(&s);
            let mut run = Run::new();
            for page in 0..4 {
                assert_no_status_dots(&mut run, &mut view, &mut s, false, &format!("wizard page {page}"));
                view.wizard.as_mut().unwrap().next();
            }
        }
    }

    #[test]
    fn the_welcome_fits_the_smallest_window() {
        use crate::setup_wizard::SMALLEST_WINDOW;
        let mut title_tops = Vec::new();
        for step in [1, 2] {
            let (mut s, _tmp) = wizard_session(true);
            // A dry run draws no bar, so it takes no room.
            s.set_dry_run();
            let mut view = View::new(&s);
            let mut run = Run::new();
            if step == 2 {
                run.click(&mut view, &mut s, "I live in lowsec.");
                run.wait(0.5);
            }
            let drawn = run.settled(&mut view, &mut s);
            assert!(run.panel("status-bar").is_none(), "step {step} draws no status bar");
            let footer = run.panel("welcome-footer");
            // Both steps draw the footer panel, so the body has the same room and the title stays put.
            assert!(footer.is_some(), "step {step} draws the footer panel");
            title_tops.push(find(&drawn, "EVE Router").expect("the title is drawn").1.y);
            if step == 2 {
                assert!(find(&drawn, "Do you have an EVE Metro subscription?").is_some(), "the subscription row is shown");
            }
            let body_end = footer.map_or(SMALLEST_WINDOW.y, |footer| footer.top());
            for (text, pos, size) in &drawn {
                let bottom = pos.y + size.y;
                assert!(bottom <= SMALLEST_WINDOW.y, "step {step}: \"{text}\" ends at {bottom}");
                if let Some(footer) = footer.filter(|_| text == "Back" || text == "Continue") {
                    assert!(pos.y >= footer.top() && bottom <= SMALLEST_WINDOW.y, "step {step}: \"{text}\" is in the footer");
                } else {
                    assert!(bottom <= body_end, "step {step}: \"{text}\" ends at {bottom}, above {body_end}");
                }
            }
        }
        assert!((title_tops[0] - title_tops[1]).abs() <= 0.5, "the title moves between the steps: {title_tops:?}");
    }

    /// Push F5, Ctrl+F and the down arrow, and assert that no shortcut of the route view runs: no
    /// refresh (the test session has no refresh worker, so F5 would show a notice), no focus.
    fn assert_no_shortcut(run: &mut Run, view: &mut View, s: &mut Session, what: &str) {
        let keys = [(Key::F5, Modifiers::NONE), (Key::F, Modifiers::COMMAND), (Key::ArrowDown, Modifiers::NONE)];
        let events = keys
            .iter()
            .map(|&(key, modifiers)| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers })
            .collect();
        run.output(view, s, events);
        let drawn = run.settled(view, s);
        assert_eq!(s.status, Status::Ready, "{what}: the status");
        assert!(find(&drawn, crate::app::REFRESH_STOPPED).is_none() && s.notices.is_empty(), "{what}: F5 ran a refresh");
        assert!(run.ctx.memory(|m| m.focused()).is_none(), "{what}: a widget has the focus");
    }

    #[test]
    fn welcome_frames_write_no_file() {
        for dry_run in [false, true] {
            let (mut s, tmp) = wizard_session(true);
            if dry_run {
                s.set_dry_run();
            }
            let mut view = View::new(&s);
            let mut run = Run::new();
            let empty = |tmp: &tempfile::TempDir, what: &str| {
                assert!(tmp.path().read_dir().unwrap().next().is_none(), "dry run {dry_run}: a file exists {what}");
            };
            for _ in 0..5 {
                run.texts(&mut view, &mut s);
            }
            assert_no_shortcut(&mut run, &mut view, &mut s, "step 1");
            empty(&tmp, "after step 1");
            run.click(&mut view, &mut s, "I live in lowsec.");
            run.wait(0.5);
            for _ in 0..5 {
                run.texts(&mut view, &mut s);
            }
            assert_no_shortcut(&mut run, &mut view, &mut s, "step 2");
            empty(&tmp, "after step 2");
            // Change the three answers.
            run.click_nth(&mut view, &mut s, "No", 0);
            run.click_nth(&mut view, &mut s, "No", 1);
            run.click_nth(&mut view, &mut s, "Yes", 1);
            run.click_nth(&mut view, &mut s, "Yes", 2);
            empty(&tmp, "after the answers");
            // Back to step 1 and a second preset, then on to the wizard.
            run.click(&mut view, &mut s, "Back");
            run.wait(0.5);
            run.click(&mut view, &mut s, "I live in highsec.");
            run.wait(0.5);
            run.click(&mut view, &mut s, "Continue");
            assert!(view.wizard.is_some());
            for _ in 0..5 {
                run.texts(&mut view, &mut s);
            }
            assert_no_shortcut(&mut run, &mut view, &mut s, "wizard");
            empty(&tmp, "after Continue");
            // Back on Essentials, Back on step 2, and the last preset.
            run.wait(0.5);
            run.click(&mut view, &mut s, "Back");
            assert!(view.wizard.is_none());
            for _ in 0..5 {
                run.texts(&mut view, &mut s);
            }
            run.wait(0.5);
            run.click(&mut view, &mut s, "Back");
            run.wait(0.5);
            run.click(&mut view, &mut s, "I'll configure things myself, thanks.");
            assert!(view.wizard.is_some());
            empty(&tmp, "before Finish");
            for _ in 0..3 {
                view.wizard.as_mut().unwrap().next();
            }
            run.wait(0.5);
            run.click(&mut view, &mut s, "Finish");
            assert!(view.wizard.is_none() && view.welcome.is_none());
            assert_eq!(tmp.path().join("eve-router.json").exists(), !dry_run, "Finish writes the config, but not in a dry run");
        }
    }

    #[test]
    fn status_text_clears_after_status_time() {
        let mut timer = StatusTimer::default();
        let start = Instant::now();
        assert_eq!(timer.update(Status::Refreshing, start), Some(STATUS_TIME));
        assert_eq!(timer.update(Status::Refreshing, start + Duration::from_secs(1)), Some(Duration::from_secs(3)));
        assert_eq!(timer.update(Status::Refreshing, start + Duration::from_secs(3)), Some(Duration::from_secs(1)));
        assert_eq!(timer.update(Status::Refreshing, start + Duration::from_secs(4)), None);
        // After the clear, the same status again shows for a full time.
        assert_eq!(timer.update(Status::Ready, start + Duration::from_secs(4)), Some(STATUS_TIME));
        assert_eq!(timer.update(Status::Refreshing, start + Duration::from_secs(8)), Some(STATUS_TIME));
    }
}
