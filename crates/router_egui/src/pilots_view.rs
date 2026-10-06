//! The EVE login, the route start, the active route and the pilot avatars.

use crate::app::Session;
use crate::theme::{self, panel, panel_with};
use egui::load::SizedTexture;
use egui::{
    Align, Button, Color32, ColorImage, CornerRadius, Frame, Id, Label, Layout, Margin, Modal, RichText, Sense, Stroke, StrokeKind,
    TextEdit, TextureHandle, TextureOptions, Ui, vec2,
};
use egui_extras::{Column, TableBuilder};
use router_core::esi::active::{ActiveRoute, Hop};
use router_core::esi::client::{IMAGES_URL, portrait};
use router_core::esi::pilots::{HullSync, PilotView, Pilots, StartPlan};
use router_core::labels::{hull_label, ship_text, wormhole_hint};
use router_core::route::Stop;
use router_core::settings::HullSource;
use router_core::universe::display_sec;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

/// At most this many avatars show in one cell. More give "+n other characters".
pub const MAX_AVATARS: usize = 3;
/// The size of an avatar in the route table and in the Pilots panel.
pub const AVATAR: f32 = 20.0;
/// The portrait size to download. 64 px is sharp at 32 px on a 2x display.
const PORTRAIT_SIZE: u32 = 64;

/// "+1 other character" or "+4 other characters".
pub fn overflow_text(hidden: usize) -> String {
    if hidden == 1 { "+1 other character".into() } else { format!("+{hidden} other characters") }
}

/// "Alice Ander" gives "AA".
pub fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}

/// The portraits. Each one downloads one time, on a thread, and stays in a texture.
pub struct Portraits {
    /// `None` while the download runs, and after a failure. Then the avatar shows initials.
    textures: HashMap<u64, Option<TextureHandle>>,
    tx: Sender<(u64, Option<ColorImage>)>,
    rx: Receiver<(u64, Option<ColorImage>)>,
}

impl Default for Portraits {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Portraits { textures: HashMap::new(), tx, rx }
    }
}

impl Portraits {
    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((id, image)) = self.rx.try_recv() {
            let texture = image.map(|image| ctx.load_texture(format!("portrait-{id}"), image, TextureOptions::LINEAR));
            self.textures.insert(id, texture);
        }
    }

    fn get(&mut self, ctx: &egui::Context, id: u64) -> Option<&TextureHandle> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.textures.entry(id) {
            entry.insert(None);
            let (tx, ctx) = (self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let image = portrait(IMAGES_URL, id, PORTRAIT_SIZE).ok().and_then(|bytes| decode(&bytes));
                let _ = tx.send((id, image));
                ctx.request_repaint();
            });
        }
        self.textures.get(&id)?.as_ref()
    }
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// One avatar, with the name on hover. The active pilot has an accent ring. An offline pilot
/// shows at 40 % alpha. Before the portrait loads, the initials show.
pub fn avatar(ui: &mut Ui, portraits: &mut Portraits, pilot: &PilotView, size: f32) -> egui::Response {
    let tint = if pilot.live.online == Some(false) { Color32::from_white_alpha(102) } else { Color32::WHITE };
    let radius = CornerRadius::same(3);
    let response = match portraits.get(ui.ctx(), pilot.id) {
        Some(texture) => ui.add(
            egui::Image::new(SizedTexture::new(texture.id(), vec2(size, size))).corner_radius(radius).tint(tint).sense(Sense::hover()),
        ),
        None => {
            let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
            ui.painter().rect_filled(rect, radius, theme::HEADER);
            let font = egui::FontId::proportional(size * 0.45);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initials(&pilot.name),
                font,
                theme::TEXT.gamma_multiply(tint.a() as f32 / 255.0),
            );
            response
        }
    };
    if pilot.active {
        ui.painter().rect_stroke(response.rect, radius, Stroke::new(1.0, theme::ACCENT), StrokeKind::Outside);
    }
    response.on_hover_text(&pilot.name)
}

/// The avatars of the pilots in one system: at most `MAX_AVATARS`, then "+n other characters".
/// The hidden names show on hover of that text.
pub fn avatar_row(ui: &mut Ui, portraits: &mut Portraits, here: &[PilotView]) {
    ui.spacing_mut().item_spacing.x = 4.0;
    for pilot in here.iter().take(MAX_AVATARS) {
        avatar(ui, portraits, pilot, AVATAR);
    }
    if here.len() > MAX_AVATARS {
        let size = egui::TextStyle::Body.resolve(ui.style()).size * 0.85;
        let text = RichText::new(overflow_text(here.len() - MAX_AVATARS)).italics().size(size).color(theme::TEXT_DIM);
        ui.add(Label::new(text).truncate().sense(Sense::hover())).on_hover_ui(|ui| {
            for pilot in &here[MAX_AVATARS..] {
                ui.label(RichText::new(&pilot.name).color(theme::TEXT));
            }
        });
    }
}

/// The pilots in a system, in the order of the character list: the active pilot first.
pub fn pilots_in(characters: &[PilotView], system: u32) -> Vec<PilotView> {
    characters.iter().filter(|p| p.live.system == Some(system)).cloned().collect()
}

/// The step of a route start.
pub enum Start {
    /// Two or more characters: choose one.
    Pick {
        route: usize,
        chosen: u64,
    },
    Confirm(Box<StartPlan>),
}

/// The UI state of the login, the route start and the active route.
#[derive(Default)]
pub struct PilotsUi {
    pub portraits: Portraits,
    pub characters_open: bool,
    /// "Remove <name>?" for this character.
    remove: Option<u64>,
    paste: String,
    pub start: Option<Start>,
    /// The route to start after the current login.
    start_after_login: Option<usize>,
    stop: bool,
    reroute: Option<Box<ActiveRoute>>,
    show_offline: bool,
    /// The progress at the last frame, to scroll the table to a new step.
    last_progress: Option<usize>,
    /// True when the table must scroll to the current step.
    pub scroll: bool,
}

impl PilotsUi {
    /// Read the tracker and the login. Call this each frame.
    pub fn update(&mut self, ctx: &egui::Context, s: &mut Session) {
        self.portraits.poll(ctx);
        let had_login = s.pilots.login.is_some();
        s.pilots.update();
        match s.pilots.sync_hull(&mut s.settings) {
            HullSync::None => {}
            HullSync::Source => s.save_quietly(),
            // The active route stays as it is. The pilot chooses when to re-route.
            HullSync::Hull if s.pilots.active.is_some() => {
                let kind = s.settings.rules.hull.map_or("an unknown ship".into(), |h| h.name.clone());
                s.status = format!("Ship changed to {kind}. The routes update when this route ends.");
                s.save_quietly();
            }
            HullSync::Hull => {
                s.recompute();
                s.save_quietly();
            }
        }
        for notice in std::mem::take(&mut s.pilots.notices) {
            s.status = notice;
        }
        // The login is done: continue the route start.
        if had_login
            && s.pilots.login.is_none()
            && let Some(route) = self.start_after_login.take()
        {
            self.characters_open = false;
            self.begin_start(s, route);
        }
        let progress = s.pilots.active.as_ref().map(|a| a.progress);
        if progress != self.last_progress {
            self.scroll = progress.is_some();
            self.last_progress = progress;
        }
        // A login or a send waits for a thread. Draw again soon to show the result.
        if s.pilots.login.is_some() || s.pilots.send.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn login(&mut self, s: &mut Session) {
        self.paste.clear();
        if let Err(e) = s.pilots.start_login() {
            s.status = e;
        }
        self.characters_open = true;
    }

    /// "Start route #n": choose the character, then confirm.
    pub fn begin_start(&mut self, s: &mut Session, route: usize) {
        let ids: Vec<u64> = s.pilots.senders().into_iter().map(|c| c.id).collect();
        match ids.len() {
            // A waypoint does nothing without the game client, so an offline pilot is not in the list.
            0 if s.pilots.characters().iter().any(|c| !c.live.expired) => s.status = Pilots::NO_SENDER.into(),
            0 => {
                self.start_after_login = Some(route);
                self.login(s);
            }
            1 => self.confirm(s, route, ids[0]),
            _ => {
                let chosen = s.pilots.last_used().filter(|id| ids.contains(id)).unwrap_or(ids[0]);
                self.start = Some(Start::Pick { route, chosen });
            }
        }
    }

    fn confirm(&mut self, s: &mut Session, route: usize, id: u64) {
        let Some(pilot) = s.pilots.characters().into_iter().find(|c| c.id == id) else { return };
        let Some(r) = s.routes.get(route) else { return };
        let plan = StartPlan::new(&s.uni, &s.settings, r, route + 1, &pilot, router_core::wormhole::now());
        if let Some(e) = &plan.error {
            s.status.clone_from(e);
        }
        self.start = Some(Start::Confirm(Box::new(plan)));
    }

    /// The "Characters (n)" button of the top bar.
    pub fn characters_button(&mut self, ui: &mut Ui, s: &Session) {
        if !s.pilots.shows_characters() {
            return;
        }
        let count = s.pilots.characters().len();
        if ui.button(format!("Characters ({count})")).clicked() {
            self.characters_open = true;
        }
    }

    /// All windows of this module.
    pub fn windows(&mut self, ui: &mut Ui, s: &mut Session) {
        self.resume_window(ui, s);
        self.characters_window(ui, s);
        self.start_window(ui, s);
        self.sending_window(ui, s);
        self.stop_window(ui, s);
        self.reroute_window(ui, s);
    }

    fn resume_window(&mut self, ui: &mut Ui, s: &mut Session) {
        let Some(route) = &s.pilots.resume else { return };
        let destination = destination(s, route);
        let text = format!("Resume route #{} to {destination} for {}?", route.number, route.character_name);
        match question(ui, "resume", "Resume route", &text, &[("Resume", true), ("Discard", false)]) {
            Some(0) => s.pilots.resume_route(),
            Some(_) => s.pilots.discard_resume(),
            None => {}
        }
    }

    fn characters_window(&mut self, ui: &mut Ui, s: &mut Session) {
        if !self.characters_open {
            return;
        }
        let response = Modal::new(Id::new("characters")).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
            ui.set_width(520.0);
            crate::settings_window::title(ui, "Characters");
            // No keyring: offer a login for this session only.
            if s.pilots.accounts.is_none() && s.pilots.keyring_error.is_some() {
                ui.label(RichText::new(s.pilots.keyring_error.clone().unwrap_or_default()).color(theme::TEXT_DIM));
                ui.label(RichText::new("No system keyring found. Keep login for this session only?").color(theme::TEXT));
                ui.horizontal(|ui| {
                    if ui.button("Session only").clicked() {
                        s.pilots.use_session_only();
                    }
                    if ui.button("Cancel").clicked() {
                        self.characters_open = false;
                    }
                });
                return;
            }
            let rows = s.pilots.characters();
            if rows.is_empty() && s.pilots.login.is_none() {
                ui.label(RichText::new("No characters. Add one to send routes to the game.").color(theme::TEXT_DIM));
            }
            for row in &rows {
                ui.horizontal(|ui| {
                    avatar(ui, &mut self.portraits, row, 32.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&row.name).family(theme::bold()).color(Color32::WHITE));
                        status_text(ui, s, row);
                        ship_row(ui, row);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            self.remove = Some(row.id);
                        }
                        let again = if row.live.expired { Some("Log in again") } else { row.needs_reauth.then_some("Re-authorize") };
                        if let Some(text) = again
                            && ui.button(text).clicked()
                        {
                            self.login(s);
                        }
                    });
                });
                if self.remove == Some(row.id) {
                    ui.horizontal(|ui| {
                        let text = format!("Remove {}? This revokes access and deletes stored tokens.", row.name);
                        ui.label(RichText::new(text).color(theme::WARN));
                        if ui.button("Remove").clicked() {
                            s.pilots.remove(row.id);
                            self.remove = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.remove = None;
                        }
                    });
                }
                ui.separator();
            }
            self.login_section(ui, s);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let blocked = s.pilots.login_blocked();
                if s.pilots.login.is_none() {
                    let add = ui.add_enabled(blocked.is_none(), Button::new("Add character"));
                    if let Some(reason) = &blocked {
                        add.on_disabled_hover_text(reason);
                    } else if add.clicked() {
                        self.login(s);
                    }
                }
                if ui.button("Close").clicked() {
                    self.characters_open = false;
                }
            });
            if s.pilots.accounts.as_ref().is_some_and(|a| a.is_session_only()) {
                ui.label(RichText::new("Session only: the logins end when the app closes.").color(theme::TEXT_DIM).small());
            }
        });
        if response.should_close() && s.pilots.login.is_none() {
            self.characters_open = false;
        }
    }

    fn login_section(&mut self, ui: &mut Ui, s: &mut Session) {
        let Some(flow) = &s.pilots.login else { return };
        let url = flow.login.url.clone();
        let listen_error = flow.login.listen_error().map(str::to_owned);
        let (busy, error) = (flow.busy(), flow.error.clone());
        ui.label(RichText::new("Opening EVE login in your browser…").color(theme::TEXT));
        ui.horizontal(|ui| {
            ui.label(RichText::new("Not opened? Copy this URL:").color(theme::TEXT_DIM));
            if ui.small_button("Copy URL").clicked() {
                ui.ctx().copy_text(url.clone());
                s.status = "Login URL copied to the clipboard".into();
            }
        });
        if let Some(e) = listen_error {
            ui.label(RichText::new(e).color(theme::WARN));
        }
        ui.label(RichText::new("Paste the redirected URL here:").color(theme::TEXT_DIM));
        ui.horizontal(|ui| {
            let field =
                ui.add(TextEdit::singleline(&mut self.paste).hint_text("http://localhost:21404/callback?code=…").desired_width(360.0));
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add_enabled(!busy && !self.paste.trim().is_empty(), Button::new("Use URL")).clicked() || enter) && !busy {
                s.pilots.paste(&self.paste);
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().color(theme::ACCENT));
                ui.label(RichText::new("Logging in…").color(theme::TEXT_DIM));
            });
        }
        if let Some(e) = error {
            ui.label(RichText::new(e).color(theme::ERROR));
        }
        if ui.button("Cancel login").clicked() {
            s.pilots.cancel_login();
            self.start_after_login = None;
        }
    }

    fn start_window(&mut self, ui: &mut Ui, s: &mut Session) {
        let Some(start) = &mut self.start else { return };
        let mut next = None;
        let mut close = false;
        let mut plan_for = None;
        let response = Modal::new(Id::new("start")).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
            ui.set_width(480.0);
            match start {
                Start::Pick { route, chosen } => {
                    crate::settings_window::title(ui, &format!("Send route #{} to…", *route + 1));
                    for pilot in s.pilots.senders() {
                        ui.horizontal(|ui| {
                            ui.radio_value(chosen, pilot.id, "");
                            avatar(ui, &mut self.portraits, &pilot, 24.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&pilot.name).color(Color32::WHITE));
                                status_text(ui, s, &pilot);
                                ship_row(ui, &pilot);
                            });
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Continue").clicked() {
                            next = Some((*route, *chosen));
                        }
                        close = ui.button("Cancel").clicked();
                    });
                }
                Start::Confirm(plan) => {
                    let name = plan.planned.character_name.clone();
                    crate::settings_window::title(ui, &format!("Send route #{} to {name}?", plan.planned.number));
                    let route = plan.default_route();
                    let manual = route.manual_hops();
                    let counts = format!(
                        "{} jumps · {} waypoints · {manual} manual jump{}",
                        route.jumps(),
                        route.waypoint_count(),
                        if manual == 1 { "" } else { "s" }
                    );
                    ui.label(RichText::new(counts).color(theme::TEXT));
                    let here = plan.here.map(|id| system_name(s, id));
                    if let Some(here) = &here {
                        ui.label(RichText::new(format!("{name} is in {here}, not on this route.")).color(theme::WARN));
                    }
                    if plan.online == Some(false) {
                        let text = format!("{name} appears offline. Waypoints need the game client running.");
                        ui.label(RichText::new(text).color(theme::WARN));
                    }
                    if let Some(flown) = plan.flown {
                        let planned = s.settings.rules.hull.map_or("no hull".into(), hull_label);
                        ui.label(RichText::new(format!("Planned for {planned}. {name} flies a {}.", flown.name)).color(theme::WARN));
                        if ui.button(format!("Plan for {}", flown.name)).clicked() {
                            plan_for = Some((plan.planned.character, flown.name.clone()));
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let send = if plan.online == Some(false) { "Send anyway" } else { "Send" };
                        match (&here, &plan.from_here) {
                            (Some(here), Some(from_here)) => {
                                if ui.add(Button::new(format!("Route from {here}")).fill(theme::ACCENT.gamma_multiply(0.35))).clicked() {
                                    s.pilots.start_route(from_here.clone());
                                    close = true;
                                }
                                if ui.button("Send as planned").clicked() {
                                    s.pilots.start_route(plan.planned.clone());
                                    close = true;
                                }
                            }
                            _ => {
                                if ui.add(Button::new(send).fill(theme::ACCENT.gamma_multiply(0.35))).clicked() {
                                    s.pilots.start_route(plan.planned.clone());
                                    close = true;
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if close || response.should_close() {
            self.start = None;
        }
        // The route list changes, so the pilot starts the route again from the new list.
        if let Some((id, kind)) = plan_for {
            self.start = None;
            s.settings.hull_source = HullSource::Pilot(id);
            s.pilots.sync_hull(&mut s.settings);
            s.recompute();
            s.save_quietly();
            s.status = format!("Routes planned for the {kind}. Start the route again.");
        }
        if let Some((route, id)) = next {
            self.confirm(s, route, id);
        }
    }

    /// The first send of a route: the progress, or the failure with Retry and Cancel.
    fn sending_window(&mut self, ui: &mut Ui, s: &mut Session) {
        if s.pilots.pending.is_none() {
            return;
        }
        let Some(send) = s.pilots.send.clone() else { return };
        Modal::new(Id::new("sending")).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            let total = send.systems.len();
            match &send.failed {
                None => {
                    crate::settings_window::title(ui, "Start route");
                    ui.label(RichText::new(format!("Sending waypoints {}/{total}…", send.done + 1)).color(theme::TEXT));
                    ui.add(egui::ProgressBar::new(send.done as f32 / total.max(1) as f32).fill(theme::ACCENT));
                }
                Some(error) => {
                    crate::settings_window::title(ui, "Send failed");
                    ui.label(RichText::new(error).color(theme::WARN));
                    ui.horizontal(|ui| {
                        if ui.button("Retry").clicked() {
                            s.pilots.retry_send();
                        }
                        if ui.button("Cancel").clicked() {
                            s.pilots.cancel_send();
                        }
                    });
                }
            }
        });
    }

    fn stop_window(&mut self, ui: &mut Ui, s: &mut Session) {
        if !self.stop {
            return;
        }
        let number = s.pilots.active.as_ref().map_or(0, |a| a.number);
        let text = format!("Stop route #{number}? In-game waypoints stay set; EVE does not allow clearing them from here.");
        match question(ui, "stop", "Stop route", &text, &[("Stop route", true), ("Keep going", false)]) {
            Some(0) => {
                s.pilots.stop_route();
                s.status = "Route stopped. The in-game waypoints stay.".into();
                self.stop = false;
            }
            Some(_) => self.stop = false,
            None => {}
        }
    }

    fn reroute_window(&mut self, ui: &mut Ui, s: &mut Session) {
        let Some(route) = &self.reroute else { return };
        let here = route.steps.first().map(|st| system_name(s, st.system)).unwrap_or_default();
        let text = format!("New route from {here}: {} jumps. Replace in-game waypoints?", route.jumps());
        match question(ui, "reroute", "Re-route", &text, &[("Replace", true), ("Cancel", false)]) {
            Some(0) => {
                if let Some(route) = self.reroute.take() {
                    s.pilots.replace_route(*route);
                }
            }
            Some(_) => self.reroute = None,
            None => {}
        }
    }

    /// The active route: the header, the progress bar, the banner and the step table. It takes
    /// the place of the planner and the route list.
    pub fn active_view(&mut self, ui: &mut Ui, s: &mut Session) {
        let Some(active) = s.pilots.active.clone() else { return };
        let destination = destination(s, &active);
        let jumps = active.jumps();
        let mut stop = false;
        let header = |ui: &mut Ui| stop = ui.button("Stop route").clicked();
        panel_with(ui, "Active route", header, false, |ui| {
            ui.horizontal(|ui| {
                // Oxanium and the egui fallback fonts have no right arrow glyph. Oxanium has "»".
                ui.label(RichText::new(format!("Route #{} » {destination} · ", active.number)).color(theme::TEXT));
                ui.label(RichText::new(&active.character_name).family(theme::bold()).color(theme::ACCENT));
                ui.label(RichText::new(format!(" · {}/{jumps} jumps", active.progress)).color(theme::TEXT));
            });
            let ratio = if jumps == 0 { 1.0 } else { active.progress as f32 / jumps as f32 };
            ui.add(egui::ProgressBar::new(ratio).fill(theme::ACCENT).desired_height(6.0));
            self.banner(ui, s, &active, &destination);
        });
        if stop {
            self.stop = true;
        }
        ui.add_space(8.0);
        self.active_table(ui, s, &active);
    }

    fn banner(&mut self, ui: &mut Ui, s: &mut Session, active: &ActiveRoute, destination: &str) {
        let name = &active.character_name;
        let live = s.pilots.live.get(&active.character).cloned().unwrap_or_default();
        let send_error = s.pilots.send.as_ref().and_then(|x| x.failed.clone());
        let bar = |ui: &mut Ui, color: Color32, add: &mut dyn FnMut(&mut Ui)| {
            ui.add_space(6.0);
            Frame::new().fill(color.gamma_multiply(0.15)).stroke(Stroke::new(1.0, color)).inner_margin(Margin::symmetric(8, 4)).show(
                ui,
                |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| add(ui));
                },
            );
        };
        if let Some(error) = send_error {
            bar(ui, theme::WARN, &mut |ui| {
                ui.label(RichText::new(&error).color(theme::WARN));
                if ui.button("Retry").clicked() {
                    s.pilots.retry_send();
                }
            });
        } else if live.expired {
            bar(ui, theme::WARN, &mut |ui| {
                ui.label(RichText::new(format!("Tracking paused — {name}'s login expired.")).color(theme::WARN));
                if ui.button("Log in again").clicked() {
                    self.login(s);
                }
            });
        } else if active.arrived() {
            bar(ui, theme::OK, &mut |ui| {
                ui.label(RichText::new(format!("Arrived at {destination}.")).color(theme::OK));
                if ui.button("Done").clicked() {
                    s.pilots.stop_route();
                }
            });
        } else if active.is_off_route() {
            let here = live.system.map_or_else(|| "?".into(), |id| system_name(s, id));
            bar(ui, theme::WARN, &mut |ui| {
                ui.label(RichText::new(format!("Off route — {name} is in {here}.")).color(theme::WARN));
                if ui.button("Re-route from here").clicked() {
                    match s.pilots.reroute(&s.uni, &s.settings, router_core::wormhole::now()) {
                        Ok(route) => self.reroute = Some(Box::new(route)),
                        Err(e) => s.status = e,
                    }
                }
                if ui.button("Stop route").clicked() {
                    self.stop = true;
                }
            });
        } else if let Some(hint) = wormhole_hint(&s.uni, active) {
            bar(ui, theme::WORMHOLE, &mut |ui| _ = ui.label(RichText::new(&hint).color(theme::WORMHOLE)));
        } else if s.pilots.limited {
            ui.label(RichText::new("ESI limited — slowing updates").color(theme::TEXT_DIM).small());
        }
    }

    fn active_table(&mut self, ui: &mut Ui, s: &mut Session, active: &ActiveRoute) {
        let characters = s.pilots.characters();
        let scroll = std::mem::take(&mut self.scroll);
        panel(ui, &format!("Route #{}", active.number), "", true, |ui| {
            let header = |ui: &mut Ui, text: &str| _ = ui.label(theme::header_text(text));
            let mut table = TableBuilder::new(ui)
                .id_salt("active-table")
                .striped(false)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(16.0))
                .column(Column::exact(32.0))
                .column(Column::exact(100.0))
                .column(Column::initial(140.0).at_least(90.0).resizable(true))
                .column(Column::exact(44.0))
                .column(Column::initial(190.0).at_least(70.0).resizable(true).clip(true))
                .column(Column::initial(140.0).at_least(80.0).resizable(true))
                .column(Column::remainder().at_least(120.0))
                .auto_shrink(false);
            if scroll {
                table = table.scroll_to_row(active.progress, Some(Align::Center));
            }
            table
                .header(20.0, |mut row| {
                    for text in ["", "#", "Stop", "System", "Sec", "Pilots", "Region", "Via"] {
                        row.col(|ui| header(ui, text));
                    }
                })
                .body(|body| {
                    body.rows(24.0, active.steps.len(), |mut row| {
                        let i = row.index();
                        let step = &active.steps[i];
                        let passed = i < active.progress;
                        let current = i == active.progress;
                        let sys = s.uni.by_id.get(&step.system).map(|&n| s.uni.system(n));
                        let dim = |color: Color32| if passed { theme::TEXT_DIM } else { color };
                        row.set_selected(current);
                        // A check mark for a passed row, ▶ for the current row. The marks give the
                        // progress without color. The fonts have no check mark glyph, so it is painted.
                        row.col(|ui| {
                            if passed {
                                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                let (l, c, r) = (rect.left_center(), rect.center_bottom() - vec2(1.0, 1.0), rect.right_top());
                                let stroke = Stroke::new(1.5, theme::TEXT_DIM);
                                ui.painter().line_segment([l, c], stroke);
                                ui.painter().line_segment([c, r], stroke);
                            } else if current {
                                ui.label(RichText::new("▶").color(theme::ACCENT));
                            }
                        });
                        row.col(|ui| _ = ui.label(RichText::new(i.to_string()).color(theme::TEXT_DIM)));
                        row.col(|ui| {
                            if let Some(stop) = active.stop_at(i) {
                                let text = RichText::new(stop.label().to_uppercase()).size(11.0).extra_letter_spacing(1.0);
                                ui.label(text.color(dim(theme::ACCENT)));
                            }
                        });
                        row.col(|ui| {
                            let name = sys.map_or_else(|| step.system.to_string(), |sys| sys.name.clone());
                            let text = RichText::new(name).color(dim(if current { Color32::WHITE } else { theme::TEXT }));
                            let text = if current || matches!(active.stop_at(i), Some(Stop::Start | Stop::Destination)) {
                                text.family(theme::bold())
                            } else {
                                text
                            };
                            ui.add(Label::new(text).truncate());
                        });
                        row.col(|ui| {
                            if let Some(sys) = sys {
                                let sec = RichText::new(format!("{:.1}", display_sec(sys.security)));
                                ui.label(sec.color(dim(theme::sec_color(sys.security))));
                            }
                        });
                        row.col(|ui| avatar_row(ui, &mut self.portraits, &pilots_in(&characters, step.system)));
                        row.col(|ui| {
                            let region = sys.map(|sys| sys.region.as_str()).unwrap_or_default();
                            ui.add(Label::new(RichText::new(region).color(theme::TEXT_DIM)).truncate());
                        });
                        row.col(|ui| {
                            let (via, color) = match step.hop {
                                Hop::Wormhole => (format!("{} — manual", step.via), theme::WORMHOLE),
                                Hop::Bridge => (step.via.clone(), theme::BRIDGE),
                                _ => (step.via.clone(), theme::TEXT_DIM),
                            };
                            let via = via.replace('\u{2192}', "»");
                            ui.add(Label::new(RichText::new(&via).color(dim(color))).truncate()).on_hover_text(&via);
                        });
                    });
                });
        });
    }

    /// The "Pilots" panel of the sidebar: the online pilots, and a line for the offline ones.
    /// A click on a pilot sets the pilot's system as the route start.
    pub fn sidebar_panel(&mut self, ui: &mut Ui, s: &mut Session) {
        let rows = s.pilots.characters();
        if rows.is_empty() {
            return;
        }
        let (online, offline): (Vec<&PilotView>, Vec<&PilotView>) = rows.iter().partition(|p| p.live.online != Some(false));
        let mut set_start = None;
        panel(ui, &format!("Pilots ({})", online.len()), "", false, |ui| {
            let mut draw = |ui: &mut Ui, pilot: &PilotView| {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), AVATAR + 4.0), Sense::click());
                if response.hovered() {
                    ui.painter().rect_filled(rect, 0.0, theme::ACCENT_DIM);
                    theme::selection_bar(ui, rect);
                }
                let mut child = ui
                    .new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 2.0))).layout(Layout::left_to_right(Align::Center)));
                avatar(&mut child, &mut self.portraits, pilot, AVATAR);
                child.add(Label::new(RichText::new(&pilot.name).color(theme::TEXT)).truncate());
                let node = pilot.live.system.and_then(|id| s.uni.by_id.get(&id).copied());
                if let Some(node) = node {
                    let sys = s.uni.system(node);
                    child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(&sys.name).color(theme::sec_color(sys.security)));
                    });
                }
                let ship = pilot.live.ship.as_ref().map_or("ship unknown".to_string(), |sh| ship_text(sh, &pilot.name).to_string());
                let region = node.map_or("location unknown".to_string(), |n| s.uni.system(n).region.clone());
                let mut hover = format!("{ship} · {region}");
                if let Some(node) = node.filter(|_| s.pilots.active.is_none()) {
                    hover.push_str(&format!("\nStart the route from {}", s.uni.name(node)));
                    if response.clicked() {
                        set_start = Some(node);
                    }
                }
                response.on_hover_text(hover);
            };
            for pilot in &online {
                draw(ui, pilot);
            }
            if !offline.is_empty() {
                let text = RichText::new(format!("{} offline", offline.len())).color(theme::TEXT_DIM).small();
                if ui.add(Label::new(text).sense(Sense::click())).on_hover_text("Show or hide").clicked() {
                    self.show_offline = !self.show_offline;
                }
                if self.show_offline {
                    for pilot in &offline {
                        draw(ui, pilot);
                    }
                }
            }
        });
        if let Some(node) = set_start {
            s.set_start(node);
        }
    }
}

/// A painted dot, then "Online · Jita". The word is always there, not only the color.
fn status_text(ui: &mut Ui, s: &Session, pilot: &PilotView) {
    if pilot.live.expired {
        ui.label(RichText::new("Login expired").color(theme::WARN).small());
        return;
    }
    if pilot.needs_reauth {
        ui.label(RichText::new("Re-authorize: a scope is missing").color(theme::WARN).small());
        return;
    }
    let (dot, word) = match pilot.live.online {
        Some(true) => (theme::OK, "Online"),
        Some(false) => (theme::TEXT_DIM, "Offline"),
        None => (theme::TEXT_DIM, "Checking…"),
    };
    let mut text = word.to_string();
    if let Some(id) = pilot.live.system {
        text.push_str(&format!(" · {}", system_name(s, id)));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // A painted dot: the fonts have no circle glyph.
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, dot);
        ui.label(RichText::new(text).color(dot).small());
    });
}

/// `Apocalypse · "apoc"` under the status line. The type is bright, the name is dim.
fn ship_row(ui: &mut Ui, pilot: &PilotView) {
    let Some(ship) = &pilot.live.ship else { return };
    let text = ship_text(ship, &pilot.name);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(RichText::new(&text.kind).color(theme::TEXT).small());
        if let Some(name) = &text.name {
            ui.label(RichText::new(format!(" · \"{name}\"")).color(theme::TEXT_DIM).small());
        }
        if text.no_bridges {
            ui.label(RichText::new(" · no bridges").color(theme::WARN).small());
        }
    });
}

fn system_name(s: &Session, id: u32) -> String {
    s.uni.by_id.get(&id).map_or_else(|| id.to_string(), |&n| s.uni.name(n).to_string())
}

fn destination(s: &Session, route: &ActiveRoute) -> String {
    route.steps.last().map(|st| system_name(s, st.system)).unwrap_or_default()
}

/// A modal question. The result is the index of the clicked button. A button with `true` is the
/// primary one.
fn question(ui: &mut Ui, id: &str, title: &str, text: &str, buttons: &[(&str, bool)]) -> Option<usize> {
    let mut clicked = None;
    Modal::new(Id::new(id)).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
        ui.set_width(440.0);
        crate::settings_window::title(ui, title);
        ui.label(RichText::new(text).color(theme::TEXT));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (i, (label, primary)) in buttons.iter().enumerate() {
                let button = Button::new(*label);
                let button = if *primary { button.fill(theme::ACCENT.gamma_multiply(0.35)) } else { button };
                if ui.add(button).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_wording() {
        assert_eq!(overflow_text(1), "+1 other character");
        assert_eq!(overflow_text(4), "+4 other characters");
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Alice Ander"), "AA");
        assert_eq!(initials("bob"), "B");
    }
}
