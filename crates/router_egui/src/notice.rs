//! Notices: the messages that the app shows as toasts. An error stays until the pilot closes it
//! with a click. The other levels end after a time.

use crate::theme;
use egui::{Align2, Color32, Direction, Frame, Id, Label, Margin, Response, RichText, Sense, Stroke, Ui};
use egui_phosphor_icons::icons;
use egui_toast::{Toast, ToastKind, ToastOptions, Toasts};
use std::time::Duration;

/// How important a notice is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// A fact that the pilot asked for, for example "Added 2 systems".
    Info,
    /// A save or a copy that worked.
    Success,
    /// Something that the pilot should know, but the router goes on.
    Warning,
    /// A failure. It stays on the screen until the pilot closes it.
    Error,
}

impl Level {
    /// How long the toast stays. `None`: until the pilot closes it.
    pub fn duration(self) -> Option<Duration> {
        match self {
            Level::Error => None,
            Level::Warning => Some(Duration::from_secs(10)),
            Level::Info | Level::Success => Some(Duration::from_secs(3)),
        }
    }

    fn kind(self) -> ToastKind {
        match self {
            Level::Info => ToastKind::Info,
            Level::Success => ToastKind::Success,
            Level::Warning => ToastKind::Warning,
            Level::Error => ToastKind::Error,
        }
    }

    fn color(self) -> Color32 {
        match self {
            Level::Info => theme::TEXT_SOFT,
            Level::Success => theme::OK,
            Level::Warning => theme::WARN,
            Level::Error => theme::ERROR,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Level::Info => icons::INFO,
            Level::Success => icons::CHECK,
            Level::Warning => icons::WARNING,
            Level::Error => icons::X,
        }
        .as_str()
    }
}

/// A message for the pilot.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

/// The widest toast, in points. A longer text wraps.
const TOAST_WIDTH: f32 = 380.0;

/// The toasts in the corner of the window.
pub struct Toaster {
    id: Id,
}

impl Default for Toaster {
    fn default() -> Self {
        Toaster { id: Id::new("router-toasts") }
    }
}

impl Toaster {
    /// Move `notices` to the screen, and draw all toasts. A text that is on the screen does not
    /// stack a second time.
    pub fn show(&self, ui: &mut Ui, notices: &mut Vec<Notice>) {
        let mut toasts = Toasts::with_id(self.id).anchor(Align2::RIGHT_BOTTOM, (-14.0, -38.0)).direction(Direction::BottomUp);
        for level in [Level::Info, Level::Success, Level::Warning, Level::Error] {
            toasts = toasts.custom_contents(level.kind(), move |ui, toast| toast_contents(ui, toast, level));
        }
        let on_screen: Vec<String> =
            ui.data(|d| d.get_temp::<Vec<Toast>>(self.id)).unwrap_or_default().iter().map(|t| t.text.text().to_string()).collect();
        let mut seen = on_screen;
        for notice in notices.drain(..) {
            if seen.contains(&notice.text) {
                continue;
            }
            seen.push(notice.text.clone());
            let options = ToastOptions::default().duration(notice.level.duration()).show_progress(false).show_icon(false);
            toasts.add(Toast::new().kind(notice.level.kind()).text(notice.text).options(options));
        }
        toasts.show(ui);
    }
}

/// One toast: a colored icon and the wrapped text, on the panel color. A click closes it.
fn toast_contents(ui: &mut Ui, toast: &mut Toast, level: Level) -> Response {
    let frame = Frame::new().fill(theme::HEADER).stroke(Stroke::new(1.0, level.color())).inner_margin(Margin::symmetric(10, 8));
    let response = frame
        .show(ui, |ui| {
            ui.set_max_width(TOAST_WIDTH);
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(level.icon()).color(level.color()).strong());
                ui.add(Label::new(RichText::new(toast.text.text()).color(theme::TEXT)).wrap());
            });
        })
        .response
        .interact(Sense::click());
    if response.clicked() {
        toast.close();
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_error_stays_until_it_is_closed() {
        assert_eq!(Level::Error.duration(), None);
        assert_eq!(Level::Warning.duration(), Some(Duration::from_secs(10)));
        assert_eq!(Level::Info.duration(), Some(Duration::from_secs(3)));
        assert_eq!(Level::Success.duration(), Some(Duration::from_secs(3)));
    }

    #[test]
    fn the_same_text_does_not_stack() {
        let ctx = egui::Context::default();
        theme::apply_default(&ctx);
        theme::set_font(&ctx);
        let toaster = Toaster::default();
        let count = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<Vec<Toast>>(toaster.id)).unwrap_or_default().len();
        let mut notices = vec![
            Notice { level: Level::Error, text: "No route".into() },
            Notice { level: Level::Error, text: "No route".into() },
            Notice { level: Level::Info, text: "Added 1 system".into() },
        ];
        // Draw one frame. The frame output holds texture changes. A test with no renderer drops them.
        let draw = |notices: &mut Vec<Notice>| {
            let mut output = ctx.run_ui(Default::default(), |ui| toaster.show(ui, notices));
            output.textures_delta.clear();
        };
        draw(&mut notices);
        assert!(notices.is_empty(), "the notices move to the screen");
        assert_eq!(count(&ctx), 2);
        // The same text again, while it is on the screen.
        notices.push(Notice { level: Level::Error, text: "No route".into() });
        draw(&mut notices);
        assert_eq!(count(&ctx), 2);
    }
}
