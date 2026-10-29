//! The setup wizard: the pages, the draft, the navigation, Finish and the drawing of the pages.
//! The wizard changes nothing in the session before Finish. The one exception is the login of a
//! character, which stores its token at once, as in Settings.

use crate::app::Session;
use crate::pilots_view::{PilotsUi, Portraits};
use crate::search::SearchBox;
use crate::settings_window::{
    CHARACTER_ROW, FIELD, METRO_NEEDS_CHARACTER, RowLook, character_table, eve_metro_available, group, keyring_warning, pilot_status,
};
use crate::theme::{self, CARD_GAP, FORM_GAP, FactionTheme, GROUP_GAP, ROW_GAP};
use crate::welcome::{Answers, Preset, pages_for};
use crate::widgets::{AppButton, AppCheckbox, UiExt};
use egui::{Align, Color32, Layout, RichText, ScrollArea, TextEdit, Ui, vec2};
use egui_phosphor_icons::icons;
use petgraph::graph::NodeIndex;
use router_core::config::{self, ApiKey};
use router_core::esi::pilots::PilotView;
use router_core::sources::{
    self,
    nexum::{self, MapInfo},
};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use router_core::settings::Features;

/// A page of the wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardPage {
    /// Updates, theme and the three feature switches.
    Essentials,
    /// The alliance capital and the jump bridges.
    Alliance,
    /// The Nexum URL, the key and the signature switch.
    Nexum,
    /// The EVE Metro switch and the character.
    EveMetro,
}

impl WizardPage {
    /// The name of the page in the step list. It is also the title of the page.
    pub fn label(self) -> &'static str {
        match self {
            WizardPage::Essentials => "Essentials",
            WizardPage::Alliance => "Alliance",
            WizardPage::Nexum => "Nexum",
            WizardPage::EveMetro => "EVE Metro",
        }
    }

    /// The line under the title of the page. It says what the page is for in one sentence.
    fn intro(self) -> &'static str {
        match self {
            WizardPage::Essentials => "The basics: your pilots, updates, theme and features.",
            WizardPage::Alliance => "Set the capital of your alliance and add its jump bridges.",
            WizardPage::Nexum => "Connect the router to your Nexum maps.",
            WizardPage::EveMetro => "Third-party map of Pochven wormholes. A subscription is required.",
        }
    }
}

/// What the wizard does after a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// The wizard stays open.
    Open,
    /// The last page is done. The caller calls `Wizard::finish`.
    Done,
    /// The pilot asked to go back to the welcome screen.
    Welcome,
}

/// The values that the pages edit. A page edits a copy, so nothing reaches the session before Finish.
#[derive(Clone, Debug, PartialEq)]
struct Draft {
    /// Look for a new version at the start.
    check_for_updates: bool,
    /// Install a new version with no question.
    auto_update: bool,
    /// The faction theme.
    theme: FactionTheme,
    /// The three feature switches. They start all on, whatever the stance of the session.
    features: Features,
    /// True after the pilot passed the Essentials page with Next or Back. Only then does Finish write the switches.
    features_kept: bool,
    /// The alliance capital.
    capital: Option<NodeIndex>,
    /// The pasted list of jump bridges.
    bridge_text: String,
    /// The lines of the list that did not give a bridge.
    bridge_problems: Vec<String>,
    /// The list text that Finish imports. It is `Some` only after an import that gave a bridge.
    bridges: Option<String>,
    /// The text of the Nexum URL field.
    nexum_url: String,
    /// The text of the Nexum key field. It starts empty, so the key never shows.
    nexum_key: String,
    /// Skip the connections that have no known signature.
    skip_unsigned: bool,
    /// The IDs of the Nexum maps that the router reads.
    nexum_maps: Vec<String>,
    /// Use EVE Metro.
    metro_enabled: bool,
    /// The character that identifies the pilot to EVE Metro.
    metro_character: Option<u64>,
}

impl Draft {
    /// The draft that shows the values of the session.
    fn from_session(s: &Session) -> Draft {
        Draft {
            check_for_updates: s.cfg.checks_for_updates(),
            auto_update: s.cfg.auto_update,
            theme: s.faction_theme(),
            features: Features::default(),
            features_kept: false,
            capital: s.settings.rules.capital,
            bridge_text: String::new(),
            bridge_problems: Vec::new(),
            bridges: None,
            nexum_url: s.cfg.nexum.url.clone().unwrap_or_default(),
            nexum_key: String::new(),
            skip_unsigned: !s.cfg.nexum.keep_unsigned,
            nexum_maps: s.cfg.nexum.map_ids.clone(),
            metro_enabled: s.cfg.eve_metro.enabled,
            metro_character: s.cfg.eve_metro.character,
        }
    }
}

/// The Nexum map list of the Nexum page: the maps of the last fetch, the fetch that runs, and the
/// last error. The wizard keeps it apart from the session, because the fetch uses the URL and the
/// key that the pilot typed, and these reach the session only at Finish.
#[derive(Default)]
struct MapsLoad {
    /// The maps of the last fetch that worked.
    list: Vec<MapInfo>,
    /// The answer of the fetch on its thread. It is `Some` while the fetch runs.
    rx: Option<Receiver<Result<Vec<MapInfo>, sources::FetchError>>>,
    /// The text of the last error.
    error: Option<String>,
    /// True after the pilot asked for the list.
    tried: bool,
}

/// The search box of the capital.
fn capital_box() -> SearchBox {
    SearchBox::new("wizard-capital").sovnull_only()
}

/// The state of the wizard: the page, the kept values and the values that the page edits.
pub struct Wizard {
    /// The pages of this run.
    pages: Vec<WizardPage>,
    /// The index of the current page.
    at: usize,
    /// The values of the pages that the pilot passed with Next or Back.
    kept: Draft,
    /// The values that the current page edits.
    edit: Draft,
    /// The error of the current page.
    error: Option<String>,
    /// The search box of the Alliance page.
    capital: SearchBox,
    /// The theme that the window shows now. It differs from the edited theme until the next draw.
    applied: FactionTheme,
    /// The value of the EVE Metro Enable box in the session. Skip on the EVE Metro page restores it.
    metro_session: bool,
    /// The Nexum maps of the Nexum page.
    maps: MapsLoad,
}

impl Wizard {
    /// Start at the first page, with the values of the session and no preset. The pages and the
    /// starting values are those of `Preset::Custom`. Only the tests use it. The app builds the
    /// wizard with `with_preset`.
    #[cfg(test)]
    pub fn new(s: &Session) -> Wizard {
        Wizard::with_preset(s, Preset::Custom, Preset::Custom.answers())
    }

    /// Start at the first page, with the values of the session. The preset and its answers give
    /// the pages and the starting switches of the Essentials page. Finish writes no switch until
    /// the pilot passes the Essentials page. The EVE Metro Enable pre-check is different: `kept`
    /// holds it from the start, so Next on the EVE Metro page keeps it. Skip on the EVE Metro page
    /// restores the session value of Enable.
    pub fn with_preset(s: &Session, preset: Preset, answers: Answers) -> Wizard {
        let pages = pages_for(preset, answers, s.pilots.shows_characters(), eve_metro_available(s));
        let mut draft = Draft::from_session(s);
        let metro_session = draft.metro_enabled;
        draft.features = answers.features();
        // The subscription answer Yes turns EVE Metro on, but only when the page shows and a character is
        // linked. A hidden page must not change a setting.
        if preset.asks_travel() && pages.contains(&WizardPage::EveMetro) && !s.pilots.characters().is_empty() {
            draft.metro_enabled = true;
        }
        Wizard {
            pages,
            at: 0,
            applied: draft.theme,
            kept: draft.clone(),
            edit: draft,
            error: None,
            capital: capital_box(),
            metro_session,
            maps: MapsLoad::default(),
        }
    }

    /// The current page.
    pub fn page(&self) -> WizardPage {
        self.pages[self.at]
    }

    /// The pages of this run, in order.
    #[cfg(test)]
    pub(crate) fn pages(&self) -> &[WizardPage] {
        &self.pages
    }

    /// The feature switches that the Essentials page shows now.
    #[cfg(test)]
    pub(crate) fn features(&self) -> Features {
        self.edit.features
    }

    /// Check the page, then keep the edits. Return false if the page has an error. Then nothing is kept.
    fn keep_edits(&mut self) -> bool {
        match self.page() {
            // Without this flag, Finish writes no stance.
            WizardPage::Essentials => self.edit.features_kept = true,
            // The pilot did not type an unchanged URL, so it needs no check. It can be a bad URL
            // from a file that a person edited by hand. Finish keeps the old URL then.
            WizardPage::Nexum if self.edit.nexum_url != self.kept.nexum_url => {
                if let Err(e) = config::parse_nexum_url(&self.edit.nexum_url) {
                    self.error = Some(e.to_string());
                    return false;
                }
            }
            WizardPage::Nexum => {}
            WizardPage::Alliance | WizardPage::EveMetro => {}
        }
        self.error = None;
        self.kept = self.edit.clone();
        true
    }

    /// Move to the next page. On the last page, return `Flow::Done`.
    fn advance(&mut self) -> Flow {
        if self.at + 1 == self.pages.len() {
            return Flow::Done;
        }
        self.at += 1;
        Flow::Open
    }

    /// Keep the edits of the page and move on. A page with an error stays open.
    pub fn next(&mut self) -> Flow {
        if self.keep_edits() { self.advance() } else { Flow::Open }
    }

    /// Keep the edits of the page and move back. On the first page, do nothing.
    pub fn back(&mut self) {
        if self.at == 0 {
            return;
        }
        // A bad URL never stops Back. It drops the URL edit, and the other edits stay.
        if self.page() == WizardPage::Nexum
            && self.edit.nexum_url != self.kept.nexum_url
            && config::parse_nexum_url(&self.edit.nexum_url).is_err()
        {
            self.edit.nexum_url.clone_from(&self.kept.nexum_url);
        }
        self.keep_edits();
        self.at -= 1;
    }

    /// Drop the edits of the page and move on. On the last page, return `Flow::Done`. The
    /// Essentials page is the exception: Skip accepts its starting values, as Next does.
    pub fn skip(&mut self) -> Flow {
        if self.page() == WizardPage::Essentials {
            // This page has no error, so the edits are always kept.
            self.keep_edits();
        } else {
            if self.page() == WizardPage::EveMetro {
                // `kept` holds the Enable pre-check of the preset. A skipped page changes no setting.
                self.kept.metro_enabled = self.metro_session;
            }
            self.edit = self.kept.clone();
            self.error = None;
        }
        // The text that the pilot typed in the search box goes too.
        self.capital = capital_box();
        self.advance()
    }

    /// The Import button: read the pasted list, and keep the text for Finish only if it gives a bridge.
    pub fn import(&mut self, s: &Session) {
        let (pairs, problems) = s.read_bridges(&self.edit.bridge_text);
        self.edit.bridge_problems = problems;
        self.edit.bridges = (!pairs.is_empty()).then(|| self.edit.bridge_text.clone());
    }

    /// True when the URL of the field is a good URL and a key is known, from the field or from the
    /// session. Only then can the page ask for the map list.
    fn maps_ready(&self, s: &Session) -> bool {
        let url = config::parse_nexum_url(&self.edit.nexum_url).ok().flatten().is_some();
        url && (!self.edit.nexum_key.trim().is_empty() || s.cfg.nexum.key.is_some())
    }

    /// Start the fetch of the map list on a thread. It uses the URL and the key of the fields. An
    /// empty key field keeps the key of the session.
    fn load_maps(&mut self, s: &Session, ctx: &egui::Context) {
        let mut cfg = s.cfg.nexum.clone();
        if let Ok(url) = config::parse_nexum_url(&self.edit.nexum_url) {
            cfg.url = url;
        }
        let key = self.edit.nexum_key.trim();
        if !key.is_empty() {
            cfg.key = Some(ApiKey(key.to_string()));
        }
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(nexum::fetch_maps(&cfg, sources::TIMEOUT));
            ctx.request_repaint();
        });
        self.maps = MapsLoad { list: std::mem::take(&mut self.maps.list), rx: Some(rx), error: None, tried: true };
    }

    /// Take the answer of the fetch, when it is there.
    fn poll_maps(&mut self) {
        let Some(rx) = &self.maps.rx else { return };
        match rx.try_recv() {
            Ok(Ok(list)) => {
                self.maps.list = list;
                self.maps.rx = None;
            }
            Ok(Err(e)) => {
                self.maps.error = Some(e.to_string());
                self.maps.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.maps.error = Some("The map list did not arrive.".into());
                self.maps.rx = None;
            }
        }
    }

    /// The rows of the map table: the maps of the fetch, or the maps that the session knows, then the
    /// maps that are on but not in the list, so the pilot can turn them off.
    fn map_rows(&self, s: &Session) -> Vec<(String, String)> {
        let known: Vec<&MapInfo> = if self.maps.list.is_empty() { s.map_names.iter().collect() } else { self.maps.list.iter().collect() };
        let mut rows: Vec<(String, String)> = known.iter().map(|m| (m.id.clone(), m.name.clone())).collect();
        for id in &self.edit.nexum_maps {
            if !rows.iter().any(|(r, _)| r == id) {
                rows.push((id.clone(), "Not in the list".into()));
            }
        }
        rows
    }

    /// The theme to show now. It is the edited theme, so the pilot sees a preview.
    pub fn theme(&self) -> FactionTheme {
        self.edit.theme
    }

    /// Write the kept values to the session with the setters, and save the config one time.
    /// A value that the pilot did not change is not written. This never fails.
    pub fn finish(self, s: &mut Session, ctx: &egui::Context) {
        let kept = self.kept;
        // "Changed" means different from the values of the session now.
        let base = Draft::from_session(s);
        s.write_once(|s| {
            if (kept.check_for_updates, kept.auto_update) != (base.check_for_updates, base.auto_update) {
                s.set_update_checks(kept.check_for_updates, kept.auto_update);
            }
            if kept.theme != base.theme {
                s.set_faction_theme(kept.theme, ctx);
            }
            // The switches always start on, so only the flag shows that the pilot passed the page.
            if kept.features_kept {
                s.apply_features(kept.features);
            }
            if kept.capital != base.capital {
                s.set_capital(kept.capital);
            }
            if let Some(text) = &kept.bridges {
                s.import_bridges(text, false);
            }
            finish_nexum(s, &kept, &base);
            if (kept.metro_enabled, kept.metro_character) != (base.metro_enabled, base.metro_character) {
                s.cfg.eve_metro.enabled = kept.metro_enabled;
                s.cfg.eve_metro.character = kept.metro_character;
                s.eve_metro_saved();
            }
        });
        // A preview that the pilot dropped goes back to the saved theme.
        s.apply_theme(ctx);
    }
}

/// Write the Nexum values of the kept draft. A key in the field replaces the key. An empty field keeps it.
fn finish_nexum(s: &mut Session, kept: &Draft, base: &Draft) {
    let mut changed = false;
    // The URL or the key changes: the map list of the Nexum is old.
    let mut maps_stale = false;
    if kept.nexum_url != base.nexum_url {
        match config::parse_nexum_url(&kept.nexum_url) {
            // A difference of only a final "/" is no change.
            Ok(url) if url != s.cfg.nexum.url => {
                s.cfg.nexum.url = url;
                changed = true;
                maps_stale = true;
            }
            Ok(_) => {}
            // Only a URL from a file that a person edited by hand can fail here. The pages check the URL.
            Err(e) => tracing::warn!(error = %e, "the setup wizard keeps the old Nexum URL"),
        }
    }
    if kept.skip_unsigned != base.skip_unsigned {
        s.cfg.nexum.keep_unsigned = !kept.skip_unsigned;
        changed = true;
    }
    if kept.nexum_maps != base.nexum_maps {
        s.cfg.nexum.map_ids.clone_from(&kept.nexum_maps);
        changed = true;
    }
    let key = kept.nexum_key.trim();
    maps_stale |= !key.is_empty();
    if maps_stale {
        s.map_names.clear();
    }
    if !key.is_empty() {
        // This also saves the config and tells the worker.
        s.set_nexum_key(Some(ApiKey(key.to_string())));
    } else if changed {
        s.nexum_saved();
    }
}

/// The height that a scope of the Updates and Theme row may take. The cards keep their own height.
const ROW_ROOM: f32 = 10_000.0;

/// The number of character rows that the table of the EVE Metro page shows before it scrolls.
const METRO_ROWS_SHOWN: usize = 6;

/// The height of one row of the map table.
const MAP_ROW: f32 = 28.0;

/// The number of map rows that the table of the Nexum page shows before it scrolls.
const MAP_ROWS_SHOWN: usize = 6;

/// The share of the row that the Updates card takes. The Theme card takes the rest.
const UPDATES_SHARE: f32 = 0.4;

/// The width of the column of a page. All the pages and the welcome screen use it, so the content
/// keeps the same edges when the pilot goes from one screen to the next.
const PAGE_COLUMN: f32 = theme::SETUP_COLUMN;

/// The number of switches in the Features card. They share one row in equal columns.
const FEATURE_COUNT: f32 = 5.0;

/// The number of character rows that the table shows before it scrolls.
const CHARACTER_ROWS_SHOWN: usize = 3;

/// The height of the box with the note "No pilot is linked yet."
const EMPTY_BOX: f32 = 72.0;

/// The height of the "+ Add character" button.
const ADD_HEIGHT: f32 = 32.0;

/// The size of the title of a page, in points.
const PAGE_TITLE_SIZE: f32 = 20.0;

/// The height of the row of the title of a page.
const PAGE_TITLE_ROW: f32 = 28.0;

/// The height of the row of the intro line of a page.
const PAGE_INTRO_ROW: f32 = 20.0;

/// The room that the head of a page takes above its first card: the title, the intro line and the
/// gap. The cards of a page start this far under the top of the page area.
const HEAD_ROOM: f32 = PAGE_TITLE_ROW + PAGE_INTRO_ROW + CARD_GAP;

/// The number of problem lines that the Alliance page shows. One dim line counts the others.
const PROBLEM_LINES: usize = 5;

/// The height of the paste box on the Alliance page, about 5 lines of text. The box scrolls inside.
const PASTE_BOX: f32 = 110.0;

/// The number of bridge rows that the table of the Alliance page shows before it scrolls.
const BRIDGE_ROWS_SHOWN: usize = 5;

/// The height of one row of the bridge table.
const BRIDGE_ROW: f32 = 28.0;

/// The inner size of the smallest window of the app, in points: 1280 by 748, less the title bar of 32.
#[cfg(test)]
pub(crate) const SMALLEST_WINDOW: egui::Vec2 = egui::vec2(1280.0, 716.0);

/// The room that the wizard frame takes above and below the page, in the smallest window: 64
/// above (the margin, the steps row with its strip, and the gap) and 102 below (the margin of
/// the page, the footer of 49 and the status bar of 37). This is the worst case: it counts the bar
/// of a dry run, which shows only the dry-run note and is not taller than 37. Outside a dry run the
/// wizard draws no bar, so the real room is smaller. The page tests use it. The view test
/// `the_wizard_fits_the_smallest_window` measures the real room in a dry run and keeps it at most
/// this value.
#[cfg(test)]
pub(crate) const FRAME_ROOM: f32 = 166.0;

/// The gap between two step labels, in points.
const STEP_GAP: f32 = 24.0;

/// The gap between the step labels and the progress strip, in points.
const STRIP_GAP: f32 = 6.0;

/// The radius of the dot that marks the active step, in points.
const DOT_RADIUS: f32 = 4.0;

/// The height that the strip and its gap add under the step labels: the gap plus the diameter of the dot.
const STRIP_ROOM: f32 = STRIP_GAP + 2.0 * DOT_RADIUS;

/// The time that the dot takes to slide from one step label to the next, in seconds.
const SLIDE_SECS: f32 = 0.25;

/// A dim text, for a label or a note.
fn dim(text: &str) -> RichText {
    RichText::new(text).color(theme::TEXT_DIM)
}

impl Wizard {
    /// The names of the pages in one row, centered in the width of the panel. The current page
    /// shows in the accent color. Under the row, a strip shows the progress: the accent color
    /// runs to the active step, and a dot marks that step. The dot slides to a new step.
    pub fn steps(&self, ui: &mut Ui) {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let galleys: Vec<_> = self
            .pages
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let color = if index == self.at { theme::accent(ui) } else { theme::TEXT_DIM };
                ui.painter().layout_no_wrap(page.label().to_string(), font.clone(), color)
            })
            .collect();
        let row_width = galleys.iter().map(|g| g.size().x).sum::<f32>() + STEP_GAP * galleys.len().saturating_sub(1) as f32;
        let row_height = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max);
        let (area, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height + STRIP_ROOM), egui::Sense::hover());
        let left = area.center().x - row_width / 2.0;
        let strip_y = area.top() + row_height + STRIP_GAP + DOT_RADIUS;
        // Lay the labels out from the left end of the row and note the center of each one.
        let mut x = left;
        let mut centers = Vec::with_capacity(self.pages.len());
        for (index, (page, galley)) in self.pages.iter().zip(galleys).enumerate() {
            let width = galley.size().x;
            centers.push(x + width / 2.0);
            // A painted text has no widget, so give each label a hover response with its name for
            // the accessibility tree.
            let rect = egui::Rect::from_min_size(egui::pos2(x, area.top()), egui::vec2(width, row_height));
            let response = ui.interact(rect, ui.id().with(("step", index)), egui::Sense::hover());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), page.label()));
            ui.painter().galley(rect.min, galley, theme::TEXT_DIM);
            x += width + STEP_GAP;
        }
        // Animate the index of the active step. The x comes from the centers of this frame, so a
        // resize during the slide stays correct. With animations off, the time is 0 and the dot jumps.
        let secs = if ui.ctx().global_style().animation_time == 0.0 { 0.0 } else { SLIDE_SECS };
        let value = ui.ctx().animate_value_with_time(ui.id().with("step-dot"), self.at as f32, secs);
        let last = centers.len().saturating_sub(1);
        let from = (value.floor().max(0.0) as usize).min(last);
        let to = (from + 1).min(last);
        let ease = egui::emath::easing::cubic_out((value - from as f32).clamp(0.0, 1.0));
        let active_x = centers.get(from).map_or(left, |&a| egui::lerp(a..=centers[to], ease));
        // The strip is dim over the whole row, with the accent color over it up to the dot.
        let strip = |from: f32, to: f32| egui::Rect::from_min_max(egui::pos2(from, strip_y - 1.0), egui::pos2(to, strip_y + 1.0));
        ui.painter().rect_filled(strip(left, left + row_width), 1.0, theme::TEXT_OFF);
        ui.painter().rect_filled(strip(left, active_x), 1.0, theme::accent(ui));
        ui.painter().circle_filled(egui::pos2(active_x, strip_y), DOT_RADIUS, theme::accent(ui));
    }

    /// The buttons of the footer: Back at the left, Skip and Next at the right. Next reads "Finish"
    /// on the last page. Back is on every page. On the first page, it asks for the welcome screen
    /// and changes nothing. Return what the wizard does after the button that the pilot pushed.
    pub fn footer(&mut self, ui: &mut Ui) -> Flow {
        let mut flow = Flow::Open;
        let last = self.at + 1 == self.pages.len();
        ui.horizontal(|ui| {
            if ui.add(AppButton::new("Back")).clicked() {
                if self.at == 0 {
                    flow = Flow::Welcome;
                } else {
                    self.back();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(AppButton::new(if last { "Finish" } else { "Next" }).primary()).clicked() {
                    flow = self.next();
                }
                if ui.app_button("Skip").clicked() {
                    flow = self.skip();
                }
            });
        });
        flow
    }

    /// Draw the current page. The page edits the draft only. The one call that reaches the session
    /// is the login of "+ Add character". A new theme shows in the next frame as a preview.
    pub fn body(&mut self, ui: &mut Ui, s: &mut Session, pilots: &mut PilotsUi) {
        match self.page() {
            WizardPage::Essentials => essentials_page(ui, self, s, pilots),
            WizardPage::Alliance => alliance_page(ui, self, s),
            WizardPage::Nexum => {
                self.poll_maps();
                nexum_page(ui, self, s);
            }
            WizardPage::EveMetro => page_shell(ui, WizardPage::EveMetro, |ui| eve_metro_page(ui, self, s, &mut pilots.portraits)),
        }
        // After the page, so a theme button of this frame shows in the next frame. A change from
        // Skip also comes here, at the end of this frame or of the next one.
        self.preview_theme(ui.ctx(), s.cfg.compact);
    }

    /// Apply the edited theme when the window does not show it yet. A `Ui` keeps the style that
    /// it had at the start of the frame, and a style change asks for no new frame. Thus ask for
    /// one, so the preview shows without an input event.
    fn preview_theme(&mut self, ctx: &egui::Context, compact: bool) {
        if self.theme() != self.applied {
            theme::apply(ctx, self.theme(), compact);
            self.applied = self.theme();
            ctx.request_repaint();
        }
    }
}

/// The Essentials page: the keyring warning, then one centered column with the characters (when
/// the build has an EVE client ID) and the cards for updates, theme and features. Each card keeps
/// its own height. The column scrolls when the window is too small for it.
fn essentials_page(ui: &mut Ui, w: &mut Wizard, s: &mut Session, pilots: &mut PilotsUi) {
    keyring_warning(ui, s);
    page_shell(ui, WizardPage::Essentials, |ui| {
        if s.pilots.shows_characters() {
            characters_card(ui, s, pilots);
            ui.add_space(card_gap(ui));
        }
        option_cards(ui, w);
    });
}

/// The space that a page adds between two cards: the card gap, less the space that the layout adds after a card.
fn card_gap(ui: &Ui) -> f32 {
    CARD_GAP - ui.spacing().item_spacing.y
}

/// The room of the column of a page:`PAGE_COLUMN` points wide, centered, as high as the page has room.
fn page_area(ui: &Ui) -> egui::Rect {
    let area = ui.available_rect_before_wrap();
    let width = PAGE_COLUMN.min(area.width());
    egui::Rect::from_min_size(egui::pos2(area.center().x - width / 2.0, area.top()), vec2(width, area.height()))
}

/// The shell of a page: one centered column with the centered title and intro line of `page`, then the
/// cards that `add` draws. The column scrolls when the window is too small for them. The head
/// takes `HEAD_ROOM`, so every page puts its first card at the same place.
fn page_shell(ui: &mut Ui, page: WizardPage, add: impl FnOnce(&mut Ui)) {
    let column = page_area(ui);
    ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            page_head(ui, page);
            add(ui);
        });
    });
}

/// The title and the intro line of `page`, in rows of a fixed height, then the gap to the first
/// card. The rows have no spacing between them, so the head is `HEAD_ROOM` high.
fn page_head(ui: &mut Ui, page: WizardPage) {
    // The layout adds this space after the head, so the gap is the card gap less this space.
    let after = ui.spacing().item_spacing.y;
    let width = ui.available_width();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let title = RichText::new(page.label()).font(egui::FontId::new(PAGE_TITLE_SIZE, theme::bold())).color(theme::TEXT);
        ui.add_sized([width, PAGE_TITLE_ROW], egui::Label::new(title));
        ui.add_sized([width, PAGE_INTRO_ROW], egui::Label::new(dim(page.intro())));
        ui.add_space(HEAD_ROOM - PAGE_TITLE_ROW - PAGE_INTRO_ROW - after);
    });
}

/// The Characters card: a hint, the table of the linked characters, then "+ Add character".
fn characters_card(ui: &mut Ui, s: &mut Session, pilots: &mut PilotsUi) {
    theme::panel(ui, "Characters", "", false, |ui| {
        ui.label(dim("Log in each pilot that you fly."));
        character_list(ui, s, pilots);
        let blocked = s.pilots.login_blocked();
        let busy = s.pilots.login.is_some();
        let empty = s.pilots.characters().is_empty();
        let mut add = AppButton::new("+ Add character").min_size(vec2(ui.available_width(), ADD_HEIGHT));
        if empty {
            add = add.primary();
        }
        let mut button = ui.add_enabled(blocked.is_none() && !busy, add);
        if let Some(reason) = &blocked {
            button = button.on_disabled_hover_text(reason.as_str());
        }
        // The login opens the characters window with the login steps.
        if button.clicked() {
            pilots.login(s);
        }
    });
}

/// The height that a table of characters gets for `rows` rows. The table takes the height that is
/// left and keeps 8 points free below its rows.
fn table_room(ui: &Ui, rows: usize) -> f32 {
    rows as f32 * (CHARACTER_ROW + ui.spacing().item_spacing.y) + 8.0
}

/// The linked characters in the table of the Settings window, with a Status column, or a box with
/// a note when no pilot is linked. The table shows `CHARACTER_ROWS_SHOWN` rows and scrolls inside,
/// so the page keeps its height for 1 to 100 characters.
fn character_list(ui: &mut Ui, s: &Session, pilots: &mut PilotsUi) {
    let characters = s.pilots.characters();
    if characters.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), EMPTY_BOX), egui::Sense::hover());
        ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
        let font = egui::FontId::proportional(14.0);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "No pilot is linked yet.", font, theme::TEXT_DIM);
        return;
    }
    let (room, width) = (table_room(ui, CHARACTER_ROWS_SHOWN), ui.available_width());
    ui.allocate_ui(vec2(width, room), |ui| {
        use egui_extras::Column;
        let extra = [("Status", Column::exact(110.0))];
        let look = |_: &PilotView| RowLook::default();
        character_table(ui, "wizard-characters", &characters, &mut pilots.portraits, &extra, look, |ui, pilot, _| {
            let (text, color) = pilot_status(pilot);
            ui.label(RichText::new(text).color(color));
        });
    });
}

/// One card of the column: a panel with `title` that keeps the height of its content.
fn option_card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    theme::panel(ui, title, "", false, add);
}

/// The cards of the Essentials page after the characters: Updates and Theme side by side, then
/// Features. The two cards of the row have the height of two checkbox rows.
fn option_cards(ui: &mut Ui, w: &mut Wizard) {
    let gap = card_gap(ui);
    let row = 2.0 * ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    // The Theme card needs more room than the Updates card: its four buttons stay in one row. Each
    // card gets a rectangle of the row, and the row is as high as the taller card.
    let free = ui.available_width() - CARD_GAP;
    let updates = free * UPDATES_SHARE;
    let top = ui.cursor().min;
    let left = egui::Rect::from_min_size(top, vec2(updates, ROW_ROOM));
    let right = egui::Rect::from_min_size(top + vec2(updates + CARD_GAP, 0.0), vec2(free - updates, ROW_ROOM));
    let first = ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
        option_card(ui, "Updates", |ui| {
            ui.set_min_height(row);
            ui.app_checkbox(&mut w.edit.check_for_updates, "Check for updates");
            let auto = ui.add_enabled_ui(w.edit.check_for_updates, |ui| ui.app_checkbox(&mut w.edit.auto_update, "Install automatically"));
            auto.response.on_disabled_hover_text("Checking for updates must be enabled.");
        });
    });
    let second = ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
        option_card(ui, "Theme", |ui| {
            ui.set_min_height(row);
            ui.horizontal(|ui| {
                for faction in FactionTheme::ALL {
                    if ui.add(AppButton::new(faction.label()).choice(w.edit.theme == faction)).clicked() {
                        w.edit.theme = faction;
                    }
                }
            });
        });
    });
    let bottom = first.response.rect.bottom().max(second.response.rect.bottom());
    let whole = egui::Rect::from_min_max(top, egui::pos2(right.right(), bottom));
    ui.allocate_rect(whole, egui::Sense::hover());
    ui.add_space(gap);
    option_card(ui, "Features", |ui| {
        // One row of equal columns fills the width of the card.
        let column = (ui.available_width() - (FEATURE_COUNT - 1.0) * ui.spacing().item_spacing.x) / FEATURE_COUNT;
        egui::Grid::new("wizard-features").num_columns(FEATURE_COUNT as usize).min_col_width(column).show(ui, |ui| {
            ui.app_checkbox(&mut w.edit.features.filaments, "Filaments");
            ui.app_checkbox(&mut w.edit.features.pochven, "Pochven");
            // The box is on when all six classes are on, and shows a dash when some are on. A click
            // on a dash switches all six classes on.
            let mut all = w.edit.features.classes.iter().all(|on| *on);
            let partial = w.edit.features.jspace() && !all;
            let mut response = ui.add(AppCheckbox::new(&mut all, "J-space").mixed(partial));
            if partial {
                response = response.on_hover_text("Some classes are off. A click switches all on.");
            }
            if response.changed() {
                w.edit.features.classes = [all; 6];
            }
            ui.app_checkbox(&mut w.edit.features.thera, "Thera");
            ui.app_checkbox(&mut w.edit.features.turnur, "Turnur");
            ui.end_row();
        });
    });
}

/// The Alliance page: two stacked cards. The first card has the two groups of the Ansiblex page of
/// the Settings window: the capital, then the box for a list of jump bridges with its Import button
/// and the problem lines of the last import. The second card has the table of the bridges that
/// Finish adds. The table has no zones: the zone of a bridge end needs the capital, and the capital
/// of the session changes at Finish.
fn alliance_page(ui: &mut Ui, w: &mut Wizard, s: &Session) {
    // Finish adds only the bridges that the list does not hold. `read` counts all the bridges of
    // the imported text, so the page can tell "nothing imported" from "all in the list already".
    let read = w.edit.bridges.as_ref().map_or(0, |text| s.read_bridges(text).0.len());
    let staged: Vec<_> = match &w.edit.bridges {
        Some(text) => s.read_bridges(text).0.into_iter().filter(|pair| !s.bridges.contains(pair)).collect(),
        None => Vec::new(),
    };
    page_shell(ui, WizardPage::Alliance, |ui| {
        theme::panel(ui, "Ansiblex", "", false, |ui| {
            group(ui, "Alliance capital");
            egui::Grid::new("wizard-capital").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
                ui.label(dim("System"));
                // The name and Clear while a capital is set, else the search box.
                if let Some(capital) = w.edit.capital {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(s.uni.name(capital)).color(Color32::WHITE));
                        if ui.app_button("Clear").clicked() {
                            w.edit.capital = None;
                        }
                    });
                } else if let Some((_, node)) = w.capital.show(ui, &s.uni, FIELD, "Search system\u{2026} (none: jump bridges off)", &[]) {
                    w.edit.capital = Some(node);
                }
                ui.end_row();
            });
            ui.add_space(GROUP_GAP);
            group(ui, "Import");
            // Only the first problem lines show, and one dim line counts the others.
            let shown = w.edit.bridge_problems.len().min(PROBLEM_LINES);
            let hidden = w.edit.bridge_problems.len() - shown;
            // The box keeps one height and scrolls inside, so a long list cannot push Import down.
            ScrollArea::vertical().id_salt("wizard-bridges").min_scrolled_height(PASTE_BOX).max_height(PASTE_BOX).auto_shrink([false, false]).show(
                ui,
                |ui| {
                    ui.add(
                        TextEdit::multiline(&mut w.edit.bridge_text)
                            .hint_text("Paste a list of bridges in SMT format.")
                            .desired_width(f32::INFINITY)
                            .min_size(vec2(0.0, PASTE_BOX)),
                    );
                },
            );
            for problem in w.edit.bridge_problems.iter().take(PROBLEM_LINES) {
                ui.label(RichText::new(problem).color(theme::ERROR));
            }
            if hidden > 0 {
                ui.label(dim(&format!("and {hidden} more.")));
            }
            let empty = w.edit.bridge_text.trim().is_empty();
            if ui.add_enabled(!empty, AppButton::new("Import").primary()).clicked() {
                w.import(s);
            }
        });
        ui.add_space(card_gap(ui));
        theme::panel(ui, &format!("Bridges to add ({})", staged.len()), "", false, |ui| {
            if read > 0 && staged.is_empty() {
                let plural = if read == 1 { "bridge is" } else { "bridges are" };
                ui.label(dim(&format!("The {read} {plural} in your list already. Finish adds nothing.")));
            } else if staged.is_empty() {
                ui.label(dim("Nothing to add yet. Paste a list, then push Import."));
            } else {
                bridge_table(ui, s, &staged);
            }
        });
    });
}

/// The table of the bridges that Finish adds: a system, the arrow, a system. The table shows
/// `BRIDGE_ROWS_SHOWN` rows, then it scrolls inside.
fn bridge_table(ui: &mut Ui, s: &Session, staged: &[router_core::overlay::Pair]) {
    use egui_extras::Column;
    egui_extras::TableBuilder::new(ui)
        .id_salt("wizard-bridge-table")
        .striped(true)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(110.0).clip(true))
        .column(Column::exact(30.0))
        .column(Column::remainder().at_least(110.0).clip(true))
        .min_scrolled_height(0.0)
        .max_scroll_height(BRIDGE_ROW * BRIDGE_ROWS_SHOWN as f32)
        .auto_shrink([false, true])
        .header(22.0, |mut header| {
            for text in ["System", "", "System"] {
                header.col(|ui| _ = ui.label(theme::header_text(text)));
            }
        })
        .body(|body| {
            body.rows(BRIDGE_ROW, staged.len(), |mut row| {
                let (a, b) = staged[row.index()];
                row.col(|ui| _ = ui.label(RichText::new(s.uni.name(a)).color(theme::TEXT)));
                row.col(|ui| _ = ui.label(RichText::new(icons::ARROWS_LEFT_RIGHT.as_str()).color(theme::BRIDGE)));
                row.col(|ui| _ = ui.label(RichText::new(s.uni.name(b)).color(theme::TEXT)));
            });
        });
}

/// The Nexum page: the keyring warning, then two stacked cards. The first card has the connection:
/// the URL and its warning, the key, the signature switch and the error of the page. The second
/// card has the maps of the Settings window, loaded with the URL and the key of the fields.
fn nexum_page(ui: &mut Ui, w: &mut Wizard, s: &Session) {
    keyring_warning(ui, s);
    page_shell(ui, WizardPage::Nexum, |ui| {
        connection_card(ui, w, s);
        ui.add_space(card_gap(ui));
        maps_card(ui, w, s);
    });
}

/// The connection card of the Nexum page.
fn connection_card(ui: &mut Ui, w: &mut Wizard, s: &Session) {
    theme::panel(ui, "Connection", "", false, |ui| {
        egui::Grid::new("wizard-nexum").num_columns(2).spacing(FORM_GAP).show(ui, |ui| {
            ui.label(dim("URL"));
            ui.add(TextEdit::singleline(&mut w.edit.nexum_url).hint_text("https://nexum.example").desired_width(f32::INFINITY));
            ui.end_row();
            // A plain http:// URL to a remote host sends the key in clear text.
            if let Some(warning) = config::url_warning(&w.edit.nexum_url) {
                ui.label("");
                ui.label(RichText::new(warning).color(theme::WARN));
                ui.end_row();
            }
            ui.label(dim("Key"));
            // The key never shows. The field starts empty, and an empty field keeps a key that is set.
            let hint = if s.cfg.nexum.key.is_some() { "A key is set. An empty field keeps it." } else { "paste a key with the read scope" };
            ui.add(TextEdit::singleline(&mut w.edit.nexum_key).password(true).hint_text(hint).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label(dim("Signatures"));
            ui.app_checkbox(&mut w.edit.skip_unsigned, "Skip connections with no known signatures");
            ui.end_row();
        });
        if let Some(error) = &w.error {
            ui.add_space(ROW_GAP);
            ui.label(RichText::new(error).color(theme::ERROR));
        }
    });
}

/// The maps card of the Nexum page: the button that loads the list, then a table with a switch for
/// each map. The router reads the maps that are on. The table takes the height that is left, and
/// scrolls inside.
fn maps_card(ui: &mut Ui, w: &mut Wizard, s: &Session) {
    theme::panel(ui, "Maps", "", false, |ui| {
        if !w.maps_ready(s) {
            ui.label(dim("Set the URL and the key to list the maps."));
            return;
        }
        let loading = w.maps.rx.is_some();
        ui.horizontal(|ui| {
            let text = if w.maps.tried { "Reload maps" } else { "Load maps" };
            if ui.add_enabled(!loading, AppButton::new(text)).clicked() {
                w.load_maps(s, ui.ctx());
            }
            if loading {
                ui.add(egui::Spinner::new().color(theme::accent(ui)));
            }
        });
        if let Some(error) = &w.maps.error {
            ui.label(RichText::new(error).color(theme::ERROR));
        }
        let rows = w.map_rows(s);
        if rows.is_empty() {
            let note = if w.maps.tried && !loading && w.maps.error.is_none() {
                "The key has access to no maps."
            } else {
                "Load the maps, then switch on the ones that the router reads."
            };
            ui.label(dim(note));
            return;
        }
        let mut toggled: Option<(String, bool)> = None;
        use egui_extras::Column;
        egui_extras::TableBuilder::new(ui)
            .id_salt("wizard-maps")
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(44.0))
            .column(Column::remainder().at_least(120.0).clip(true))
            .column(Column::exact(110.0).clip(true))
            .min_scrolled_height(0.0)
            .max_scroll_height(MAP_ROW * MAP_ROWS_SHOWN as f32)
            .auto_shrink([false, true])
            .header(22.0, |mut header| {
                for text in ["On", "Map", ""] {
                    header.col(|ui| _ = ui.label(theme::header_text(text)));
                }
            })
            .body(|body| {
                body.rows(MAP_ROW, rows.len(), |mut row| {
                    let (id, name) = rows[row.index()].clone();
                    let mut on = w.edit.nexum_maps.contains(&id);
                    row.col(|ui| {
                        if ui.app_checkbox(&mut on, "").changed() {
                            toggled = Some((id.clone(), on));
                        }
                    });
                    row.col(|ui| _ = ui.label(RichText::new(name).color(if on { theme::TEXT } else { theme::TEXT_DIM })));
                    row.col(|ui| _ = ui.label(dim(&id)));
                });
            });
        if let Some((id, on)) = toggled {
            if on {
                w.edit.nexum_maps.push(id);
            } else {
                w.edit.nexum_maps.retain(|m| *m != id);
            }
        }
        if w.edit.nexum_maps.is_empty() {
            ui.label(dim("No map is on: the router reads no Nexum data."));
        }
    });
}

/// The EVE Metro page: one card with the Enable switch and a table of the linked characters. A
/// "Use" button picks the character that identifies the pilot to EVE Metro. While Enable is on and
/// no character is picked, the page shows the warning of the Settings page. The table shows
/// `METRO_ROWS_SHOWN` rows, then it scrolls inside.
fn eve_metro_page(ui: &mut Ui, w: &mut Wizard, s: &Session, portraits: &mut Portraits) {
    theme::panel(ui, "Account", "", false, |ui| {
        ui.app_checkbox(&mut w.edit.metro_enabled, "Enable");
        ui.add_space(GROUP_GAP);
        group(ui, "Character");
        // Finish saves Enable with no character, but EVE Metro stays off until a character is picked.
        if w.edit.metro_enabled && w.edit.metro_character.is_none() {
            ui.label(RichText::new(METRO_NEEDS_CHARACTER).color(theme::WARN));
        }
        let characters = s.pilots.characters();
        if characters.is_empty() {
            ui.label(dim("No linked character. Add one on the Essentials page."));
            return;
        }
        let picked = w.edit.metro_character;
        let mut pick = None;
        use egui_extras::Column;
        let extra = [("Status", Column::exact(110.0)), ("", Column::exact(84.0))];
        let (room, width) = (table_room(ui, METRO_ROWS_SHOWN), ui.available_width());
        ui.allocate_ui(vec2(width, room), |ui| {
            character_table(
                ui,
                "wizard-metro-characters",
                &characters,
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
                        if ui.add(AppButton::new("Use")).clicked() {
                            pick = Some(pilot.id);
                        }
                    }
                },
            );
        });
        if let Some(id) = pick {
            w.edit.metro_character = Some(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pilots_view::{PilotsUi, Portraits};
    use crate::settings_window::KEYRING_WARNING;
    use crate::test_ui::{self, drawn_texts};
    use crate::theme::FactionColors;
    use router_core::esi::pilots::Pilots;
    use router_core::test_support::{overlay_universe, settings};
    use router_core::wormhole_space::{Stance, WormholeSpace};

    /// A session with a config in a temporary directory. With `client_id`, it has an EVE client ID
    /// and the characters Alice and Bo. With `metro_key`, it has an EVE Metro key.
    fn session(client_id: bool, metro_key: bool) -> (Session, tempfile::TempDir) {
        let uni = overlay_universe();
        let settings = settings(&uni, Some("black-ops"));
        let shortcuts =
            router_core::labels::Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default(), &Default::default());
        let tmp = tempfile::tempdir().unwrap();
        let mut s =
            Session::new(std::sync::Arc::new(uni), settings, config::Config::default(), tmp.path().join("eve-router.json"), shortcuts);
        let active = tmp.path().join("active-route.json");
        s.pilots =
            if client_id { Pilots::offline_with_client_id(active, &[(1, "Alice Ander"), (2, "Bo Berg")]) } else { Pilots::offline(active) };
        if metro_key {
            s.cfg.eve_metro.key = Some(ApiKey("metro_test_key".into()));
        }
        (s, tmp)
    }

    /// The session values that the pages edit. The key shows masked in the config text, so a test
    /// that looks at a key reads the key itself.
    fn state(s: &Session) -> (String, Stance, WormholeSpace, Option<NodeIndex>, usize) {
        (format!("{:?}", s.cfg), s.settings.filaments, s.settings.space, s.settings.rules.capital, s.bridges.len())
    }

    /// Edit every page and press Next on each. The last Next gives `Flow::Done`.
    fn walk(s: &Session) -> Wizard {
        let mut w = Wizard::new(s);
        w.edit.check_for_updates = false;
        w.edit.theme = FactionTheme::Gallente;
        w.edit.features = Features { classes: [false; 6], ..Features::default() };
        assert_eq!(w.next(), Flow::Open);
        w.edit.capital = s.uni.exact("UALX-3");
        w.edit.bridge_text = "UALX-3 --> IL-YTR".into();
        w.import(s);
        assert_eq!(w.next(), Flow::Open);
        w.edit.nexum_url = "https://nexum.example/".into();
        w.edit.nexum_key = "nxm_test_key".into();
        w.edit.skip_unsigned = false;
        assert_eq!(w.next(), Flow::Open);
        w.edit.metro_enabled = true;
        w.edit.metro_character = Some(2);
        assert_eq!(w.next(), Flow::Done);
        w
    }

    #[test]
    fn the_steps_follow_the_gates() {
        use WizardPage::*;
        let custom = |client_id, metro| pages_for(Preset::Custom, Preset::Custom.answers(), client_id, metro);
        assert_eq!(custom(true, true), [Essentials, Alliance, Nexum, EveMetro]);
        assert_eq!(custom(true, false), [Essentials, Alliance, Nexum]);
        assert_eq!(custom(false, true), [Essentials, Alliance, Nexum]);
        assert_eq!(custom(false, false), [Essentials, Alliance, Nexum]);
        // Wizard::new reads the gates from the session. Without a client ID, a key is not enough.
        let page_list = |client_id, metro_key| Wizard::new(&session(client_id, metro_key).0).pages;
        assert_eq!(page_list(true, true), [Essentials, Alliance, Nexum, EveMetro]);
        assert_eq!(page_list(true, false), [Essentials, Alliance, Nexum]);
        assert_eq!(page_list(false, true), [Essentials, Alliance, Nexum]);
        let labels: Vec<_> = custom(true, true).into_iter().map(WizardPage::label).collect();
        assert_eq!(labels, ["Essentials", "Alliance", "Nexum", "EVE Metro"]);
    }

    #[test]
    fn next_keeps_and_skip_drops_the_edits_of_a_page() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        w.edit.theme = FactionTheme::Amarr;
        assert_eq!(w.next(), Flow::Open);
        assert_eq!((w.kept.theme, w.kept.features_kept), (FactionTheme::Amarr, true));
        // Skip drops the edits of a page that is not the Essentials page.
        assert_eq!(w.page(), WizardPage::Alliance);
        w.edit.capital = s.uni.exact("UALX-3");
        assert!(w.edit.capital.is_some());
        assert_eq!(w.skip(), Flow::Open);
        assert_eq!(w.page(), WizardPage::Nexum);
        assert_eq!(w.edit.capital, s.settings.rules.capital, "skip drops the capital");
        assert_eq!(w.kept.capital, s.settings.rules.capital);
    }

    #[test]
    fn skip_drops_the_typed_capital_search() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.page(), WizardPage::Alliance);
        w.capital.set_query("UAL");
        assert_eq!(w.capital.query(), "UAL");
        assert_eq!(w.skip(), Flow::Open);
        w.back();
        assert_eq!(w.page(), WizardPage::Alliance);
        assert_eq!(w.capital.query(), "", "skip drops the text that the pilot typed");
        // Back and Next keep the box as it is.
        w.capital.set_query("UAL");
        w.back();
        assert_eq!(w.capital.query(), "UAL");
    }

    #[test]
    fn back_keeps_the_edits_and_does_nothing_on_the_first_page() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        w.back();
        assert_eq!(w.page(), WizardPage::Essentials);
        assert!(!w.kept.features_kept, "Back on the first page keeps nothing");
        assert_eq!(w.next(), Flow::Open);
        let ualx = s.uni.exact("UALX-3");
        assert!(ualx.is_some());
        w.edit.capital = ualx;
        w.back();
        assert_eq!(w.page(), WizardPage::Essentials);
        assert_eq!(w.kept.capital, ualx);
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.edit.capital, ualx);
    }

    #[test]
    fn next_and_skip_on_the_last_page_finish() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        for _ in 0..3 {
            assert_eq!(w.next(), Flow::Open);
        }
        assert_eq!(w.page(), WizardPage::EveMetro);
        assert_eq!(w.next(), Flow::Done);
        let mut w = Wizard::new(&s);
        for _ in 0..3 {
            assert_eq!(w.skip(), Flow::Open);
        }
        assert_eq!(w.skip(), Flow::Done);
        // Without a client ID, the Nexum page is the last page.
        let (s, _tmp) = session(false, true);
        let mut w = Wizard::new(&s);
        assert_eq!((w.next(), w.next()), (Flow::Open, Flow::Open));
        assert_eq!(w.page(), WizardPage::Nexum);
        assert_eq!(w.next(), Flow::Done);
        let mut w = Wizard::new(&s);
        assert_eq!((w.skip(), w.skip()), (Flow::Open, Flow::Open));
        assert_eq!(w.skip(), Flow::Done);
    }

    #[test]
    fn no_page_writes_before_finish() {
        let (s, tmp) = session(true, true);
        let before = state(&s);
        // Edit a value on each page, import a list, and press Next on all pages. Finish never runs.
        let w = walk(&s);
        assert_eq!(w.kept.nexum_key, "nxm_test_key", "the walk kept the edits");
        assert!(w.kept.bridges.is_some() && w.kept.capital.is_some() && w.kept.metro_enabled);
        drop(w);
        assert_eq!(state(&s), before);
        assert!(tmp.path().read_dir().unwrap().next().is_none(), "no file on the disk");
    }

    #[test]
    fn finish_writes_the_kept_draft_through_the_setters() {
        let (mut s, tmp) = session(true, true);
        s.settings.space.drifter = Stance::Avoid;
        let ctx = egui::Context::default();
        walk(&s).finish(&mut s, &ctx);
        let ualx = s.uni.exact("UALX-3");
        assert_eq!(s.cfg.check_for_updates, Some(false));
        assert_eq!(s.faction_theme(), FactionTheme::Gallente);
        let space = s.settings.space;
        assert_eq!([space.c1, space.c2, space.c3, space.c4, space.c5, space.c6], [Stance::Block; 6]);
        assert_eq!((space.drifter, space.pochven, s.settings.filaments), (Stance::Avoid, Stance::Allow, Stance::Allow));
        assert_eq!(s.settings.rules.capital, ualx);
        assert_eq!(s.bridges.len(), 1);
        assert_eq!(s.cfg.nexum.url.as_deref(), Some("https://nexum.example"));
        assert_eq!(s.cfg.nexum.key, Some(ApiKey("nxm_test_key".into())));
        assert!(s.cfg.nexum.keep_unsigned);
        assert!(s.cfg.eve_metro.enabled);
        assert_eq!(s.cfg.eve_metro.character, Some(2));
        // The file holds the same values. The file never holds the key that the pilot enters.
        let path = tmp.path().join("eve-router.json");
        let file = config::Config::load(&path).unwrap();
        assert_eq!(file.capital.as_deref(), Some("UALX-3"));
        assert_eq!((file.check_for_updates, file.theme), (Some(false), Some(FactionTheme::Gallente)));
        assert_eq!(
            (file.wormhole_space.c1, file.wormhole_space.c6, file.wormhole_space.drifter),
            (Stance::Block, Stance::Block, Stance::Avoid)
        );
        assert_eq!(file.nexum.url.as_deref(), Some("https://nexum.example"));
        assert!(file.nexum.keep_unsigned && file.nexum.key.is_none());
        assert_eq!((file.eve_metro.enabled, file.eve_metro.character), (true, Some(2)));
        assert!(tmp.path().join(config::BRIDGES_FILE).exists());
    }

    #[test]
    fn finish_writes_no_stance_before_the_essentials_page_is_passed() {
        let (mut s, tmp) = session(true, true);
        s.settings.filaments = Stance::Avoid;
        s.settings.space.pochven = Stance::Block;
        let mut w = Wizard::new(&s);
        w.edit.features = Features { filaments: false, classes: [false; 6], pochven: false, thera: false, turnur: false };
        assert!(!w.kept.features_kept);
        w.finish(&mut s, &egui::Context::default());
        assert_eq!((s.settings.filaments, s.settings.space.pochven), (Stance::Avoid, Stance::Block));
        assert_eq!(s.settings.space.c1, Stance::Allow, "the J-space switch did not run");
        // Finish always writes the config one time. The stances are in the file.
        let file = config::Config::load(&tmp.path().join("eve-router.json")).unwrap();
        assert_eq!(file.filaments, Some(Stance::Avoid));
        assert_eq!(file.wormhole_space.pochven, Stance::Block);
    }

    #[test]
    fn a_dry_run_finish_writes_nothing() {
        let (mut s, tmp) = session(true, true);
        s.set_dry_run();
        assert!(s.is_dry_run());
        walk(&s).finish(&mut s, &egui::Context::default());
        assert!(tmp.path().read_dir().unwrap().next().is_none(), "no file on the disk");
        // The session holds the values.
        assert!(s.is_dry_run());
        assert_eq!(s.cfg.check_for_updates, Some(false));
        assert_eq!(s.faction_theme(), FactionTheme::Gallente);
        assert_eq!(s.settings.space.c1, Stance::Block);
        assert_eq!(s.bridges.len(), 1);
        assert_eq!(s.cfg.nexum.key, Some(ApiKey("nxm_test_key".into())));
        assert_eq!(s.cfg.eve_metro.character, Some(2));
    }

    #[test]
    fn a_bad_nexum_url_never_reaches_kept() {
        let (mut s, _tmp) = session(true, true);
        s.cfg.nexum.url = Some("https://old.example".into());
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.page(), WizardPage::Nexum);
        w.edit.nexum_url = "nexum.example".into();
        w.edit.skip_unsigned = false;
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.page(), WizardPage::Nexum);
        assert!(w.error.is_some());
        assert_eq!(w.kept.nexum_url, "https://old.example");
        assert!(w.kept.skip_unsigned, "the other edits of the page also wait");
        // Back drops the URL edit, keeps the other edits, and clears the error.
        w.back();
        assert_eq!(w.page(), WizardPage::Alliance);
        assert!(w.error.is_none());
        assert_eq!(w.kept.nexum_url, "https://old.example");
        assert!(!w.kept.skip_unsigned);
        // Skip drops the bad edit and the error.
        assert_eq!(w.next(), Flow::Open);
        w.edit.nexum_url = "nexum.example".into();
        assert_eq!(w.next(), Flow::Open);
        assert!(w.error.is_some());
        assert_eq!(w.skip(), Flow::Open);
        assert!(w.error.is_none());
        assert_eq!(w.page(), WizardPage::EveMetro);
        assert_eq!(w.kept.nexum_url, "https://old.example");
        assert_eq!(w.edit.nexum_url, "https://old.example");
        // A URL that parses goes through.
        w.back();
        w.edit.nexum_url = "https://new.example".into();
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.kept.nexum_url, "https://new.example");
        assert!(w.error.is_none());
    }

    #[test]
    fn a_bad_url_in_kept_does_not_stop_finish() {
        // A URL from a file that a person edited by hand is the only way to a bad kept URL.
        let (mut s, tmp) = session(true, true);
        s.cfg.nexum.url = Some("https://old.example".into());
        let mut w = Wizard::new(&s);
        w.kept.nexum_url = "nexum.example".into();
        w.kept.skip_unsigned = false;
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.cfg.nexum.url.as_deref(), Some("https://old.example"));
        assert!(s.cfg.nexum.keep_unsigned, "the other Nexum values still go in");
        assert!(tmp.path().join("eve-router.json").exists());
    }

    #[test]
    fn an_empty_key_field_keeps_the_key() {
        let (mut s, _tmp) = session(true, true);
        s.cfg.nexum.key = Some(ApiKey("nxm_old_key".into()));
        // Pass 1: the URL and the switch change, the key field stays empty. The key stays.
        let mut w = Wizard::new(&s);
        w.edit.nexum_url = "https://nexum.example".into();
        w.edit.skip_unsigned = false;
        for _ in 0..3 {
            w.next();
        }
        assert_eq!(w.next(), Flow::Done);
        assert_eq!(w.kept.nexum_key, "");
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.cfg.nexum.key, Some(ApiKey("nxm_old_key".into())));
        assert_eq!(s.cfg.nexum.url.as_deref(), Some("https://nexum.example"));
        // Pass 2: nothing changes on the Nexum page. The key stays. A field with only spaces is empty.
        let mut w = Wizard::new(&s);
        w.edit.nexum_key = "   ".into();
        for _ in 0..3 {
            w.next();
        }
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.cfg.nexum.key, Some(ApiKey("nxm_old_key".into())));
    }

    #[test]
    fn an_import_with_no_bridge_keeps_nothing() {
        let (mut s, tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        w.edit.bridge_text = "Nope --> Jita".into();
        w.import(&s);
        assert!(!w.edit.bridge_problems.is_empty());
        assert_eq!(w.edit.bridges, None);
        // A good list first, then a bad one: the bad import drops the earlier list.
        w.edit.bridge_text = "UALX-3 --> IL-YTR".into();
        w.import(&s);
        assert!(w.edit.bridges.is_some() && w.edit.bridge_problems.is_empty());
        w.edit.bridge_text = "Nope --> Jita".into();
        w.import(&s);
        assert_eq!(w.edit.bridges, None);
        for _ in 0..3 {
            w.next();
        }
        w.finish(&mut s, &egui::Context::default());
        assert!(s.bridges.is_empty());
        assert!(!tmp.path().join(config::BRIDGES_FILE).exists());
    }

    /// A session whose file holds a Nexum URL that a person edited by hand and that does not parse.
    fn session_with_bad_url() -> (Session, tempfile::TempDir) {
        let (mut s, tmp) = session(true, true);
        s.cfg.nexum.url = Some("nexum.example".into());
        (s, tmp)
    }

    /// Go to the Nexum page with Next.
    fn to_nexum(w: &mut Wizard) {
        assert_eq!((w.next(), w.next()), (Flow::Open, Flow::Open));
        assert_eq!(w.page(), WizardPage::Nexum);
    }

    #[test]
    fn a_bad_session_url_does_not_block_next_or_back() {
        let (s, _tmp) = session_with_bad_url();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_key = "nxm_typed_key".into();
        // The pilot did not type the URL, so Next needs no check.
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.page(), WizardPage::EveMetro);
        assert!(w.error.is_none());
        assert_eq!(w.kept.nexum_key, "nxm_typed_key");
        // Back to the Nexum page, then to the Alliance page: the key stays, and no error shows.
        w.back();
        assert_eq!(w.page(), WizardPage::Nexum);
        assert!(w.error.is_none());
        assert_eq!(w.edit.nexum_key, "nxm_typed_key");
        w.back();
        assert_eq!(w.page(), WizardPage::Alliance);
        assert!(w.error.is_none());
        assert_eq!((w.kept.nexum_key.as_str(), w.kept.nexum_url.as_str()), ("nxm_typed_key", "nexum.example"));
    }

    #[test]
    fn back_from_a_bad_session_url_keeps_the_edits_for_a_skip() {
        let (mut s, _tmp) = session_with_bad_url();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_key = "nxm_typed_key".into();
        w.edit.skip_unsigned = false;
        w.back();
        assert_eq!(w.page(), WizardPage::Alliance);
        assert!(w.error.is_none());
        // Skip on the Alliance page drops only the edits of that page. The Nexum edits stay.
        assert_eq!(w.skip(), Flow::Open);
        assert_eq!(w.page(), WizardPage::Nexum);
        assert_eq!((w.kept.nexum_key.as_str(), w.kept.skip_unsigned), ("nxm_typed_key", false));
        for _ in 0..2 {
            w.skip();
        }
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.cfg.nexum.key, Some(ApiKey("nxm_typed_key".into())));
        assert!(s.cfg.nexum.keep_unsigned);
        assert_eq!(s.cfg.nexum.url.as_deref(), Some("nexum.example"), "Finish keeps the bad URL of the file");
    }

    /// True if a notice says that the Nexum settings were saved.
    fn nexum_saved_notice(s: &Session) -> bool {
        s.notices.iter().any(|n| n.text.starts_with("Nexum settings saved"))
    }

    #[test]
    fn a_final_slash_is_no_change_to_the_nexum_url() {
        use router_core::sources::nexum::MapInfo;
        let (mut s, _tmp) = session(true, true);
        s.cfg.nexum.url = Some("https://nexum.example".into());
        s.map_names = vec![MapInfo { id: "1".into(), name: "Home".into() }];
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_url = "https://nexum.example/".into();
        w.next();
        s.notices.clear();
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.cfg.nexum.url.as_deref(), Some("https://nexum.example"));
        assert!(!nexum_saved_notice(&s), "no change, so no save of Nexum and no new map");
        assert_eq!(s.map_names.len(), 1, "the map list is still good");
        // A real change gives the save. This shows that the notice check works.
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_url = "https://other.example/".into();
        w.next();
        w.finish(&mut s, &egui::Context::default());
        assert!(nexum_saved_notice(&s));
    }

    #[test]
    fn a_changed_url_or_key_clears_the_map_list() {
        use router_core::sources::nexum::MapInfo;
        let maps = || vec![MapInfo { id: "1".into(), name: "Home".into() }];
        // A new URL.
        let (mut s, _tmp) = session(true, true);
        s.map_names = maps();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_url = "https://nexum.example".into();
        w.next();
        w.finish(&mut s, &egui::Context::default());
        assert!(s.map_names.is_empty());
        // A new key, with the same URL.
        s.map_names = maps();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_key = "nxm_new_key".into();
        w.next();
        w.finish(&mut s, &egui::Context::default());
        assert!(s.map_names.is_empty());
        // A change of only the signature switch leaves the list.
        s.map_names = maps();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.skip_unsigned = !w.edit.skip_unsigned;
        w.next();
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.map_names, maps());
    }

    /// The texts of a frame that draws `add`. The context is new, so every frame starts the same.
    fn texts(add: impl FnMut(&mut egui::Ui)) -> Vec<(String, egui::Pos2, egui::Vec2)> {
        let mut output = test_ui::ctx().run_ui(Default::default(), add);
        // The frame output holds texture changes. A test with no renderer drops them.
        output.textures_delta.clear();
        drawn_texts(&output)
    }

    /// True if one of the drawn texts is `text`.
    fn has(texts: &[(String, egui::Pos2, egui::Vec2)], text: &str) -> bool {
        texts.iter().any(|(t, ..)| t == text)
    }

    /// The pilot state of a draw test: the portraits of the characters `ids` need no download.
    // The fields of `PilotsUi` are private, so a struct update does not work here.
    #[allow(clippy::field_reassign_with_default)]
    fn pilots_ui_for(ids: &[u64]) -> PilotsUi {
        let mut pilots = PilotsUi::default();
        pilots.portraits = Portraits::without_download(ids);
        pilots
    }

    /// The pilot state of a draw test with the characters 1 and 2.
    fn pilots_ui() -> PilotsUi {
        pilots_ui_for(&[1, 2])
    }

    #[test]
    fn each_page_draws_with_and_without_a_client_id() {
        let ctx = test_ui::ctx();
        for client_id in [true, false] {
            let (mut s, tmp) = session(client_id, true);
            let mut pilots = pilots_ui();
            let mut w = Wizard::new(&s);
            loop {
                let mut output = ctx.run_ui(Default::default(), |ui| w.body(ui, &mut s, &mut pilots));
                output.textures_delta.clear();
                let drawn = drawn_texts(&output);
                if w.page() == WizardPage::Essentials {
                    assert_eq!(has(&drawn, "+ Add character"), client_id);
                }
                if w.next() == Flow::Done {
                    break;
                }
            }
            // Drawing writes no file.
            assert!(tmp.path().read_dir().unwrap().next().is_none());
        }
    }

    #[test]
    fn the_keyring_warning_shows_on_pages_1_and_3_only() {
        for keyring_missing in [true, false] {
            let (mut s, _tmp) = session(true, true);
            s.keyring_missing = keyring_missing;
            let mut pilots = pilots_ui();
            let mut w = Wizard::new(&s);
            loop {
                let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
                let page = w.page();
                let on_page = matches!(page, WizardPage::Essentials | WizardPage::Nexum);
                assert_eq!(has(&drawn, KEYRING_WARNING), keyring_missing && on_page, "{page:?}, keyring_missing {keyring_missing}");
                if w.next() == Flow::Done {
                    break;
                }
            }
        }
    }

    /// The room above the page in the smallest window: the margin, the steps row with its strip, and the gap.
    const ROOM_ABOVE: f32 = 64.0;

    /// A session with a client ID and the characters `ids`, named "Pilot <id>", and the pilot
    /// state with their portraits.
    fn session_with_characters(ids: &[u64]) -> (Session, tempfile::TempDir, PilotsUi) {
        let names: Vec<String> = ids.iter().map(|id| format!("Pilot {id}")).collect();
        let list: Vec<(u64, &str)> = ids.iter().copied().zip(names.iter().map(String::as_str)).collect();
        let (mut s, tmp) = session(true, true);
        s.pilots = Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &list);
        (s, tmp, pilots_ui_for(ids))
    }

    /// The area of the page in a window of `size`: the frame of the app takes `ROOM_ABOVE` above it,
    /// `FRAME_ROOM - ROOM_ABOVE` below it, and 32 points at each side.
    fn page_area(size: egui::Vec2) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(32.0, ROOM_ABOVE), egui::pos2(size.x - 32.0, size.y - (FRAME_ROOM - ROOM_ABOVE)))
    }

    /// Draw the current page of `w` in the page area of a window of `size`. The cards read their
    /// natural heights in one frame and use them in the next frame, so the context draws three frames.
    fn page_frame(w: &mut Wizard, s: &mut Session, pilots: &mut PilotsUi, size: egui::Vec2) -> egui::FullOutput {
        let ctx = test_ui::ctx();
        let area = page_area(size);
        let mut last = None;
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| w.body(ui, s, pilots));
            });
            output.textures_delta.clear();
            last = Some(output);
        }
        last.expect("three frames ran")
    }

    #[test]
    fn the_characters_table_keeps_its_height() {
        // The place of the "+ Add character" button with these characters, and the names that draw.
        let measure = |count: u64| {
            let ids: Vec<u64> = (1..=count).collect();
            let (mut s, _tmp, mut pilots) = session_with_characters(&ids);
            let mut w = Wizard::new(&s);
            let drawn = drawn_texts(&page_frame(&mut w, &mut s, &mut pilots, SMALLEST_WINDOW));
            let names = drawn.iter().filter(|(t, ..)| t.starts_with("Pilot ")).count();
            (position(&drawn, "+ Add character"), names)
        };
        let (one, one_names) = measure(1);
        let (three, three_names) = measure(CHARACTER_ROWS_SHOWN as u64);
        let (many, many_names) = measure(100);
        // The table has a row for each character up to the limit, then it scrolls inside.
        assert_eq!((one_names, three_names), (1, CHARACTER_ROWS_SHOWN));
        assert!(one.y < three.y, "one row is shorter than {CHARACTER_ROWS_SHOWN} rows");
        assert_eq!(three, many, "100 characters take the room of {CHARACTER_ROWS_SHOWN}");
        assert!(many_names <= CHARACTER_ROWS_SHOWN + 1, "{many_names} names draw: only the rows in view draw");
    }

    #[test]
    fn the_characters_card_draws_a_hint_and_one_status_for_each_row() {
        use router_core::esi::pilots::PilotState;
        let (mut s, _tmp, mut pilots) = session_with_characters(&[1, 2, 3]);
        // Pilot 1 is online, pilot 2 is offline, and the state of pilot 3 is not known.
        s.pilots.live.insert(1, PilotState { online: Some(true), ..Default::default() });
        s.pilots.live.insert(2, PilotState { online: Some(false), ..Default::default() });
        let mut w = Wizard::new(&s);
        let output = page_frame(&mut w, &mut s, &mut pilots, SMALLEST_WINDOW);
        let drawn = drawn_texts(&output);
        assert!(has(&drawn, "Log in each pilot that you fly."));
        assert!(has(&drawn, "CHARACTER") && has(&drawn, "STATUS"), "the header of the table");
        // One status for each pilot, on the line of the name. No dot repeats it. The state of
        // pilot 3 is not known, and the table shows it as offline.
        for (index, status) in ["Online", "Offline", "Offline"].into_iter().enumerate() {
            let name = position(&drawn, &format!("Pilot {}", index + 1));
            let on_line = |(text, pos, _): &(String, egui::Pos2, egui::Vec2)| text == status && (pos.y - name.y).abs() <= 2.0;
            assert!(drawn.iter().any(on_line), "row {index}: the status {status} is on the line of the name");
        }
        let circles = test_ui::drawn_shapes(&output).into_iter().filter(|s| matches!(s, egui::epaint::Shape::Circle(_))).count();
        assert_eq!(circles, 0, "no status dot");
    }

    #[test]
    fn pages_2_to_4_draw_stacked_cards_with_their_headings() {
        let (mut s, _tmp, mut pilots) = session_with_characters(&[1, 2]);
        let mut w = Wizard::new(&s);
        // Each page: the texts from the top to the bottom of the card.
        let pages: [(WizardPage, &[&str]); 3] = [
            (WizardPage::Alliance, &["Ansiblex", "ALLIANCE CAPITAL", "System", "IMPORT", "Import"]),
            (WizardPage::Nexum, &["Connection", "URL", "Key", "Signatures"]),
            (
                WizardPage::EveMetro,
                &["EVE Metro", "Third-party map of Pochven wormholes. A subscription is required.", "Enable", "CHARACTER", "Use"],
            ),
        ];
        let area = page_area(SMALLEST_WINDOW);
        for (page, order) in pages {
            assert_eq!(w.next(), Flow::Open);
            assert_eq!(w.page(), page);
            let output = page_frame(&mut w, &mut s, &mut pilots, SMALLEST_WINDOW);
            let drawn = drawn_texts(&output);
            let ys: Vec<f32> = order.iter().map(|text| position(&drawn, text).y).collect();
            assert!(ys.windows(2).all(|pair| pair[0] < pair[1]), "{page:?}: the texts {order:?} come in order, at {ys:?}");
            assert!(!drawn.iter().any(|(t, ..)| t.contains("Save")), "{page:?}: no Save button");
            let cards = test_ui::card_rects(&output);
            // The cards are stacked in the one column of the page: centered, under the head.
            assert_eq!(cards.len(), if page == WizardPage::EveMetro { 1 } else { 2 }, "{page:?}: the number of cards");
            for card in &cards {
                assert!((card.width() - PAGE_COLUMN).abs() <= 1.0, "{page:?}: a card is {} points wide", card.width());
                assert!((card.center().x - SMALLEST_WINDOW.x / 2.0).abs() <= 1.0, "{page:?}: a card is centered at {}", card.center().x);
            }
            assert!((cards[0].top() - area.top() - HEAD_ROOM).abs() <= 1.0, "{page:?}: the card starts at {}", cards[0].top());
            if let Some(second) = cards.get(1) {
                assert!((second.top() - cards[0].bottom() - CARD_GAP).abs() <= 1.0, "{page:?}: the gap is {}", second.top() - cards[0].bottom());
            }
            match page {
                WizardPage::Alliance => assert!(has(&drawn, "Bridges to add (0)")),
                WizardPage::Nexum => assert!(has(&drawn, "Set the URL and the key to list the maps.") || has(&drawn, "Load maps")),
                _ => {}
            }
            // The Metro page has the heading and the header of the table.
            if page == WizardPage::EveMetro {
                assert_eq!(drawn.iter().filter(|(t, ..)| t == "CHARACTER").count(), 2);
            }
        }
    }

    #[test]
    fn the_first_page_without_a_client_id_is_one_centered_column() {
        let (mut s, _tmp) = session(false, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        for size in [SMALLEST_WINDOW, egui::vec2(1500.0, 900.0)] {
            let output = page_frame(&mut w, &mut s, &mut pilots, size);
            assert!(!has(&drawn_texts(&output), "+ Add character"));
            let cards = test_ui::card_rects(&output);
            assert_eq!(cards.len(), 3, "Updates, Theme and Features");
            let (updates, theme, features) = (cards[0], cards[1], cards[2]);
            // Features has the width of the column. Updates and Theme share the row over it.
            assert!((features.width() - PAGE_COLUMN).abs() <= 1.0, "the column is {} points wide", features.width());
            assert!((features.center().x - size.x / 2.0).abs() <= 1.0, "the column is centered at {}", features.center().x);
            assert!((updates.left() - features.left()).abs() <= 1.0 && (theme.right() - features.right()).abs() <= 1.0);
            // The cards start under the head of the page, and keep their own height.
            let area = page_area(size);
            assert!((updates.top() - area.top() - HEAD_ROOM).abs() <= 1.0, "the cards start at {}", updates.top());
            assert!((features.top() - updates.bottom() - CARD_GAP).abs() <= 1.0);
        }
    }

    /// Draw the footer in a frame that has the pointer on the button `label`, press and release
    /// the primary button there, and return what the footer gives. A first frame lays the footer out.
    fn click_footer(w: &mut Wizard, label: &str) -> Flow {
        let ctx = test_ui::ctx();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
        let mut run = |events: Vec<egui::Event>, time: f64| {
            let input = egui::RawInput { time: Some(time), screen_rect: Some(screen), events, ..Default::default() };
            let mut flow = Flow::Open;
            let mut output = ctx.run_ui(input, |ui| flow = w.footer(ui));
            output.textures_delta.clear();
            (flow, drawn_texts(&output))
        };
        let (_, laid_out) = run(Vec::new(), 0.0);
        let (_, pos, size) = laid_out.iter().find(|(t, ..)| t == label).unwrap_or_else(|| panic!("no {label} button")).clone();
        let at = pos + size / 2.0;
        let button =
            |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run(vec![egui::Event::PointerMoved(at)], 0.1);
        run(vec![button(true)], 0.2);
        run(vec![button(false)], 0.3).0
    }

    #[test]
    fn back_on_the_first_page_asks_for_the_welcome_screen() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        w.edit.theme = FactionTheme::Amarr;
        w.edit.check_for_updates = false;
        w.error = Some("old".into());
        w.capital.set_query("UAL");
        let (kept, edit) = (w.kept.clone(), w.edit.clone());
        assert_eq!(click_footer(&mut w, "Back"), Flow::Welcome);
        assert_eq!((w.page(), &w.kept, &w.edit), (WizardPage::Essentials, &kept, &edit), "the footer changes no state");
        assert_eq!((w.error.as_deref(), w.capital.query()), (Some("old"), "UAL"), "the footer changes no state");
        // The model rule stays: Back on the first page does nothing.
        w.back();
        assert_eq!((w.page(), &w.kept, &w.edit), (WizardPage::Essentials, &kept, &edit));
        assert_eq!((w.error.as_deref(), w.capital.query()), (Some("old"), "UAL"));
        // The helper reaches the other buttons, so the click above is a real click.
        assert_eq!(click_footer(&mut w, "Next"), Flow::Open);
        assert_eq!(w.page(), WizardPage::Alliance);
        // Back on a later page moves back and keeps the wizard open.
        assert_eq!(click_footer(&mut w, "Back"), Flow::Open);
        assert_eq!(w.page(), WizardPage::Essentials);
    }

    #[test]
    fn with_preset_builds_the_pages_and_the_toggles() {
        let (s, _tmp) = session(true, true);
        for preset in Preset::ALL {
            let answers = preset.answers();
            let w = Wizard::with_preset(&s, preset, answers);
            let expected = pages_for(preset, answers, s.pilots.shows_characters(), eve_metro_available(&s));
            assert_eq!(w.pages, expected, "{preset:?}");
            assert_eq!(w.edit.features, answers.features(), "{preset:?}");
            assert_eq!(w.kept, w.edit, "{preset:?}");
            assert!(!w.edit.features_kept && !w.kept.features_kept, "{preset:?}");
            assert_eq!(w.page(), WizardPage::Essentials);
        }
        // A spot check of the table through the wizard.
        let highsec = Wizard::with_preset(&s, Preset::Highsec, Preset::Highsec.answers());
        assert_eq!(highsec.pages, [WizardPage::Essentials]);
        assert_eq!(highsec.edit.features, Features { filaments: false, classes: [false; 6], pochven: false, thera: false, turnur: false });
        // Wizard::new is the Custom preset.
        let custom = Wizard::with_preset(&s, Preset::Custom, Preset::Custom.answers());
        let new = Wizard::new(&s);
        assert_eq!((new.pages, new.edit), (custom.pages, custom.edit));
    }

    #[test]
    fn a_subscription_answer_checks_enable_when_a_character_is_linked() {
        let subscribed = Answers { subscription: true, ..Preset::Lowsec.answers() };
        // A character is linked (Alice and Bo).
        let (s, _tmp) = session(true, true);
        assert!(!s.cfg.eve_metro.enabled);
        assert!(Wizard::with_preset(&s, Preset::Lowsec, subscribed).edit.metro_enabled);
        assert!(!Wizard::with_preset(&s, Preset::Lowsec, Preset::Lowsec.answers()).edit.metro_enabled, "no subscription answer");
        // No character: the box starts unchecked.
        let (s, _tmp) = session(false, true);
        assert!(s.pilots.characters().is_empty());
        assert!(!Wizard::with_preset(&s, Preset::Lowsec, subscribed).edit.metro_enabled);
        // The page is hidden without the key. A hidden page changes nothing.
        let (s, _tmp) = session(true, false);
        assert!(!Wizard::with_preset(&s, Preset::Lowsec, subscribed).edit.metro_enabled);
        // Custom never changes the value of the session.
        for enabled in [false, true] {
            let (mut s, _tmp) = session(true, true);
            s.cfg.eve_metro.enabled = enabled;
            let w = Wizard::with_preset(&s, Preset::Custom, Preset::Custom.answers());
            assert_eq!((w.edit.metro_enabled, w.kept.metro_enabled), (enabled, enabled));
        }
    }

    /// A Lowsec wizard with the subscription answer, on the EVE Metro page. A character is linked
    /// and the session has a Metro key, so the Enable box starts checked.
    fn on_the_metro_page(s: &Session) -> Wizard {
        let subscribed = Answers { subscription: true, ..Preset::Lowsec.answers() };
        let mut w = Wizard::with_preset(s, Preset::Lowsec, subscribed);
        assert!(w.edit.metro_enabled, "the pre-check");
        while w.page() != WizardPage::EveMetro {
            assert_eq!(w.next(), Flow::Open);
        }
        w
    }

    #[test]
    fn the_metro_page_warns_while_enable_is_on_and_no_character_is_picked() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = on_the_metro_page(&s);
        assert_eq!(w.edit.metro_character, None, "the pre-check picks no character");
        let ctx = test_ui::ctx();
        let mut frame = |w: &mut Wizard, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| w.body(ui, &mut s, &mut pilots));
            output.textures_delta.clear();
            drawn_texts(&output)
        };
        frame(&mut w, vec![]);
        let drawn = frame(&mut w, vec![]);
        assert!(has(&drawn, METRO_NEEDS_CHARACTER), "Enable is on and no character is picked");
        // A real click on the "Use" button of Bo, the lower of the two.
        let mut uses: Vec<_> = drawn.iter().filter(|(t, ..)| t == "Use").collect();
        uses.sort_by(|a, b| a.1.y.partial_cmp(&b.1.y).unwrap());
        let (_, pos, size) = uses.get(1).expect("two \"Use\" buttons");
        let at = *pos + *size / 2.0;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut w, vec![egui::Event::PointerMoved(at)]);
        frame(&mut w, vec![button(true)]);
        frame(&mut w, vec![button(false)]);
        assert_eq!(w.edit.metro_character, Some(2), "the click picks Bo");
        assert!(!has(&frame(&mut w, vec![]), METRO_NEEDS_CHARACTER), "a character is picked");
        // With Enable off and no character, the page shows no warning.
        w.edit.metro_enabled = false;
        w.edit.metro_character = None;
        assert!(!has(&frame(&mut w, vec![]), METRO_NEEDS_CHARACTER), "Enable is off");
    }

    #[test]
    fn skip_on_the_metro_page_keeps_the_session_value_of_enable() {
        let (mut s, _tmp) = session(true, true);
        assert!(!s.cfg.eve_metro.enabled);
        let mut w = on_the_metro_page(&s);
        assert!(w.kept.metro_enabled, "Next on the earlier pages copies the pre-check into kept");
        assert_eq!(w.skip(), Flow::Done);
        assert!(!w.kept.metro_enabled && !w.edit.metro_enabled);
        w.finish(&mut s, &egui::Context::default());
        assert!(!s.cfg.eve_metro.enabled, "a skipped page changes no setting");

        // The session value is true: Skip keeps it.
        let (mut s, _tmp) = session(true, true);
        s.cfg.eve_metro.enabled = true;
        let mut w = on_the_metro_page(&s);
        w.edit.metro_enabled = false;
        assert_eq!(w.skip(), Flow::Done);
        w.finish(&mut s, &egui::Context::default());
        assert!(s.cfg.eve_metro.enabled);
    }

    #[test]
    fn next_on_the_metro_page_keeps_the_box_as_the_pilot_left_it() {
        // The pilot accepts the pre-check.
        let (mut s, _tmp) = session(true, true);
        let mut w = on_the_metro_page(&s);
        assert_eq!(w.next(), Flow::Done);
        w.finish(&mut s, &egui::Context::default());
        assert!(s.cfg.eve_metro.enabled, "Finish writes the pre-check");

        // The pilot clears the box.
        let (mut s, _tmp) = session(true, true);
        let mut w = on_the_metro_page(&s);
        w.edit.metro_enabled = false;
        assert_eq!(w.next(), Flow::Done);
        w.finish(&mut s, &egui::Context::default());
        assert!(!s.cfg.eve_metro.enabled, "Finish writes the cleared box");
    }

    /// Skip on every page that is left. The last Skip gives `Flow::Done`.
    fn skip_all(w: &mut Wizard) {
        while w.skip() == Flow::Open {}
    }

    #[test]
    fn skip_on_essentials_accepts_the_starting_values() {
        let (mut s, tmp) = session(true, true);
        s.settings.space.drifter = Stance::Avoid;
        let mut w = Wizard::with_preset(&s, Preset::Highsec, Preset::Highsec.answers());
        assert!(!w.kept.features_kept);
        assert_eq!(w.skip(), Flow::Done, "the highsec wizard has one page");
        assert!(w.kept.features_kept);
        w.finish(&mut s, &egui::Context::default());
        let space = s.settings.space;
        assert_eq!(space.pochven, Stance::Block);
        assert_eq!([space.c1, space.c2, space.c3, space.c4, space.c5, space.c6], [Stance::Block; 6]);
        assert_eq!(space.drifter, Stance::Avoid, "Drifter is not a switch of the page");
        assert_eq!(s.settings.filaments, Stance::Block, "no Pochven answer, no filaments");
        assert!(tmp.path().join("eve-router.json").exists());

        // A wizard with more pages: Skip on page 1 keeps the toggles.
        let (mut s, _tmp) = session(true, true);
        s.settings.space.pochven = Stance::Block;
        let mut w = Wizard::with_preset(&s, Preset::Lowsec, Preset::Lowsec.answers());
        assert_eq!(w.skip(), Flow::Open);
        assert!(w.kept.features_kept);
        assert_eq!(w.page(), WizardPage::Nexum);
        skip_all(&mut w);
        w.finish(&mut s, &egui::Context::default());
        assert_eq!((s.settings.space.pochven, s.settings.space.c1), (Stance::Allow, Stance::Allow));

        // Custom: Skip then Finish writes Allow over an Avoid stance, as Next does.
        let (mut s, _tmp) = session(true, true);
        s.settings.filaments = Stance::Avoid;
        let mut w = Wizard::new(&s);
        assert_eq!(w.skip(), Flow::Open);
        skip_all(&mut w);
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.settings.filaments, Stance::Allow);
    }

    #[test]
    fn skip_on_essentials_keeps_all_the_edits_of_the_page() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::with_preset(&s, Preset::Highsec, Preset::Highsec.answers());
        w.edit.theme = FactionTheme::Amarr;
        w.edit.check_for_updates = false;
        w.capital.set_query("UAL");
        w.error = Some("old".into());
        w.skip();
        assert_eq!((w.kept.theme, w.kept.check_for_updates), (FactionTheme::Amarr, false));
        assert_eq!(w.edit, w.kept);
        assert!(w.error.is_none());
        assert_eq!(w.capital.query(), "", "the capital search box resets");
    }

    #[test]
    fn hidden_pages_leave_their_settings_alone() {
        let (mut s, _tmp) = session(true, true);
        let capital = s.uni.exact("UALX-3");
        s.set_capital(capital);
        s.import_bridges("UALX-3 --> IL-YTR", false);
        s.cfg.nexum.url = Some("https://nexum.example".into());
        s.cfg.eve_metro.enabled = true;
        s.cfg.eve_metro.character = Some(2);
        let values = |s: &Session| (s.settings.rules.capital, s.bridges.len(), s.cfg.nexum.url.clone(), s.cfg.eve_metro.clone());
        let before = values(&s);
        assert!(capital.is_some() && before.1 == 1, "the setup of the test took");
        // Highsec shows the Essentials page only.
        let mut w = Wizard::with_preset(&s, Preset::Highsec, Preset::Highsec.answers());
        assert_eq!(w.skip(), Flow::Done);
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(values(&s), before);
        // Lowsec hides the Alliance page and, with no subscription, the EVE Metro page. The Nexum
        // page shows, so this part also tests Skip on the Nexum page.
        let mut w = Wizard::with_preset(&s, Preset::Lowsec, Preset::Lowsec.answers());
        skip_all(&mut w);
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(values(&s), before);
    }

    #[test]
    fn the_footer_reads_finish_on_the_last_page_and_back_shows_on_every_page() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        let first = texts(|ui| _ = w.footer(ui));
        assert!(has(&first, "Back") && has(&first, "Skip") && has(&first, "Next"));
        assert!(!has(&first, "Finish"));
        // The model rule: Back does nothing on the first page.
        w.back();
        assert_eq!(w.page(), WizardPage::Essentials);
        while w.page() != WizardPage::EveMetro {
            assert_eq!(w.next(), Flow::Open);
        }
        let last = texts(|ui| _ = w.footer(ui));
        assert!(has(&last, "Finish") && has(&last, "Back") && has(&last, "Skip"));
        assert!(!has(&last, "Next"));
    }

    #[test]
    fn the_nexum_page_warns_about_a_remote_http_url() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        let clear_text =
            |drawn: &[(String, egui::Pos2, egui::Vec2)]| drawn.iter().any(|(t, ..)| t.contains("crosses the network in clear text"));
        assert!(!clear_text(&texts(|ui| w.body(ui, &mut s, &mut pilots))));
        w.edit.nexum_url = "http://203.0.113.5".into();
        assert!(clear_text(&texts(|ui| w.body(ui, &mut s, &mut pilots))));
    }

    #[test]
    fn the_nexum_page_shows_the_error_and_the_key_hint() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.error = Some("The URL is bad.".into());
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "The URL is bad."));
        assert!(has(&drawn, "Connection") && has(&drawn, "Maps"));
        // A field hint is a text edit hint, so it shows as a drawn text.
        assert!(has(&drawn, "paste a key with the read scope"));
        // With a key in the session, the hint says that the key stays.
        s.cfg.nexum.key = Some(ApiKey("nxm_old_key".into()));
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "A key is set. An empty field keeps it."));
        assert!(!has(&drawn, "paste a key with the read scope"));
    }

    #[test]
    fn the_alliance_page_says_when_the_list_holds_the_bridges_already() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        w.edit.bridge_text = "UALX-3 --> IL-YTR".into();
        w.import(&s);
        while w.next() == Flow::Open {}
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.bridges.len(), 1);
        // The same list again: the page reads it, and Finish has nothing to add.
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        w.edit.bridge_text = "UALX-3 --> IL-YTR".into();
        w.import(&s);
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "The 1 bridge is in your list already. Finish adds nothing."));
        assert!(has(&drawn, "Bridges to add (0)"));
    }

    #[test]
    fn the_alliance_and_metro_pages_draw_their_states() {
        let (mut s, tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        // A capital shows its name with Clear. A good import fills the table of the bridges to add.
        w.edit.capital = s.uni.exact("UALX-3");
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "Clear") && has(&drawn, "Import"));
        w.edit.bridge_text = "UALX-3 --> IL-YTR".into();
        w.import(&s);
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "Bridges to add (1)"));
        // A bad import shows its problem lines and no bridge to add.
        w.edit.bridge_text = "Nope --> Jita".into();
        w.import(&s);
        assert!(!w.edit.bridge_problems.is_empty());
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(w.edit.bridge_problems.iter().all(|p| has(&drawn, p)));
        assert!(has(&drawn, "Bridges to add (0)"));
        // The EVE Metro page: the picked character reads "In use", the other one has a "Use" button.
        while w.page() != WizardPage::EveMetro {
            assert_eq!(w.next(), Flow::Open);
        }
        w.edit.metro_character = Some(2);
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "In use") && has(&drawn, "Use") && has(&drawn, "STATUS"));
        // With no linked character, the page says so.
        s.pilots = Pilots::offline_with_client_id(tmp.path().join("active-route.json"), &[]);
        let drawn = texts(|ui| w.body(ui, &mut s, &mut pilots));
        assert!(has(&drawn, "No linked character. Add one on the Essentials page."));
    }

    #[test]
    fn the_body_previews_the_edited_theme() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        let ctx = test_ui::ctx();
        w.edit.theme = FactionTheme::Gallente;
        // A new style reaches the widgets in the next frame.
        let mut accent = None;
        for _ in 0..2 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                w.body(ui, &mut s, &mut pilots);
                accent = Some(crate::theme::accent(ui));
            });
            output.textures_delta.clear();
        }
        assert_eq!(accent, Some(FactionTheme::Gallente.accent()));
        // The preview is not a saved choice.
        assert!(s.cfg.theme.is_none());
    }

    /// One frame of the page with these input events. Return the accent of the style that the
    /// frame starts with, the drawn texts, and the delay until the frame that egui asks for.
    fn frame(
        ctx: &egui::Context,
        w: &mut Wizard,
        s: &mut Session,
        pilots: &mut PilotsUi,
        events: Vec<egui::Event>,
    ) -> (egui::Color32, Vec<(String, egui::Pos2, egui::Vec2)>, std::time::Duration) {
        let mut accent = None;
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            accent = Some(theme::accent(ui));
            w.body(ui, s, pilots);
        });
        output.textures_delta.clear();
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        (accent.expect("the frame ran"), drawn_texts(&output), delay)
    }

    #[test]
    fn a_theme_click_shows_in_the_next_frame() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        let ctx = test_ui::ctx();
        let old = FactionTheme::default().accent();
        assert_ne!(old, FactionTheme::Gallente.accent());
        // Two frames, so the click finds the button of the frame before.
        frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        let (_, drawn, _) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        let (_, pos, size) = drawn.iter().find(|(t, ..)| t == "Gallente").expect("the Gallente button is drawn");
        let at = *pos + *size / 2.0;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut w, &mut s, &mut pilots, vec![egui::Event::PointerMoved(at), button(true)]);
        let (accent, ..) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![button(false)]);
        assert_eq!(w.theme(), FactionTheme::Gallente, "the click picked the theme");
        assert_eq!(accent, old, "the click frame started with the old style");
        // The frame after the click has no input, and it starts with the new style.
        let (accent, ..) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        assert_eq!(accent, FactionTheme::Gallente.accent());
    }

    #[test]
    fn a_theme_change_asks_for_one_more_frame() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        let ctx = test_ui::ctx();
        // The first frames of a new context ask for more frames. After them, the page is still.
        for _ in 0..3 {
            frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        }
        let (.., delay) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        assert_eq!(delay, std::time::Duration::MAX, "a still page asks for no frame");
        // A change with no input event, as from Skip: the frame asks for the next frame at once.
        w.edit.theme = FactionTheme::Gallente;
        let (.., delay) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        assert_eq!(delay, std::time::Duration::ZERO);
        // That frame shows the new theme.
        let (accent, ..) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        assert_eq!(accent, FactionTheme::Gallente.accent());
        // egui gives two frames for each request. After them, the page is still again.
        let (.., delay) = frame(&ctx, &mut w, &mut s, &mut pilots, vec![]);
        assert_eq!(delay, std::time::Duration::MAX, "the preview asks for no endless frames");
    }

    /// The Alliance page in the smallest window after an import of `bad` lines that give no bridge.
    /// The list also holds one good line, so the page shows one bridge to add.
    fn alliance_with_problems(bad: usize) -> Vec<(String, egui::Pos2, egui::Vec2)> {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        assert_eq!(w.page(), WizardPage::Alliance);
        w.edit.bridge_text = "UALX-3 --> IL-YTR\nBad --> IL-YTR".into();
        w.import(&s);
        // An import gives one problem for each bad line. The test sets the problems directly, so one short list gives any count.
        w.edit.bridge_problems = (1..=bad).map(|i| format!("Unknown system \"Bad{i}\"")).collect();
        drawn_texts(&page_frame(&mut w, &mut s, &mut pilots, SMALLEST_WINDOW))
    }

    /// The position of the drawn text that starts with `start`.
    fn position(drawn: &[(String, egui::Pos2, egui::Vec2)], start: &str) -> egui::Pos2 {
        drawn.iter().find(|(t, ..)| t.starts_with(start)).map(|(_, pos, _)| *pos).unwrap_or_else(|| panic!("{start} is drawn"))
    }

    #[test]
    fn many_problem_lines_do_not_move_the_import_button_past_the_limit() {
        // The page shows `PROBLEM_LINES` lines and one count line. More problems add no line.
        let limit = alliance_with_problems(PROBLEM_LINES + 1);
        let many = alliance_with_problems(100);
        assert_eq!(position(&limit, "Import"), position(&many, "Import"));
        assert_eq!(position(&limit, "Bridges to add"), position(&many, "Bridges to add"));
    }

    #[test]
    fn the_problem_lines_come_before_the_import_button() {
        let drawn = alliance_with_problems(3);
        let first_problem = position(&drawn, "Unknown system").y;
        assert!(first_problem < position(&drawn, "Import").y);
        // The bridge table stands in the card under the Import card.
        assert!(has(&drawn, "Bridges to add (1)"));
    }

    #[test]
    fn the_alliance_page_shows_five_problem_lines_and_counts_the_others() {
        let problem_lines =
            |drawn: &[(String, egui::Pos2, egui::Vec2)]| drawn.iter().filter(|(t, ..)| t.starts_with("Unknown system")).count();
        let more_line = |drawn: &[(String, egui::Pos2, egui::Vec2)]| drawn.iter().any(|(t, ..)| t.ends_with(" more."));
        let drawn = alliance_with_problems(100);
        assert_eq!(problem_lines(&drawn), PROBLEM_LINES);
        assert!(has(&drawn, "and 95 more."));
        assert!(position(&drawn, "Unknown system").y < position(&drawn, "and 95 more.").y);
        for bad in [1, PROBLEM_LINES] {
            let drawn = alliance_with_problems(bad);
            assert_eq!(problem_lines(&drawn), bad);
            assert!(!more_line(&drawn), "{bad} problems need no count line");
        }
        // One line more than the cap gives a count of 1.
        assert!(has(&alliance_with_problems(PROBLEM_LINES + 1), "and 1 more."));
    }

    /// Draw the Alliance page with `text` in the paste box, in the smallest window. With `import`,
    /// push Import first.
    fn alliance_in_smallest_window(text: &str, import: bool) -> (Vec<(String, egui::Pos2, egui::Vec2)>, Wizard) {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        assert_eq!(w.next(), Flow::Open);
        w.edit.bridge_text = text.into();
        if import {
            w.import(&s);
        }
        let drawn = drawn_texts(&page_frame(&mut w, &mut s, &mut pilots, SMALLEST_WINDOW));
        (drawn, w)
    }

    /// The bottom edge of the drawn text that starts with `start`.
    fn bottom(drawn: &[(String, egui::Pos2, egui::Vec2)], start: &str) -> f32 {
        drawn.iter().find(|(t, ..)| t.starts_with(start)).map(|(_, pos, size)| pos.y + size.y).unwrap_or_else(|| panic!("{start} is drawn"))
    }

    /// A list of `n` lines of bridge text.
    fn bridge_lines(n: usize, from: &str) -> String {
        (1..=n).map(|i| format!("{i} {from} --> IL-YTR")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_long_paste_does_not_push_import_off_the_screen() {
        let (one, _) = alliance_in_smallest_window(&bridge_lines(1, "UALX-3"), false);
        let (many, _) = alliance_in_smallest_window(&bridge_lines(100, "UALX-3"), false);
        assert_eq!(position(&one, "Import"), position(&many, "Import"), "the paste box keeps its height");
        assert!(bottom(&many, "Import") <= page_area(SMALLEST_WINDOW).bottom(), "Import is on the screen");
    }

    #[test]
    fn a_long_bad_paste_keeps_import_and_the_count_on_the_screen() {
        // Each line has a name that is not a system, so each line gives one problem.
        let (drawn, w) = alliance_in_smallest_window(&bridge_lines(100, "Bad"), true);
        assert_eq!(w.edit.bridge_problems.len(), 100);
        let limit = page_area(SMALLEST_WINDOW).bottom();
        assert!(bottom(&drawn, "Import") <= limit, "Import is on the screen");
        assert!(has(&drawn, "and 95 more."));
        assert!(bottom(&drawn, "and 95 more.") <= limit, "the count line is on the screen");
    }

    #[test]
    fn the_wizard_takes_the_answer_of_the_map_fetch() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        let map = |id: &str, name: &str| MapInfo { id: id.into(), name: name.into() };
        // No answer yet: the fetch stays.
        let (tx, rx) = mpsc::channel();
        w.maps.rx = Some(rx);
        w.poll_maps();
        assert!(w.maps.rx.is_some(), "the fetch runs until the answer comes");
        // The list comes.
        tx.send(Ok(vec![map("m1", "Alpha")])).expect("the wizard holds the receiver");
        w.poll_maps();
        assert!(w.maps.rx.is_none());
        assert_eq!(w.maps.list, [map("m1", "Alpha")]);
        // An error shows its text.
        let (tx, rx) = mpsc::channel();
        w.maps.rx = Some(rx);
        tx.send(Err(sources::FetchError::Offline("Nexum is down".into()))).expect("the wizard holds the receiver");
        w.poll_maps();
        assert!(w.maps.error.as_deref().is_some_and(|e| e.contains("Nexum is down")), "{:?}", w.maps.error);
        // A thread that ends with no answer gives an error too.
        let (tx, rx) = mpsc::channel::<Result<Vec<MapInfo>, sources::FetchError>>();
        drop(tx);
        w.maps.rx = Some(rx);
        w.poll_maps();
        assert_eq!(w.maps.error.as_deref(), Some("The map list did not arrive."));
    }

    #[test]
    fn the_maps_card_draws_its_states() {
        let (mut s, _tmp) = session(true, true);
        let mut pilots = pilots_ui();
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        let map = |id: &str, name: &str| MapInfo { id: id.into(), name: name.into() };
        let mut drawn = |w: &mut Wizard, s: &mut Session| drawn_texts(&page_frame(w, s, &mut pilots, SMALLEST_WINDOW));
        // With no URL and no key, the card asks for them.
        s.cfg.nexum.url = None;
        s.cfg.nexum.key = None;
        w.edit.nexum_url.clear();
        assert!(has(&drawn(&mut w, &mut s), "Set the URL and the key to list the maps."));
        // With both, it shows the button and the hint.
        w.edit.nexum_url = "https://nexum.example".into();
        w.edit.nexum_key = "nxm_key".into();
        let shown = drawn(&mut w, &mut s);
        assert!(has(&shown, "Load maps") && has(&shown, "Load the maps, then switch on the ones that the router reads."));
        // After a fetch, it lists the maps. No map is on.
        w.maps.list = vec![map("m1", "Alpha"), map("m2", "Beta")];
        w.maps.tried = true;
        let shown = drawn(&mut w, &mut s);
        assert!(has(&shown, "Alpha") && has(&shown, "Beta") && has(&shown, "Reload maps"));
        assert!(has(&shown, "No map is on: the router reads no Nexum data."));
        // A map that is on removes the note. A map that is on but not in the list still shows.
        w.edit.nexum_maps = vec!["m1".into(), "gone".into()];
        let shown = drawn(&mut w, &mut s);
        assert!(!has(&shown, "No map is on: the router reads no Nexum data."));
        assert!(has(&shown, "Not in the list"));
    }

    #[test]
    fn finish_writes_the_map_switches() {
        let (mut s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        to_nexum(&mut w);
        w.edit.nexum_maps = vec!["m1".into(), "m2".into()];
        assert_eq!(w.next(), Flow::Open);
        skip_all(&mut w);
        w.finish(&mut s, &egui::Context::default());
        assert_eq!(s.cfg.nexum.map_ids, ["m1", "m2"]);
    }

    /// Draw the steps row in a central panel of the full screen. Return the frame, with the wizard at page `at`.
    fn steps_frame(metro: bool, at: usize) -> egui::FullOutput {
        let (s, _tmp) = session(metro, metro);
        let mut w = Wizard::new(&s);
        w.at = at;
        let ctx = test_ui::ctx();
        // The accessibility test reads the AccessKit nodes of the same frame.
        ctx.enable_accesskit();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SMALLEST_WINDOW);
        let frame = egui::Frame::new().inner_margin(egui::Margin::same(24));
        let run = || {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(frame).show(ui, |ui| w.steps(ui));
            });
            output.textures_delta.clear();
            output
        };
        // The first frame loads the fonts. The second frame has the real sizes.
        run();
        run()
    }

    /// The left edge, the right edge and the center x of each step label, in order.
    fn label_spans(output: &egui::FullOutput) -> Vec<(f32, f32, f32)> {
        drawn_texts(output).iter().map(|(_, pos, size)| (pos.x, pos.x + size.x, pos.x + size.x / 2.0)).collect()
    }

    #[test]
    fn the_step_labels_are_centered_in_the_screen() {
        for (metro, count) in [(false, 3), (true, 4)] {
            let spans = label_spans(&steps_frame(metro, 0));
            assert_eq!(spans.len(), count);
            let center = (spans[0].0 + spans[count - 1].1) / 2.0;
            assert!((center - SMALLEST_WINDOW.x / 2.0).abs() <= 1.0, "{count} pages: the row is centered at {center}");
        }
    }

    #[test]
    fn one_dot_marks_the_active_step_under_its_label() {
        for metro in [false, true] {
            for at in 0..(3 + usize::from(metro)) {
                let output = steps_frame(metro, at);
                let spans = label_spans(&output);
                let bottom = drawn_texts(&output)[0].1.y + drawn_texts(&output)[0].2.y;
                let circles: Vec<_> = test_ui::drawn_shapes(&output)
                    .into_iter()
                    .filter_map(|shape| if let egui::epaint::Shape::Circle(c) = shape { Some(c) } else { None })
                    .collect();
                assert_eq!(circles.len(), 1, "page {at}: one dot");
                assert!((circles[0].center.x - spans[at].2).abs() <= 1.0, "page {at}: the dot is under its label");
                assert!(circles[0].center.y > bottom, "page {at}: the dot is under the labels");
            }
        }
    }

    #[test]
    fn the_strip_spans_the_label_row() {
        for metro in [false, true] {
            let output = steps_frame(metro, 1);
            let spans = label_spans(&output);
            let strips: Vec<_> = test_ui::drawn_shapes(&output)
                .into_iter()
                .filter_map(|shape| if let egui::epaint::Shape::Rect(r) = shape { Some(r.rect) } else { None })
                .filter(|rect| (rect.height() - 2.0).abs() < 0.01)
                .collect();
            assert!(!strips.is_empty(), "the strip is drawn");
            let left = strips.iter().map(|r| r.left()).fold(f32::MAX, f32::min);
            let right = strips.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
            assert!((left - spans[0].0).abs() <= 1.0, "the strip starts at the first label");
            assert!((right - spans[spans.len() - 1].1).abs() <= 1.0, "the strip ends at the last label");
        }
    }

    #[test]
    fn each_step_label_is_in_the_accessibility_tree() {
        for metro in [false, true] {
            let output = steps_frame(metro, 0);
            let mut texts: Vec<String> = drawn_texts(&output).into_iter().map(|(text, _, _)| text).collect();
            let mut names: Vec<String> = output
                .platform_output
                .accesskit_update
                .expect("accesskit is on")
                .nodes
                .into_iter()
                .filter(|(_, node)| node.role() == egui::accesskit::Role::Label)
                .filter_map(|(_, node)| node.value().map(str::to_string))
                .collect();
            // The update holds the nodes in no set order, so compare the sorted names.
            texts.sort();
            names.sort();
            assert_eq!(texts.len(), 3 + usize::from(metro));
            assert_eq!(names, texts, "each painted step label has a label node with its name");
        }
    }

    /// What one frame of the steps row shows, for the slide tests.
    struct StepsShot {
        /// The center x of each label.
        centers: Vec<f32>,
        /// The center x of the dot.
        dot_x: f32,
        /// The right end of the accent part of the strip.
        accent_end: f32,
        /// The delay until the frame that egui asks for.
        delay: std::time::Duration,
    }

    /// Draw the steps row of `w` in one frame at the time `t`, in seconds.
    fn steps_at(w: &Wizard, ctx: &egui::Context, t: f64) -> StepsShot {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SMALLEST_WINDOW);
        let frame = egui::Frame::new().inner_margin(egui::Margin::same(24));
        let input = egui::RawInput { time: Some(t), screen_rect: Some(screen), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().frame(frame).show(ui, |ui| w.steps(ui));
        });
        output.textures_delta.clear();
        let shapes = test_ui::drawn_shapes(&output);
        let dot_x = shapes
            .iter()
            .find_map(|shape| if let egui::epaint::Shape::Circle(c) = shape { Some(c.center.x) } else { None })
            .expect("the dot is drawn");
        // The accent part is the 2 point rect that is not the dim strip. At page 0 it can have no width.
        let accent_end = shapes
            .iter()
            .filter_map(|shape| if let egui::epaint::Shape::Rect(r) = shape { Some(r) } else { None })
            .filter(|r| (r.rect.height() - 2.0).abs() < 0.01 && r.fill != theme::TEXT_OFF)
            .map(|r| r.rect.right())
            .next()
            .expect("the accent part of the strip is drawn");
        StepsShot {
            centers: label_spans(&output).iter().map(|span| span.2).collect(),
            dot_x,
            accent_end,
            delay: output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        }
    }

    /// A wizard at page `at` and a context where the first frames have run, so the dot has settled.
    fn settled_steps(metro: bool, at: usize) -> (Wizard, egui::Context, f64) {
        let (s, _tmp) = session(metro, metro);
        let mut w = Wizard::new(&s);
        w.at = at;
        let ctx = test_ui::ctx();
        // The first frame loads the fonts. The next frames have the real sizes.
        steps_at(&w, &ctx, 0.0);
        steps_at(&w, &ctx, 0.1);
        steps_at(&w, &ctx, 1.0);
        (w, ctx, 1.0)
    }

    #[test]
    fn the_dot_is_under_the_active_label_at_rest() {
        for metro in [false, true] {
            for at in 0..(3 + usize::from(metro)) {
                let (w, ctx, t) = settled_steps(metro, at);
                let shot = steps_at(&w, &ctx, t + 1.0);
                assert!((shot.dot_x - shot.centers[at]).abs() <= 1.0, "page {at}: the dot is under its label");
            }
        }
    }

    #[test]
    fn the_first_frames_have_no_slide() {
        let (s, _tmp) = session(true, true);
        let mut w = Wizard::new(&s);
        w.at = 2;
        let ctx = test_ui::ctx();
        for t in [0.0, 0.01, 0.02] {
            let shot = steps_at(&w, &ctx, t);
            // The first frame has the fonts of the default size, so only the later frames must match.
            if t > 0.0 {
                assert!((shot.dot_x - shot.centers[2]).abs() <= 1.0, "t {t}: the dot is under label 2");
            }
        }
    }

    /// Move the wizard from page `from` to page `to` and check the slide, frame by frame.
    fn check_slide(from: usize, to: usize) {
        let (mut w, ctx, t) = settled_steps(true, from);
        if to > from {
            assert_eq!(w.next(), Flow::Open);
        } else {
            w.back();
        }
        assert_eq!(w.at, to);
        let t0 = t + 1.0;
        let start = steps_at(&w, &ctx, t0);
        let (a, b) = (start.centers[from], start.centers[to]);
        assert!((start.dot_x - a).abs() <= 1.0, "the slide starts under the old label");
        let mid = steps_at(&w, &ctx, t0 + f64::from(SLIDE_SECS) / 2.0);
        let (low, high) = (a.min(b), a.max(b));
        assert!(mid.dot_x > low + 2.0 && mid.dot_x < high - 2.0, "{from} to {to}: the dot is between the labels, at {}", mid.dot_x);
        assert!(mid.delay < std::time::Duration::MAX, "{from} to {to}: the slide asks for a repaint");
        for step in 1..=5 {
            let shot = steps_at(&w, &ctx, t0 + f64::from(SLIDE_SECS) * f64::from(step) / 5.0 - 0.01);
            assert!((shot.accent_end - shot.dot_x).abs() <= 1.0, "{from} to {to}: the strip ends at the dot");
        }
        let end = steps_at(&w, &ctx, t0 + f64::from(SLIDE_SECS) + 0.1);
        assert!((end.dot_x - b).abs() <= 1.0, "{from} to {to}: the dot ends under the new label");
        assert!((end.accent_end - b).abs() <= 1.0, "{from} to {to}: the strip ends under the new label");
        // The frame that ends the slide can still hold the last request, so look at the next frame.
        let rest = steps_at(&w, &ctx, t0 + f64::from(SLIDE_SECS) + 0.2);
        assert_eq!(rest.delay, std::time::Duration::MAX, "{from} to {to}: no repaint at rest");
    }

    #[test]
    fn the_dot_slides_to_the_next_label() {
        check_slide(0, 1);
        check_slide(2, 3);
    }

    #[test]
    fn the_dot_slides_back_to_the_last_label() {
        check_slide(1, 0);
        check_slide(3, 2);
    }

    #[test]
    fn no_repaint_is_asked_at_rest() {
        let (w, ctx, t) = settled_steps(true, 1);
        assert_eq!(steps_at(&w, &ctx, t + 1.0).delay, std::time::Duration::MAX);
    }
}
