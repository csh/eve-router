//! The modal windows: the settings, the Pilot picker, the Nexum map list and the messages.

use crate::app::Session;
use crate::overlay::Overlay;
use crate::pilots_view::{Portraits, pilot_portrait};
use crate::search::SearchBox;
use crate::theme;
use crate::view::View;
use crate::widgets::{AppButton, UiExt};
use egui::{Button, Color32, Frame, Id, Key, Margin, Modal, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, vec2};
use egui_phosphor_icons::icons;
use router_core::ansiblex::HullClass;
use router_core::config::{self, ApiKey};
use router_core::esi::pilots::{PilotRow, PilotView};
use router_core::labels::ship_text;
use router_core::settings::{HullSource, RouteCosts, parse_max_cap};
use router_core::sources::{
    self,
    nexum::{self, MapInfo},
};
use router_core::universe::display_sec;
use std::time::{Duration, Instant};

pub enum Popup {
    /// The Pilot picker. The rows are `Pilots::pilot_rows(filter)`.
    Pilot { filter: String },
    /// The avoid list. `confirm` is true while the window asks to clear all entries.
    Avoid { confirm: bool, search: SearchBox, region: String },
    /// A pasted list of systems. `problems` holds the names that the last try did not add.
    List { text: String, problems: Vec<String> },
}

/// The text fields of the settings window.
pub struct SettingsForm {
    page: Page,
    /// True after the window asked for the map list. A new URL or key sets it to false again.
    maps_tried: bool,
    maps_error: Option<String>,
    capital: SearchBox,
    favourite: SearchBox,
    max_cap: String,
    /// The pasted SMT list, and the lines of the last import that gave no bridge.
    bridge_text: String,
    bridge_problems: Vec<String>,
    /// True while the window asks to remove all bridges.
    bridge_confirm: bool,
    replace_confirm: bool,
    url: String,
    /// The key field starts empty, so the key never shows on the screen.
    key: String,
    /// The EVE Metro key field. It starts empty too.
    metro_key: String,
    error: Option<String>,
    /// The field that the pilot saved last, and when. "Saved" shows next to it for `SAVED_TIME`.
    saved: Option<(Saved, Instant)>,
}

/// A field with a Save or Apply button.
#[derive(Clone, Copy, PartialEq)]
enum Saved {
    Url,
    Key,
    MetroKey,
    MaxCap,
}

const SAVED_TIME: Duration = Duration::from_secs(3);
/// The row height of the two tables of the Avoid window, and the rows that each shows before it scrolls.
const AVOID_ROW: f32 = 28.0;
/// The header row of those tables. The scroll height counts it.
const AVOID_HEAD: f32 = 22.0;
const AVOID_ROWS: f32 = 7.0;

impl SettingsForm {
    /// Show "Saved" after a button, for a short time, when `field` was saved last.
    fn saved_label(&self, ui: &mut Ui, field: Saved) {
        let Some((last, at)) = self.saved else { return };
        let left = SAVED_TIME.saturating_sub(at.elapsed());
        if last == field && !left.is_zero() {
            ui.label(RichText::new("Saved").color(theme::TEXT_DIM));
            ui.ctx().request_repaint_after(left);
        }
    }

    pub fn new(s: &Session, page: Page) -> Self {
        SettingsForm {
            page,
            maps_tried: false,
            maps_error: None,
            capital: SearchBox::new("capital-search"),
            favourite: SearchBox::new("favourite-search"),
            max_cap: s.settings.rules.max_cap.map(|m| m.to_string()).unwrap_or_default(),
            bridge_text: String::new(),
            bridge_problems: Vec::new(),
            bridge_confirm: false,
            replace_confirm: false,
            url: s.cfg.nexum.url.clone().unwrap_or_default(),
            key: String::new(),
            metro_key: String::new(),
            error: None,
            saved: None,
        }
    }
}

impl SettingsForm {
    pub fn set_maps_error(&mut self, error: Option<String>) {
        self.maps_error = error;
    }

    /// The URL or the key changed: the map list is old.
    fn maps_stale(&mut self, s: &mut Session) {
        s.map_names.clear();
        self.maps_tried = false;
        self.maps_error = None;
    }
}

/// Start the fetch of the map list on a thread.
fn fetch_map_list(s: &Session, ctx: &egui::Context) -> std::sync::mpsc::Receiver<Result<Vec<MapInfo>, sources::FetchError>> {
    let nexum_cfg = s.cfg.nexum.clone();
    let ctx = ctx.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(nexum::fetch_maps(&nexum_cfg, sources::TIMEOUT));
        ctx.request_repaint();
    });
    rx
}

pub fn modal_frame() -> Frame {
    Frame::new().fill(theme::PANEL).stroke(Stroke::new(1.0, theme::LINE)).inner_margin(Margin::same(14))
}

/// The title of a modal window, with an accent line below it.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).family(theme::bold()).color(theme::accent(ui)).extra_letter_spacing(2.0));
    let rect = ui.available_rect_before_wrap();
    ui.painter().line_segment([rect.left_top(), rect.left_top() + vec2(rect.width(), 0.0)], Stroke::new(1.0, theme::LINE));
    ui.add_space(8.0);
}

/// True when a modal window must close because of a click outside it or the Escape key. A press on the
/// title bar moves the app window, so it does not close the modal.
fn closes<T>(ctx: &egui::Context, response: &egui::ModalResponse<T>) -> bool {
    response.should_close() && (!crate::chrome::title_press(ctx) || ctx.input(|i| i.key_pressed(Key::Escape)))
}

/// Show the open modal windows.
pub fn show(ui: &mut Ui, view: &mut View, s: &mut Session) {
    if let Some(form) = &mut view.settings {
        let page = form.page;
        // The list loads when the window opens, if the URL and the key are set and the list is empty.
        let ready = s.cfg.nexum.url.is_some() && s.cfg.nexum.key.is_some();
        if ready && !form.maps_tried && s.map_names.is_empty() && view.maps.is_none() {
            form.maps_tried = true;
            view.maps = Some(fetch_map_list(s, ui.ctx()));
        }
        let loading = view.maps.is_some();
        let portraits = &mut view.pilots.portraits;
        let overlay = &mut view.overlay;
        let response =
            Modal::new(Id::new("settings")).frame(modal_frame()).show(ui.ctx(), |ui| settings(ui, form, s, loading, portraits, overlay));
        match response.inner {
            Some(Action::LoadMaps) => {
                if view.maps.is_none() {
                    form.maps_error = None;
                    view.maps = Some(fetch_map_list(s, ui.ctx()));
                }
            }
            Some(Action::Close) => view.settings = None,
            None if closes(ui.ctx(), &response) && view.popup.is_none() => view.settings = None,
            None => {}
        }
        if view.settings.is_none() {
            view.settings_page = page;
            // The dummy window of the position editor belongs to the page.
            #[cfg(windows)]
            view.overlay.cancel_edit();
            s.flush_apply();
            s.save();
        }
    }

    let close = match &mut view.popup {
        None => false,
        Some(Popup::Pilot { filter }) => {
            let response = Modal::new(Id::new("pilot")).frame(modal_frame()).show(ui.ctx(), |ui| pilot_picker(ui, filter, s));
            response.inner || closes(ui.ctx(), &response)
        }
        Some(Popup::Avoid { confirm, search, region }) => {
            let response =
                Modal::new(Id::new("avoid")).frame(modal_frame()).show(ui.ctx(), |ui| avoid_list(ui, confirm, search, region, s));
            response.inner || closes(ui.ctx(), &response)
        }
        Some(Popup::List { text, problems }) => {
            let response = Modal::new(Id::new("list")).frame(modal_frame()).show(ui.ctx(), |ui| paste_list(ui, text, problems, s));
            response.inner || closes(ui.ctx(), &response)
        }
    };
    if close {
        view.popup = None;
    }
}

/// A row of the avoid table: an entry of the system list, or of the region list.
#[derive(Clone, Copy)]
enum AvoidRow {
    System(usize),
    Region(usize),
}

/// A change that the avoid table asks for.
enum AvoidAct {
    Remove(AvoidRow),
    Never(AvoidRow, bool),
}

/// The names of the regions that contain `text`, up to `limit`, in alphabetical order.
fn region_matches(uni: &router_core::universe::Universe, text: &str, limit: usize) -> Vec<String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Vec::new();
    }
    let regions: std::collections::BTreeSet<&str> = uni.graph.node_weights().map(|sys| sys.region.as_str()).collect();
    regions.into_iter().filter(|r| r.to_lowercase().contains(&text)).take(limit).map(String::from).collect()
}

/// The avoid list: two search boxes that add an entry, a table of system, security, region, a
/// "Never/Prefer" switch and a remove button, and a clear-all button that asks first.
/// Returns true when the window must close.
fn avoid_list(ui: &mut Ui, confirm: &mut bool, search: &mut SearchBox, region_text: &mut String, s: &mut Session) -> bool {
    title(ui, "Avoid");
    ui.set_width(crate::theme::modal_width(ui, 760.0));
    ui.horizontal_wrapped(|ui| {
        if let Some((_, node)) = search.show(ui, &s.uni, 300.0, "Add a system…", &[])
            && !s.settings.avoid.has_system(node)
        {
            s.toggle_avoid_system(node);
        }
        ui.add(TextEdit::singleline(region_text).hint_text("Add a region…").desired_width(220.0));
    });
    for region in region_matches(&s.uni, region_text, 6) {
        if ui.small_button(&region).clicked() {
            if !s.settings.avoid.has_region(&region) {
                s.toggle_avoid_region(&region);
            }
            region_text.clear();
        }
    }
    ui.add_space(8.0);
    let avoid = &s.settings.avoid;
    let mut act = None;
    if avoid.is_empty() {
        ui.label(RichText::new("Nothing is avoided. Search above, or right-click a system in the route table.").color(theme::TEXT_DIM));
    } else {
        let rows: Vec<AvoidRow> =
            (0..avoid.systems.len()).map(AvoidRow::System).chain((0..avoid.regions.len()).map(AvoidRow::Region)).collect();
        let never_switch = |ui: &mut Ui, never: bool| -> bool {
            let (text, color) = if never { ("Never", theme::ERROR) } else { ("Prefer", theme::WARN) };
            let hover = if never {
                "Never go through here, even if that means no route at all"
            } else {
                "Steer clear if possible, but go through if there's no other way"
            };
            ui.add(AppButton::new(RichText::new(text).color(color)).min_size(vec2(64.0, 0.0))).on_hover_text(hover).clicked()
        };
        use egui_extras::Column;
        egui_extras::TableBuilder::new(ui)
            .id_salt("avoid-table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::exact(80.0))
            .column(Column::initial(200.0).at_least(120.0).clip(true))
            .column(Column::exact(84.0))
            .column(Column::exact(84.0))
            .max_scroll_height(AVOID_HEAD + AVOID_ROWS * AVOID_ROW)
            .auto_shrink([false, true])
            .header(AVOID_HEAD, |mut header| {
                for text in ["System", "Security", "Region", "", ""] {
                    header.col(|ui| _ = ui.label(theme::header_text(text)));
                }
            })
            .body(|body| {
                body.rows(AVOID_ROW, rows.len(), |mut row| {
                    let kind = rows[row.index()];
                    match kind {
                        AvoidRow::System(i) => {
                            let entry = &avoid.systems[i];
                            let sys = s.uni.system(entry.item);
                            row.col(|ui| _ = ui.label(RichText::new(&sys.name).color(theme::TEXT)));
                            row.col(|ui| {
                                _ = ui.label(
                                    RichText::new(format!("{:.1}", display_sec(sys.security))).color(theme::sec_color(sys.security)),
                                );
                            });
                            row.col(|ui| _ = ui.label(RichText::new(&sys.region).color(theme::TEXT_DIM)));
                            row.col(|ui| {
                                if never_switch(ui, entry.never) {
                                    act = Some(AvoidAct::Never(kind, !entry.never));
                                }
                            });
                        }
                        AvoidRow::Region(i) => {
                            let entry = &avoid.regions[i];
                            row.col(|ui| _ = ui.label(RichText::new("All systems").color(theme::TEXT_DIM)));
                            row.col(|_| {});
                            row.col(|ui| _ = ui.label(RichText::new(&entry.item).color(theme::TEXT)));
                            row.col(|ui| {
                                if never_switch(ui, entry.never) {
                                    act = Some(AvoidAct::Never(kind, !entry.never));
                                }
                            });
                        }
                    }
                    row.col(|ui| {
                        if ui.app_button("Remove").clicked() {
                            act = Some(AvoidAct::Remove(kind));
                        }
                    });
                });
            });
    }
    let mut reopen = None;
    if !s.closed.is_empty() {
        ui.add_space(12.0);
        ui.label(theme::header_text("Closed wormholes"));
        ui.label(RichText::new("Ignore these connections until the next app restart.").color(theme::TEXT_DIM));
        ui.add_space(4.0);
        let name = |id: u32| s.uni.by_id.get(&id).map_or_else(|| id.to_string(), |&n| s.uni.name(n).to_string());
        // The columns have the widths of the avoid table above, so the two tables line up.
        use egui_extras::Column;
        egui_extras::TableBuilder::new(ui)
            .id_salt("closed-table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::exact(80.0))
            .column(Column::initial(200.0).at_least(120.0).clip(true))
            .column(Column::exact(84.0))
            .column(Column::exact(84.0))
            .max_scroll_height(AVOID_HEAD + AVOID_ROWS * AVOID_ROW)
            .auto_shrink([false, true])
            .header(AVOID_HEAD, |mut header| {
                for text in ["Start", "Sig", "End", "Sig", ""] {
                    header.col(|ui| _ = ui.label(theme::header_text(text)));
                }
            })
            .body(|body| {
                body.rows(AVOID_ROW, s.closed.len(), |mut row| {
                    let i = row.index();
                    let hole = &s.closed[i];
                    let sig = |ui: &mut Ui, sig: Option<&str>| match sig {
                        Some(letters) => _ = ui.label(RichText::new(letters).color(theme::TEXT)),
                        None => _ = ui.label(RichText::new("Unknown").color(theme::TEXT_DIM)),
                    };
                    let [(start, start_sig), (end, end_sig)] = hole.ends();
                    row.col(|ui| _ = ui.label(RichText::new(name(start)).color(theme::TEXT)));
                    row.col(|ui| sig(ui, start_sig));
                    row.col(|ui| _ = ui.label(RichText::new(name(end)).color(theme::TEXT)));
                    row.col(|ui| sig(ui, end_sig));
                    row.col(|ui| {
                        if ui.app_button("Reopen").clicked() {
                            reopen = Some(i);
                        }
                    });
                });
            });
    }
    let count = avoid.len();
    ui.add_space(8.0);
    let mut close = false;
    let mut clear = false;
    ui.horizontal(|ui| {
        if *confirm {
            ui.label(RichText::new(format!("Remove all {count} entries?")).color(theme::WARN));
            if ui.app_button("Clear all").clicked() {
                clear = true;
            }
            if ui.app_button("Cancel").clicked() {
                *confirm = false;
            }
        } else {
            if ui.add_enabled(count > 0, AppButton::new("Clear all")).clicked() {
                *confirm = true;
            }
            close = ui.app_button("Close").clicked();
        }
    });
    match act {
        Some(AvoidAct::Remove(AvoidRow::System(i))) => {
            let node = s.settings.avoid.systems[i].item;
            s.toggle_avoid_system(node);
        }
        Some(AvoidAct::Remove(AvoidRow::Region(i))) => {
            let region = s.settings.avoid.regions[i].item.clone();
            s.toggle_avoid_region(&region);
        }
        Some(AvoidAct::Never(AvoidRow::System(i), never)) => {
            let node = s.settings.avoid.systems[i].item;
            s.set_avoid_never_system(node, never);
        }
        Some(AvoidAct::Never(AvoidRow::Region(i), never)) => {
            let region = s.settings.avoid.regions[i].item.clone();
            s.set_avoid_never_region(&region, never);
        }
        None => {}
    }
    if let Some(i) = reopen {
        let hole = s.closed[i].clone();
        s.reopen_link(&hole);
    }
    if clear {
        s.clear_avoid();
        *confirm = false;
    }
    close
}

enum Action {
    LoadMaps,
    Close,
}

fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
}

/// The hover text of a control that a route start locks.
pub const LOCKED: &str = "Locked while route is active - stop the route to change this";

/// The hover text of a control that the Pochven Access switch locks.
const ACCESS_OFF: &str = "Turn on Pochven access above first";

/// The pages of the settings window, in the order of the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Page {
    #[default]
    Favourites,
    Theme,
    Wormholes,
    /// Filaments and EVE Metro: the two features almost all about travel via Pochven.
    Pochven,
    Ansiblex,
    /// The routing overlay: a window above the game. It works on Windows only.
    #[cfg(windows)]
    Overlay,
    Engine,
}

impl Page {
    #[cfg(windows)]
    pub const ALL: [Page; 7] = [Page::Favourites, Page::Theme, Page::Wormholes, Page::Pochven, Page::Ansiblex, Page::Overlay, Page::Engine];
    #[cfg(not(windows))]
    pub const ALL: [Page; 6] = [Page::Favourites, Page::Theme, Page::Wormholes, Page::Pochven, Page::Ansiblex, Page::Engine];

    pub fn label(self) -> &'static str {
        match self {
            Page::Favourites => "Favourites",
            Page::Theme => "Theme",
            Page::Wormholes => "Wormholes",
            Page::Pochven => "Pochven",
            Page::Ansiblex => "Ansiblex",
            #[cfg(windows)]
            Page::Overlay => "Overlay",
            Page::Engine => "Engine",
        }
    }
}

/// A small heading for a group of rows on a page.
fn group(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).family(theme::bold()).color(theme::accent(ui)).extra_letter_spacing(1.5));
    ui.add_space(2.0);
}

/// The note of the Settings window while a route is active. The pages that do not change a route stay editable.
const LOCK_NOTE: &str = if cfg!(windows) {
    "Routing settings are locked while a route is active. Favourites, Theme and Overlay stay editable."
} else {
    "Routing settings are locked while a route is active. Favourites and Theme stay editable."
};

fn settings(
    ui: &mut Ui,
    form: &mut SettingsForm,
    s: &mut Session,
    loading: bool,
    portraits: &mut Portraits,
    overlay: &mut Overlay,
) -> Option<Action> {
    let mut action = None;
    ui.set_width(crate::theme::modal_width(ui, 720.0));
    title(ui, "Settings");
    // While a route is active, only the pages that do not change a route can change.
    let locked = s.pilots.active.is_some();
    if locked {
        ui.label(RichText::new(LOCK_NOTE).color(theme::WARN));
    }
    #[cfg(not(windows))]
    let _ = &overlay;
    let body = (ui.ctx().content_rect().height() - 230.0).clamp(240.0, 480.0);
    let list = 140.0;
    let pad = 16.0;
    let page_width = (ui.available_width() - list - pad - 28.0).max(200.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(list, body), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for page in Page::ALL {
                // Muted text that brightens on hover. The open page shows in the accent color.
                let (rect, response) = ui.allocate_exact_size(vec2(list, 26.0), Sense::click());
                let color = if form.page == page {
                    theme::accent(ui)
                } else if response.hovered() {
                    theme::TEXT
                } else {
                    theme::TEXT_DIM
                };
                let font = egui::TextStyle::Body.resolve(ui.style());
                ui.painter().text(rect.left_center() + vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, page.label(), font, color);
                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    form.page = page;
                }
            }
        });
        // A separator in a row takes the whole height that is left, so draw a line of the body height.
        let (rect, _) = ui.allocate_exact_size(vec2(1.0, body), Sense::hover());
        ui.painter().line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(1.0, theme::LINE));
        ui.add_space(pad - ui.spacing().item_spacing.x);
        ui.allocate_ui_with_layout(vec2(page_width, body), egui::Layout::top_down(egui::Align::Min), |ui| {
            // The Ansiblex and Pochven pages have their own scrolling table, so the page itself stays still.
            match form.page {
                Page::Ansiblex => return ansiblex_page(ui, form, s, locked),
                Page::Pochven => return pochven_page(ui, form, s, locked, portraits),
                #[cfg(windows)]
                Page::Overlay => return overlay_page(ui, s, portraits, overlay),
                _ => {}
            }
            ScrollArea::vertical().max_height(body).auto_shrink([false, false]).show(ui, |ui| match form.page {
                Page::Favourites => favourites_page(ui, form, s),
                Page::Theme => theme_page(ui, s),
                Page::Wormholes => action = wormholes_page(ui, form, s, locked, loading),
                Page::Pochven | Page::Ansiblex => {}
                #[cfg(windows)]
                Page::Overlay => {}
                Page::Engine => engine_page(ui, form, s, locked),
            });
        });
    });

    ui.add_space(10.0);
    if let Some(e) = &form.error {
        ui.label(RichText::new(e).color(theme::ERROR));
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.app_button("Close").clicked() {
                action = Some(Action::Close);
            }
        });
    });
    action
}

const FIELD: f32 = 300.0;

fn favourites_page(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session) {
    let field = FIELD;
    // The favourites, in the order of the sidebar. Drag a name to move it, or use the buttons.
    ui.vertical(|ui| {
        let mut edit: Option<(usize, i32)> = None;
        let count = s.settings.favourites.len();
        let favs = s.settings.favourites.clone();
        let response = egui_dnd::dnd(ui, "favourites-dnd").show(favs.iter().copied(), |ui, fav, handle, _| {
            let i = favs.iter().position(|&f| f == fav).unwrap_or(0);
            // The name is at the left. The buttons are at the right, in the width of the search box.
            ui.allocate_ui_with_layout(vec2(field, 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(Button::new(icons::X.as_str()).small()).on_hover_text("Remove").clicked() {
                    edit = Some((i, 0));
                }
                if ui.add_enabled(i + 1 < count, Button::new(icons::CARET_DOWN.as_str()).small()).on_hover_text("Move down").clicked() {
                    edit = Some((i, 1));
                }
                if ui.add_enabled(i > 0, Button::new(icons::CARET_UP.as_str()).small()).on_hover_text("Move up").clicked() {
                    edit = Some((i, -1));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    handle.ui(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(s.uni.name(fav)).color(theme::TEXT)).truncate());
                    });
                });
            });
        });
        if response.final_update().is_some() {
            response.update_vec(&mut s.settings.favourites);
            s.recompute();
            s.save();
        } else if let Some((i, step)) = edit {
            let favs = &mut s.settings.favourites;
            match step {
                0 => _ = favs.remove(i),
                -1 => favs.swap(i, i - 1),
                _ => favs.swap(i, i + 1),
            }
            s.recompute();
            s.save();
        }
        if let Some((_, node)) = form.favourite.show(ui, &s.uni, field, "Add favourite…", &[]) {
            s.add_favourite(node);
        }
    });
}

fn theme_page(ui: &mut Ui, s: &mut Session) {
    egui::Grid::new("settings-theme").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        // Photon UI theme and layout settings.
        label(ui, "Theme");
        ui.horizontal_wrapped(|ui| {
            let mut current_theme = s.faction_theme();
            egui::ComboBox::from_id_salt("faction-theme-combo").selected_text(current_theme.label()).show_ui(ui, |ui| {
                for &t in &theme::FactionTheme::ALL {
                    if ui.selectable_value(&mut current_theme, t, t.label()).clicked() {
                        s.set_faction_theme(t, ui.ctx());
                    }
                }
            });
        });
        ui.end_row();
        label(ui, "Compact Mode");
        ui.horizontal_wrapped(|ui| {
            let mut compact = s.compact_mode();
            if ui.app_checkbox(&mut compact, "").changed() {
                s.set_compact_mode(compact, ui.ctx());
            }
        });
        ui.end_row();
    });
}

fn wormholes_page(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, locked: bool, loading: bool) -> Option<Action> {
    let mut action = None;
    let field = FIELD;
    group(ui, "Nexum");
    egui::Grid::new("settings-nexum").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        // Nexum: the URL, the key and the map. A change applies at the next start.
        label(ui, "URL");
        ui.add_enabled_ui(!locked, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add(TextEdit::singleline(&mut form.url).hint_text("https://nexum.example").desired_width(field));
                if ui.add(AppButton::new("Save").primary()).clicked() {
                    match config::parse_nexum_url(&form.url) {
                        Ok(url) => {
                            s.cfg.nexum.url = url;
                            form.error = None;
                            form.maps_stale(s);
                            s.nexum_saved();
                            form.saved = Some((Saved::Url, Instant::now()));
                        }
                        Err(e) => form.error = Some(e.to_string()),
                    }
                }
                form.saved_label(ui, Saved::Url);
            })
        })
        .response
        .on_disabled_hover_text(LOCKED);
        ui.end_row();

        label(ui, "Key");
        ui.add_enabled_ui(!locked, |ui| {
            // The masked key and Clear while a key is set, else the input and Save.
            ui.horizontal_wrapped(|ui| {
                if let Some(key) = &s.cfg.nexum.key {
                    ui.label(RichText::new(key.masked()).color(theme::TEXT));
                    if ui.app_button("Clear").clicked() {
                        s.cfg.nexum.key = None;
                        form.maps_stale(s);
                        s.nexum_saved();
                    }
                } else {
                    ui.add(
                        TextEdit::singleline(&mut form.key)
                            .password(true)
                            .hint_text("paste a key with the read scope")
                            .desired_width(field),
                    );
                    if ui.add_enabled(!form.key.trim().is_empty(), AppButton::new("Save").primary()).clicked() {
                        s.cfg.nexum.key = Some(ApiKey(form.key.trim().to_string()));
                        form.key.clear();
                        form.maps_stale(s);
                        s.nexum_saved();
                        form.saved = Some((Saved::Key, Instant::now()));
                    }
                }
                form.saved_label(ui, Saved::Key);
            })
        })
        .response
        .on_disabled_hover_text(LOCKED);
        ui.end_row();

        label(ui, "Signatures");
        let row = ui.add_enabled_ui(!locked, |ui| {
            let mut skip = !s.cfg.nexum.keep_unsigned;
            let changed = ui
                .app_checkbox(&mut skip, "Skip connections with no known signatures")
                .on_hover_text("A wormhole whose two ends have no known signature is considered unreliable data.")
                .changed();
            (changed, skip)
        });
        row.response.on_disabled_hover_text(LOCKED);
        if row.inner.0 {
            s.cfg.nexum.keep_unsigned = !row.inner.1;
            s.nexum_saved();
        }
        ui.end_row();
    });
    ui.add_space(14.0);
    group(ui, "Maps");
    if let Some(a) = maps_list(ui, form, s, locked, loading) {
        action = Some(a);
    }
    ui.add_space(14.0);
    group(ui, "EVE Scout");
    egui::Grid::new("settings-scout").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        // The EVE-Scout hub switches act at route time.
        let mut changed = false;
        for (name, on) in [("Allow travel via Thera", &mut s.settings.hubs.thera), ("Allow travel via Turnur", &mut s.settings.hubs.turnur)]
        {
            label(ui, name);
            let row = ui.add_enabled_ui(!locked, |ui| ui.app_checkbox(on, "").changed());
            changed |= row.inner;
            row.response.on_disabled_hover_text(LOCKED);
            ui.end_row();
        }
        if changed {
            s.recompute();
            s.save();
        }
    });
    action
}

/// The Pochven group of the Pochven page: the access switch. It acts at route time, like the
/// EVE-Scout hub switches. Filament travel needs no
/// client ID, unlike EVE Metro, so it always shows.
fn pochven_group(ui: &mut Ui, s: &mut Session, locked: bool) {
    group(ui, "Pochven");
    ui.label(
        RichText::new("Filaments transport one or more pilots to a random system of a clade, assuming an average number of jumps.")
            .color(theme::TEXT_DIM),
    );
    ui.add_space(4.0);
    egui::Grid::new("settings-filaments").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        label(ui, "Allow travel via Pochven");
        let row = ui.add_enabled_ui(!locked, |ui| ui.app_checkbox(&mut s.settings.pochven, "").changed());
        row.response.on_disabled_hover_text(LOCKED);
        if row.inner {
            s.recompute();
            s.save();
        }
        ui.end_row();
    });
}

/// The Nexum maps, in a table with a checkbox for each. The router reads the maps that are on.
/// After 10 rows the table scrolls.
fn maps_list(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, locked: bool, loading: bool) -> Option<Action> {
    const MAX_ROWS: f32 = 10.0;
    const ROW: f32 = 28.0;
    let dim = |text: &str| RichText::new(text).color(theme::TEXT_DIM);
    if s.cfg.nexum.url.is_none() || s.cfg.nexum.key.is_none() {
        ui.label(dim("Set the URL and the key to list the maps."));
        return None;
    }
    let mut action = None;
    // A map that is on but not in the list still shows, so the pilot can turn it off.
    let mut rows: Vec<(String, String, bool)> = s.map_names.iter().map(|m| (m.id.clone(), m.name.clone(), false)).collect();
    for id in &s.cfg.nexum.map_ids {
        if !rows.iter().any(|(r, ..)| r == id) {
            rows.push((id.clone(), "Not in the list".into(), true));
        }
    }
    for row in &mut rows {
        row.2 = s.cfg.nexum.map_ids.contains(&row.0);
    }
    let mut toggled: Option<(String, bool)> = None;
    use egui_extras::Column;
    egui_extras::TableBuilder::new(ui)
        .id_salt("nexum-maps")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(44.0))
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::exact(150.0).clip(true))
        .max_scroll_height(MAX_ROWS * ROW)
        .auto_shrink([false, true])
        .header(24.0, |mut header| {
            header.col(|ui| _ = ui.label(theme::header_text("On")));
            header.col(|ui| _ = ui.label(theme::header_text("Map")));
            header.col(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if loading {
                        ui.add(egui::Spinner::new().color(theme::accent(ui)));
                    } else if ui.small_button("Reload maps").on_hover_text("Load the list of maps again").clicked() {
                        action = Some(Action::LoadMaps);
                    }
                });
            });
        })
        .body(|body| {
            body.rows(ROW, rows.len(), |mut row| {
                let (id, name, mut on) = rows[row.index()].clone();
                row.col(|ui| {
                    let changed = ui.add_enabled_ui(!locked, |ui| ui.app_checkbox(&mut on, "").changed()).inner;
                    if changed {
                        toggled = Some((id.clone(), on));
                    }
                });
                row.col(|ui| _ = ui.label(RichText::new(name).color(if on { theme::TEXT } else { theme::TEXT_DIM })));
                row.col(|ui| _ = ui.label(dim(&id)));
            });
        });
    if let Some(e) = &form.maps_error {
        ui.label(RichText::new(e).color(theme::ERROR));
    } else if s.map_names.is_empty() && !loading && form.maps_tried {
        ui.label(dim("The key has access to no maps."));
    } else if s.cfg.nexum.map_ids.is_empty() {
        ui.label(dim("No map is on: the router reads no Nexum data."));
    }
    if let Some((id, on)) = toggled {
        let ids = &mut s.cfg.nexum.map_ids;
        if on {
            ids.push(id);
        } else {
            ids.retain(|m| *m != id);
        }
        s.nexum_saved();
    }
    action
}

/// The status of a character for the EVE Metro table: the text and its color.
fn pilot_status(pilot: &PilotView) -> (&'static str, Color32) {
    if pilot.live.expired || pilot.needs_reauth {
        return ("Log in again", theme::WARN);
    }
    match pilot.live.online {
        Some(true) => ("Online", theme::OK),
        Some(false) => ("Offline", theme::TEXT_DIM),
        None => ("-", theme::TEXT_DIM),
    }
}

/// The buttons of one place. At rest: Position, and Reset when the place has a position. While the
/// dummy window of this place is open: Save and Cancel. The two places never edit together.
#[cfg(windows)]
fn position_buttons(ui: &mut Ui, s: &mut Session, overlay: &mut Overlay, place: config::Place) {
    let editing = overlay.editing();
    if editing == Some(place) {
        // Save waits until the dummy window has drawn and reported its position.
        let ready = overlay.edit_position().is_some();
        if ui.add_enabled(ready, AppButton::new("Save").primary()).clicked()
            && let Some((place, pos)) = overlay.finish_edit()
        {
            s.set_overlay_position(place, Some(pos));
        }
        if ui.app_button("Cancel").clicked() {
            overlay.cancel_edit();
        }
        return;
    }
    let busy = editing.is_some();
    const BUSY: &str = "Save or cancel the other position first";
    if ui.add_enabled(!busy, AppButton::new("Adjust Position")).on_disabled_hover_text(BUSY).clicked() {
        overlay.begin_edit(place);
    }
    if s.cfg.routing_overlay.position_of(place).is_some()
        && ui.add_enabled(!busy, AppButton::new("Reset")).on_disabled_hover_text(BUSY).clicked()
    {
        s.set_overlay_position(place, None);
    }
}

/// True when the client may run in fullscreen mode, and so can hide the overlay. The warning shows
/// for the one shape that the overlay cannot tell from a fullscreen window. A mode that the overlay
/// did not see yet gives no warning.
#[cfg(windows)]
fn may_be_fullscreen(mode: Option<crate::overlay::DisplayMode>) -> bool {
    mode == Some(crate::overlay::DisplayMode::Other)
}

/// The Overlay page: the switch, the global position, and a table with the position of each
/// character. The page itself stays still, and the table scrolls.
#[cfg(windows)]
fn overlay_page(ui: &mut Ui, s: &mut Session, portraits: &mut Portraits, overlay: &mut Overlay) {
    use config::Place;
    use egui_extras::Column;
    let dim = |text: &str| RichText::new(text).color(theme::TEXT_DIM);
    group(ui, "Wormhole overlay");
    ui.label(dim(
        "Shows what to do at the next wormhole or filament of the active route, in the system where the jump starts. It shows only while the client of the route character has the focus.",
    ));
    ui.add_space(4.0);
    egui::Grid::new("settings-overlay").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        label(ui, "Enable");
        let mut on = s.cfg.routing_overlay.enabled;
        if ui.app_checkbox(&mut on, "").changed() {
            s.set_overlay_enabled(on);
            if !on {
                overlay.cancel_edit();
            }
        }
        ui.end_row();
    });
    ui.label(dim("The game client must run in windowed or borderless mode. A fullscreen client hides the overlay."));
    ui.add_space(14.0);

    group(ui, "Global");
    ui.horizontal(|ui| position_buttons(ui, s, overlay, Place::Global));
    ui.add_space(14.0);

    group(ui, "Position override");
    ui.label(dim("Adjust the overlay position per-character."));
    ui.add_space(4.0);
    let pilots = s.pilots.characters();
    if pilots.is_empty() {
        ui.label(dim("No linked character. Add one in the Pilots panel."));
        return;
    }
    let editing = overlay.editing();
    // The columns are narrow, so the name column has room for a long name.
    let extra = [("Position", Column::exact(72.0)), ("", Column::exact(176.0))];
    // The overlay cannot show above a fullscreen client. The table warns at the name.
    let warnings: Vec<(u64, bool)> = pilots.iter().map(|p| (p.id, may_be_fullscreen(overlay.display_mode(&p.name)))).collect();
    let fullscreen = |pilot: &PilotView| warnings.iter().any(|&(id, warn)| id == pilot.id && warn);
    character_table(
        ui,
        "overlay-characters",
        &pilots,
        portraits,
        &extra,
        |pilot| RowLook {
            selected: editing == Some(Place::Character(pilot.id)),
            warning: fullscreen(pilot).then_some("A fullscreen client hides the overlay. Use windowed or borderless mode."),
        },
        |ui, pilot, column| match column {
            0 => match s.cfg.routing_overlay.position_of(Place::Character(pilot.id)) {
                Some(_) => _ = ui.label(RichText::new("Personal").color(theme::TEXT)),
                None => _ = ui.label(dim("Global")),
            },
            _ => position_buttons(ui, s, overlay, Place::Character(pilot.id)),
        },
    );
}

/// The look of a row of a table of characters.
#[derive(Default)]
struct RowLook {
    /// The row has the selected look.
    selected: bool,
    /// The text of a warning. A row with a warning shows a warning icon before the name, the name in
    /// the warning color, and the text as the tooltip of both.
    warning: Option<&'static str>,
}

/// The row height of a table of characters.
const CHARACTER_ROW: f32 = 40.0;

/// The space between a long name and the next column of a table of characters.
const NAME_GAP: f32 = 8.0;

/// A table of the linked characters: the portrait and the name, then the columns of `extra`. Each
/// has a header text and a width. `cell` draws one cell of `extra` for a character, by the index
/// in `extra`. `look` gives the look of a row. The table takes the height that is left, and scrolls.
fn character_table(
    ui: &mut Ui,
    id_salt: &str,
    pilots: &[PilotView],
    portraits: &mut Portraits,
    extra: &[(&str, egui_extras::Column)],
    look: impl Fn(&PilotView) -> RowLook,
    mut cell: impl FnMut(&mut Ui, &PilotView, usize),
) {
    use egui_extras::Column;
    let table_height = (ui.available_height() - 8.0).max(CHARACTER_ROW * 2.0);
    let mut table = egui_extras::TableBuilder::new(ui)
        .id_salt(id_salt)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(48.0))
        .column(Column::remainder().at_least(120.0).clip(true));
    for (_, column) in extra {
        table = table.column(*column);
    }
    table
        .max_scroll_height(table_height)
        .auto_shrink([false, true])
        .header(22.0, |mut header| {
            for text in ["", "Character"].into_iter().chain(extra.iter().map(|(text, _)| *text)) {
                header.col(|ui| _ = ui.label(theme::header_text(text)));
            }
        })
        .body(|body| {
            body.rows(CHARACTER_ROW, pilots.len(), |mut row| {
                let pilot = &pilots[row.index()];
                let RowLook { selected: is_selected, warning: warn } = look(pilot);
                row.set_selected(is_selected);
                row.col(|ui| _ = pilot_portrait(ui, portraits, pilot, 32.0));
                row.col(|ui| {
                    let color = match (warn, is_selected) {
                        (Some(_), _) => theme::WARN,
                        (None, true) => Color32::WHITE,
                        (None, false) => theme::TEXT,
                    };
                    // A long name ends with "…" a few points before the next column, not at its edge.
                    ui.set_max_width((ui.available_width() - NAME_GAP).max(0.0));
                    // The tooltip goes on each label. The labels sense the pointer, so each one is hovered.
                    // The response of the row around them is not hovered when a label is under the pointer.
                    ui.horizontal(|ui| {
                        let tip = |response: egui::Response| {
                            if let Some(text) = warn {
                                response.on_hover_text(text);
                            }
                        };
                        if warn.is_some() {
                            tip(ui.add(egui::Label::new(RichText::new(icons::WARNING.as_str()).color(theme::WARN)).sense(Sense::hover())));
                        }
                        tip(ui.add(egui::Label::new(RichText::new(&pilot.name).color(color)).truncate().sense(Sense::hover())));
                    });
                });
                for column in 0..extra.len() {
                    row.col(|ui| cell(ui, pilot, column));
                }
            });
        });
}

/// The Pochven page: filament travel and the EVE Metro wormhole map, the two features almost all
/// about travel via Pochven. Filament travel needs no client ID. The EVE Metro switch, the key,
/// and a table of the linked characters with their portraits follow it. The service checks the
/// picked character, its corporation and its alliance. Like the Ansiblex page, the page itself
/// stays still and the table scrolls.
fn pochven_page(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, locked: bool, portraits: &mut Portraits) {
    let dim = |text: &str| RichText::new(text).color(theme::TEXT_DIM);
    pochven_group(ui, s, locked);
    ui.add_space(14.0);
    // The source needs a logged-in character, so it needs the EVE client ID, like each SSO
    // feature. Without one, EVE Metro gives nothing, so the whole group stays off the page.
    if !s.pilots.shows_characters() {
        return;
    }
    let field = FIELD;
    // A route cannot use EVE Metro while the Access switch above is off. The switch gives the
    // reason, so the controls here only dim, with no second line of text to repeat it.
    let pochven_off = !s.settings.pochven;
    let disabled = locked || pochven_off;
    let disabled_hover = if locked { LOCKED } else { ACCESS_OFF };
    group(ui, "EVE Metro");
    ui.label(dim("Activation can take a few minutes after the ingame payment arrives."));
    ui.add_space(4.0);
    egui::Grid::new("settings-metro").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        label(ui, "Enable");
        let row = ui.add_enabled_ui(!disabled, |ui| {
            let mut on = s.cfg.eve_metro.enabled;
            let changed = ui.app_checkbox(&mut on, "").changed();
            (changed, on)
        });
        row.response.on_disabled_hover_text(disabled_hover);
        if row.inner.0 {
            s.cfg.eve_metro.enabled = row.inner.1;
            s.eve_metro_saved();
        }
        ui.end_row();

        label(ui, "API Key");
        ui.add_enabled_ui(!disabled, |ui| {
            // The masked key and Clear while a key is set, else the input and Save.
            ui.horizontal_wrapped(|ui| {
                if let Some(key) = &s.cfg.eve_metro.key {
                    ui.label(RichText::new(key.masked()).color(theme::TEXT));
                    if ui.app_button("Clear").clicked() {
                        s.cfg.eve_metro.key = None;
                        s.eve_metro_saved();
                    }
                } else {
                    ui.add(
                        TextEdit::singleline(&mut form.metro_key).password(true).hint_text("paste your EVE Metro key").desired_width(field),
                    );
                    if ui.add_enabled(!form.metro_key.trim().is_empty(), AppButton::new("Save").primary()).clicked() {
                        s.cfg.eve_metro.key = Some(ApiKey(form.metro_key.trim().to_string()));
                        form.metro_key.clear();
                        s.eve_metro_saved();
                        form.saved = Some((Saved::MetroKey, Instant::now()));
                    }
                }
                form.saved_label(ui, Saved::MetroKey);
            })
        })
        .response
        .on_disabled_hover_text(disabled_hover);
        ui.end_row();
    });
    ui.add_space(14.0);
    group(ui, "Character");
    ui.horizontal_wrapped(|ui| {
        ui.label(dim("The selected character, or its corporation or its alliance, must have a valid"));
        ui.hyperlink_to("EVE Metro", "https://evemetro.com/");
        ui.label(dim("subscription, a third-party service."));
    });
    ui.add_space(4.0);
    let pilots = s.pilots.characters();
    if pilots.is_empty() {
        ui.label(dim("No linked character. Add one in the Pilots panel."));
        return;
    }
    let picked = s.cfg.eve_metro.character;
    if picked.is_none() {
        ui.label(RichText::new("Select a character. EVE Metro stays off until you do.").color(theme::WARN));
    } else if !pilots.iter().any(|p| Some(p.id) == picked) {
        ui.label(RichText::new("The selected character is no longer linked. Select another one.").color(theme::WARN));
    }
    let mut pick = None;
    use egui_extras::Column;
    let extra = [("Status", Column::exact(110.0)), ("", Column::exact(84.0))];
    character_table(
        ui,
        "eve-metro-characters",
        &pilots,
        portraits,
        &extra,
        |pilot| RowLook { selected: picked == Some(pilot.id), ..Default::default() },
        |ui, pilot, column| match column {
            0 => {
                let (text, color) = pilot_status(pilot);
                ui.label(RichText::new(text).color(color));
            }
            _ if picked == Some(pilot.id) => _ = ui.label(RichText::new("In use").color(theme::accent(ui))),
            _ => {
                if ui.add_enabled(!disabled, AppButton::new("Use")).on_disabled_hover_text(disabled_hover).clicked() {
                    pick = Some(pilot.id);
                }
            }
        },
    );
    if let Some(id) = pick {
        s.cfg.eve_metro.character = Some(id);
        s.eve_metro_saved();
    }
}

/// The Ansiblex page: the alliance capital, a box to import a list, and the bridges. A bridge works
/// in both directions, so each row shows A <-> B one time. The zone of an end is the zone of a jump
/// that leaves from that end.
fn ansiblex_page(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, locked: bool) {
    let field = FIELD;
    group(ui, "Alliance capital");
    egui::Grid::new("settings-ansiblex").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        label(ui, "System");
        ui.add_enabled_ui(!locked, |ui| {
            // The name and Clear while a capital is set, else the search box.
            if let Some(capital) = s.settings.rules.capital {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(s.uni.name(capital)).color(Color32::WHITE));
                    if ui.app_button("Clear").clicked() {
                        s.settings.rules.capital = None;
                        s.recompute();
                        s.save();
                    }
                });
            } else if let Some((_, node)) = form.capital.show(ui, &s.uni, field, "Search system… (none: jump bridges off)", &[]) {
                s.settings.rules.capital = Some(node);
                s.recompute();
                s.save();
            }
        })
        .response
        .on_disabled_hover_text(LOCKED);
        ui.end_row();
    });
    ui.add_space(14.0);
    group(ui, "Import");
    ui.label(RichText::new("Paste a list in SMT format: one line for each bridge, like \"0 A --> B\".").color(theme::TEXT_DIM));
    ui.add_space(4.0);
    ui.add_enabled_ui(!locked, |ui| {
        let edit =
            TextEdit::multiline(&mut form.bridge_text).hint_text("0 Jita --> Perimeter").desired_rows(3).desired_width(f32::INFINITY);
        ui.add(edit);
        for p in &form.bridge_problems {
            ui.label(RichText::new(p).color(theme::ERROR));
        }
        ui.horizontal(|ui| {
            let empty = form.bridge_text.trim().is_empty();
            let (mut add, mut replace) = (false, false);
            if form.replace_confirm {
                ui.label(RichText::new(format!("Remove the {} bridges in the list?", s.bridges.len())).color(theme::WARN));
                replace = ui.app_button("Replace list").clicked();
                if ui.app_button("Cancel").clicked() || replace {
                    form.replace_confirm = false;
                }
            } else {
                add = ui.add_enabled(!empty, AppButton::new("Add to list").primary()).clicked();
                if ui.add_enabled(!empty, AppButton::new("Replace list")).clicked() {
                    // An empty list loses nothing, so it needs no confirmation.
                    form.replace_confirm = !s.bridges.is_empty();
                    replace = !form.replace_confirm;
                }
            }
            if add || replace {
                form.bridge_problems = s.import_bridges(&form.bridge_text, replace);
                if form.bridge_problems.is_empty() {
                    form.bridge_text.clear();
                }
            }
        });
    })
    .response
    .on_disabled_hover_text(LOCKED);
    ui.add_space(14.0);
    group(ui, &format!("Bridges ({})", s.bridges.len()));
    if s.bridges.is_empty() {
        ui.label(RichText::new("No bridges. Paste a list above.").color(theme::TEXT_DIM));
        return;
    }
    if s.settings.rules.capital.is_none() {
        ui.label(RichText::new("Set the alliance capital to see the zones.").color(theme::TEXT_DIM));
    }
    let rows: Vec<_> = s.bridges.iter().map(|&(a, b)| (a, b, zone_text(s, a), zone_text(s, b))).collect();
    let mut remove = None;
    // The table takes the height that is left, minus the row of the Clear all button.
    let table_height = (ui.available_height() - 44.0).max(84.0);
    use egui_extras::Column;
    egui_extras::TableBuilder::new(ui)
        .id_salt("ansiblex-bridges")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(110.0).clip(true))
        .column(Column::exact(56.0))
        .column(Column::exact(30.0))
        .column(Column::remainder().at_least(110.0).clip(true))
        .column(Column::exact(56.0))
        .column(Column::exact(80.0))
        .max_scroll_height(table_height)
        .auto_shrink([false, false])
        .header(22.0, |mut header| {
            for text in ["System", "Zone", "", "System", "Zone", ""] {
                header.col(|ui| _ = ui.label(theme::header_text(text)));
            }
        })
        .body(|body| {
            body.rows(28.0, rows.len(), |mut row| {
                let index = row.index();
                let (a, b, za, zb) = &rows[index];
                for (node, zone) in [(*a, za), (*b, zb)] {
                    row.col(|ui| _ = ui.label(RichText::new(s.uni.name(node)).color(theme::TEXT)));
                    row.col(|ui| {
                        let color = if zone.is_some() { theme::BRIDGE } else { theme::TEXT_DIM };
                        let (text, hover) =
                            zone.clone().unwrap_or(("-".into(), "You must set a capital system to calculate ansiblex zones.".into()));
                        ui.label(RichText::new(text).color(color)).on_hover_text(hover);
                    });
                    if node == *a {
                        row.col(|ui| _ = ui.label(RichText::new(icons::ARROWS_LEFT_RIGHT.as_str()).color(theme::BRIDGE)));
                    }
                }
                row.col(|ui| {
                    if ui.add_enabled(!locked, AppButton::new("Remove")).clicked() {
                        remove = Some(index);
                    }
                });
            });
        });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if form.bridge_confirm {
            ui.label(RichText::new(format!("Remove all {} bridges?", s.bridges.len())).color(theme::WARN));
            if ui.app_button("Clear all").clicked() {
                s.clear_bridges();
                form.bridge_confirm = false;
            }
            if ui.app_button("Cancel").clicked() {
                form.bridge_confirm = false;
            }
        } else if ui.add_enabled(!locked, AppButton::new("Clear all")).clicked() {
            form.bridge_confirm = true;
        }
    });
    if let Some(i) = remove {
        s.remove_bridge(i);
    }
}

/// The zone text of a bridge end, for example "Zone 3", and the hover text. `None` without a capital.
fn zone_text(s: &Session, node: petgraph::graph::NodeIndex) -> Option<(String, String)> {
    let capital = s.settings.rules.capital?;
    let ly = s.uni.distance_ly(capital, node);
    let (zone, multiplier) = router_core::ansiblex::zone(ly);
    Some((format!("Zone {zone}"), format!("{ly:.1} ly from {}. A jump from here costs {multiplier}x the base cost.", s.uni.name(capital))))
}

fn engine_page(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, locked: bool) {
    egui::Grid::new("settings-engine").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        // The max TJ for one bridge jump.
        label(ui, "Max TJ per bridge jump");
        ui.add_enabled_ui(!locked, |ui| {
            ui.horizontal_wrapped(|ui| {
                let response = ui.add(TextEdit::singleline(&mut form.max_cap).hint_text("no limit").desired_width(120.0));
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.add(AppButton::new("Apply").primary()).clicked() || enter {
                    match parse_max_cap(&form.max_cap, &s.settings.hulls) {
                        Ok(max_cap) => {
                            s.settings.rules.max_cap = max_cap;
                            form.error = None;
                            s.recompute();
                            s.save();
                            form.saved = Some((Saved::MaxCap, Instant::now()));
                        }
                        Err(e) => form.error = Some(e.to_string()),
                    }
                }
                form.saved_label(ui, Saved::MaxCap);
            })
        })
        .response
        .on_disabled_hover_text(LOCKED);
        ui.end_row();
        // The soft costs of a route: the bridge capacitor and the unknown wormhole signature.
        label(ui, "Bridge capacitor cost");
        ui.add_enabled_ui(!locked, |ui| {
            let drag = egui::DragValue::new(&mut s.settings.costs.cap_weight)
                .speed(0.05)
                .range(0.0..=RouteCosts::MAX)
                .max_decimals(2)
                .suffix(" jumps per 1% of a gate");
            if ui
                .add(drag)
                .on_hover_text("Raise this to make routes avoid bridge jumps that use much gate capacitor. At 0.5, a jump that uses 20% counts as 10 extra jumps.")
                .changed()
            {
                s.apply_later(Instant::now());
            }
        })
        .response
        .on_disabled_hover_text(LOCKED);
        ui.end_row();
        label(ui, "Unknown signature cost");
        ui.add_enabled_ui(!locked, |ui| {
            ui.horizontal_wrapped(|ui| {
                let costs = &mut s.settings.costs;
                let drag = egui::DragValue::new(&mut costs.unknown_sig_penalty)
                    .speed(0.1)
                    .range(0.0..=RouteCosts::MAX)
                    .max_decimals(1)
                    .suffix(" jumps");
                if ui.add(drag).on_hover_text("A wormhole with no known signature costs this much extra, for the scan").changed() {
                    s.apply_later(Instant::now());
                }
            })
        })
        .response
        .on_disabled_hover_text(LOCKED);
        ui.end_row();
    });
}

/// A text area for a list of systems. Return true when the window closes.
fn paste_list(ui: &mut Ui, text: &mut String, problems: &mut Vec<String>, s: &mut Session) -> bool {
    ui.set_width(crate::theme::modal_width(ui, 460.0));
    title(ui, "Paste list");
    ui.label(RichText::new("One system for each line, or names separated by commas.").color(theme::TEXT_DIM));
    ui.add_space(4.0);
    let edit = TextEdit::multiline(text).hint_text("Jita\nAmarr\nDodixie").desired_rows(12).desired_width(f32::INFINITY);
    let response = ui.add(edit);
    if text.is_empty() && !response.has_focus() {
        response.request_focus();
    }
    for p in problems.iter() {
        ui.label(RichText::new(p).color(theme::ERROR));
    }
    ui.add_space(6.0);
    let mut close = false;
    let mut action = None;
    ui.horizontal(|ui| {
        let empty = text.trim().is_empty();
        if ui.add_enabled(!empty, AppButton::new("Add to route").primary()).clicked() {
            action = Some(false);
        }
        if ui.add_enabled(!empty, AppButton::new("Replace route")).clicked() {
            action = Some(true);
        }
        if ui.app_button("Cancel").clicked() {
            close = true;
        }
    });
    if let Some(replace) = action {
        *problems = if replace { s.replace_list(text) } else { s.add_list(text) };
        // The window stays open while a name has a problem. The added names leave the text,
        // so a second try adds only the fixed names.
        if problems.is_empty() {
            close = true;
        } else {
            *text = router_core::settings::split_systems(text)
                .into_iter()
                .filter(|name| s.uni.resolve(name).is_err())
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    close
}

/// A pick in the Pilot picker.
enum Picked {
    Pilot(u64),
    Hull(Option<HullClass>),
}

/// The characters and the hulls, with one filter. Return true when a row is picked.
fn pilot_picker(ui: &mut Ui, filter: &mut String, s: &mut Session) -> bool {
    ui.set_width(crate::theme::modal_width(ui, 560.0));
    let rows = s.pilots.pilot_rows(filter, &s.settings.hulls);
    title(ui, "Pilot or hull");
    let response = ui.add(TextEdit::singleline(filter).hint_text("Filter by character, ship or group…").desired_width(f32::INFINITY));
    if !response.has_focus() && filter.is_empty() {
        response.request_focus();
    }
    // Enter picks the first match, not the "Unselected" row.
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let mut picked = None;
    if enter {
        picked = rows.iter().find(|r| !matches!(r, PilotRow::Hull(None))).map(|r| match r {
            PilotRow::Pilot(v) => Picked::Pilot(v.id),
            PilotRow::Hull(h) => Picked::Hull(h.clone()),
        });
    }
    ui.add_space(6.0);
    // A fixed height keeps the window the same size for each filter. A window that shrinks to a short
    // list keeps the small size when the list grows again.
    let list_height = 420.0_f32.min((ui.ctx().content_rect().height() - 200.0).max(110.0));
    ScrollArea::vertical().min_scrolled_height(list_height).max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
        for row in &rows {
            let (rect, click) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            if row.is_current(&s.settings) {
                ui.painter().rect_filled(rect, 0.0, theme::ROW_FILL);
                theme::selection_bar(ui, rect);
            } else if click.hovered() {
                ui.painter().rect_filled(rect, 0.0, theme::accent(ui).linear_multiply(0.18));
            }
            let font = egui::TextStyle::Body.resolve(ui.style());
            let p = ui.painter();
            let left = rect.left_center() + vec2(10.0, 0.0);
            match row {
                PilotRow::Pilot(v) => {
                    p.text(left, egui::Align2::LEFT_CENTER, &v.name, font.clone(), Color32::WHITE);
                    let ship =
                        v.live.ship.as_ref().map_or("ship unknown".into(), |sh| ship_text(sh, &v.name, &s.settings.hulls).to_string());
                    p.text(left + vec2(200.0, 0.0), egui::Align2::LEFT_CENTER, ship, font, theme::TEXT_DIM);
                }
                PilotRow::Hull(None) => _ = p.text(left, egui::Align2::LEFT_CENTER, "Unselected", font, theme::TEXT),
                PilotRow::Hull(Some(h)) => {
                    let color = if h.base_tj.is_some() { theme::TEXT } else { theme::TEXT_DIM };
                    p.text(left, egui::Align2::LEFT_CENTER, &h.name, font.clone(), color);
                    p.text(left + vec2(200.0, 0.0), egui::Align2::LEFT_CENTER, &h.group, font.clone(), theme::TEXT_DIM);
                    let tj = h.base_tj.map_or("no JB".into(), |tj| format!("{tj} TJ"));
                    p.text(rect.right_center() - vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, tj, font, color);
                }
            }
            if click.clicked() {
                picked = Some(match row {
                    PilotRow::Pilot(v) => Picked::Pilot(v.id),
                    PilotRow::Hull(h) => Picked::Hull(h.clone()),
                });
            }
        }
    });
    match picked {
        Some(Picked::Pilot(id)) => {
            s.settings.hull_source = HullSource::Pilot(id);
            s.pilots.sync_hull(&mut s.settings);
        }
        Some(Picked::Hull(hull)) => {
            let changed = s.settings.rules.hull.as_ref().map(|h| &h.name) != hull.as_ref().map(|h| &h.name);
            let followed = match s.settings.hull_source {
                HullSource::Pilot(id) => s.pilots.characters().into_iter().find(|c| c.id == id).map(|c| c.name),
                HullSource::Manual => None,
            };
            s.settings.hull_source = HullSource::Manual;
            s.settings.rules.hull = hull;
            if let Some(name) = followed {
                s.notify(crate::notice::Level::Info, format!("No longer tracking {name}'s ship type."));
            }
            if let (true, Some(h)) = (changed, &s.settings.rules.hull) {
                let text = format!("Routes planned for the {}. A route may not work for a ship that is larger.", h.name);
                s.recompute();
                s.notify(crate::notice::Level::Warning, text);
                s.save();
                return true;
            }
        }
        None => return false,
    }
    s.recompute();
    s.save();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::test_support::overlay_universe;

    #[test]
    fn each_page_has_one_entry_and_a_label() {
        let labels: std::collections::HashSet<_> = Page::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(labels.len(), Page::ALL.len());
        assert!(labels.iter().all(|l| !l.is_empty()));
    }

    #[test]
    fn the_status_of_a_character() {
        use router_core::esi::pilots::PilotState;
        let pilot = |online, expired, needs_reauth| PilotView {
            id: 1,
            name: "Alice Ander".into(),
            live: PilotState { online, expired, ..PilotState::default() },
            needs_reauth,
            active: false,
        };
        assert_eq!(pilot_status(&pilot(Some(true), false, false)).0, "Online");
        assert_eq!(pilot_status(&pilot(Some(false), false, false)).0, "Offline");
        assert_eq!(pilot_status(&pilot(None, false, false)).0, "-");
        // A login that is gone, or has a missing scope, needs a new login. This wins over "Online".
        assert_eq!(pilot_status(&pilot(Some(true), true, false)).0, "Log in again");
        assert_eq!(pilot_status(&pilot(Some(true), false, true)).0, "Log in again");
    }

    /// A session with a client ID, the characters Alice and Bo, and a config in a temporary directory.
    fn session_with_pilots() -> (Session, tempfile::TempDir) {
        use router_core::esi::pilots::Pilots;
        use router_core::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts =
            router_core::labels::Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default(), &Default::default());
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Session::new(
            std::sync::Arc::new(uni),
            settings,
            router_core::config::Config::default(),
            tmp.path().join("eve-router.json"),
            shortcuts,
        );
        s.pilots = Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[(1, "Alice Ander"), (2, "Bo Berg")]);
        (s, tmp)
    }

    #[test]
    fn the_pochven_page_draws_every_state() {
        let (mut s, tmp) = session_with_pilots();
        let mut form = SettingsForm::new(&s, Page::Pochven);
        let mut portraits = Portraits::without_download(&[1, 2]);
        // The app registers its fonts and theme first: the page uses the bold font.
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let mut draw = |s: &mut Session, locked: bool| {
            // The frame output holds texture changes. A test with no renderer drops them.
            let mut output = ctx.run_ui(Default::default(), |ui| pochven_page(ui, &mut form, s, locked, &mut portraits));
            output.textures_delta.clear();
        };
        // A character is picked, so its row is selected. The key is set, so the masked key shows.
        s.cfg.eve_metro.character = Some(2);
        s.cfg.eve_metro.key = Some(ApiKey("metro_test_key".into()));
        draw(&mut s, false);
        // A picked character that is not linked, a locked page, and no characters at all.
        s.cfg.eve_metro.character = Some(99);
        draw(&mut s, true);
        s.pilots = router_core::esi::pilots::Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[]);
        draw(&mut s, false);
        // Without a client ID, the EVE Metro part of the page draws only the reason, but the Pochven
        // switch draws regardless: filament travel needs no client ID.
        s.pilots = router_core::esi::pilots::Pilots::offline(tmp.path().join("active-route.json"));
        draw(&mut s, false);
        // Drawing changes no setting.
        assert_eq!(s.cfg.eve_metro.character, Some(99));
        assert!(!s.settings.pochven);
    }

    #[cfg(windows)]
    #[test]
    fn the_overlay_page_draws_every_state() {
        use config::{Place, Pos};
        let (mut s, tmp) = session_with_pilots();
        let mut portraits = Portraits::without_download(&[1, 2]);
        let mut overlay = Overlay::default();
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let mut draw = |s: &mut Session, overlay: &mut Overlay| {
            let mut output = ctx.run_ui(Default::default(), |ui| overlay_page(ui, s, &mut portraits, overlay));
            output.textures_delta.clear();
        };
        // The overlay is off and no position is set.
        draw(&mut s, &mut overlay);
        // The overlay is on, with a global position and the own position of one character.
        s.cfg.routing_overlay.enabled = true;
        s.cfg.routing_overlay.set_position(Place::Global, Some(Pos { x: 10, y: 20 }));
        s.cfg.routing_overlay.set_position(Place::Character(2), Some(Pos { x: -5, y: 7 }));
        draw(&mut s, &mut overlay);
        // The editor is open for the global place, then for a character.
        overlay.begin_edit(Place::Global);
        draw(&mut s, &mut overlay);
        overlay.begin_edit(Place::Character(1));
        draw(&mut s, &mut overlay);
        // No linked character.
        s.pilots = router_core::esi::pilots::Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[]);
        draw(&mut s, &mut overlay);
        // Drawing changes no setting and no position.
        assert!(s.cfg.routing_overlay.enabled);
        assert_eq!(s.cfg.routing_overlay.position_of(Place::Global), Some(Pos { x: 10, y: 20 }));
        assert_eq!(s.cfg.routing_overlay.position_of(Place::Character(2)), Some(Pos { x: -5, y: 7 }));
        assert_eq!(overlay.editing(), Some(Place::Character(1)));
    }

    #[test]
    fn a_character_table_draws_a_warning_row() {
        let (s, _tmp) = session_with_pilots();
        let mut portraits = Portraits::without_download(&[1, 2]);
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let pilots = s.pilots.characters();
        let extra = [("Status", egui_extras::Column::exact(110.0))];
        let mut cells = 0;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            // The first character has a warning and the selected look. The second has neither.
            let look = |pilot: &PilotView| RowLook { selected: pilot.id == 1, warning: (pilot.id == 1).then_some("A warning.") };
            character_table(ui, "test-characters", &pilots, &mut portraits, &extra, look, |_, _, _| cells += 1);
        });
        output.textures_delta.clear();
        assert_eq!(cells, pilots.len());
    }

    /// The texts that a frame draws, with the position and the size of each.
    fn drawn_texts(output: &egui::FullOutput) -> Vec<(String, egui::Pos2, egui::Vec2)> {
        fn walk(shape: &egui::epaint::Shape, found: &mut Vec<(String, egui::Pos2, egui::Vec2)>) {
            match shape {
                egui::epaint::Shape::Text(text) => found.push((text.galley.text().to_string(), text.pos, text.galley.size())),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        output.shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut found));
        found
    }

    #[test]
    fn the_warning_tooltip_shows_when_the_pointer_is_on_the_name() {
        let (s, _tmp) = session_with_pilots();
        let mut portraits = Portraits::without_download(&[1, 2]);
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        ctx.all_styles_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let pilots = s.pilots.characters();
        let extra = [("Status", egui_extras::Column::exact(110.0))];
        let mut frame = |pointer: Option<egui::Pos2>, time: f64| {
            let mut input = egui::RawInput { time: Some(time), ..Default::default() };
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0)));
            input.events.extend(pointer.map(egui::Event::PointerMoved));
            let look = |pilot: &PilotView| RowLook { selected: false, warning: (pilot.id == 1).then_some("Tooltip of the warning.") };
            let mut output = ctx.run_ui(input, |ui| {
                character_table(ui, "tooltip-characters", &pilots, &mut portraits, &extra, look, |_, _, _| {});
            });
            output.textures_delta.clear();
            drawn_texts(&output)
        };
        // The first frame lays the table out. No pointer, so no tooltip.
        let texts = frame(None, 0.0);
        assert!(!texts.iter().any(|(t, ..)| t.contains("Tooltip of the warning.")));
        let (_, pos, size) = texts.iter().find(|(t, ..)| t == "Alice Ander").expect("the name is drawn").clone();
        let on_name = pos + size / 2.0;
        // The pointer rests on the name for some frames.
        let mut shown = false;
        for step in 1..=4 {
            shown |= frame(Some(on_name), f64::from(step)).iter().any(|(t, ..)| t.contains("Tooltip of the warning."));
        }
        assert!(shown, "the tooltip did not show");
        // A name with no warning shows no tooltip.
        let (_, pos, size) = frame(None, 5.0).iter().find(|(t, ..)| t == "Bo Berg").expect("the second name is drawn").clone();
        let mut shown = false;
        for step in 6..=9 {
            shown |= frame(Some(pos + size / 2.0), f64::from(step)).iter().any(|(t, ..)| t.contains("Tooltip of the warning."));
        }
        assert!(!shown, "a row with no warning showed the tooltip");
    }

    #[cfg(windows)]
    #[test]
    fn only_the_unknown_shape_warns_about_fullscreen() {
        use crate::overlay::DisplayMode;
        assert!(!may_be_fullscreen(None));
        assert!(!may_be_fullscreen(Some(DisplayMode::Borderless)));
        assert!(!may_be_fullscreen(Some(DisplayMode::Windowed)));
        assert!(may_be_fullscreen(Some(DisplayMode::Other)));
    }

    #[test]
    fn region_search_matches_any_part_of_a_name() {
        let uni = overlay_universe();
        assert_eq!(region_matches(&uni, " forg", 6), ["The Forge"]);
        assert_eq!(region_matches(&uni, "FORGE", 6), ["The Forge"]);
        assert!(region_matches(&uni, "", 6).is_empty());
        assert!(region_matches(&uni, "e", 3).len() <= 3);
    }
}
