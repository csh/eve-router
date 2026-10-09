//! The system search box. Type a part of a name, then pick a system from the result list.
//! Enter or a click picks the highlighted system. A right click opens more actions, as the
//! context menu of a system in EVE.

use crate::theme;
use egui::{Align2, Area, FontId, Frame, Id, Key, Margin, Modifiers, Order, RichText, Sense, Stroke, TextEdit, Ui, vec2};
use petgraph::graph::NodeIndex;
use router_core::universe::{Universe, display_sec};

/// The most results that the list shows.
const LIMIT: usize = 10;
const ROW_HEIGHT: f32 = 22.0;

/// What to do with a picked system.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    /// Enter, or a click. The caller decides what it means.
    Default,
    SetStart,
    AddWaypoint,
    AddFavourite,
}

impl Pick {
    fn label(self) -> &'static str {
        match self {
            Pick::Default => "Select",
            Pick::SetStart => "Set as start",
            Pick::AddWaypoint => "Add waypoint",
            Pick::AddFavourite => "Add to favourites",
        }
    }
}

pub struct SearchBox {
    id: Id,
    query: String,
    results: Vec<NodeIndex>,
    highlighted: usize,
    /// The result list shows while this is true.
    open: bool,
    /// A pasted list of systems, for the caller to add. See `take_list`.
    list: Option<String>,
    /// True if the box takes a list of systems. See `with_lists`.
    lists: bool,
}

/// True if `text` holds more than one system name.
/// The least width of the result list: the name, the security and the longest region name.
const MIN_LIST_WIDTH: f32 = 400.0;

/// The width that the frame of the result list adds: 2 margins of 2 and 2 strokes of 1.
const LIST_FRAME: f32 = 6.0;

/// The width of the result list for a field of `field` width. A narrow field gets a list that fits
/// the longest region name. A wide field gets a list of its own width.
fn list_width(field: f32) -> f32 {
    field.max(MIN_LIST_WIDTH)
}

pub fn is_list(text: &str) -> bool {
    text.contains(['>', ',', ';', '\t', '\n'])
}

impl SearchBox {
    pub fn new(id: &str) -> Self {
        SearchBox { id: Id::new(id), query: String::new(), results: Vec::new(), highlighted: 0, open: false, list: None, lists: false }
    }

    /// A box that also takes a list of systems: a paste with line breaks or commas, or a
    /// typed list with commas and then Enter.
    pub fn with_lists(mut self) -> Self {
        self.lists = true;
        self
    }

    /// Give the keyboard focus to the text field.
    pub fn focus(&self, ui: &Ui) {
        ui.memory_mut(|m| m.request_focus(self.id));
    }

    /// Take a list of systems that was pasted, or typed with commas and then Enter.
    pub fn take_list(&mut self) -> Option<String> {
        self.list.take()
    }

    /// Take the highlighted result, for an "Add" button next to the box. Clear the box.
    pub fn take_highlighted(&mut self) -> Option<NodeIndex> {
        let node = self.results.get(self.highlighted).copied().filter(|_| !self.query.trim().is_empty());
        if node.is_some() {
            self.query.clear();
            self.results.clear();
            self.open = false;
        }
        node
    }

    /// Show the box, with `width` and the hint text. `menu` gives the items of the right-click
    /// menu on a result. Return the picked system, if any.
    pub fn show(&mut self, ui: &mut Ui, uni: &Universe, width: f32, hint: &str, menu: &[Pick]) -> Option<(Pick, NodeIndex)> {
        // The arrow keys move the highlight, so the text field must not get them.
        let focused = ui.memory(|m| m.has_focus(self.id));
        if focused && self.open && !self.results.is_empty() {
            let (down, up) =
                ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::ArrowUp)));
            if down {
                self.highlighted = (self.highlighted + 1).min(self.results.len() - 1);
            }
            if up {
                self.highlighted = self.highlighted.saturating_sub(1);
            }
        }

        // A single-line field changes line breaks to spaces, and a name can hold a space. So a
        // pasted list goes to `list` before the field gets it.
        if focused && self.lists {
            ui.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Paste(text) if is_list(text) => {
                        self.list = Some(text.clone());
                        false
                    }
                    _ => true,
                })
            });
        }

        let edit = TextEdit::singleline(&mut self.query)
            .id(self.id)
            .hint_text(RichText::new(hint).color(theme::TEXT_DIM))
            .desired_width(width)
            .margin(Margin::symmetric(8, 4));
        let response = ui.add(edit);
        // The list goes on top of the other windows when it opens, for example a modal window.
        let raise = response.changed() || response.gained_focus();
        if response.changed() {
            self.results = uni.search(&self.query, LIMIT);
            self.highlighted = 0;
            self.open = true;
        }
        if response.gained_focus() && !self.query.is_empty() {
            self.open = true;
        }
        if response.has_focus() {
            ui.painter().line_segment([response.rect.left_bottom(), response.rect.right_bottom()], Stroke::new(1.0, theme::accent(ui)));
        }

        let mut picked = None;
        // A single-line field loses the focus on Enter.
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            if self.lists && is_list(&self.query) {
                self.list = Some(std::mem::take(&mut self.query));
                self.results.clear();
                self.open = false;
                self.focus(ui);
            } else if let Some(&node) = self.results.get(self.highlighted).filter(|_| self.open) {
                picked = Some((Pick::Default, node));
            }
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) && (response.has_focus() || response.lost_focus()) {
            self.open = false;
        }

        if self.open && !self.query.trim().is_empty() {
            let area = Area::new(self.id.with("results"))
                .order(Order::Foreground)
                .fixed_pos(response.rect.left_bottom() + vec2(0.0, 2.0))
                .show(ui.ctx(), |ui| {
                    Frame::new().fill(theme::HEADER).stroke(Stroke::new(1.0, theme::accent(ui))).inner_margin(Margin::same(2)).show(
                        ui,
                        |ui| {
                            // The frame adds its margin and its stroke, so the list is as wide as the field.
                            ui.set_width(list_width(response.rect.width()) - LIST_FRAME);
                            if self.results.is_empty() {
                                ui.label(RichText::new("No system found").color(theme::TEXT_DIM));
                            }
                            for (i, node) in self.results.clone().into_iter().enumerate() {
                                if let Some(pick) = self.result_row(ui, uni, i, node, menu) {
                                    picked = Some((pick, node));
                                }
                            }
                        },
                    );
                });
            if raise {
                ui.ctx().move_to_top(area.response.layer_id);
            }
            // A click outside the box and the list closes the list.
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let clicked = ui.input(|i| i.pointer.any_click());
            let inside = pointer.is_some_and(|p| area.response.rect.contains(p) || response.rect.contains(p));
            if clicked && !inside && picked.is_none() {
                self.open = false;
            }
        }

        if picked.is_some() {
            self.query.clear();
            self.results.clear();
            self.open = false;
            // Keep the focus, so the next system can be typed at once.
            self.focus(ui);
        }
        picked
    }

    /// One result: the name, the security and the region.
    fn result_row(&mut self, ui: &mut Ui, uni: &Universe, i: usize, node: NodeIndex, menu: &[Pick]) -> Option<Pick> {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        if response.hovered() {
            self.highlighted = i;
        }
        let painter = ui.painter();
        if i == self.highlighted {
            painter.rect_filled(rect, 0.0, theme::ROW_FILL);
            theme::selection_bar(ui, rect);
        }
        let sys = uni.system(node);
        let font = FontId::proportional(13.0);
        let y = rect.center().y;
        painter.text(rect.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &sys.name, font.clone(), theme::TEXT);
        let sec = format!("{:.1}", display_sec(sys.security));
        painter.text(egui::pos2(rect.left() + 170.0, y), Align2::LEFT_CENTER, sec, font.clone(), theme::sec_color(sys.security));
        painter.text(egui::pos2(rect.left() + 220.0, y), Align2::LEFT_CENTER, &sys.region, font, theme::TEXT_DIM);

        let mut picked = response.clicked().then_some(Pick::Default);
        if !menu.is_empty() {
            response.context_menu(|ui| {
                ui.label(RichText::new(&sys.name).color(theme::accent(ui)));
                ui.separator();
                for &pick in menu {
                    if ui.button(pick.label()).clicked() {
                        picked = Some(pick);
                        ui.close();
                    }
                }
            });
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_as_wide_as_a_wide_field_and_fits_a_narrow_one() {
        assert_eq!(list_width(900.0), 900.0);
        assert_eq!(list_width(300.0), MIN_LIST_WIDTH);
    }
}
