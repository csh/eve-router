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
use router_core::settings::{HullSource, parse_max_cap};
use router_core::sources::{
    self,
    nexum::{self, MapInfo},
};

pub enum Popup {
    /// The Pilot picker. The rows are `Pilots::pilot_rows(filter)`.
    Pilot {
        filter: String,
    },
    Maps(Vec<MapInfo>),
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
    Frame::new().fill(theme::PANEL).stroke(Stroke::new(1.0, theme::ACCENT)).inner_margin(Margin::same(14))
}

/// The title of a modal window, with an accent line below it.
pub(crate) fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).family(theme::bold()).color(theme::ACCENT).extra_letter_spacing(2.0));
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

enum Action {
    LoadMaps,
    Close,
}

fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
}

/// The hover text of a control that a route start locks.
pub const LOCKED: &str = "Locked while route is active — stop the route to change this";

fn settings(ui: &mut Ui, form: &mut SettingsForm, s: &mut Session, loading: bool) -> Option<Action> {
    let mut action = None;
    ui.set_width(560.0);
    title(ui, "Settings");
    // While a route is active, only the favourites can change.
    let locked = s.pilots.active.is_some();
    if locked {
        ui.label(RichText::new("Routing settings are locked while a route is active. Favourites stay editable.").color(theme::WARN));
    }
    ScrollArea::vertical().max_height(ui.ctx().content_rect().height() - 160.0).show(ui, |ui| {
        egui::Grid::new("settings-grid").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
            // The alliance capital.
            label(ui, "Alliance capital");
            ui.add_enabled_ui(!locked, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let text = s.settings.rules.capital.map_or("none (jump bridges off)".into(), |n| s.uni.name(n).to_string());
                        ui.label(RichText::new(text).color(Color32::WHITE));
                        if s.settings.rules.capital.is_some() && ui.small_button("Clear").clicked() {
                            s.settings.rules.capital = None;
                            s.recompute();
                            s.save();
                        }
                    });
                    if let Some((_, node)) = form.capital.show(ui, &s.uni, 300.0, "Search system…", &[]) {
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
            label(ui, "Max TJ per bridge jump");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
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

            // The favourites, in the order of the sidebar.
            label(ui, "Favourites");
            ui.vertical(|ui| {
                let mut edit: Option<(usize, i32)> = None;
                let count = s.settings.favourites.len();
                for (i, &fav) in s.settings.favourites.iter().enumerate() {
                    ui.horizontal(|ui| {
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
                if let Some((_, node)) = form.favourite.show(ui, &s.uni, 300.0, "Add favourite…", &[]) {
                    s.add_favourite(node);
                }
            });
            ui.end_row();

            // Nexum: the URL, the key and the map. A change applies at the next start.
            label(ui, "Nexum URL");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut form.url).hint_text("https://nexum.example").desired_width(300.0));
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

            label(ui, "Nexum key");
            ui.add_enabled_ui(!locked, |ui| {
                ui.vertical(|ui| {
                    let current = s.cfg.nexum.key.as_ref().map_or("none".into(), ApiKey::masked);
                    ui.label(RichText::new(current).color(theme::TEXT));
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut form.key)
                                .password(true)
                                .hint_text("paste a key with the read scope")
                                .desired_width(300.0),
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

            label(ui, "Nexum map");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
                    let name = match &s.cfg.nexum.map_id {
                        None => "none".into(),
                        Some(id) => s.map_names.iter().find(|m| &m.id == id).map_or(id.clone(), |m| m.name.clone()),
                    };
                    ui.label(RichText::new(name).color(theme::TEXT));
                    if loading {
                        ui.add(egui::Spinner::new().color(theme::ACCENT));
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
            label(ui, "EVE-Scout");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
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
    ui.set_width(460.0);
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
    ui.set_width(560.0);
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
                ui.painter().rect_filled(rect, 0.0, theme::ACCENT_DIM);
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
