//! The Log window: a separate window with one row for each fetch of a wormhole source.

use crate::app::Session;
use crate::theme;
use egui::{Align, Context, Frame, Layout, Margin, RichText, ViewportBuilder, ViewportId};
use egui_extras::{Column, TableBuilder};
use router_core::log::{LogEntry, local_clock_text};

/// The text of the result column. A text, not only a color, tells the two results apart.
fn result_text(entry: &LogEntry) -> &'static str {
    if entry.ok { "ok" } else { "failed" }
}

/// Show the Log window while `open`. A close request of the window sets `open` to false.
pub fn show(ctx: &Context, open: &mut bool, s: &Session) {
    if !*open {
        return;
    }
    let builder =
        ViewportBuilder::default().with_title("EVE Router - Log").with_inner_size([660.0, 380.0]).with_min_inner_size([420.0, 180.0]);
    ctx.show_viewport_immediate(ViewportId::from_hash_of("log-window"), builder, |ui, _class| {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            *open = false;
            return;
        }
        let frame = Frame::new().fill(theme::BG).inner_margin(Margin::same(10));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| table(ui, &s.log));
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
