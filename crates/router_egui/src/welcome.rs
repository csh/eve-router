//! The welcome screen: the presets, the answers, the page list and the two steps that the pilot sees
//! before the setup wizard. The screen writes nothing. It only gives the choice to its caller.

use router_core::settings::Features;

use crate::app::Session;
use crate::settings_window::eve_metro_available;
use crate::setup_wizard::WizardPage;
use crate::theme;
use crate::widgets::AppButton;
use egui::{Align, Color32, FontId, Label, Layout, Rect, RichText, Ui, UiBuilder, Vec2, pos2, vec2};

/// The place a pilot lives in. It picks the wizard pages and the starting answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Highsec space.
    Highsec,
    /// Lowsec space.
    Lowsec,
    /// Nullsec space.
    Nullsec,
    /// Wormhole space.
    Wormholes,
    /// The pilot sets everything by hand.
    Custom,
}

impl Preset {
    /// Every preset, in the order of the buttons.
    pub const ALL: [Preset; 5] = [Preset::Highsec, Preset::Lowsec, Preset::Nullsec, Preset::Wormholes, Preset::Custom];

    /// The text of the button for this preset.
    pub fn label(self) -> &'static str {
        match self {
            Preset::Highsec => "I live in highsec.",
            Preset::Lowsec => "I live in lowsec.",
            Preset::Nullsec => "I live in nullsec.",
            Preset::Wormholes => "I live in wormholes.",
            Preset::Custom => "I'll configure things myself, thanks.",
        }
    }

    /// True when the welcome screen asks the travel questions for this preset.
    pub fn asks_travel(self) -> bool {
        match self {
            Preset::Highsec | Preset::Lowsec | Preset::Nullsec | Preset::Wormholes => true,
            Preset::Custom => false,
        }
    }

    /// True when the wizard shows the Alliance page for this preset.
    pub fn show_alliance_setup(self) -> bool {
        match self {
            Preset::Nullsec | Preset::Custom => true,
            Preset::Highsec | Preset::Lowsec | Preset::Wormholes => false,
        }
    }

    /// The answers this preset starts with.
    pub fn answers(self) -> Answers {
        match self {
            Preset::Highsec => Answers { wormholes: false, pochven: false, subscription: false },
            Preset::Lowsec | Preset::Nullsec | Preset::Wormholes => Answers { wormholes: true, pochven: true, subscription: false },
            Preset::Custom => Answers { wormholes: true, pochven: true, subscription: true },
        }
    }
}

/// The three travel answers of the welcome screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answers {
    /// The pilot travels through wormholes.
    pub wormholes: bool,
    /// The pilot travels through Pochven.
    pub pochven: bool,
    /// The pilot has an EVE Metro subscription.
    pub subscription: bool,
}

impl Answers {
    /// The starting toggles of the Essentials page. Filaments start on.
    pub fn features(self) -> Features {
        Features { filaments: true, jspace: self.wormholes, pochven: self.pochven }
    }
}

/// The wizard pages for a preset and its answers. The two gates tell if the client ID and the Metro key exist.
pub fn pages_for(preset: Preset, answers: Answers, client_id: bool, metro: bool) -> Vec<WizardPage> {
    let mut pages = vec![WizardPage::Essentials];
    if preset.show_alliance_setup() {
        pages.push(WizardPage::Alliance);
    }
    if answers.wormholes {
        pages.push(WizardPage::Nexum);
    }
    if client_id && metro && answers.pochven && answers.subscription {
        pages.push(WizardPage::EveMetro);
    }
    pages
}

/// The width of the column that holds the content. The longest text fits in it.
const COLUMN: f32 = 640.0;
/// The width of the stack of preset buttons and of each pair of answer buttons.
const STACK_WIDTH: f32 = 320.0;
/// The width of one answer button. Two of them and the gap fill `STACK_WIDTH`.
const ANSWER_WIDTH: f32 = (STACK_WIDTH - GAP) / 2.0;
/// The height of each button.
const BUTTON_HEIGHT: f32 = 32.0;
/// The gap between two buttons, in a stack and in a pair.
const GAP: f32 = 8.0;
/// The size of the title text.
const TITLE_SIZE: f32 = 64.0;
/// The height of the row of the title. It is the height of the title text at `TITLE_SIZE`, so
/// the text fills the row and the row does not grow.
const TITLE_ROW: f32 = 80.0;
/// The space under the title.
const TITLE_GAP: f32 = 14.0;
/// The size of the two welcome lines.
const WELCOME_SIZE: f32 = 15.0;
/// The height of the row of one welcome line.
const WELCOME_ROW: f32 = 22.0;
/// The space under the welcome lines.
const WELCOME_GAP: f32 = 30.0;
/// The size of a question.
const QUESTION_SIZE: f32 = 16.0;
/// The height of the row of a question.
const QUESTION_ROW: f32 = 22.0;
/// The space under a question.
const QUESTION_GAP: f32 = 10.0;
/// The space above the second and the third question.
const ROW_GAP: f32 = 22.0;
/// The extra space above the quiet last preset button.
const QUIET_GAP: f32 = 8.0;
/// The time in seconds after a step change in which the screen ignores a click on the buttons of
/// the new step. The buttons of the two steps overlap on the screen, so the second click of a
/// double-click would hit a button of the new step without this guard.
const STEP_CLICK_GUARD: f64 = 0.4;

/// The height of the content of step 1. The rows have fixed heights, so the height is known in the
/// first frame and the content can sit in the middle at once.
fn preset_height() -> f32 {
    let buttons = Preset::ALL.len() as f32;
    TITLE_ROW
        + TITLE_GAP
        + 2.0 * WELCOME_ROW
        + WELCOME_GAP
        + QUESTION_ROW
        + QUESTION_GAP
        + buttons * BUTTON_HEIGHT
        + (buttons - 1.0) * GAP
        + QUIET_GAP
}

/// The height of the content of step 2, with or without the subscription question.
fn travel_height(subscription: bool) -> f32 {
    let pair = QUESTION_ROW + QUESTION_GAP + BUTTON_HEIGHT;
    let questions = if subscription { 3.0 } else { 2.0 };
    TITLE_ROW + TITLE_GAP + questions * pair + (questions - 1.0) * ROW_GAP
}

/// A centered line of text in a row of a fixed height.
fn text_row(ui: &mut Ui, text: &str, size: f32, color: Color32, row: f32) {
    ui.add_sized([COLUMN, row], Label::new(RichText::new(text).size(size).color(color)).extend());
}

/// A question and its pair of answer buttons. The pair has its own widget ids. A click on a
/// button sets `value` and nothing else. When `armed` is false, the buttons show but a click does
/// nothing.
fn question(ui: &mut Ui, salt: &str, text: &str, (yes, no): (&str, &str), value: &mut bool, armed: bool) {
    text_row(ui, text, QUESTION_SIZE, theme::TEXT, QUESTION_ROW);
    ui.add_space(QUESTION_GAP);
    ui.push_id(salt, |ui| {
        ui.allocate_ui_with_layout(vec2(STACK_WIDTH, BUTTON_HEIGHT), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            let size = vec2(ANSWER_WIDTH, BUTTON_HEIGHT);
            if ui.add(AppButton::new(yes).choice(*value).min_size(size)).clicked() && armed {
                *value = true;
            }
            if ui.add(AppButton::new(no).choice(!*value).min_size(size)).clicked() && armed {
                *value = false;
            }
        });
    });
}

/// The step of the welcome screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The title and the question "Where do you live?".
    Preset,
    /// The travel questions.
    Travel,
}

/// What the welcome screen asks of its caller after a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeFlow {
    /// The screen stays open.
    Open,
    /// The pilot is done. Start the wizard with this preset and these answers.
    Start(Preset, Answers),
}

/// The state of the welcome screen: the preset, the three answers, the step and the time of the
/// last step change.
pub struct Welcome {
    preset: Preset,
    answers: Answers,
    step: Step,
    /// The egui time of the last step change by a click. The buttons of the step ignore a click
    /// until `STEP_CLICK_GUARD` seconds after this time.
    settled_at: f64,
}

impl Welcome {
    /// A welcome screen at step 1. The preset is `Custom` until the pilot chooses one.
    pub fn new() -> Welcome {
        Welcome { preset: Preset::Custom, answers: Preset::Custom.answers(), step: Step::Preset, settled_at: f64::NEG_INFINITY }
    }

    /// True when the guard time after the last step change is over, so a click on a button of the
    /// step counts.
    fn armed(&self, ui: &Ui) -> bool {
        ui.input(|i| i.time) - self.settled_at >= STEP_CLICK_GUARD
    }

    /// Record the current egui time as the time of a step change.
    fn settle(&mut self, ui: &Ui) {
        self.settled_at = ui.input(|i| i.time);
    }

    /// The current step.
    pub fn step(&self) -> Step {
        self.step
    }

    /// The preset that the pilot chose last. Only the tests read it.
    #[cfg(test)]
    pub fn preset(&self) -> Preset {
        self.preset
    }

    /// The three answers. Only the tests read them.
    #[cfg(test)]
    pub fn answers(&self) -> Answers {
        self.answers
    }

    /// Choose a preset. `Custom` skips step 2 and starts the wizard at once. Any other preset fills
    /// the answers from its table row and moves to step 2.
    pub fn choose(&mut self, preset: Preset) -> WelcomeFlow {
        if !preset.asks_travel() {
            return WelcomeFlow::Start(preset, preset.answers());
        }
        self.preset = preset;
        self.answers = preset.answers();
        self.step = Step::Travel;
        WelcomeFlow::Open
    }

    /// Go back from step 2 to step 1. The answers stay. The next `choose` fills them again.
    pub fn back(&mut self) {
        self.step = Step::Preset;
    }

    /// Draw the content of the current step in the middle of the room that is left: the title, the
    /// texts and the buttons. The caller makes the panel and the footer.
    pub fn body(&mut self, ui: &mut Ui, s: &Session) -> WelcomeFlow {
        let show_subscription = self.answers.pochven && eve_metro_available(s);
        let height = match self.step {
            Step::Preset => preset_height(),
            Step::Travel => travel_height(show_subscription),
        };
        let area = ui.available_rect_before_wrap();
        let top = (area.center().y - height / 2.0).max(area.top());
        let rect = Rect::from_min_size(pos2(area.center().x - COLUMN / 2.0, top), vec2(COLUMN, height));
        let mut flow = WelcomeFlow::Open;
        let layout = Layout::top_down(Align::Center);
        ui.scope_builder(UiBuilder::new().id_salt("welcome-body").max_rect(rect).layout(layout), |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let title = RichText::new("EVE Router").font(FontId::new(TITLE_SIZE, theme::bold())).color(theme::accent(ui));
            ui.add_sized([COLUMN, TITLE_ROW], Label::new(title).extend());
            ui.add_space(TITLE_GAP);
            match self.step {
                Step::Preset => flow = self.preset_step(ui),
                Step::Travel => self.travel_step(ui, show_subscription),
            }
        });
        flow
    }

    /// Step 1: the two welcome lines, the question and the stack of preset buttons.
    fn preset_step(&mut self, ui: &mut Ui) -> WelcomeFlow {
        text_row(ui, &format!("Welcome to EVE Router v{}.", env!("CARGO_PKG_VERSION")), WELCOME_SIZE, theme::TEXT_DIM, WELCOME_ROW);
        text_row(ui, "Several presets are available to help you hit the ground running.", WELCOME_SIZE, theme::TEXT_DIM, WELCOME_ROW);
        ui.add_space(WELCOME_GAP);
        text_row(ui, "Where do you live?", QUESTION_SIZE, theme::TEXT, QUESTION_ROW);
        ui.add_space(QUESTION_GAP);
        let armed = self.armed(ui);
        let mut chosen = None;
        ui.push_id("welcome-presets", |ui| {
            for (i, preset) in Preset::ALL.into_iter().enumerate() {
                let quiet = preset == Preset::Custom;
                if i > 0 {
                    ui.add_space(if quiet { GAP + QUIET_GAP } else { GAP });
                }
                let button = |ui: &mut Ui, text: RichText| ui.add(AppButton::new(text).min_size(vec2(STACK_WIDTH, BUTTON_HEIGHT)));
                let response = if quiet {
                    // The quiet button has a grey line and a dim text. The fill stays transparent.
                    ui.scope(|ui| {
                        ui.visuals_mut().widgets.inactive.bg_stroke.color = theme::LINE;
                        button(ui, RichText::new(preset.label()).color(theme::TEXT_DIM))
                    })
                    .inner
                } else {
                    button(ui, RichText::new(preset.label()))
                };
                if response.clicked() && armed {
                    chosen = Some(preset);
                }
            }
        });
        let flow = chosen.map_or(WelcomeFlow::Open, |preset| self.choose(preset));
        if chosen.is_some() && self.step == Step::Travel {
            self.settle(ui);
        }
        flow
    }

    /// Step 2: the Wormholes and Pochven questions, and the subscription question when it applies.
    fn travel_step(&mut self, ui: &mut Ui, show_subscription: bool) {
        let armed = self.armed(ui);
        let answers = &mut self.answers;
        let wormholes = "Are you comfortable travelling through Wormholes?";
        question(ui, "welcome-wormholes", wormholes, ("Yes", "No"), &mut answers.wormholes, armed);
        ui.add_space(ROW_GAP);
        let pochven = "Are you comfortable travelling through Pochven?";
        question(ui, "welcome-pochven", pochven, ("Yes", "No"), &mut answers.pochven, armed);
        if show_subscription {
            ui.add_space(ROW_GAP);
            let text = "Do you have an EVE Metro subscription?";
            question(ui, "welcome-subscription", text, ("I do", "I do not"), &mut answers.subscription, armed);
        }
    }

    /// Draw the footer of step 2: Back at the left, Continue at the right. Step 1 has no footer. The
    /// caller decides when to call it.
    pub fn footer(&mut self, ui: &mut Ui) -> WelcomeFlow {
        let mut flow = WelcomeFlow::Open;
        if self.step != Step::Travel {
            return flow;
        }
        ui.horizontal(|ui| {
            if ui.add(AppButton::new("Back")).clicked() {
                self.back();
                self.settle(ui);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(AppButton::new("Continue").primary()).clicked() {
                    flow = WelcomeFlow::Start(self.preset, self.answers);
                }
            });
        });
        flow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(wormholes: bool, pochven: bool, subscription: bool) -> Answers {
        Answers { wormholes, pochven, subscription }
    }

    #[test]
    fn the_labels_are_the_button_texts() {
        let labels: Vec<&str> = Preset::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(
            labels,
            [
                "I live in highsec.",
                "I live in lowsec.",
                "I live in nullsec.",
                "I live in wormholes.",
                "I'll configure things myself, thanks.",
            ]
        );
    }

    #[test]
    fn the_preset_table_gives_the_answers() {
        // (preset, wormholes, pochven, subscription, asks_travel, show_alliance_setup)
        let table = [
            (Preset::Highsec, false, false, false, true, false),
            (Preset::Lowsec, true, true, false, true, false),
            (Preset::Nullsec, true, true, false, true, true),
            (Preset::Wormholes, true, true, false, true, false),
            (Preset::Custom, true, true, true, false, true),
        ];
        for (preset, w, p, s, travel, alliance) in table {
            assert_eq!(preset.answers(), answers(w, p, s), "{preset:?}");
            assert_eq!(preset.asks_travel(), travel, "{preset:?}");
            assert_eq!(preset.show_alliance_setup(), alliance, "{preset:?}");
        }
    }

    #[test]
    fn only_custom_skips_the_travel_step() {
        for preset in Preset::ALL {
            assert_eq!(preset.asks_travel(), preset != Preset::Custom);
        }
    }

    #[test]
    fn only_nullsec_and_custom_show_the_alliance_setup() {
        for preset in Preset::ALL {
            let expected = matches!(preset, Preset::Nullsec | Preset::Custom);
            assert_eq!(preset.show_alliance_setup(), expected, "{preset:?}");
        }
    }

    #[test]
    fn the_answers_set_the_starting_toggles() {
        for wormholes in [false, true] {
            for pochven in [false, true] {
                let features = answers(wormholes, pochven, false).features();
                assert_eq!(features.jspace, wormholes);
                assert_eq!(features.pochven, pochven);
                assert!(features.filaments);
            }
        }
    }

    #[test]
    fn the_pages_follow_the_answers() {
        use WizardPage::{Alliance, Essentials, EveMetro, Nexum};
        let pages = |p: Preset, s: bool, c: bool, m: bool| {
            let mut a = p.answers();
            a.subscription = s;
            pages_for(p, a, c, m)
        };
        assert_eq!(pages(Preset::Highsec, false, true, true), [Essentials]);
        assert_eq!(pages(Preset::Lowsec, false, true, true), [Essentials, Nexum]);
        assert_eq!(pages(Preset::Lowsec, true, true, true), [Essentials, Nexum, EveMetro]);
        assert_eq!(pages(Preset::Nullsec, false, true, true), [Essentials, Alliance, Nexum]);
        assert_eq!(pages(Preset::Wormholes, false, true, true), [Essentials, Nexum]);
        assert_eq!(pages(Preset::Custom, true, true, true), [Essentials, Alliance, Nexum, EveMetro]);
        assert_eq!(pages(Preset::Custom, true, false, true), [Essentials, Alliance, Nexum]);
        assert_eq!(pages(Preset::Custom, true, true, false), [Essentials, Alliance, Nexum]);
    }

    use crate::test_ui::{self, drawn_shapes, drawn_texts};
    use egui::{Pos2, Rect};
    use router_core::config::{self, ApiKey};
    use router_core::esi::pilots::Pilots;
    use router_core::test_support::{overlay_universe, settings};

    /// The content of the smallest window.
    const WINDOW: egui::Vec2 = crate::setup_wizard::SMALLEST_WINDOW;

    /// A session with a config in a temporary directory. With `client_id`, it has an EVE client ID.
    /// With `metro_key`, it has an EVE Metro key. This copies the helper of the tests of
    /// `setup_wizard.rs`, which is private to that module.
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

    /// The drawing of one frame: what `draw` gives, and what it paints.
    type Draw<'a> = &'a mut dyn FnMut(&mut Ui) -> WelcomeFlow;

    /// Run one frame of `draw` in the smallest window.
    fn frame(ctx: &egui::Context, draw: Draw, events: Vec<egui::Event>, time: f64) -> (WelcomeFlow, egui::FullOutput) {
        let screen = Rect::from_min_size(Pos2::ZERO, WINDOW);
        let input = egui::RawInput { time: Some(time), screen_rect: Some(screen), events, ..Default::default() };
        let mut flow = WelcomeFlow::Open;
        let mut output = ctx.run_ui(input, |ui| flow = draw(ui));
        output.textures_delta.clear();
        (flow, output)
    }

    /// The texts of a frame of the body, from the top to the bottom.
    fn body_texts(w: &mut Welcome, s: &Session) -> Vec<(String, Pos2, egui::Vec2)> {
        let ctx = test_ui::ctx();
        let (_, output) = frame(&ctx, &mut |ui| w.body(ui, s), Vec::new(), 0.0);
        let mut texts = drawn_texts(&output);
        texts.sort_by(|a, b| (a.1.y, a.1.x).partial_cmp(&(b.1.y, b.1.x)).unwrap());
        texts
    }

    /// Draw `draw`, then put the pointer on the `nth` text `label` from the top (and from the
    /// left), press and release the primary button, and return what the last frame gave.
    fn click(draw: Draw, label: &str, nth: usize) -> WelcomeFlow {
        let ctx = test_ui::ctx();
        let (_, output) = frame(&ctx, draw, Vec::new(), 0.0);
        let mut found: Vec<_> = drawn_texts(&output).into_iter().filter(|(t, ..)| t == label).collect();
        found.sort_by(|a, b| (a.1.y, a.1.x).partial_cmp(&(b.1.y, b.1.x)).unwrap());
        let (_, pos, size) = *found.get(nth).unwrap_or_else(|| panic!("no {label} button number {nth}"));
        let at = pos + size / 2.0;
        let button =
            |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, draw, vec![egui::Event::PointerMoved(at)], 0.1);
        frame(&ctx, draw, vec![button(true)], 0.2);
        frame(&ctx, draw, vec![button(false)], 0.3).0
    }

    /// Click a button of the body.
    fn click_body(w: &mut Welcome, s: &Session, label: &str, nth: usize) -> WelcomeFlow {
        click(&mut |ui| w.body(ui, s), label, nth)
    }

    /// The labels of the texts, in the order of the list.
    fn labels(texts: &[(String, Pos2, egui::Vec2)]) -> Vec<&str> {
        texts.iter().map(|(t, ..)| t.as_str()).collect()
    }

    /// A welcome screen at step 2 for `preset`.
    fn at_travel(preset: Preset) -> Welcome {
        let mut w = Welcome::new();
        assert_eq!(w.choose(preset), WelcomeFlow::Open);
        w
    }

    /// The rects of the buttons, which are 32 points high, from the top to the bottom and from the left to the right.
    fn button_rects(w: &mut Welcome, s: &Session) -> Vec<Rect> {
        let ctx = test_ui::ctx();
        let (_, output) = frame(&ctx, &mut |ui| w.body(ui, s), Vec::new(), 0.0);
        let mut rects: Vec<Rect> = drawn_shapes(&output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Rect(r) if (r.rect.height() - 32.0).abs() <= 1.0 => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| (a.min.y, a.min.x).partial_cmp(&(b.min.y, b.min.x)).unwrap());
        rects
    }

    #[test]
    fn step_one_draws_the_exact_texts() {
        let (s, _tmp) = session(true, true);
        let texts = body_texts(&mut Welcome::new(), &s);
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            labels(&texts),
            [
                "EVE Router",
                format!("Welcome to EVE Router v{version}.").as_str(),
                "Several presets are available to help you hit the ground running.",
                "Where do you live?",
                "I live in highsec.",
                "I live in lowsec.",
                "I live in nullsec.",
                "I live in wormholes.",
                "I'll configure things myself, thanks.",
            ]
        );
        let mut w = Welcome::new();
        let ctx = test_ui::ctx();
        let (flow, output) = frame(&ctx, &mut |ui| w.footer(ui), Vec::new(), 0.0);
        assert_eq!(flow, WelcomeFlow::Open);
        assert!(drawn_texts(&output).is_empty(), "step 1 has no Back and no Continue");
        assert!(texts[0].2.y >= 64.0, "the title is 64 points high at least: {}", texts[0].2.y);
    }

    #[test]
    fn the_version_text_uses_the_package_version_with_one_v() {
        let (s, _tmp) = session(true, true);
        let texts = body_texts(&mut Welcome::new(), &s);
        let expected = format!("Welcome to EVE Router v{}.", env!("CARGO_PKG_VERSION"));
        assert_eq!(texts.iter().filter(|(t, ..)| *t == expected).count(), 1);
        assert!(!expected.contains("vv"), "{expected}");
    }

    #[test]
    fn step_two_draws_the_questions_and_the_footer() {
        let (s, _tmp) = session(true, true);
        let mut w = at_travel(Preset::Highsec);
        let texts = body_texts(&mut w, &s);
        assert_eq!(
            labels(&texts),
            [
                "EVE Router",
                "Are you comfortable travelling through Wormholes?",
                "Yes",
                "No",
                "Are you comfortable travelling through Pochven?",
                "Yes",
                "No",
            ]
        );
        let ctx = test_ui::ctx();
        let (flow, output) = frame(&ctx, &mut |ui| w.footer(ui), Vec::new(), 0.0);
        assert_eq!(flow, WelcomeFlow::Open);
        let mut footer = drawn_texts(&output);
        footer.sort_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap());
        assert_eq!(labels(&footer), ["Back", "Continue"], "Back at the left, Continue at the right");
    }

    #[test]
    fn a_preset_click_goes_to_step_two_with_the_prefilled_answers() {
        let (s, _tmp) = session(true, true);
        for preset in [Preset::Highsec, Preset::Lowsec, Preset::Nullsec, Preset::Wormholes] {
            let mut w = Welcome::new();
            assert_eq!(click_body(&mut w, &s, preset.label(), 0), WelcomeFlow::Open, "{preset:?}");
            assert_eq!(w.step(), Step::Travel, "{preset:?}");
            assert_eq!(w.preset(), preset);
            assert_eq!(w.answers(), preset.answers(), "{preset:?}");
        }
    }

    #[test]
    fn custom_starts_the_wizard_at_once() {
        let mut w = Welcome::new();
        assert_eq!(w.choose(Preset::Custom), WelcomeFlow::Start(Preset::Custom, Preset::Custom.answers()));
        assert_eq!(w.step(), Step::Preset);
        // The same through a real click on the last button.
        let (s, _tmp) = session(true, true);
        let mut w = Welcome::new();
        let flow = click_body(&mut w, &s, "I'll configure things myself, thanks.", 0);
        assert_eq!(flow, WelcomeFlow::Start(Preset::Custom, Preset::Custom.answers()));
        assert_eq!(w.step(), Step::Preset);
    }

    #[test]
    fn choose_moves_on_for_the_other_presets() {
        for preset in [Preset::Highsec, Preset::Lowsec, Preset::Nullsec, Preset::Wormholes] {
            let mut w = Welcome::new();
            assert_eq!(w.choose(preset), WelcomeFlow::Open);
            assert_eq!((w.step(), w.preset(), w.answers()), (Step::Travel, preset, preset.answers()));
        }
    }

    #[test]
    fn an_answer_click_changes_only_that_answer() {
        let (s, _tmp) = session(true, true);
        // Lowsec starts with Yes, Yes and "I do not". The subscription row shows.
        let mut w = at_travel(Preset::Lowsec);
        let start = w.answers();
        click_body(&mut w, &s, "No", 0);
        assert_eq!(w.answers(), Answers { wormholes: false, ..start });
        click_body(&mut w, &s, "Yes", 0);
        assert_eq!(w.answers(), start);
        click_body(&mut w, &s, "No", 1);
        assert_eq!(w.answers(), Answers { pochven: false, ..start });
        click_body(&mut w, &s, "Yes", 1);
        assert_eq!(w.answers(), start);
        click_body(&mut w, &s, "I do", 0);
        assert_eq!(w.answers(), Answers { subscription: true, ..start });
        click_body(&mut w, &s, "I do not", 0);
        assert_eq!(w.answers(), start);
        // A click on the chosen answer keeps it.
        click_body(&mut w, &s, "Yes", 0);
        assert_eq!(w.answers(), start);
    }

    #[test]
    fn the_subscription_row_follows_pochven_and_the_gates() {
        let (s, _tmp) = session(true, true);
        let row = |w: &mut Welcome| body_texts(w, &s).iter().any(|(t, ..)| t == "Do you have an EVE Metro subscription?");
        let mut w = at_travel(Preset::Highsec);
        assert!(!row(&mut w), "Pochven is No");
        click_body(&mut w, &s, "Yes", 1);
        assert!(w.answers().pochven);
        assert!(row(&mut w), "Pochven is Yes");
        let texts = body_texts(&mut w, &s);
        assert!(labels(&texts).ends_with(&["Do you have an EVE Metro subscription?", "I do", "I do not"]));
        click_body(&mut w, &s, "I do", 0);
        click_body(&mut w, &s, "No", 1);
        assert!(!row(&mut w), "Pochven is No again");
        click_body(&mut w, &s, "Yes", 1);
        assert!(row(&mut w) && w.answers().subscription, "the row keeps its last answer");
    }

    #[test]
    fn without_the_metro_gates_the_subscription_row_is_hidden() {
        for (client_id, metro_key) in [(false, true), (true, false), (false, false)] {
            let (s, _tmp) = session(client_id, metro_key);
            let mut w = at_travel(Preset::Lowsec);
            let texts = body_texts(&mut w, &s);
            assert!(w.answers().pochven);
            assert!(!labels(&texts).contains(&"Do you have an EVE Metro subscription?"), "{client_id} {metro_key}");
            assert!(!labels(&texts).contains(&"I do"));
            // The Pochven buttons still work.
            click_body(&mut w, &s, "No", 1);
            assert_eq!(w.answers(), Answers { pochven: false, ..Preset::Lowsec.answers() });
        }
    }

    #[test]
    fn choosing_a_preset_again_fills_the_answers_again() {
        let (s, _tmp) = session(true, true);
        let mut w = at_travel(Preset::Lowsec);
        click_body(&mut w, &s, "No", 0);
        click_body(&mut w, &s, "I do", 0);
        assert_ne!(w.answers(), Preset::Lowsec.answers());
        w.back();
        assert_eq!(w.step(), Step::Preset);
        assert_ne!(w.answers(), Preset::Lowsec.answers(), "Back keeps the answers");
        w.choose(Preset::Lowsec);
        assert_eq!(w.answers(), Preset::Lowsec.answers());
        assert_eq!(w.step(), Step::Travel);
    }

    #[test]
    fn the_footer_buttons_work() {
        let mut w = at_travel(Preset::Nullsec);
        assert_eq!(click(&mut |ui| w.footer(ui), "Continue", 0), WelcomeFlow::Start(Preset::Nullsec, Preset::Nullsec.answers()));
        assert_eq!(w.step(), Step::Travel);
        assert_eq!(click(&mut |ui| w.footer(ui), "Back", 0), WelcomeFlow::Open);
        assert_eq!(w.step(), Step::Preset);
    }

    #[test]
    fn the_answer_buttons_keep_their_widths() {
        let (s, _tmp) = session(true, true);
        let stack = button_rects(&mut Welcome::new(), &s);
        assert_eq!(stack.len(), 5);
        for r in &stack {
            assert!((r.width() - 320.0).abs() <= 1.0 && (r.height() - 32.0).abs() <= 1.0, "{r:?}");
        }
        assert!((stack[1].top() - stack[0].bottom() - 8.0).abs() <= 1.0, "8 px gap");
        let rects = button_rects(&mut at_travel(Preset::Lowsec), &s);
        assert_eq!(rects.len(), 6, "three pairs");
        for pair in rects.chunks(2) {
            assert!((pair[0].width() - 156.0).abs() <= 1.0 && (pair[1].width() - 156.0).abs() <= 1.0, "{pair:?}");
            assert!((pair[1].right() - pair[0].left() - 320.0).abs() <= 1.0, "the pair is 320 wide");
            assert!((pair[1].left() - pair[0].right() - 8.0).abs() <= 1.0, "8 px gap");
            assert!((pair[0].left() + 160.0 - WINDOW.x / 2.0).abs() <= 1.0, "centered");
        }
    }

    /// Draw one frame of `draw` at `time` and return the output.
    fn frame_at(ctx: &egui::Context, draw: Draw, time: f64) -> egui::FullOutput {
        frame(ctx, draw, Vec::new(), time).1
    }

    /// The middle of the `nth` text `label` of `output`, from the top and from the left.
    fn center_of(output: &egui::FullOutput, label: &str, nth: usize) -> Pos2 {
        let mut found: Vec<Rect> = drawn_shapes(output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2())),
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| (a.min.y, a.min.x).partial_cmp(&(b.min.y, b.min.x)).unwrap());
        found.get(nth).unwrap_or_else(|| panic!("no {label} text number {nth}")).center()
    }

    /// Click at `at` in the shared context `ctx`: press at `time`, release 0.01 s later. Return
    /// what the frame of the release gave.
    fn click_at(ctx: &egui::Context, draw: Draw, at: Pos2, time: f64) -> WelcomeFlow {
        let button =
            |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(ctx, draw, vec![egui::Event::PointerMoved(at), button(true)], time);
        frame(ctx, draw, vec![button(false)], time + 0.01).0
    }

    #[test]
    fn a_double_click_on_a_preset_does_not_change_an_answer() {
        let (s, _tmp) = session(true, true);
        let mut w = Welcome::new();
        let ctx = test_ui::ctx();
        let draw = &mut |ui: &mut Ui| w.body(ui, &s);
        let output = frame_at(&ctx, draw, 0.0);
        let highsec = center_of(&output, Preset::Highsec.label(), 0);
        // The step changes in the frame of the release, at 0.02 s.
        click_at(&ctx, draw, highsec, 0.01);
        let output = frame_at(&ctx, draw, 0.03);
        let yes = center_of(&output, "Yes", 0);
        // The second click of a double-click, 0.1 s after the step change.
        click_at(&ctx, draw, yes, 0.11);
        assert_eq!((w.step(), w.answers()), (Step::Travel, Preset::Highsec.answers()), "the early click does nothing");
        let draw = &mut |ui: &mut Ui| w.body(ui, &s);
        click_at(&ctx, draw, yes, 0.61);
        assert_eq!(w.answers(), Answers { wormholes: true, ..Preset::Highsec.answers() }, "a later click works");
    }

    #[test]
    fn a_double_click_on_back_does_not_choose_a_preset() {
        let (s, _tmp) = session(true, true);
        let mut w = at_travel(Preset::Highsec);
        let ctx = test_ui::ctx();
        // The footer and the body in one frame, as the caller draws them.
        let draw = &mut |ui: &mut Ui| {
            let footer = w.footer(ui);
            let body = w.body(ui, &s);
            if footer == WelcomeFlow::Open { body } else { footer }
        };
        let output = frame_at(&ctx, draw, 0.0);
        let back = center_of(&output, "Back", 0);
        click_at(&ctx, draw, back, 0.01);
        let output = frame_at(&ctx, draw, 0.03);
        let lowsec = center_of(&output, Preset::Lowsec.label(), 0);
        click_at(&ctx, draw, lowsec, 0.11);
        assert_eq!(w.step(), Step::Preset, "the early click does nothing");
        let draw = &mut |ui: &mut Ui| w.body(ui, &s);
        click_at(&ctx, draw, lowsec, 0.61);
        assert_eq!((w.step(), w.preset()), (Step::Travel, Preset::Lowsec), "a later click works");
    }

    #[test]
    fn the_quiet_button_has_a_grey_line_and_no_fill() {
        let (s, _tmp) = session(true, true);
        let ctx = test_ui::ctx();
        let mut w = Welcome::new();
        let output = frame_at(&ctx, &mut |ui| w.body(ui, &s), 0.0);
        let mut buttons: Vec<egui::epaint::RectShape> = drawn_shapes(&output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Rect(r) if (r.rect.height() - BUTTON_HEIGHT).abs() <= 1.0 => Some(r),
                _ => None,
            })
            .collect();
        buttons.sort_by(|a, b| a.rect.min.y.partial_cmp(&b.rect.min.y).unwrap());
        assert_eq!(buttons.len(), 5);
        let quiet = &buttons[4];
        assert_eq!(quiet.rect.size(), vec2(STACK_WIDTH, BUTTON_HEIGHT), "the size stays");
        assert_eq!(quiet.stroke.color, theme::LINE, "a grey line");
        assert_eq!(quiet.fill, Color32::TRANSPARENT, "no fill");
        for b in &buttons[..4] {
            assert_ne!(b.stroke.color, theme::LINE, "the other buttons keep the accent line");
        }
    }

    #[test]
    fn the_title_row_is_the_height_of_the_title() {
        let (s, _tmp) = session(true, true);
        let texts = body_texts(&mut Welcome::new(), &s);
        assert_eq!(texts[0].0, "EVE Router");
        assert!((texts[0].2.y - TITLE_ROW).abs() <= 0.5, "the title is {} high", texts[0].2.y);
    }

    #[test]
    fn the_stack_and_the_texts_are_centered_across() {
        let (s, _tmp) = session(true, true);
        let middle = WINDOW.x / 2.0;
        for mut w in [Welcome::new(), at_travel(Preset::Lowsec)] {
            let ctx = test_ui::ctx();
            let output = frame_at(&ctx, &mut |ui| w.body(ui, &s), 0.0);
            for shape in drawn_shapes(&output) {
                if let egui::epaint::Shape::Text(t) = shape {
                    let x = t.galley.rect.translate(t.pos.to_vec2()).center().x;
                    let pair = ["Yes", "No", "I do", "I do not"].contains(&t.galley.text());
                    if !pair {
                        assert!((x - middle).abs() <= 1.0, "{} at {x}", t.galley.text());
                    }
                }
            }
        }
        for r in button_rects(&mut Welcome::new(), &s) {
            assert!((r.center().x - middle).abs() <= 1.0, "{r:?}");
        }
    }

    #[test]
    fn nothing_clips_in_the_smallest_window_and_the_content_is_centered() {
        let (s, _tmp) = session(true, true);
        let window = Rect::from_min_size(Pos2::ZERO, WINDOW);
        for mut w in [Welcome::new(), at_travel(Preset::Lowsec)] {
            let texts = body_texts(&mut w, &s);
            for (text, pos, size) in &texts {
                assert!(window.contains_rect(Rect::from_min_size(*pos, *size)), "{text} clips");
                assert!(size.x < COLUMN, "{text} fits the column");
            }
            // The title fills its row, so its top is the top of the content. The last button
            // is the bottom of the content.
            let top = texts.first().unwrap().1.y;
            let bottom = button_rects(&mut w, &s).iter().map(|r| r.bottom()).fold(0.0, f32::max);
            assert!(((top + bottom) / 2.0 - WINDOW.y / 2.0).abs() <= 2.0, "centered: {top} to {bottom}");
        }
    }
}
