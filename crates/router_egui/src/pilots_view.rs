//! The EVE login, the route start, the active route and the pilot avatars.

use crate::app::Session;
use crate::notice::Level;
use crate::theme::{self, panel, panel_with};
use crate::widgets::{self, AppButton, UiExt};
use egui::load::SizedTexture;
use egui::{
    Align, Color32, ColorImage, CornerRadius, Frame, Id, Label, Layout, Margin, Modal, RichText, Sense, Stroke, TextEdit, TextureHandle,
    TextureOptions, Ui, vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_phosphor_icons::icons;
use router_core::esi::active::{ActiveRoute, Hop};
use router_core::esi::client::{EsiError, IMAGES_URL, portrait};
use router_core::esi::pilots::{HullSync, PilotView, Pilots, StartPlan, pilots_by_step};
use router_core::labels::{LinkView, hull_label, manual_hop_hint, ship_text};
use router_core::refresh::kept_route;
use router_core::route::{Path, Stop};
use router_core::settings::HullSource;
use router_core::universe::{Universe, display_sec};
use router_core::wormhole::{ClosedHole, SourceId, Wormhole};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

/// At most this many avatars show in one cell. More give "+n other characters".
pub const MAX_AVATARS: usize = 3;
/// The size of an avatar in the route table and in the Pilots panel.
pub const AVATAR: f32 = 20.0;
/// The portrait size to download. 64 px is sharp at 32 px on a 2x display.
const PORTRAIT_SIZE: u32 = 64;
/// The time between two frames while a login or a send waits for its thread.
const LOGIN_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// The wait before the one retry of a failed portrait download.
const PORTRAIT_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

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
    /// A set that starts no download for `ids`: their avatars show initials. A test uses it,
    /// so no test needs the network.
    #[cfg(test)]
    pub fn without_download(ids: &[u64]) -> Portraits {
        let mut portraits = Portraits::default();
        for &id in ids {
            portraits.textures.insert(id, None);
        }
        portraits
    }

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
                let image = download(id).and_then(|bytes| decode(&bytes));
                let _ = tx.send((id, image));
                ctx.request_repaint();
            });
        }
        self.textures.get(&id)?.as_ref()
    }
}

/// The portrait bytes. A network failure gets one more try after a short wait. A status error
/// (for example 404) does not, because the server has answered.
fn download(id: u64) -> Option<Vec<u8>> {
    match portrait(IMAGES_URL, id, PORTRAIT_SIZE) {
        Err(EsiError::Offline(_)) => {
            std::thread::sleep(PORTRAIT_RETRY_WAIT);
            portrait(IMAGES_URL, id, PORTRAIT_SIZE).ok()
        }
        result => result.ok(),
    }
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// One avatar. The name is not on hover, because the name shows beside the avatar. The route
/// table has no name beside it, so `avatar_row` adds the hover. An offline pilot shows at 40 %
/// alpha. Before the portrait loads, the initials show.
pub fn avatar(ui: &mut Ui, portraits: &mut Portraits, pilot: &PilotView, size: f32) -> egui::Response {
    let tint = if pilot.live.online == Some(false) { Color32::from_white_alpha(102) } else { Color32::WHITE };
    portrait_picture(ui, portraits, pilot.id, &pilot.name, tint, size)
}

/// The portrait of a character, with no ring and no dimming: a list that picks a character uses
/// it. Before the portrait loads, the initials show.
pub fn pilot_portrait(ui: &mut Ui, portraits: &mut Portraits, pilot: &PilotView, size: f32) -> egui::Response {
    portrait_picture(ui, portraits, pilot.id, &pilot.name, Color32::WHITE, size)
}

/// The picture of a character: the portrait, or the initials on a dark square.
fn portrait_picture(ui: &mut Ui, portraits: &mut Portraits, id: u64, name: &str, tint: Color32, size: f32) -> egui::Response {
    let radius = CornerRadius::same(3);
    match portraits.get(ui.ctx(), id) {
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
                initials(name),
                font,
                theme::TEXT.gamma_multiply(tint.a() as f32 / 255.0),
            );
            response
        }
    }
}

/// The avatars of the pilots in one system: at most `MAX_AVATARS`, then "+n other characters".
/// The hidden names show on hover of that text.
pub fn avatar_row(ui: &mut Ui, portraits: &mut Portraits, here: &[PilotView]) {
    ui.spacing_mut().item_spacing.x = 4.0;
    for pilot in here.iter().take(MAX_AVATARS) {
        avatar(ui, portraits, pilot, AVATAR).on_hover_text(&pilot.name);
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

/// The status text when a route start finds that its route is not in the list any more.
pub const ROUTE_GONE: &str = "The routes changed, and that route is gone. Start the route again.";

/// The route of a route start: its path, and the map of that path. A refresh can move the
/// route in the list, so the app finds the route again with `kept_route`.
pub struct ChosenRoute {
    uni: Arc<Universe>,
    path: Path,
}

impl ChosenRoute {
    /// Route `route` of the route list.
    fn new(s: &Session, route: usize) -> Option<ChosenRoute> {
        s.routes.get(route).map(|r| ChosenRoute { uni: Arc::clone(&s.uni), path: r.path.clone() })
    }
}

/// The step of a route start.
pub enum Start {
    /// Two or more characters: choose one.
    Pick {
        route: ChosenRoute,
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
    start_after_login: Option<ChosenRoute>,
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
        let had_login = s.pilots.login.is_some();
        self.poll_login(ctx, s);
        s.sync_metro_affiliation();
        match s.pilots.sync_hull(&mut s.settings) {
            HullSync::None => {}
            HullSync::Source => s.save(),
            // The active route stays as it is. The pilot chooses when to re-route.
            HullSync::Hull if s.pilots.active.is_some() => {
                let kind = ship_kind(s);
                s.notify(Level::Info, format!("Ship changed to {kind}. The routes update when this route ends."));
                s.save();
            }
            HullSync::Hull => {
                // The routes change with no action from the pilot, so say why. With no routes, nothing changes.
                let had_routes = !s.routes.is_empty();
                let bridges_off = s.plan();
                s.save();
                if had_routes {
                    let kind = ship_kind(s);
                    let mut text = format!("Ship changed to {kind}. The routes are planned again.");
                    if let Some(reason) = bridges_off {
                        text = format!("{text} Jump bridges off: {reason}");
                    }
                    s.notify(Level::Warning, text);
                } else if let Some(reason) = bridges_off {
                    s.notify(Level::Warning, format!("Jump bridges off: {reason}"));
                }
            }
        }
        // The login is done: continue the route start.
        if had_login && s.pilots.login.is_none() {
            self.continue_after_login(s);
        }
        let progress = s.pilots.active.as_ref().map(|a| a.progress);
        if progress != self.last_progress {
            self.scroll = progress.is_some();
            self.last_progress = progress;
        }
        // A send waits for a thread. Draw again soon to show the result.
        if s.pilots.send.is_some() {
            ctx.request_repaint_after(LOGIN_POLL);
        }
    }

    /// Read the portraits, the login and the tracker, and show the notices of the pilots. Call this
    /// each frame. It changes no setting and saves nothing, so the setup wizard calls it in place
    /// of `update`.
    pub(crate) fn poll_login(&mut self, ctx: &egui::Context, s: &mut Session) {
        self.portraits.poll(ctx);
        s.pilots.update();
        for text in std::mem::take(&mut s.pilots.notices) {
            s.notify(Level::Error, text);
        }
        // A login waits for a thread. Draw again soon to show the result.
        if s.pilots.login.is_some() {
            ctx.request_repaint_after(LOGIN_POLL);
        }
    }

    /// Close the "Remove <name>?" question. The setup wizard calls this, so no old question
    /// shows after Finish.
    pub(crate) fn clear_remove(&mut self) {
        self.remove = None;
    }

    /// Open the "Remove <name>?" question for the character `id`, as a click on Remove does.
    #[cfg(test)]
    pub(crate) fn ask_remove(&mut self, id: u64) {
        self.remove = Some(id);
    }

    /// Start the route that waited for the login. Drop it if the route is gone.
    fn continue_after_login(&mut self, s: &mut Session) {
        let Some(chosen) = self.start_after_login.take() else { return };
        self.characters_open = false;
        if let Some(route) = resolve(s, &chosen) {
            self.begin_start(s, route);
        }
    }

    /// The index of the route of the open pick. Close the pick if the route is gone.
    fn pick_index(&mut self, s: &mut Session) -> Option<usize> {
        let Some(Start::Pick { route, .. }) = &self.start else { return None };
        let route = resolve(s, route);
        if route.is_none() {
            self.start = None;
        }
        route
    }

    /// Start the login and open the characters window with the login steps.
    pub(crate) fn login(&mut self, s: &mut Session) {
        self.paste.clear();
        if let Err(e) = s.pilots.start_login() {
            s.notify(Level::Error, e.to_string());
        }
        self.characters_open = true;
    }

    /// "Start route #n": choose the character, then confirm.
    pub fn begin_start(&mut self, s: &mut Session, route: usize) {
        let Some(picked) = ChosenRoute::new(s, route) else { return };
        let ids: Vec<u64> = s.pilots.senders().into_iter().map(|c| c.id).collect();
        match ids.len() {
            // A waypoint does nothing without the game client, so an offline pilot is not in the list.
            0 if s.pilots.characters().iter().any(|c| !c.live.expired) => s.notify(Level::Warning, Pilots::NO_SENDER),
            0 => {
                self.start_after_login = Some(picked);
                self.login(s);
            }
            1 => self.confirm(s, route, ids[0]),
            _ => {
                let chosen = preselect(s.settings.hull_source, &ids, s.pilots.last_used());
                self.start = Some(Start::Pick { route: picked, chosen });
            }
        }
    }

    fn confirm(&mut self, s: &mut Session, route: usize, id: u64) {
        let Some(pilot) = s.pilots.characters().into_iter().find(|c| c.id == id) else { return };
        let Some(r) = s.routes.get(route) else { return };
        let plan = StartPlan::new(&s.uni, &s.settings, r, route + 1, &pilot, router_core::wormhole::now());
        if let Some(e) = &plan.error {
            s.notify(Level::Error, e.to_string());
        }
        self.start = Some(Start::Confirm(Box::new(plan)));
    }

    /// The "Characters (n)" button of the top bar.
    /// Returns true when the player clicked it.
    pub fn characters_button(&mut self, ui: &mut Ui, s: &Session) -> bool {
        if !s.pilots.shows_characters() {
            return false;
        }
        let count = s.pilots.characters().len();
        let clicked = ui.app_button(format!("Characters ({count})")).clicked();
        if clicked {
            self.characters_open = true;
        }
        clicked
    }

    /// All windows of this module.
    pub fn windows(&mut self, ui: &mut Ui, s: &mut Session) {
        self.resume_window(ui, s);
        self.characters_window(ui, s, true);
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

    /// The Characters window. With `allow_remove` false (the setup wizard runs), it offers no Remove.
    pub(crate) fn characters_window(&mut self, ui: &mut Ui, s: &mut Session, allow_remove: bool) {
        if !self.characters_open {
            return;
        }
        let response = Modal::new(Id::new("characters")).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
            ui.set_width(crate::theme::modal_width(ui, 520.0));
            crate::settings_window::title(ui, "Characters");
            crate::settings_window::keyring_warning(ui, s);
            // No keyring: offer a login for this session only.
            if s.pilots.accounts.is_none() && s.pilots.keyring_error.is_some() {
                ui.label(
                    RichText::new(s.pilots.keyring_error.as_ref().map(ToString::to_string).unwrap_or_default()).color(theme::TEXT_DIM),
                );
                ui.label(RichText::new("No system keyring found. Keep login for this session only?").color(theme::TEXT));
                ui.horizontal(|ui| {
                    if ui.app_button("Session only").clicked() {
                        s.pilots.use_session_only();
                    }
                    if ui.app_button("Cancel").clicked() {
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
                        ship_row(ui, row, &s.settings.hulls);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if allow_remove && ui.app_button("Remove").clicked() {
                            self.remove = Some(row.id);
                        }
                        let again = if row.live.expired { Some("Log in again") } else { row.needs_reauth.then_some("Re-authorize") };
                        if let Some(text) = again
                            && ui.app_button(text).clicked()
                        {
                            self.login(s);
                        }
                    });
                });
                if allow_remove && self.remove == Some(row.id) {
                    let text = format!("Remove {}? This revokes access and deletes stored tokens.", row.name);
                    ui.label(RichText::new(text).color(theme::WARN));
                    ui.add_space(4.0);
                    // A bare `with_layout` in a vertical layout takes all the free height.
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.app_button("Cancel").clicked() {
                                self.remove = None;
                            }
                            if ui.app_button("Remove").clicked() {
                                s.pilots.remove(row.id);
                                self.remove = None;
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                ui.separator();
            }
            self.login_section(ui, s);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let blocked = s.pilots.login_blocked();
                if s.pilots.login.is_none() {
                    let add = ui.add_enabled(blocked.is_none(), AppButton::new("Add character").primary());
                    if let Some(reason) = &blocked {
                        add.on_disabled_hover_text(reason);
                    } else if add.clicked() {
                        self.login(s);
                    }
                }
                if ui.app_button("Close").clicked() {
                    self.characters_open = false;
                }
            });
            if s.pilots.accounts.as_ref().is_some_and(router_core::esi::store::Accounts::is_session_only) {
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
        let listen_error = flow.login.listen_error().map(ToString::to_string);
        let (busy, error) = (flow.busy(), flow.error.as_ref().map(ToString::to_string));
        ui.label(RichText::new("Opening EVE login in your browser…").color(theme::TEXT));
        ui.horizontal(|ui| {
            ui.label(RichText::new("Not opened? Copy this URL:").color(theme::TEXT_DIM));
            if ui.small_button("Copy URL").clicked() {
                ui.ctx().copy_text(url.clone());
                s.notify(Level::Success, "Login URL copied to the clipboard");
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
            if (ui.add_enabled(!busy && !self.paste.trim().is_empty(), AppButton::new("Use URL").primary()).clicked() || enter) && !busy {
                s.pilots.paste(&self.paste);
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().color(theme::accent(ui)));
                ui.label(RichText::new("Logging in…").color(theme::TEXT_DIM));
            });
        }
        if let Some(e) = error {
            ui.label(RichText::new(e).color(theme::ERROR));
        }
        if ui.app_button("Cancel login").clicked() {
            s.pilots.cancel_login();
            self.start_after_login = None;
        }
    }

    fn start_window(&mut self, ui: &mut Ui, s: &mut Session) {
        // Find the route again: a refresh or a hull change can move it or remove it.
        let route = if matches!(self.start, Some(Start::Pick { .. })) {
            let Some(route) = self.pick_index(s) else { return };
            route
        } else {
            0
        };
        let Some(start) = &mut self.start else { return };
        let mut next = None;
        let mut close = false;
        let mut plan_for = None;
        let response = Modal::new(Id::new("start")).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
            ui.set_width(crate::theme::modal_width(ui, 480.0));
            match start {
                Start::Pick { chosen, .. } => {
                    crate::settings_window::title(ui, &format!("Send route #{} to…", route + 1));
                    for pilot in s.pilots.senders() {
                        ui.horizontal(|ui| {
                            ui.radio_value(chosen, pilot.id, "");
                            avatar(ui, &mut self.portraits, &pilot, 24.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&pilot.name).color(Color32::WHITE));
                                status_text(ui, s, &pilot);
                                ship_row(ui, &pilot, &s.settings.hulls);
                            });
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(AppButton::new("Continue").primary()).clicked() {
                            next = Some((route, *chosen));
                        }
                        close = ui.app_button("Cancel").clicked();
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
                    if let Some(flown) = &plan.flown {
                        let planned = s.settings.rules.hull.as_deref().map_or("no hull".into(), hull_label);
                        ui.label(RichText::new(format!("Planned for {planned}. {name} flies a {}.", flown.name)).color(theme::WARN));
                        if ui.app_button(format!("Plan for {}", flown.name)).clicked() {
                            plan_for = Some((plan.planned.character, flown.name.clone()));
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let send = if plan.online == Some(false) { "Send anyway" } else { "Send" };
                        match (&here, &plan.from_here) {
                            (Some(here), Some(from_here)) => {
                                if ui.add(AppButton::new(format!("Route from {here}")).primary()).clicked() {
                                    s.pilots.start_route(from_here.clone());
                                    close = true;
                                }
                                if ui.app_button("Send as planned").clicked() {
                                    s.pilots.start_route(plan.planned.clone());
                                    close = true;
                                }
                            }
                            _ => {
                                if ui.add(AppButton::new(send).primary()).clicked() {
                                    s.pilots.start_route(plan.planned.clone());
                                    close = true;
                                }
                            }
                        }
                        if ui.app_button("Cancel").clicked() {
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
            s.save();
            s.notify(Level::Info, format!("Routes planned for the {kind}. Start the route again."));
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
            ui.set_width(crate::theme::modal_width(ui, 420.0));
            let total = send.systems.len();
            match &send.failed {
                None => {
                    crate::settings_window::title(ui, "Start route");
                    ui.label(RichText::new(format!("Sending waypoints {}/{total}…", send.done + 1)).color(theme::TEXT));
                    ui.add(egui::ProgressBar::new(send.done as f32 / total.max(1) as f32).fill(theme::accent(ui)));
                }
                Some(error) => {
                    crate::settings_window::title(ui, "Send failed");
                    ui.label(RichText::new(error.to_string()).color(theme::WARN));
                    ui.horizontal(|ui| {
                        if ui.app_button("Retry").clicked() {
                            s.pilots.retry_send();
                        }
                        if ui.app_button("Cancel").clicked() {
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
                s.notify(Level::Warning, "Route stopped. The in-game waypoints stay.");
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
        let header = |ui: &mut Ui| stop = ui.app_button("Stop route").clicked();
        panel_with(ui, "Active route", header, false, |ui| {
            ui.horizontal(|ui| {
                // Oxanium and the egui fallback fonts have no right arrow glyph. Oxanium has "»".
                ui.label(RichText::new(format!("Route #{} » {destination} · ", active.number)).color(theme::TEXT));
                ui.label(RichText::new(&active.character_name).family(theme::bold()).color(theme::accent(ui)));
                ui.label(RichText::new(format!(" · {}/{jumps} jumps", active.progress)).color(theme::TEXT));
            });
            let ratio = if jumps == 0 { 1.0 } else { active.progress as f32 / jumps as f32 };
            ui.add(egui::ProgressBar::new(ratio).fill(theme::accent(ui)).desired_height(6.0));
            self.banner(ui, s, &active, &destination);
        });
        if stop {
            self.stop = true;
        }
        ui.add_space(8.0);
        self.active_table(ui, s, &active);
    }

    /// The "Re-route from here" button. A click finds a new route and opens the confirm window.
    fn reroute_button(&mut self, ui: &mut Ui, s: &mut Session) {
        if ui.app_button("Re-route from here").clicked() {
            match s.pilots.reroute(&s.uni, &s.settings, router_core::wormhole::now()) {
                Ok(route) => self.reroute = Some(Box::new(route)),
                Err(e) => s.notify(Level::Error, e.to_string()),
            }
        }
    }

    fn banner(&mut self, ui: &mut Ui, s: &mut Session, active: &ActiveRoute, destination: &str) {
        let name = &active.character_name;
        let live = s.pilots.live.get(&active.character).cloned().unwrap_or_default();
        let send_error = s.pilots.send.as_ref().and_then(|x| x.failed.as_ref().map(ToString::to_string));
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
                if ui.app_button("Retry").clicked() {
                    s.pilots.retry_send();
                }
            });
        } else if live.expired {
            bar(ui, theme::WARN, &mut |ui| {
                ui.label(RichText::new(format!("Tracking paused - {name}'s login expired.")).color(theme::WARN));
                if ui.app_button("Log in again").clicked() {
                    self.login(s);
                }
            });
        } else if active.arrived() {
            bar(ui, theme::OK, &mut |ui| {
                ui.label(RichText::new(format!("Arrived at {destination}.")).color(theme::OK));
                if ui.app_button("Done").clicked() {
                    s.pilots.stop_route();
                }
            });
        } else if active.is_off_route() {
            let here = live.system.map_or_else(|| "?".into(), |id| system_name(s, id));
            bar(ui, theme::WARN, &mut |ui| {
                ui.label(RichText::new(format!("Off route - {name} is in {here}.")).color(theme::WARN));
                self.reroute_button(ui, s);
                if ui.app_button("Stop route").clicked() {
                    self.stop = true;
                }
            });
        } else if let Some((from, to)) = closed_ahead(&s.closed, active) {
            let text = format!("A closed wormhole is ahead: {} » {}.", system_name(s, from), system_name(s, to));
            bar(ui, theme::WARN, &mut |ui| {
                ui.label(RichText::new(&text).color(theme::WARN));
                self.reroute_button(ui, s);
                if ui.app_button("Stop route").clicked() {
                    self.stop = true;
                }
            });
        } else if let Some(hint) = manual_hop_hint(&s.uni, active) {
            bar(ui, theme::WORMHOLE, &mut |ui| _ = ui.label(RichText::new(&hint).color(theme::WORMHOLE)));
        } else if s.pilots.limited {
            ui.label(RichText::new("ESI limited - slowing updates").color(theme::TEXT_DIM).small());
        }
    }

    fn active_table(&mut self, ui: &mut Ui, s: &mut Session, active: &ActiveRoute) {
        let characters = s.pilots.characters();
        let systems: Vec<u32> = active.steps.iter().map(|st| st.system).collect();
        let by_step = pilots_by_step(&systems, &characters, Some(active));
        let scroll = std::mem::take(&mut self.scroll);
        let mut close = None;
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
                .column(Column::exact(72.0))
                .column(Column::initial(190.0).at_least(70.0).resizable(true).clip(true))
                .column(Column::initial(140.0).at_least(80.0).resizable(true))
                .column(Column::remainder().at_least(120.0))
                .auto_shrink(false);
            if scroll {
                table = table.scroll_to_row(active.progress, Some(Align::Center));
            }
            table
                .header(20.0, |mut row| {
                    for text in ["", "#", "Role", "System", "Security", "Pilots", "Region", "Via"] {
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
                                ui.label(RichText::new(icons::CARET_RIGHT.as_str()).color(theme::accent(ui)));
                            }
                        });
                        row.col(|ui| _ = ui.label(RichText::new(i.to_string()).color(theme::TEXT_DIM)));
                        row.col(|ui| {
                            if let Some(stop) = active.stop_at(i) {
                                let text = RichText::new(stop.to_string().to_uppercase()).size(11.0).extra_letter_spacing(1.0);
                                ui.label(text.color(dim(theme::accent(ui))));
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
                        row.col(|ui| avatar_row(ui, &mut self.portraits, &by_step[i]));
                        row.col(|ui| {
                            let region = sys.map(|sys| sys.region.as_str()).unwrap_or_default();
                            ui.add(Label::new(RichText::new(region).color(theme::TEXT_DIM)).truncate());
                        });
                        row.col(|ui| {
                            let via = step.label(router_core::wormhole::now());
                            let via = if step.hop.is_manual() { format!("{via} - manual") } else { via };
                            let color = theme::link_color(step.hop);
                            let via = via.replace('\u{2192}', "»");
                            let label = ui.add(Label::new(RichText::new(&via).color(dim(color))).truncate().sense(Sense::click()));
                            // Only a wormhole that is still ahead can close.
                            if let Some(hole) = closable(active, i) {
                                widgets::context_menu(&label, |ui| {
                                    let tip = "Ignore this connection until the next app restart. Use it when the wormhole has collapsed.";
                                    if ui.app_button("Mark wormhole closed").on_hover_text(tip).clicked() {
                                        close = Some(hole);
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
                });
        });
        if let Some(hole) = close {
            s.close_link(hole);
        }
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
                    ui.painter().rect_filled(rect, 0.0, theme::accent(ui).linear_multiply(0.18));
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
                let ship = pilot
                    .live
                    .ship
                    .as_ref()
                    .map_or("ship unknown".to_string(), |sh| ship_text(sh, &pilot.name, &s.settings.hulls).to_string());
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
                if ui.add(Label::new(text).sense(Sense::click())).on_hover_text("Toggle offline pilots").clicked() {
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
fn ship_row(ui: &mut Ui, pilot: &PilotView, hulls: &router_core::ansiblex::HullTable) {
    let Some(ship) = &pilot.live.ship else { return };
    let text = ship_text(ship, &pilot.name, hulls);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(RichText::new(text.kind.name()).color(theme::TEXT).small());
        if let Some(name) = &text.name {
            ui.label(RichText::new(format!(" · \"{name}\"")).color(theme::TEXT_DIM).small());
        }
        if text.no_bridges {
            ui.label(RichText::new(" · no bridges").color(theme::WARN).small());
        }
    });
}

/// The name of the hull that the pilot flies now, for a notice.
fn ship_kind(s: &Session) -> String {
    s.settings.rules.hull.as_ref().map_or("an unknown ship".into(), |h| h.name.clone())
}

/// The pilot that the send dialog selects. A pilot that the user picked in the Pilot picker wins.
/// Else the last pilot to send a route. Else the first one. `ids` lists the pilots that can send.
fn preselect(source: HullSource, ids: &[u64], last: Option<u64>) -> u64 {
    let picked = match source {
        HullSource::Pilot(id) => Some(id),
        HullSource::Manual => None,
    };
    picked.into_iter().chain(last).find(|id| ids.contains(id)).unwrap_or(ids[0])
}

/// The index of the chosen route in the route list. Set `ROUTE_GONE` if there is none.
fn resolve(s: &mut Session, chosen: &ChosenRoute) -> Option<usize> {
    let route = kept_route(&s.routes, &s.uni, &chosen.uni, &chosen.path);
    if route.is_none() {
        s.notify(Level::Warning, ROUTE_GONE);
    }
    route
}

pub(crate) fn system_name(s: &Session, id: u32) -> String {
    s.uni.by_id.get(&id).map_or_else(|| id.to_string(), |&n| s.uni.name(n).to_string())
}

/// The wormhole into step `i` of the active route, if the pilot has not made that jump yet.
/// The route holds only the signature of the system that the jump leaves, so the entry has no
/// other end.
fn closable(active: &ActiveRoute, i: usize) -> Option<ClosedHole> {
    let step = active.steps.get(i).filter(|st| st.hop == Hop::Wormhole && i > active.progress)?;
    let (size, expiry) = match step.link {
        Some(LinkView::Wormhole { size, expiry, .. }) => (size, expiry),
        _ => (None, None),
    };
    Some(ClosedHole::from_jump(active.steps[i - 1].system, step.system, step.sig.as_deref(), size, expiry))
}

/// The first wormhole ahead on the active route that matches a closed entry: the IDs of its ends.
fn closed_ahead(closed: &[ClosedHole], active: &ActiveRoute) -> Option<(u32, u32)> {
    if closed.is_empty() {
        return None;
    }
    (active.progress + 1..active.steps.len()).find_map(|i| {
        let step = &active.steps[i];
        let from = active.steps[i - 1].system;
        if step.hop != Hop::Wormhole {
            return None;
        }
        let hole = Wormhole::new(from, step.system, step.sig.clone(), None, SourceId::Nexum);
        closed.iter().any(|c| c.matches(&hole)).then_some((from, step.system))
    })
}

fn destination(s: &Session, route: &ActiveRoute) -> String {
    route.steps.last().map(|st| system_name(s, st.system)).unwrap_or_default()
}

/// A modal question. The result is the index of the clicked button. A button with `true` is the
/// primary one.
fn question(ui: &mut Ui, id: &str, title: &str, text: &str, buttons: &[(&str, bool)]) -> Option<usize> {
    let mut clicked = None;
    Modal::new(Id::new(id)).frame(crate::settings_window::modal_frame()).show(ui.ctx(), |ui| {
        ui.set_width(crate::theme::modal_width(ui, 440.0));
        crate::settings_window::title(ui, title);
        ui.label(RichText::new(text).color(theme::TEXT));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (i, (label, primary)) in buttons.iter().enumerate() {
                let button = AppButton::new(*label);
                let button = if *primary { button.primary() } else { button };
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
    use router_core::config::Config;
    use router_core::labels::Shortcuts;
    use router_core::test_support::{hole, overlay_universe, settings, snapshot};
    use router_core::wormhole::Wormhole;
    use std::sync::Arc;

    const JITA: u32 = 30000142;
    const PERIMETER: u32 = 30000144;

    /// Jita, then Perimeter by a wormhole with the signature ABC at Jita, then Amarr by a gate.
    fn wormhole_route(progress: usize) -> ActiveRoute {
        use router_core::esi::active::Step;
        let step = |system, hop, sig: Option<&str>| Step { system, hop, link: None, sig: sig.map(Into::into) };
        let steps = vec![step(JITA, Hop::Start, None), step(PERIMETER, Hop::Wormhole, Some("ABC")), step(30002187, Hop::Gate, None)];
        let mut route = ActiveRoute::from_steps(steps, vec![0, 2], 1, 1, "Pilot");
        route.progress = progress;
        route
    }

    #[test]
    fn only_a_wormhole_ahead_can_close() {
        let route = wormhole_route(0);
        assert_eq!(closable(&route, 0), None);
        assert_eq!(closable(&route, 2), None);
        let hole = closable(&route, 1).expect("the wormhole is ahead");
        assert_eq!((hole.a, hole.b, hole.sig_a.as_deref(), hole.sig_b), (JITA, PERIMETER, Some("ABC"), None));
        // After the jump the wormhole is behind the pilot.
        assert_eq!(closable(&wormhole_route(1), 1), None);
    }

    #[test]
    fn a_closed_entry_for_a_step_ahead_shows_the_banner_state() {
        let closed = vec![closable(&wormhole_route(0), 1).unwrap()];
        assert_eq!(closed_ahead(&closed, &wormhole_route(0)), Some((JITA, PERIMETER)));
        assert_eq!(closed_ahead(&closed, &wormhole_route(1)), None);
        assert_eq!(closed_ahead(&[], &wormhole_route(0)), None);
        // A closed hole with another signature on the same pair does not match.
        let other = vec![ClosedHole::from_jump(JITA, PERIMETER, Some("DEF"), None, None)];
        assert_eq!(closed_ahead(&other, &wormhole_route(0)), None);
    }

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

    fn session_with_routes() -> (Session, tempfile::TempDir) {
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default(), &Default::default());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("eve-router.json");
        let mut s = Session::new(Arc::new(uni), settings, Config::default(), path, shortcuts);
        s.add_list("Jita, Amarr");
        assert!(s.routes.len() >= 2, "{:?}", s.status);
        (s, tmp)
    }

    #[test]
    fn a_moved_route_is_still_the_one_sent() {
        let (mut s, _tmp) = session_with_routes();
        let path = s.routes[1].path.clone();
        let mut ui = PilotsUi { start: Some(Start::Pick { route: ChosenRoute::new(&s, 1).unwrap(), chosen: 1 }), ..Default::default() };
        s.routes.swap(0, 1);
        assert_eq!(ui.pick_index(&mut s), Some(0));
        assert_eq!(s.routes[0].path, path);
        assert!(ui.start.is_some());
    }

    /// Review focus: a stargate and a wormhole between the same two systems.
    #[test]
    fn the_pick_of_a_parallel_route_stays_on_that_route() {
        let hole_route = || Wormhole { sig_a: Some("ABC-123".into()), ..hole(JITA, PERIMETER) };
        let (mut s, _tmp) = session_with_routes();
        s.uni = snapshot(vec![hole_route()]).uni;
        s.replace_list("Jita, Perimeter");
        assert_eq!(s.routes[0].path.nodes, s.routes[1].path.nodes);
        assert_eq!((s.routes[0].wormholes, s.routes[1].wormholes), (0, 1));
        let mut ui = PilotsUi { start: Some(Start::Pick { route: ChosenRoute::new(&s, 1).unwrap(), chosen: 1 }), ..Default::default() };
        assert_eq!(ui.pick_index(&mut s), Some(1));
        // A refresh with the same wormhole gives a new map. The pick stays on the wormhole route.
        s.apply_snapshot(snapshot(vec![hole_route()]));
        assert_eq!(ui.pick_index(&mut s), Some(1));
        assert_eq!(s.routes[1].wormholes, 1);
    }

    #[test]
    fn a_gone_route_closes_the_pick() {
        let (mut s, _tmp) = session_with_routes();
        let mut ui = PilotsUi { start: Some(Start::Pick { route: ChosenRoute::new(&s, 1).unwrap(), chosen: 1 }), ..Default::default() };
        s.routes.truncate(1);
        assert_eq!(ui.pick_index(&mut s), None);
        assert!(ui.start.is_none());
        assert_eq!(s.notices.last().map(|n| n.text.as_str()), Some(ROUTE_GONE));
    }

    #[test]
    fn a_gone_route_drops_the_start_after_login() {
        let (mut s, _tmp) = session_with_routes();
        let mut ui = PilotsUi { start_after_login: ChosenRoute::new(&s, 1), ..Default::default() };
        s.routes.truncate(1);
        ui.continue_after_login(&mut s);
        assert!(ui.start_after_login.is_none() && ui.start.is_none());
        assert_eq!(s.notices.last().map(|n| n.text.as_str()), Some(ROUTE_GONE));
    }

    #[test]
    fn a_picked_pilot_beats_the_last_sender() {
        let ids = [1, 2, 3];
        assert_eq!(preselect(HullSource::Pilot(2), &ids, Some(3)), 2);
        assert_eq!(preselect(HullSource::Manual, &ids, Some(3)), 3);
        // A picked pilot that cannot send now (offline) gives way to the last sender.
        assert_eq!(preselect(HullSource::Pilot(9), &ids, Some(3)), 3);
        assert_eq!(preselect(HullSource::Manual, &ids, None), 1);
    }

    /// A session with the character Alice, a window that is open, and the portrait of Alice.
    fn characters_setup() -> (Session, PilotsUi, tempfile::TempDir) {
        let (mut s, tmp) = session_with_routes();
        s.pilots = Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[(1, "Alice Ander")]);
        let ui = PilotsUi { portraits: Portraits::without_download(&[1]), characters_open: true, ..Default::default() };
        (s, ui, tmp)
    }

    /// The texts that the Characters window draws in one frame.
    fn characters_texts(s: &mut Session, ui_state: &mut PilotsUi, allow_remove: bool) -> Vec<String> {
        // A window sizes itself in the first frame and draws nothing, so the test reads the second frame.
        let ctx = crate::test_ui::ctx();
        let mut draw = |s: &mut Session| ctx.run_ui(Default::default(), |ui| ui_state.characters_window(ui, s, allow_remove));
        draw(s).textures_delta.clear();
        let mut output = draw(s);
        output.textures_delta.clear();
        crate::test_ui::drawn_texts(&output).into_iter().map(|(text, ..)| text).collect()
    }

    #[test]
    fn the_characters_window_shows_the_keyring_warning() {
        for keyring_missing in [true, false] {
            let (mut s, mut ui, _tmp) = characters_setup();
            s.keyring_missing = keyring_missing;
            let drawn = characters_texts(&mut s, &mut ui, true);
            let shown = drawn.iter().any(|t| t == crate::settings_window::KEYRING_WARNING);
            assert_eq!(shown, keyring_missing);
        }
    }

    #[test]
    fn the_characters_window_hides_remove_while_the_wizard_runs() {
        let (mut s, mut ui, _tmp) = characters_setup();
        let drawn = characters_texts(&mut s, &mut ui, true);
        assert!(drawn.iter().any(|t| t == "Remove"));
        let drawn = characters_texts(&mut s, &mut ui, false);
        assert!(!drawn.iter().any(|t| t == "Remove"));
        assert!(drawn.iter().any(|t| t == "Alice Ander"));
        // A confirmation that is open does not show when Remove is locked.
        ui.remove = Some(1);
        let drawn = characters_texts(&mut s, &mut ui, false);
        assert!(!drawn.iter().any(|t| t.starts_with("Remove ") || t == "Remove"));
        assert!(drawn.iter().any(|t| t == "Alice Ander"));
        let drawn = characters_texts(&mut s, &mut ui, true);
        assert!(drawn.iter().any(|t| t.starts_with("Remove Alice Ander?")));
    }
}
