//! Drawing of the active route, the character list, the login and the route start.

use super::app::{App, Popup};
use super::ui::{STOP_BG, centered, sec_color};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Table, Wrap};
use router_core::esi::active::Hop;
use router_core::esi::pilots::PilotView;
use router_core::labels::ship_text;
use router_core::universe::display_sec;

/// At most this many pilots show in a row of the step table. More give "+n".
const MAX_PILOTS: usize = 3;

/// "Alice Ander" gives "AA".
pub fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase()
}

/// "● Online · Jita". The word is always there, not only the color.
fn status_line(app: &App, row: &PilotView) -> Line<'static> {
    if row.live.expired {
        return Line::from("Login expired — Enter logs in again").yellow();
    }
    if row.needs_reauth {
        return Line::from("Re-authorize — Enter logs in again").yellow();
    }
    let mut spans = match row.live.online {
        Some(true) => vec![Span::raw("● Online").green()],
        Some(false) => vec![Span::raw("○ Offline").dark_gray()],
        None => vec![Span::raw("… Checking").dark_gray()],
    };
    if let Some(system) = row.live.system {
        spans.push(Span::raw(format!(" · {}", app.system_name(system))));
    }
    Line::from(spans)
}

/// `Apocalypse · "apoc"`, the line under the status line. An empty line while the ship is not known.
fn ship_line(row: &PilotView) -> Line<'static> {
    row.live.ship.as_ref().map_or_else(Line::default, |ship| Line::from(ship_text(ship, &row.name).to_string()).dark_gray())
}

/// The text of the banner line, and its color. The most urgent problem shows.
fn banner(app: &App) -> Option<(String, Color)> {
    let active = app.pilots.active.as_ref()?;
    let name = &active.character_name;
    let live = app.pilots.live.get(&active.character);
    if let Some(error) = app.pilots.send.as_ref().and_then(|s| s.failed.as_ref()) {
        return Some((format!("{error} r Retry"), Color::Yellow));
    }
    if live.is_some_and(|l| l.expired) {
        return Some((format!("Tracking paused — {name}'s login expired. l Log in again"), Color::Yellow));
    }
    if active.arrived() {
        let destination = app.system_name(active.steps.last()?.system);
        return Some((format!("Arrived at {destination}. Enter Done"), Color::Green));
    }
    if active.is_off_route() {
        let here = live.and_then(|l| l.system).map_or_else(|| "?".into(), |s| app.system_name(s));
        return Some((format!("Off route — {name} is in {here}. r Re-route from here · x Stop route"), Color::Yellow));
    }
    if app.pilots.limited {
        return Some(("ESI limited — slowing updates".into(), Color::DarkGray));
    }
    None
}

/// The active route: a header, a progress bar, a banner, and the step table.
pub fn draw_active(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(active) = app.pilots.active.clone() else { return };
    let banner = banner(app);
    let [header, gauge, banner_area, table_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(banner.is_some())),
        Constraint::Min(5),
    ])
    .areas(area);
    let destination = active.steps.last().map(|s| app.system_name(s.system)).unwrap_or_default();
    let jumps = active.jumps();
    let line = Line::from(vec![
        Span::raw(format!(" Route #{} → {destination} · ", active.number)),
        Span::raw(active.character_name.clone()).cyan().bold(),
        Span::raw(format!(" · {}/{jumps} jumps", active.progress)),
    ]);
    frame.render_widget(Paragraph::new(line), header);
    let ratio = if jumps == 0 { 1.0 } else { active.progress as f64 / jumps as f64 };
    frame.render_widget(Gauge::default().ratio(ratio.min(1.0)).gauge_style(Style::new().cyan().on_black()).label(""), gauge);
    if let Some((text, color)) = banner {
        frame.render_widget(Paragraph::new(text).fg(color).bold(), banner_area);
    }

    // The pilots of each system on the route.
    let pilots = app.pilots.characters();
    let pilots_at = |system: u32| -> String {
        let here: Vec<&PilotView> = pilots.iter().filter(|p| p.live.system == Some(system)).collect();
        let mut text: Vec<String> = here.iter().take(MAX_PILOTS).map(|p| initials(&p.name)).collect();
        if here.len() > MAX_PILOTS {
            text.push(format!("+{}", here.len() - MAX_PILOTS));
        }
        text.join(" ")
    };
    app.detail_page = usize::from(table_area.height.saturating_sub(3));
    let rows = active.steps.iter().enumerate().map(|(i, step)| {
        let node = app.uni.by_id.get(&step.system).copied();
        let (name, sec, region) = match node.map(|n| app.uni.system(n)) {
            Some(sys) => (sys.name.clone(), Some(sys.security), sys.region.clone()),
            None => (step.system.to_string(), None, String::new()),
        };
        // ✓ passed, ▶ current. The marks give the progress without color.
        let mark = match i.cmp(&active.progress) {
            std::cmp::Ordering::Less => "✓",
            std::cmp::Ordering::Equal => "▶",
            std::cmp::Ordering::Greater => " ",
        };
        let via = match step.hop {
            Hop::Wormhole => format!("{} — manual", step.via),
            Hop::Bridge => format!("{} — manual", step.via),
            _ => step.via.clone(),
        };
        let via_style = match step.hop {
            Hop::Wormhole => Style::new().magenta(),
            Hop::Bridge => Style::new().light_blue(),
            _ => Style::new().dark_gray(),
        };
        let stop = active.stop_at(i);
        let row = Row::new(vec![
            Cell::from(mark),
            Cell::from(i.to_string()).dark_gray(),
            Cell::from(stop.map(|s| s.label()).unwrap_or_default()).cyan(),
            Cell::from(name),
            sec.map_or_else(|| Cell::from(""), |s| Cell::from(format!("{:.1}", display_sec(s))).fg(sec_color(s))),
            Cell::from(pilots_at(step.system)).cyan(),
            Cell::from(region),
            Cell::from(via).style(via_style),
        ]);
        if i < active.progress {
            row.dark_gray()
        } else if i == active.progress {
            row.style(Style::new().bold().bg(STOP_BG))
        } else if stop.is_some() {
            row.bold()
        } else {
            row
        }
    });
    let widths = [
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Length(11),
        Constraint::Length(18),
        Constraint::Length(5),
        Constraint::Length(11),
        Constraint::Length(18),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .header(Row::new(["", "#", "Stop", "System", "Sec", "Pilots", "Region", "Via"]).add_modifier(Modifier::BOLD))
        .block(Block::bordered().title(format!(" Route #{} ", active.number)).border_style(Style::new().cyan()))
        .row_highlight_style(Style::new().reversed());
    frame.render_stateful_widget(table, table_area, &mut app.detail);
}

/// The overlay while the first send of a route runs or failed.
pub fn draw_sending(frame: &mut Frame, app: &App) {
    let Some(send) = &app.pilots.send else { return };
    let total = send.systems.len();
    let (title, lines) = match &send.failed {
        Some(error) => {
            (" Send failed ", vec![Line::from(error.clone()), Line::default(), Line::from(" r Retry   Esc Cancel ").dark_gray()])
        }
        None => (" Start route ", vec![Line::from(format!("Sending waypoints {}/{total}…", send.done + 1))]),
    };
    let area = centered(frame.area(), 64, lines.len() as u16 + 4);
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(title).border_style(Style::new().cyan());
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

/// A yes/no question.
fn question(frame: &mut Frame, title: &str, lines: Vec<Line<'static>>, keys: &str) {
    let mut lines = lines;
    lines.push(Line::default());
    lines.push(Line::from(keys.to_string()).dark_gray());
    let area = centered(frame.area(), 68, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(format!(" {title} ")).border_style(Style::new().cyan());
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

/// The popups of this module. `ui::draw` calls this for each of them.
pub fn draw_popup(frame: &mut Frame, app: &mut App) {
    let route_name =
        |app: &App| app.pilots.active.as_ref().map(|a| (a.number, a.steps.last().map(|s| app.system_name(s.system)).unwrap_or_default()));
    let mut popup = app.popup.take();
    match &mut popup {
        Some(Popup::Characters(state)) => {
            let rows = app.pilots.characters();
            let mut items: Vec<ListItem> = rows
                .iter()
                .map(|r| {
                    let name = if r.active { format!("▶ {}", r.name) } else { r.name.clone() };
                    ListItem::new(vec![Line::from(name).bold(), status_line(app, r), ship_line(r)])
                })
                .collect();
            if items.is_empty() {
                items.push(ListItem::new(Line::from("No characters. Press a to log in.").dark_gray()));
            }
            let store = if app.pilots.accounts.as_ref().is_some_and(|a| a.is_session_only()) { " · session only" } else { "" };
            let title = format!(" Characters ({}){store} ", rows.len());
            let area = centered(frame.area(), 64, (items.len() as u16 * 3).min(21) + 4);
            let [list_area, help] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
            let list = List::new(items)
                .block(Block::bordered().title(title).border_style(Style::new().cyan()))
                .highlight_style(Style::new().reversed())
                .highlight_symbol("> ");
            frame.render_widget(Clear, area);
            frame.render_stateful_widget(list, list_area, state);
            frame.render_widget(Paragraph::new(" a Add character   d Remove   Esc Close").dark_gray(), help);
        }
        Some(Popup::SessionOnly) => question(
            frame,
            "No system keyring",
            vec![
                Line::from(app.pilots.keyring_error.clone().unwrap_or_default()).dark_gray(),
                Line::from("No system keyring found. Keep login for this session only?"),
            ],
            "y Session only   n Cancel",
        ),
        Some(Popup::Login { paste }) => {
            let mut lines = vec![Line::from("Opening EVE login in your browser…")];
            if let Some(flow) = &app.pilots.login {
                lines.push(Line::default());
                lines.push(Line::from("Not opened? Copy this URL:").dark_gray());
                lines.push(Line::from(flow.login.url.clone()).cyan());
                if let Some(error) = flow.login.listen_error() {
                    lines.push(Line::default());
                    lines.push(Line::from(error.to_string()).yellow());
                }
                lines.push(Line::default());
                lines.push(Line::from("Paste the redirected URL here:").dark_gray());
                lines.push(Line::from(format!("{paste}▏")));
                if flow.busy() {
                    lines.push(Line::from("Logging in…").cyan());
                }
                if let Some(error) = &flow.error {
                    lines.push(Line::from(error.clone()).red());
                }
            }
            lines.push(Line::default());
            lines.push(Line::from("Enter Use the pasted URL   Esc Cancel").dark_gray());
            let area = centered(frame.area(), 76, 18);
            frame.render_widget(Clear, area);
            let block = Block::bordered().title(" EVE login ").border_style(Style::new().cyan());
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
        }
        Some(Popup::Pick { route, ids, state }) => {
            let rows = app.pilots.characters();
            let items: Vec<ListItem> = ids
                .iter()
                .filter_map(|id| rows.iter().find(|r| r.id == *id))
                .map(|r| ListItem::new(vec![Line::from(r.name.clone()).bold(), status_line(app, r), ship_line(r)]))
                .collect();
            let area = centered(frame.area(), 64, (items.len() as u16 * 3).min(21) + 2);
            let list = List::new(items)
                .block(Block::bordered().title(format!(" Send route #{} to… ", *route + 1)).border_style(Style::new().cyan()))
                .highlight_style(Style::new().reversed())
                .highlight_symbol("> ");
            frame.render_widget(Clear, area);
            frame.render_stateful_widget(list, area, state);
        }
        Some(Popup::Confirm(confirm)) => {
            let route = confirm.from_here.as_ref().unwrap_or(&confirm.planned);
            let name = &confirm.planned.character_name;
            let manual = route.manual_hops();
            let mut lines = vec![Line::from(format!(
                "{} jumps · {} waypoints · {manual} manual jump{}",
                route.jumps(),
                route.waypoint_count(),
                if manual == 1 { "" } else { "s" }
            ))];
            let here = confirm.here.map(|id| app.system_name(id));
            if let Some(here) = &here {
                lines.push(Line::default());
                lines.push(Line::from(format!("{name} is in {here}, not on this route.")).yellow());
            }
            if confirm.online == Some(false) {
                lines.push(Line::default());
                lines.push(Line::from(format!("{name} appears offline. Waypoints need the game client running.")).yellow());
            }
            let keys = match (&here, &confirm.from_here) {
                (Some(here), Some(_)) => format!("Enter Route from {here}   a Send as planned   Esc Cancel"),
                _ if confirm.online == Some(false) => "Enter Send anyway   Esc Cancel".into(),
                _ => "Enter Send   Esc Cancel".into(),
            };
            question(frame, &format!("Send route #{} to {name}?", confirm.planned.number), lines, &keys);
        }
        Some(Popup::RemovePilot(id)) => {
            let name = app.pilot_name(*id);
            question(
                frame,
                "Remove character",
                vec![Line::from(format!("Remove {name}? This revokes access and deletes stored tokens."))],
                "y Remove   n Cancel",
            );
        }
        Some(Popup::StopRoute) => {
            let number = route_name(app).map_or(0, |(n, _)| n);
            question(
                frame,
                "Stop route",
                vec![Line::from(format!("Stop route #{number}? In-game waypoints stay set; EVE does not allow clearing them from here."))],
                "y Stop route   n Keep going",
            );
        }
        Some(Popup::Quit) => {
            question(frame, "Quit", vec![Line::from("Quit? Route stays in game; resume on next launch.")], "y Quit   n Cancel")
        }
        Some(Popup::Resume) => {
            if let Some(r) = &app.pilots.resume {
                let destination = r.steps.last().map(|s| app.system_name(s.system)).unwrap_or_default();
                question(
                    frame,
                    "Resume",
                    vec![Line::from(format!("Resume route #{} to {destination} for {}?", r.number, r.character_name))],
                    "y Resume   n Discard",
                );
            }
        }
        Some(Popup::Reroute(route)) => {
            let here = route.steps.first().map(|s| app.system_name(s.system)).unwrap_or_default();
            question(
                frame,
                "Re-route",
                vec![Line::from(format!("New route from {here}: {} jumps. Replace in-game waypoints?", route.jumps()))],
                "y Replace   n Cancel",
            );
        }
        _ => {}
    }
    app.popup = popup;
}

/// True for a popup that `draw_popup` draws.
pub fn is_pilot_popup(popup: &Popup) -> bool {
    matches!(
        popup,
        Popup::Characters(_)
            | Popup::SessionOnly
            | Popup::Login { .. }
            | Popup::Pick { .. }
            | Popup::Confirm(_)
            | Popup::RemovePilot(_)
            | Popup::StopRoute
            | Popup::Quit
            | Popup::Resume
            | Popup::Reroute(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Alice Ander"), "AA");
        assert_eq!(initials("bob"), "B");
        assert_eq!(initials("Carl de la Vega"), "CD");
    }
}
