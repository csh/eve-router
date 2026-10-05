//! The terminal UI.

mod app;
mod ui;

use crate::Settings;
use crate::ansiblex::BridgeRules;
use crate::config::Config;
use crate::overlay::OverlayReport;
use crate::route::Route;
use crate::sources::{evescout, nexum};
use crate::universe::{Link, Universe};
use crate::wormhole::{MassStatus, SourceId, THERA, TURNUR, Wormhole, expiry_text};
use app::App;
use petgraph::graph::EdgeIndex;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;

pub fn run(
    uni: &Universe,
    settings: Settings,
    cfg: Config,
    cfg_path: PathBuf,
    input: String,
    shortcuts: Shortcuts,
) -> Result<(), String> {
    let mut app = App::new(uni, settings, cfg, cfg_path, input, shortcuts);
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        while !app.quit {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            // A blocking fetch runs after the draw, so the screen shows "Loading maps…" first.
            if app.load_maps_pending {
                app.load_maps();
                continue;
            }
            if let Event::Key(key) = event::read()? {
                // Windows also sends key release events.
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

/// The overlay counts for the "Shortcuts" sidebar box.
pub struct Shortcuts {
    pub wormholes: usize,
    pub bridges: usize,
    /// Broken or expired wormholes that the router did not load.
    pub skipped: usize,
    /// The wormholes with an end in Thera, and in Turnur.
    pub thera: usize,
    pub turnur: usize,
    /// The source and the fetch time of each set of wormhole data.
    pub sources: Vec<(SourceId, u64)>,
    /// A problem with the overlay data, for the status line at startup.
    pub warning: Option<String>,
}

impl Shortcuts {
    pub fn new(uni: &Universe, report: &OverlayReport, wh: &nexum::Load, scout: &evescout::Load) -> Self {
        let (wormholes, bridges) = uni.shortcut_counts();
        let mut warnings = Vec::new();
        if !report.unknown.is_empty() {
            warnings.push(format!("Bridges: unknown systems: {}", report.unknown.join(", ")));
        }
        for (source, unknown) in [(SourceId::Nexum, &wh.report.unknown), (SourceId::EveScout, &scout.report.unknown)] {
            if !unknown.is_empty() {
                let ids: Vec<String> = unknown.iter().map(u32::to_string).collect();
                warnings.push(format!("{}: unknown system IDs: {}", source.label(), ids.join(", ")));
            }
        }
        warnings.extend(wh.warning.clone());
        warnings.extend(scout.warning.clone());
        Shortcuts {
            wormholes,
            bridges,
            skipped: wh.report.skipped() + scout.report.skipped(),
            thera: uni.hub_count(THERA),
            turnur: uni.hub_count(TURNUR),
            sources: wh.data.iter().chain(&scout.data).map(|d| (d.source, d.fetched_at)).collect(),
            warning: (!warnings.is_empty()).then(|| warnings.join(" · ")),
        }
    }
}

/// "1 jump" or "N jumps".
pub fn jumps_label(jumps: usize) -> String {
    if jumps == 1 { "1 jump".into() } else { format!("{jumps} jumps") }
}

/// The overlay part of a route summary, for example " (2 wormholes, 1 jump bridge, 36 TJ)".
pub fn route_extras(route: &Route) -> String {
    let mut parts = Vec::new();
    if route.wormholes > 0 {
        let s = if route.wormholes == 1 { "" } else { "s" };
        parts.push(format!("{} wormhole{s}", route.wormholes));
    }
    if route.bridges > 0 {
        let s = if route.bridges == 1 { "" } else { "s" };
        parts.push(format!("{} jump bridge{s}", route.bridges));
    }
    if let Some(tj) = route.bridge_tj {
        parts.push(format!("{tj} TJ"));
    }
    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) }
}

/// How a route enters a system: a gate, a wormhole or a bridge.
pub fn link_label(uni: &Universe, rules: &BridgeRules, edge: EdgeIndex, now: u64) -> String {
    match &uni.graph[edge] {
        Link::Stargate => "gate".into(),
        Link::Wormhole(w) => wormhole_label(w, now),
        Link::JumpBridge => {
            // "Ansiblex · Zone 1 → 2 · 36 TJ". The departure zone sets the cost.
            let (from, to) = uni.graph.edge_endpoints(edge).unwrap();
            let mut parts = vec!["Ansiblex".to_string()];
            if let (Some(a), Some(b)) = (rules.cost(uni, from), rules.cost(uni, to)) {
                parts.push(format!("Zone {} → {}", a.zone, b.zone));
                parts.extend(a.tj.map(|tj| format!("{tj} TJ")));
            }
            parts.join(" · ")
        }
    }
}

/// For example "Wormhole K162 · Large · Less than 3h 10m remaining · Critical".
/// A stable mass shows no text.
pub fn wormhole_label(w: &Wormhole, now: u64) -> String {
    let mut parts = vec![w.wh_type.as_deref().map_or("Wormhole".to_string(), |t| format!("Wormhole {t}"))];
    parts.extend(w.size.map(|s| s.label().to_string()));
    parts.extend(w.expiry.map(|e| expiry_text(e, now)));
    parts.extend(w.mass.filter(|&m| m != MassStatus::Stable).map(|m| m.label().to_string()));
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansiblex::find_hull;
    use crate::route::Mode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use std::path::Path;

    #[test]
    fn wormhole_text() {
        use crate::wormhole::{Expiry, MassStatus, Size, Wormhole, tests::hole};
        let now = 1_000_000;
        let w = Wormhole {
            wh_type: Some("K162".into()),
            size: Some(Size::Large),
            expiry: Some(Expiry { at: now + 3 * 3600 + 600, exact: false }),
            mass: Some(MassStatus::Critical),
            ..hole(1, 2)
        };
        assert_eq!(wormhole_label(&w, now), "Wormhole K162 · Large · Less than 3h 10m remaining · Critical");
        // A stable mass shows no text. An unknown type shows only "Wormhole".
        let stable = Wormhole { mass: Some(MassStatus::Stable), size: Some(Size::XLarge), ..hole(1, 2) };
        assert_eq!(wormhole_label(&stable, now), "Wormhole · XL");
        let destabilized = Wormhole { mass: Some(MassStatus::Destabilized), ..hole(1, 2) };
        assert_eq!(wormhole_label(&destabilized, now), "Wormhole · Destabilized");
    }

    #[test]
    fn draws_and_handles_keys() {
        let mut uni = Universe::from_sde(crate::sde::load(Path::new("sde")).unwrap());
        crate::overlay::load_bridges(&mut uni, Path::new("tests/fixtures/ansiblex.txt")).unwrap();
        let settings = Settings {
            mode: Mode::Shortest,
            top: 3,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("black-ops"), max_cap: None },
            favourites: vec![uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()],
        };
        let mut app = App::new(&uni, settings, Config::default(), std::env::temp_dir().join("eve-router-test.json"), "Jita > UALX-3".into(), Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default()));
        assert_eq!(app.routes.len(), 3, "{}", app.status);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let keys = [
            // Open the settings page, remove Amarr, and add Rens.
            KeyCode::Char('s'),
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Char('d'),
            KeyCode::Char('a'),
            KeyCode::Char('R'),
            KeyCode::Char('e'),
            KeyCode::Char('n'),
            KeyCode::Char('s'),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Down,
            KeyCode::Char('m'),
            KeyCode::Down,
            KeyCode::Enter,
            // Filter the hull picker to the Paladin and select it.
            KeyCode::Char('h'),
            KeyCode::Char('p'),
            KeyCode::Char('a'),
            KeyCode::Char('l'),
            KeyCode::Char('a'),
            KeyCode::Char('d'),
            KeyCode::Enter,
            // Open the max TJ prompt from the settings page.
            KeyCode::Char('s'),
            KeyCode::Down,
            KeyCode::Enter,
        ];
        for key in keys {
            app.on_key(KeyEvent::from(key));
            terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        }
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("Shortest route"));
        assert!(screen.contains("Max TJ"));
        assert!(app.route_time.is_some());
        let favs: Vec<&str> = app.settings.favourites.iter().map(|&n| uni.name(n)).collect();
        assert_eq!(favs, ["Jita", "Rens"]);
        assert!(screen.contains("Save and close"));
        assert_eq!(app.shortcuts.bridges, 3);
        assert!(screen.contains("Shortcuts"));

        // Enter opens the step table, and n scrolls to the midpoint at step 30 or more.
        app.on_key(KeyEvent::from(KeyCode::Esc));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        app.input = "Jita > Amarr > UALX-3".into();
        app.recompute();
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.focus == app::Focus::Detail);
        app.on_key(KeyEvent::from(KeyCode::Char('n')));
        let midpoint = app.selected_route().unwrap().stops[1];
        assert!(midpoint > 30);
        assert_eq!(app.detail.selected(), Some(midpoint));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("Midpoint 1"));
        app.on_key(KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(app.detail.selected(), Some(0));
        app.on_key(KeyEvent::from(KeyCode::End));
        assert_eq!(app.detail.selected(), Some(app.selected_route().unwrap().jumps));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.focus == app::Focus::Routes);

        // Without any overlay connection, the box does not show.
        app.shortcuts.bridges = 0;
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(!format!("{:?}", terminal.backend().buffer()).contains("Shortcuts"));
        assert_eq!(app.settings.mode, Mode::PreferHighsec);
        assert_eq!(app.settings.rules.hull.unwrap().name, "Paladin");
    }

    /// The foreground color of the first cell of the screen row that starts with `text`.
    fn row_color(buffer: &ratatui::buffer::Buffer, text: &str) -> Option<ratatui::style::Color> {
        let area = buffer.area;
        (area.top()..area.bottom()).find_map(|y| {
            let row: String = (area.left()..area.right()).map(|x| buffer[(x, y)].symbol()).collect();
            let x = row.find(text)?;
            // The row holds only one-byte symbols before the match in the sidebar box.
            Some(buffer[(area.left() + row[..x].chars().count() as u16, y)].fg)
        })
    }

    #[test]
    fn hub_counts_and_switches() {
        use crate::wormhole::{THERA, TURNUR, tests::hole};
        use ratatui::style::Color;
        let mut uni = Universe::from_sde(crate::sde::load(Path::new("sde")).unwrap());
        uni.add_wormholes(&[hole(30000142, THERA), hole(30002187, THERA), hole(TURNUR, 30002053)]);
        let settings = Settings {
            mode: Mode::Shortest,
            top: 1,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules::default(),
            favourites: Vec::new(),
        };
        let cfg_path = std::env::temp_dir().join("eve-router-test-hubs").join("eve-router.json");
        let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        assert_eq!((shortcuts.wormholes, shortcuts.thera, shortcuts.turnur), (3, 2, 1));
        let mut app = App::new(&uni, settings, Config::default(), cfg_path.clone(), String::new(), shortcuts);
        // The s and w keys work outside the input box.
        app.focus = app::Focus::Routes;
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let thera_line = format!("{:<16}{:>5}", "  Thera:", 2);
        let turnur_line = format!("{:<16}{:>5}", "  Turnur:", 1);
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains(&thera_line) && screen.contains(&turnur_line), "{screen}");
        assert_ne!(row_color(terminal.backend().buffer(), &thera_line), Some(Color::DarkGray));

        // Enter on the Thera row of the settings page turns Thera off, and saves the config.
        let rows = app.settings_rows();
        let thera_row = rows.iter().position(|r| *r == app::SettingsRow::Thera).unwrap();
        assert_eq!(rows[thera_row + 1], app::SettingsRow::Turnur);
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        app.settings_page.as_mut().unwrap().select(Some(thera_row));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.settings.hubs, crate::wormhole::Hubs { thera: false, turnur: true });
        assert_eq!(Config::load(&cfg_path).unwrap().eve_scout, app.settings.hubs);
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("EVE-Scout") && screen.contains("Thera: off"), "{screen}");
        app.on_key(KeyEvent::from(KeyCode::Esc));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(row_color(buffer, &thera_line), Some(Color::DarkGray));
        assert_ne!(row_color(buffer, &turnur_line), Some(Color::DarkGray));
        // With all wormholes off, the hub lines are gray too.
        app.on_key(KeyEvent::from(KeyCode::Char('w')));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert_eq!(row_color(terminal.backend().buffer(), &turnur_line), Some(Color::DarkGray));
        std::fs::remove_dir_all(cfg_path.parent().unwrap()).unwrap();
    }
}
