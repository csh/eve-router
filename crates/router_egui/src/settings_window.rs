//! The modal windows: the settings, the Pilot picker, the Nexum map list and the messages.

use crate::app::Session;
use crate::search::SearchBox;
use crate::theme;
use crate::view::View;
use egui::{Button, Color32, Frame, Id, Key, Margin, Modal, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, vec2};
use router_core::ansiblex::HullClass;
use router_core::config::{self, ApiKey};
use router_core::esi::pilots::PilotRow;
use router_core::labels::{on_off, ship_text};
use router_core::settings::{HullSource, RouteCosts, parse_max_cap};
use router_core::sources::{
    self,
    nexum::{self, MapInfo},
};
use router_core::universe::display_sec;
use std::time::Instant;

pub enum Popup {
    /// The Pilot picker. The rows are `Pilots::pilot_rows(filter)`.
    Pilot {
        filter: String,
    },
    Maps(Vec<MapInfo>),
    /// The avoid list. `confirm` is true while the window asks to clear all entries.
    Avoid {
        confirm: bool,
        search: SearchBox,
        region: String,
    },
    Message(String),
    /// A pasted list of systems. `problems` holds the names that the last try did not add.
    List {
        text: String,
        problems: Vec<String>,
    },
}

/// The text fields of the settings window.
pub struct SettingsForm {
    capital: SearchBox,
    favourite: SearchBox,
    max_cap: String,
    url: String,
    /// The key field starts empty, so the key never shows on the screen.
    key: String,
    error: Option<String>,
}

impl SettingsForm {
    pub fn new(s: &Session) -> Self {
        SettingsForm {
            capital: SearchBox::new("capital-search"),
            favourite: SearchBox::new("favourite-search"),
            max_cap: s.settings.rules.max_cap.map(|m| m.to_string()).unwrap_or_default(),
            url: s.cfg.nexum.url.clone().unwrap_or_default(),
            key: String::new(),
            error: None,
        }
    }
}

pub(crate) fn modal_frame() -> Frame {
    Frame::new().fill(theme::PANEL).stroke(Stroke::new(1.0, theme::LINE)).inner_margin(Margin::same(14))
}

/// The title of a modal window, with an accent line below it.
pub(crate) fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).family(theme::bold()).color(theme::accent(ui)).extra_letter_spacing(2.0));
    let rect = ui.available_rect_before_wrap();
    ui.painter().line_segment([rect.left_top(), rect.left_top() + vec2(rect.width(), 0.0)], Stroke::new(1.0, theme::LINE));
    ui.add_space(8.0);
}

/// Show the open modal windows.
pub fn show(ui: &mut Ui, view: &mut View, s: &mut Session) {
    if let Some(form) = &mut view.settings {
        let loading = view.maps.is_some();
        let response = Modal::new(Id::new("settings")).frame(modal_frame()).show(ui.ctx(), |ui| settings(ui, form, s, loading));
        match response.inner {
            Some(Action::LoadMaps) => {
                let nexum_cfg = s.cfg.nexum.clone();
                let ctx = ui.ctx().clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(nexum::fetch_maps(&nexum_cfg, sources::TIMEOUT));
                    ctx.request_repaint();
                });
                view.maps = Some(rx);
            }
            Some(Action::Close) => view.settings = None,
            None if response.should_close() && view.popup.is_none() => view.settings = None,
            None => {}
        }
        if view.settings.is_none() {
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
        Some(Popup::Maps(maps)) => {
            let response = Modal::new(Id::new("maps")).frame(modal_frame()).show(ui.ctx(), |ui| {
                title(ui, "Nexum map");
                let mut picked = false;
                for map in maps.iter() {
                    let selected = s.cfg.nexum.map_id.as_ref() == Some(&map.id);
                    if ui.add(Button::selectable(selected, &map.name).min_size(vec2(320.0, 0.0))).clicked() {
                        s.cfg.nexum.map_id = Some(map.id.clone());
                        s.nexum_saved();
                        picked = true;
                    }
                }
                picked
            });
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
        Some(Popup::Message(text)) => {
            let response = Modal::new(Id::new("message")).frame(modal_frame()).show(ui.ctx(), |ui| {
                title(ui, "Nexum");
                ui.set_max_width(420.0);
                ui.label(RichText::new(text.as_str()).color(theme::TEXT));
                ui.add_space(8.0);
                ui.button("OK").clicked()
            });
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
    let width = crate::theme::modal_width(ui, 760.0);
    ui.set_width(width);
    // In a narrow window the table drops the Region column and the search boxes use the full width.
    let narrow = width < 640.0;
    let (system_width, region_width) = if narrow { (width - 8.0, width - 8.0) } else { (300.0, 220.0) };
    ui.horizontal_wrapped(|ui| {
        if let Some((_, node)) = search.show(ui, &s.uni, system_width, "Add a system…", &[])
            && !s.settings.avoid.has_system(node)
        {
            s.toggle_avoid_system(node);
        }
        ui.add(TextEdit::singleline(region_text).hint_text("Add a region…").desired_width(region_width));
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
            ui.add(Button::new(RichText::new(text).color(color)).min_size(vec2(64.0, 0.0))).on_hover_text(hover).clicked()
        };
        use egui_extras::Column;
        let table = egui_extras::TableBuilder::new(ui)
            .id_salt("avoid-table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        let table = if narrow {
            table
                .column(Column::remainder().at_least(80.0).clip(true))
                .column(Column::exact(40.0))
                .column(Column::exact(68.0))
                .column(Column::exact(34.0))
        } else {
            table
                .column(Column::remainder().at_least(160.0).clip(true))
                .column(Column::exact(80.0))
                .column(Column::initial(200.0).at_least(120.0).clip(true))
                .column(Column::exact(84.0))
                .column(Column::exact(84.0))
        };
        table
            .max_scroll_height(280.0)
            .auto_shrink([false, true])
            .header(22.0, |mut header| {
                let names: &[&str] = if narrow { &["System", "Sec", "", ""] } else { &["System", "Security", "Region", "", ""] };
                for text in names {
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
                            if !narrow {
                                row.col(|ui| _ = ui.label(RichText::new(&sys.region).color(theme::TEXT_DIM)));
                            }
                            row.col(|ui| {
                                if never_switch(ui, entry.never) {
                                    act = Some(AvoidAct::Never(kind, !entry.never));
                                }
                            });
                        }
                        AvoidRow::Region(i) => {
                            let entry = &avoid.regions[i];
                            if narrow {
                                // The region name takes the System column, and the Sec column stays empty.
                                row.col(|ui| _ = ui.add(egui::Label::new(RichText::new(&entry.item).color(theme::TEXT)).truncate()));
                                row.col(|_| {});
                            } else {
                                row.col(|ui| _ = ui.label(RichText::new("All systems").color(theme::TEXT_DIM)));
                                row.col(|_| {});
                                row.col(|ui| _ = ui.label(RichText::new(&entry.item).color(theme::TEXT)));
                            }
                            row.col(|ui| {
                                if never_switch(ui, entry.never) {
                                    act = Some(AvoidAct::Never(kind, !entry.never));
                                }
                            });
                        }
                    }
                    row.col(|ui| {
                        let remove = if narrow { ui.button("🗙").on_hover_text("Remove") } else { ui.button("Remove") };
                        if remove.clicked() {
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
            if ui.button("Clear all").clicked() {
                clear = true;
            }
            if ui.button("Cancel").clicked() {
                *confirm = false;
            }
        } else {
            if ui.add_enabled(count > 0, Button::new("Clear all")).clicked() {
                *confirm = true;
            }
            close = ui.button("Close").clicked();
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

/// The label of a settings row. With `narrow`, the row ends after the label, so the control gets its own row.
fn row_label(ui: &mut Ui, narrow: bool, text: &str) {
    label(ui, text);
    if narrow {
        ui.end_row();
    }
}

/// The hover text of a control that a route start locks.
pub const LOCKED: &str = "Locked while route is active — stop the route to change this";

fn settings(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, loading: bool) -> Option<Action> {
    let mut action = None;
    let width = crate::theme::modal_width(ui, 560.0);
    ui.set_width(width);
    // In a narrow window each label sits above its control, in one column.
    let narrow = width < 500.0;
    let field = if narrow { (width - 110.0).max(120.0) } else { 300.0 };
    title(ui, "Settings");
    // While a route is active, only the favourites can change.
    let locked = s.pilots.active.is_some();
    if locked {
        ui.label(RichText::new("Routing settings are locked while a route is active. Favourites stay editable.").color(theme::WARN));
    }
    ScrollArea::vertical().max_height(ui.ctx().content_rect().height() - 160.0).show(ui, |ui| {
        egui::Grid::new("settings-grid").num_columns(if narrow { 1 } else { 2 }).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
            // The alliance capital.
            row_label(ui, narrow, "Alliance capital");
            ui.add_enabled_ui(!locked, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        let text = s.settings.rules.capital.map_or("none (jump bridges off)".into(), |n| s.uni.name(n).to_string());
                        ui.label(RichText::new(text).color(Color32::WHITE));
                        if s.settings.rules.capital.is_some() && ui.small_button("Clear").clicked() {
                            s.settings.rules.capital = None;
                            s.recompute();
                            s.save();
                        }
                    });
                    if let Some((_, node)) = form.capital.show(ui, &s.uni, field, "Search system…", &[]) {
                        s.settings.rules.capital = Some(node);
                        s.recompute();
                        s.save();
                    }
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            // The max TJ for one bridge jump.
            row_label(ui, narrow, "Max TJ per bridge jump");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let response = ui.add(TextEdit::singleline(&mut form.max_cap).hint_text("no limit").desired_width(120.0));
                    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if ui.button("Apply").clicked() || enter {
                        match parse_max_cap(&form.max_cap) {
                            Ok(max_cap) => {
                                s.settings.rules.max_cap = max_cap;
                                form.error = None;
                                s.recompute();
                                s.save();
                            }
                            Err(e) => form.error = Some(e),
                        }
                    }
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            // The soft costs of a route: the bridge capacitor and the unknown wormhole signature.
            row_label(ui, narrow, "Bridge capacitor cost");
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

            row_label(ui, narrow, "Unknown signature cost");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let costs = &mut s.settings.costs;
                    let drag = egui::DragValue::new(&mut costs.unknown_sig_penalty)
                        .speed(0.1)
                        .range(0.0..=RouteCosts::MAX)
                        .max_decimals(1)
                        .suffix(" jumps");
                    if ui
                        .add_enabled(!costs.unknown_sig_broken, drag)
                        .on_hover_text("A wormhole with no known signature costs this much extra, for the scan")
                        .changed()
                    {
                        s.apply_later(Instant::now());
                    }
                    let costs = &mut s.settings.costs;
                    if ui
                        .checkbox(&mut costs.unknown_sig_broken, "Skip these wormholes instead")
                        .on_hover_text("Never route through a wormhole with no known signature. Stale data can then cut the map")
                        .changed()
                    {
                        s.recompute();
                        s.save();
                    }
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            // The favourites, in the order of the sidebar.
            row_label(ui, narrow, "Favourites");
            ui.vertical(|ui| {
                let mut edit: Option<(usize, i32)> = None;
                let count = s.settings.favourites.len();
                for (i, &fav) in s.settings.favourites.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        let name = egui::Label::new(RichText::new(s.uni.name(fav)).color(theme::TEXT)).truncate();
                        ui.allocate_ui_with_layout(vec2(180.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.add(name));
                        if ui.add_enabled(i > 0, Button::new("⏶").small()).clicked() {
                            edit = Some((i, -1));
                        }
                        if ui.add_enabled(i + 1 < count, Button::new("⏷").small()).clicked() {
                            edit = Some((i, 1));
                        }
                        if ui.add(Button::new("🗙").small()).on_hover_text("Remove").clicked() {
                            edit = Some((i, 0));
                        }
                    });
                }
                if let Some((i, step)) = edit {
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
            ui.end_row();

            // Nexum: the URL, the key and the map. A change applies at the next start.
            row_label(ui, narrow, "Nexum URL");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(TextEdit::singleline(&mut form.url).hint_text("https://nexum.example").desired_width(field));
                    if ui.button("Save").clicked() {
                        match config::parse_nexum_url(&form.url) {
                            Ok(url) => {
                                s.cfg.nexum.url = url;
                                form.error = None;
                                s.nexum_saved();
                            }
                            Err(e) => form.error = Some(e),
                        }
                    }
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            row_label(ui, narrow, "Nexum key");
            ui.add_enabled_ui(!locked, |ui| {
                ui.vertical(|ui| {
                    let current = s.cfg.nexum.key.as_ref().map_or("none".into(), ApiKey::masked);
                    ui.label(RichText::new(current).color(theme::TEXT));
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut form.key)
                                .password(true)
                                .hint_text("paste a key with the read scope")
                                .desired_width(field),
                        );
                        if ui.add_enabled(!form.key.trim().is_empty(), Button::new("Save")).clicked() {
                            s.cfg.nexum.key = Some(ApiKey(form.key.trim().to_string()));
                            form.key.clear();
                            s.nexum_saved();
                        }
                        if s.cfg.nexum.key.is_some() && ui.button("Clear").clicked() {
                            s.cfg.nexum.key = None;
                            s.nexum_saved();
                        }
                    });
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            row_label(ui, narrow, "Nexum map");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let name = s.map_name();
                    ui.label(RichText::new(name).color(theme::TEXT));
                    if loading {
                        ui.add(egui::Spinner::new().color(theme::accent(ui)));
                        ui.label(RichText::new("Loading maps…").color(theme::TEXT_DIM));
                    } else if ui.button("Choose map…").clicked() {
                        if s.cfg.nexum.url.is_none() || s.cfg.nexum.key.is_none() {
                            form.error = Some("Set the Nexum URL and key first".into());
                        } else {
                            action = Some(Action::LoadMaps);
                        }
                    }
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            // The EVE-Scout hub switches act at route time.
            row_label(ui, narrow, "EVE-Scout");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let hubs = &mut s.settings.hubs;
                    let thera_text = format!("Thera: {}", on_off(hubs.thera));
                    let thera = ui.checkbox(&mut hubs.thera, thera_text).changed();
                    let turnur_text = format!("Turnur: {}", on_off(hubs.turnur));
                    let turnur = ui.checkbox(&mut hubs.turnur, turnur_text).changed();
                    if thera || turnur {
                        s.recompute();
                        s.save();
                    }
                })
            })
            .response
            .on_disabled_hover_text(LOCKED);
            ui.end_row();

            // Photon UI theme and layout settings.
            row_label(ui, narrow, "Photon Theme");
            ui.horizontal_wrapped(|ui| {
                let mut current_theme = s.faction_theme();
                egui::ComboBox::from_id_salt("faction-theme-combo")
                    .selected_text(current_theme.label())
                    .show_ui(ui, |ui| {
                        for &t in &theme::FactionTheme::ALL {
                            if ui.selectable_value(&mut current_theme, t, t.label()).clicked() {
                                s.set_faction_theme(t, ui.ctx());
                            }
                        }
                    });
            });
            ui.end_row();

            row_label(ui, narrow, "Layout Density");
            ui.horizontal_wrapped(|ui| {
                let mut compact = s.compact_mode();
                if ui.checkbox(&mut compact, "Compact Mode (dense rows and padding)").changed() {
                    s.set_compact_mode(compact, ui.ctx());
                }
            });
            ui.end_row();
        });
    });

    ui.add_space(10.0);
    if let Some(e) = &form.error {
        ui.label(RichText::new(e).color(theme::ERROR));
    } else if !s.status.is_empty() {
        ui.add(egui::Label::new(RichText::new(&s.status).color(theme::WARN)).wrap());
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                action = Some(Action::Close);
            }
        });
    });
    action
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
        if ui.add_enabled(!empty, Button::new("Add to route")).clicked() {
            action = Some(false);
        }
        if ui.add_enabled(!empty, Button::new("Replace route")).clicked() {
            action = Some(true);
        }
        if ui.button("Cancel").clicked() {
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
    let rows = s.pilots.pilot_rows(filter);
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
            PilotRow::Hull(h) => Picked::Hull(*h),
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
                    let ship = v.live.ship.as_ref().map_or("ship unknown".into(), |sh| ship_text(sh, &v.name).to_string());
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
                    PilotRow::Hull(h) => Picked::Hull(*h),
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
    s.save_quietly();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::test_support::overlay_universe;

    #[test]
    fn region_search_matches_any_part_of_a_name() {
        let uni = overlay_universe();
        assert_eq!(region_matches(&uni, " forg", 6), ["The Forge"]);
        assert_eq!(region_matches(&uni, "FORGE", 6), ["The Forge"]);
        assert!(region_matches(&uni, "", 6).is_empty());
        assert!(region_matches(&uni, "e", 3).len() <= 3);
    }
}
