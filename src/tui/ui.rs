//! Drawing.

use super::app::{App, Focus, Popup, PromptKind, SettingsRow, hull_rows};
use super::{jumps_label, link_label, route_extras};
use crate::route::Mode;
use crate::universe::display_sec;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, List, ListItem, Paragraph, Row, Table};

/// The in-game security colors.
pub fn sec_color(security: f64) -> Color {
    let rgb = match (display_sec(security) * 10.0).round() as i32 {
        10.. => 0x2FEFEF,
        9 => 0x48F0C0,
        8 => 0x00EF47,
        7 => 0x00F000,
        6 => 0x8FEF2F,
        5 => 0xEFEF00,
        4 => 0xD77700,
        3 => 0xF06000,
        2 => 0xF04800,
        1 => 0xD73000,
        _ => 0xF00000,
    };
    Color::from_u32(rgb)
}

/// The background of a stop row in the route table.
const STOP_BG: Color = Color::Indexed(236);

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, status, help] =
        Layout::vertical([Constraint::Min(10), Constraint::Length(1), Constraint::Length(2)]).areas(frame.area());
    let [left, sidebar] = Layout::horizontal([Constraint::Min(40), Constraint::Length(24)]).areas(main);
    let [input, list, detail] =
        Layout::vertical([Constraint::Length(3), Constraint::Percentage(30), Constraint::Min(5)]).areas(left);

    draw_input(frame, app, input);
    draw_routes(frame, app, list);
    draw_detail(frame, app, detail);
    // The "Shortcuts" box goes above the hubs, and only when an overlay loaded a connection.
    if app.shortcuts.wormholes + app.shortcuts.bridges > 0 {
        let shortcut_lines = 2 + u16::from(app.shortcuts.skipped > 0);
        let [shortcuts, hubs] = Layout::vertical([Constraint::Length(shortcut_lines + 2), Constraint::Min(5)]).areas(sidebar);
        draw_shortcuts(frame, app, shortcuts);
        draw_hubs(frame, app, hubs);
    } else {
        draw_hubs(frame, app, sidebar);
    }

    frame.render_widget(Paragraph::new(app.status.as_str()).fg(Color::Yellow), status);
    frame.render_widget(Paragraph::new(help_lines(app, help.width)), help);

    if app.settings_page.is_some() {
        draw_settings(frame, app);
    }

    match &mut app.popup {
        Some(Popup::Mode(state)) => {
            let area = centered(frame.area(), 56, Mode::ALL.len() as u16 * 2 + 2);
            let items: Vec<ListItem> = Mode::ALL
                .iter()
                .map(|m| ListItem::new(vec![Line::from(m.title()).bold(), Line::from(format!("  {}", m.description())).dark_gray()]))
                .collect();
            let list = List::new(items)
                .block(Block::bordered().title(" Safety mode "))
                .highlight_style(Style::new().reversed())
                .highlight_symbol("> ");
            frame.render_widget(Clear, area);
            frame.render_stateful_widget(list, area, state);
        }
        Some(Popup::Hull { filter, state }) => {
            let area = centered(frame.area(), 74, 22);
            let rows = hull_rows(filter);
            let items: Vec<ListItem> = rows
                .iter()
                .map(|row| match row {
                    None => ListItem::new("none"),
                    Some(h) => match h.base_tj {
                        Some(tj) => ListItem::new(format!("{:<26}{:<28}{tj:>7} TJ", h.name, h.group)),
                        None => ListItem::new(format!("{:<26}{:<28}{:>10}", h.name, h.group, "no JB")).dark_gray(),
                    },
                })
                .collect();
            let title = format!(" Hull ({} ships) ", rows.len() - 1);
            let block = Block::bordered().title(title).border_style(Style::new().cyan());
            let inner = block.inner(area);
            let [search, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            let list = List::new(items).highlight_style(Style::new().reversed()).highlight_symbol("> ");
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            let search_line = Line::from(vec!["Filter: ".dark_gray(), Span::raw(filter.clone()), Span::raw("▏")]);
            frame.render_widget(Paragraph::new(search_line), search);
            frame.render_stateful_widget(list, list_area, state);
        }
        Some(Popup::Prompt { kind, text }) => {
            let area = centered(frame.area(), 50, 3);
            let title = match kind {
                PromptKind::Capital => " Alliance capital (Tab completes, empty = none) ",
                PromptKind::MaxCap => " Max TJ for one bridge jump (empty = no limit) ",
                PromptKind::Favourite => " Add favourite (Tab completes) ",
            };
            frame.render_widget(Clear, area);
            frame.render_widget(Paragraph::new(format!("{text}▏")).block(Block::bordered().title(title)), area);
        }
        None => {}
    }
}

/// The key hints. Each hint is the key, its action, and the current value where one exists.
/// A line breaks only between two hints, so a key stays next to its label.
fn help_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let s = &app.settings;
    let bridges = match s.rules.blocked_reason() {
        Some(_) => "off (no capital)".to_string(),
        None => on_off(s.bridges).to_string(),
    };
    let hints: Vec<(&str, String)> = match app.focus {
        _ if app.settings_page.is_some() => vec![
            ("↑↓", "Select".into()),
            ("Enter", "Edit".into()),
            ("a", "Add favourite".into()),
            ("d", "Remove favourite".into()),
            ("Shift+↑↓", "Move favourite".into()),
            ("Esc", "Save and close".into()),
        ],
        Focus::Input => vec![
            ("Enter", "Find routes".into()),
            ("Tab", "Complete system name".into()),
            ("Esc", "Leave the input".into()),
        ],
        Focus::Detail => vec![
            ("↑↓", "Move".into()),
            ("PgUp/PgDn", "Page".into()),
            ("Home/End", "Start/Destination".into()),
            ("n/p", "Next/previous stop".into()),
            ("Esc", "Back to routes".into()),
        ],
        Focus::Routes => vec![
            ("i", "Edit route".into()),
            ("↑↓", "Select route".into()),
            ("Enter", "Open route".into()),
            ("m", format!("Safety mode: {}", s.mode.title())),
            ("w", format!("Wormholes: {}", on_off(s.wormholes))),
            ("j", format!("Jump bridges: {bridges}")),
            ("h", format!("Hull: {}", s.rules.hull.map_or("none".into(), hull_label))),
            ("+/-", format!("Routes: {}", s.top)),
            ("s", "Settings".into()),
            ("q", "Quit".into()),
        ],
    };
    const GAP: &str = "   ";
    let mut lines = vec![Line::default()];
    for (key, label) in hints {
        let key = format!(" {key} ");
        let label = format!(" {label}");
        let hint_width = key.chars().count() + label.chars().count();
        let line = lines.last_mut().unwrap();
        if line.width() > 0 && line.width() + GAP.len() + hint_width > width as usize {
            lines.push(Line::default());
        } else if line.width() > 0 {
            line.push_span(Span::raw(GAP));
        }
        let line = lines.last_mut().unwrap();
        line.push_span(Span::styled(key, Style::new().black().on_cyan()));
        line.push_span(Span::raw(label).gray());
    }
    lines
}

/// A ship shows its group, for example "Sin (Black Ops)". A group shows only its name.
fn hull_label(h: crate::ansiblex::HullClass) -> String {
    match h.type_id {
        Some(_) => format!("{} ({})", h.name, h.group),
        None => h.name.clone(),
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
    area
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let build = app.uni.build.map_or(String::new(), |b| format!(" · SDE {b}"));
    let time = app.route_time.map_or(String::new(), |t| format!(" · {:.1} ms", t.as_secs_f64() * 1000.0));
    let title = format!(" EVE Router{build}{time} ");
    let mut block = Block::bordered().title(title);
    if app.focus == Focus::Input {
        block = block.border_style(Style::new().cyan());
    }
    let cursor = if app.focus == Focus::Input { "▏" } else { "" };
    let text = Line::from(vec!["Route: ".dark_gray(), Span::raw(&app.input), Span::raw(cursor)]);
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_routes(frame: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .routes
        .iter()
        .enumerate()
        .map(|(i, r)| ListItem::new(format!("#{} {}{}", i + 1, jumps_label(r.jumps), route_extras(r))))
        .collect();
    let title = format!(" Routes ({}) ", app.routes.len());
    let mut block = Block::bordered().title(title);
    if app.focus == Focus::Routes {
        block = block.border_style(Style::new().cyan());
    }
    let list = List::new(items).block(block).highlight_style(Style::new().reversed()).highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut app.selected);
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    // The borders and the header use 3 rows.
    app.detail_page = usize::from(area.height.saturating_sub(3));
    let Some(route) = app.selected_route() else {
        frame.render_widget(Block::bordered().title(" Detail "), area);
        return;
    };
    let uni = app.uni;
    let rows = route.path.nodes.iter().enumerate().map(|(step, &node)| {
        let sys = uni.system(node);
        let via = match step.checked_sub(1).map(|s| route.path.edges[s]) {
            Some(e) => link_label(uni, &app.settings.rules, e, app.now),
            None => String::new(),
        };
        let via_style = match via.as_str() {
            v if v.starts_with("Wormhole") => Style::new().magenta(),
            v if v.starts_with("Ansiblex") => Style::new().light_blue(),
            _ => Style::new().dark_gray(),
        };
        let stop = route.stop_at(step);
        let row = Row::new(vec![
            Cell::from(step.to_string()).dark_gray(),
            Cell::from(stop.map(|s| s.label()).unwrap_or_default()).cyan(),
            Cell::from(sys.name.clone()),
            Cell::from(format!("{:.1}", display_sec(sys.security))).fg(sec_color(sys.security)),
            Cell::from(sys.region.clone()),
            Cell::from(via).style(via_style),
        ]);
        // The start, the midpoints and the destination stand out from the other steps.
        if stop.is_some() { row.style(Style::new().bold().bg(STOP_BG)) } else { row }
    });
    let widths = [
        Constraint::Length(4),
        Constraint::Length(11),
        Constraint::Length(18),
        Constraint::Length(5),
        Constraint::Length(20),
        Constraint::Min(10),
    ];
    let index = app.selected.selected().unwrap_or(0) + 1;
    let focused = app.focus == Focus::Detail;
    let mut block = Block::bordered().title(format!(" Route #{index} "));
    if focused {
        block = block.border_style(Style::new().cyan());
    }
    let table = Table::new(rows, widths)
        .column_spacing(3)
        .header(Row::new(["#", "Stop", "System", "Sec", "Region", "Via"]).add_modifier(Modifier::BOLD))
        .block(block)
        // The selected step shows only while the table has the focus.
        .row_highlight_style(if focused { Style::new().reversed() } else { Style::new() });
    frame.render_stateful_widget(table, area, &mut app.detail);
}

fn draw_hubs(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![];
    match &app.hub_origin {
        Some(origin) => lines.push(Line::from(format!("from {origin}")).dark_gray()),
        None => lines.push(Line::from("Give a system").dark_gray()),
    }
    lines.push(Line::default());
    for &(target, jumps) in &app.hubs {
        let jumps = jumps.map_or("-".into(), |j| j.to_string());
        lines.push(Line::from(format!("{:<16}{jumps:>5}", app.uni.name(target))));
    }
    if app.settings.favourites.is_empty() {
        lines.push(Line::from("No favourites.").dark_gray());
        lines.push(Line::from("Press s to add one.").dark_gray());
    }
    let block = Block::bordered().title(" Shortest route ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_shortcuts(frame: &mut Frame, app: &App, area: Rect) {
    let s = &app.settings;
    let bridges_on = s.bridges && s.rules.blocked_reason().is_none();
    // A kind that is off for routing shows in gray.
    let line = |label: &str, count: usize, on: bool| {
        let line = Line::from(format!("{label:<16}{count:>5}"));
        if on { line } else { line.dark_gray() }
    };
    let mut lines = vec![
        line("Wormholes:", app.shortcuts.wormholes, s.wormholes),
        line("Jump bridges:", app.shortcuts.bridges, bridges_on),
    ];
    if app.shortcuts.skipped > 0 {
        lines.push(Line::from(format!("{:<16}{:>5}", "Skipped:", app.shortcuts.skipped)).dark_gray());
    }
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title(" Shortcuts ")), area);
}

fn draw_settings(frame: &mut Frame, app: &mut App) {
    let rows = app.settings_rows();
    let rules = &app.settings.rules;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let (label, value) = match *row {
                SettingsRow::Capital => {
                    ("Alliance capital", rules.capital.map_or("none (jump bridges off)".into(), |n| app.uni.name(n).to_string()))
                }
                SettingsRow::MaxCap => {
                    ("Max TJ per bridge jump", rules.max_cap.map_or("no limit".into(), |m| format!("{m} TJ")))
                }
                SettingsRow::Favourite(i) => {
                    (if i == 0 { "Favourites" } else { "" }, app.uni.name(app.settings.favourites[i]).to_string())
                }
                SettingsRow::AddFavourite => {
                    let label = if app.settings.favourites.is_empty() { "Favourites" } else { "" };
                    return ListItem::new(Line::from(vec![
                        Span::raw(format!("{label:<24}")).dark_gray(),
                        Span::raw("+ Add favourite").cyan(),
                    ]));
                }
            };
            ListItem::new(Line::from(vec![Span::raw(format!("{label:<24}")).dark_gray(), Span::raw(value)]))
        })
        .collect();
    let area = centered(frame.area(), 60, rows.len() as u16 + 2);
    let list = List::new(items)
        .block(Block::bordered().title(" Settings ").border_style(Style::new().cyan()))
        .highlight_style(Style::new().reversed())
        .highlight_symbol("> ");
    frame.render_widget(Clear, area);
    if let Some(state) = &mut app.settings_page {
        frame.render_stateful_widget(list, area, state);
    }
}
