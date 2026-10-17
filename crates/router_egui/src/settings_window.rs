//! The modal windows: the settings, the Pilot picker, the Nexum map list and the messages.

use crate::app::Session;
use crate::search::SearchBox;
use crate::theme;
use crate::view::View;
use crate::widgets::{AppButton, UiExt};
use egui::{Button, Color32, Frame, Id, Key, Margin, Modal, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, vec2};
use egui_phosphor_icons::icons;
use router_core::ansiblex::HullClass;
use router_core::config::{self, ApiKey};
use router_core::esi::pilots::PilotRow;
use router_core::labels::ship_text;
use router_core::settings::{HullSource, RouteCosts, parse_max_cap};
use router_core::sources::{
    self,
    nexum::{self, MapInfo},
};
use router_core::universe::display_sec;
use std::time::Instant;

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
    url: String,
    /// The key field starts empty, so the key never shows on the screen.
    key: String,
    error: Option<String>,
}

impl SettingsForm {
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
            url: s.cfg.nexum.url.clone().unwrap_or_default(),
            key: String::new(),
            error: None,
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
        let response = Modal::new(Id::new("settings")).frame(modal_frame()).show(ui.ctx(), |ui| settings(ui, form, s, loading));
        match response.inner {
            Some(Action::LoadMaps) => {
                if view.maps.is_none() {
                    form.maps_error = None;
                    view.maps = Some(fetch_map_list(s, ui.ctx()));
                }
            }
            Some(Action::Close) => view.settings = None,
            None if response.should_close() && view.popup.is_none() => view.settings = None,
            None => {}
        }
        if view.settings.is_none() {
            view.settings_page = page;
            s.flush_apply();
            s.save();
        }
    }

    let close = match &mut view.popup {
        None => false,
        Some(Popup::Pilot { filter }) => {
            let response = Modal::new(Id::new("pilot")).frame(modal_frame()).show(ui.ctx(), |ui| pilot_picker(ui, filter, s));
            response.inner || response.should_close()
        }
        Some(Popup::Avoid { confirm, search, region }) => {
            let response =
                Modal::new(Id::new("avoid")).frame(modal_frame()).show(ui.ctx(), |ui| avoid_list(ui, confirm, search, region, s));
            response.inner || response.should_close()
        }
        Some(Popup::List { text, problems }) => {
            let response = Modal::new(Id::new("list")).frame(modal_frame()).show(ui.ctx(), |ui| paste_list(ui, text, problems, s));
            response.inner || response.should_close()
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
            .max_scroll_height(280.0)
            .auto_shrink([false, true])
            .header(22.0, |mut header| {
                for text in ["System", "Security", "Region", "", ""] {
                    header.col(|ui| _ = ui.label(theme::header_text(text)));
                }
            })
            .body(|body| {
                body.rows(28.0, rows.len(), |mut row| {
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
pub const LOCKED: &str = "Locked while route is active — stop the route to change this";

/// The pages of the settings window, in the order of the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Page {
    #[default]
    Favourites,
    Theme,
    Wormholes,
    Ansiblex,
    Engine,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Favourites, Page::Theme, Page::Wormholes, Page::Ansiblex, Page::Engine];

    pub fn label(self) -> &'static str {
        match self {
            Page::Favourites => "Favourites",
            Page::Theme => "Theme",
            Page::Wormholes => "Wormholes",
            Page::Ansiblex => "Ansiblex",
            Page::Engine => "Engine",
        }
    }
}

/// A small heading for a group of rows on a page.
fn group(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).family(theme::bold()).color(theme::accent(ui)).extra_letter_spacing(1.5));
    ui.add_space(2.0);
}

fn settings(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, loading: bool) -> Option<Action> {
    let mut action = None;
    ui.set_width(crate::theme::modal_width(ui, 720.0));
    title(ui, "Settings");
    // While a route is active, only the favourites and the theme can change.
    let locked = s.pilots.active.is_some();
    if locked {
        ui.label(
            RichText::new("Routing settings are locked while a route is active. Favourites and Theme stay editable.").color(theme::WARN),
        );
    }
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
            // The Ansiblex page has its own scrolling table, so the page itself stays still.
            if form.page == Page::Ansiblex {
                ansiblex_page(ui, form, s, locked);
                return;
            }
            ScrollArea::vertical().max_height(body).auto_shrink([false, false]).show(ui, |ui| match form.page {
                Page::Favourites => favourites_page(ui, form, s),
                Page::Theme => theme_page(ui, s),
                Page::Wormholes => action = wormholes_page(ui, form, s, locked, loading),
                Page::Ansiblex => {}
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
                if ui.add_enabled(i + 1 < count, Button::new(icons::CARET_DOWN.as_str()).small()).clicked() {
                    edit = Some((i, 1));
                }
                if ui.add_enabled(i > 0, Button::new(icons::CARET_UP.as_str()).small()).clicked() {
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
                        }
                        Err(e) => form.error = Some(e.to_string()),
                    }
                }
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
                    }
                }
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
                    } else if ui.small_button("Refresh").on_hover_text("Load the list of maps again").clicked() {
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

/// The Ansiblex page: the alliance capital, a box to import a list, and the bridges. A bridge works
/// in both directions, so each row shows A <-> B one time. The zone of an end is the zone of a jump
/// that leaves from that end.
fn ansiblex_page(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, locked: bool) {
    let field = FIELD;
    group(ui, "Alliance capital");
    egui::Grid::new("settings-ansiblex").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
        label(ui, "Alliance capital");
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
            let add = ui.add_enabled(!empty, AppButton::new("Add to list").primary()).clicked();
            let replace = ui.add_enabled(!empty, AppButton::new("Replace list")).clicked();
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
                        let (text, hover) = zone.clone().unwrap_or(("-".into(), "Set the alliance capital to see the zone".into()));
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
                        }
                        Err(e) => form.error = Some(e.to_string()),
                    }
                }
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
                .on_hover_text("A bridge jump with a hull costs this much for each percent of the gate capacitor it uses")
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
    // Enter picks the first match, not the "none" row.
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let mut picked = None;
    if enter {
        picked = rows.iter().find(|r| !matches!(r, PilotRow::Hull(None))).map(|r| match r {
            PilotRow::Pilot(v) => Picked::Pilot(v.id),
            PilotRow::Hull(h) => Picked::Hull(h.clone()),
        });
    }
    ui.add_space(6.0);
    ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
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
                PilotRow::Hull(None) => _ = p.text(left, egui::Align2::LEFT_CENTER, "none", font, theme::TEXT),
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
            s.settings.hull_source = HullSource::Manual;
            s.settings.rules.hull = hull;
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
    fn region_search_matches_any_part_of_a_name() {
        let uni = overlay_universe();
        assert_eq!(region_matches(&uni, " forg", 6), ["The Forge"]);
        assert_eq!(region_matches(&uni, "FORGE", 6), ["The Forge"]);
        assert!(region_matches(&uni, "", 6).is_empty());
        assert!(region_matches(&uni, "e", 3).len() <= 3);
    }
}
