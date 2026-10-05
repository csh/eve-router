//! The terminal UI.

mod app;
mod ui;

use crate::Settings;
use crate::ansiblex::BridgeRules;
use crate::config::Config;
use crate::route::Route;
use crate::universe::{Link, Universe};
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
    /// A problem with the overlay files, for the status line at startup.
    pub warning: Option<String>,
}

impl Shortcuts {
    pub fn new(uni: &Universe, report: &crate::overlay::OverlayReport) -> Self {
        let (wormholes, bridges) = uni.shortcut_counts();
        let warning = (!report.unknown.is_empty())
            .then(|| format!("Overlay: unknown systems: {}", report.unknown.join(", ")));
        Shortcuts { wormholes, bridges, skipped: report.skipped_broken + report.skipped_expired, warning }
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
pub fn link_label(uni: &Universe, rules: &BridgeRules, edge: EdgeIndex) -> String {
    match &uni.graph[edge] {
        Link::Stargate => "gate".into(),
        Link::Wormhole { sig_type, size, time } => {
            // A wormhole without a known type shows only "Wormhole".
            let name = sig_type.as_deref().map_or("Wormhole".to_string(), |t| format!("Wormhole {t}"));
            let mut parts = vec![name];
            parts.extend(size.as_deref().map(wormhole_size));
            parts.extend(time.as_deref().map(wormhole_time));
            parts.join(" · ")
        }
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

/// The game name for a nexum wormhole size, for example "large" gives "Large".
fn wormhole_size(size: &str) -> String {
    match size {
        "small" => "Small".into(),
        "medium" => "Medium".into(),
        "large" => "Large".into(),
        "xlarge" => "XL".into(),
        "capital" => "Capital".into(),
        // An unknown size shows as it is, so no information is lost.
        other => other.to_string(),
    }
}

/// The game text for a nexum time status, for example "lessThan1h" gives "Less than 1 hour remaining".
fn wormhole_time(time: &str) -> String {
    match time {
        "expired" => "Expired".into(),
        "lessThan1h" => "Less than 1 hour remaining".into(),
        other => match other.strip_prefix("lessThan").and_then(|t| t.strip_suffix('h')) {
            Some(hours) => format!("Less than {hours} hours remaining"),
            // An unknown status shows as it is, so no information is lost.
            None => other.to_string(),
        },
    }
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
        assert_eq!(wormhole_size("large"), "Large");
        assert_eq!(wormhole_size("small"), "Small");
        assert_eq!(wormhole_size("medium"), "Medium");
        assert_eq!(wormhole_size("xlarge"), "XL");
        assert_eq!(wormhole_size("capital"), "Capital");
        assert_eq!(wormhole_size("somethingNew"), "somethingNew");
        assert_eq!(wormhole_time("lessThan1h"), "Less than 1 hour remaining");
        assert_eq!(wormhole_time("lessThan4h"), "Less than 4 hours remaining");
        assert_eq!(wormhole_time("lessThan24h"), "Less than 24 hours remaining");
        assert_eq!(wormhole_time("expired"), "Expired");
        assert_eq!(wormhole_time("somethingNew"), "somethingNew");
    }

    #[test]
    fn draws_and_handles_keys() {
        let mut uni = Universe::from_sde(crate::sde::load(Path::new("sde")).unwrap());
        crate::overlay::load_bridges(&mut uni, Path::new("tests/fixtures/ansiblex.txt")).unwrap();
        let settings = Settings {
            mode: Mode::Shortest,
            top: 3,
            wormholes: true,
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("black-ops"), max_cap: None },
            favourites: vec![uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()],
        };
        let mut app = App::new(&uni, settings, Config::default(), std::env::temp_dir().join("eve-router-test.json"), "Jita > UALX-3".into(), Shortcuts::new(&uni, &Default::default()));
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
}
