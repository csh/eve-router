//! The Log window: a separate window with one row for each fetch of a wormhole source.

use crate::app::Session;
use crate::theme;
use egui::{Align, Context, Frame, Id, Layout, Margin, RichText, ViewportBuilder, ViewportId};
use egui_extras::{Column, TableBuilder};
use router_core::log::{LogEntry, local_clock_text};

/// The text of the result column. A text, not only a color, tells the two results apart.
fn result_text(entry: &LogEntry) -> &'static str {
    if entry.ok { "ok" } else { "failed" }
}

/// The title of the Log window. The OS shows it in the task bar, and the app finds the window by it.
const TITLE: &str = "EVE Router - Log";

/// Show the Log window while `open`. A close request of the window sets `open` to false.
pub fn show(ctx: &Context, open: &mut bool, s: &Session) {
    if !*open {
        return;
    }
    // The window has no OS frame, as the main window has. It starts hidden and transparent. The OS
    // shows a new window with a white background for a moment, before the first frame of the app is
    // drawn. The window shows after that frame.
    let builder = ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size([660.0, 380.0])
        .with_min_inner_size([420.0, 180.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_visible(false);
    let frames_id = Id::new("log-window-frames");
    ctx.show_viewport_immediate(ViewportId::from_hash_of("log-window"), builder, |ui, _class| {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            *open = false;
            ui.ctx().data_mut(|d| d.remove::<u32>(frames_id));
            return;
        }
        let frames = ui.ctx().data_mut(|d| {
            let frames = d.get_temp_mut_or_default::<u32>(frames_id);
            *frames += 1;
            *frames
        });
        match frames {
            1 => ui.ctx().request_repaint(),
            2 => {
                crate::chrome::square_window_titled(TITLE);
                ui.ctx().request_repaint();
            }
            3 => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(true)),
            _ => {}
        }
        crate::chrome::title_bar(ui, "Log", "");
        let frame = Frame::new().fill(theme::BG).inner_margin(Margin::same(10));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| table(ui, &s.log));
        crate::chrome::paint_border(ui.ctx(), theme::accent(ui));
        crate::chrome::resize_edges(ui.ctx());
    });
}

/// The rows, newest first.
fn table(ui: &mut egui::Ui, log: &[LogEntry]) {
    if log.is_empty() {
        ui.label(RichText::new("No operations yet").color(theme::TEXT_DIM));
        return;
    }
    let header = |ui: &mut egui::Ui, text: &str| _ = ui.label(theme::header_text(text));
    TableBuilder::new(ui)
        .id_salt("log-table")
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(72.0))
        .column(Column::exact(130.0))
        .column(Column::exact(56.0))
        .column(Column::remainder().at_least(120.0).clip(true))
        .auto_shrink(false)
        .header(20.0, |mut row| {
            for text in ["Time", "Operation", "Result", "Message"] {
                row.col(|ui| header(ui, text));
            }
        })
        .body(|body| {
            body.rows(22.0, log.len(), |mut row| {
                let entry = &log[log.len() - 1 - row.index()];
                row.col(|ui| _ = ui.label(RichText::new(local_clock_text(entry.time)).color(theme::TEXT_DIM).monospace()));
                row.col(|ui| _ = ui.label(RichText::new(entry.op).color(theme::TEXT)));
                let color = if entry.ok { theme::OK } else { theme::ERROR };
                row.col(|ui| _ = ui.label(RichText::new(result_text(entry)).color(color)));
                row.col(|ui| _ = ui.label(RichText::new(&entry.reason).color(theme::TEXT)));
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_is_a_word() {
        let mut entry = LogEntry { time: 0, op: "nexum.fetch", ok: true, reason: String::new() };
        assert_eq!(result_text(&entry), "ok");
        entry.ok = false;
        assert_eq!(result_text(&entry), "failed");
    }
}
