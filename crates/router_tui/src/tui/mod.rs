//! The terminal UI.

mod app;
mod pilots;
mod ui;
mod ui_pilots;

use app::App;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use router_core::config::{Config, RunOverrides};
use router_core::esi::pilots::Pilots;
use router_core::labels::Shortcuts;
use router_core::settings::Settings;
use router_core::universe::Universe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub fn run(
    uni: &Universe,
    settings: Settings,
    cfg: Config,
    cfg_path: PathBuf,
    input: String,
    shortcuts: Shortcuts,
    overrides: RunOverrides,
) -> Result<(), String> {
    let mut app = App::new(uni, settings, cfg, cfg_path.clone(), input, shortcuts);
    app.overrides = overrides;
    // The TUI reads the tracker each 250 ms, so it needs no wake.
    app.pilots = Pilots::open(&cfg_path, Arc::new(|| {}));
    app.on_open();
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        while !app.quit {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            // A blocking fetch runs after the draw, so the screen shows "Loading maps…" first.
            if app.load_maps_pending {
                app.load_maps();
                continue;
            }
            // Wait for a key at most 250 ms, then read the tracker and the login.
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                // Windows also sends key release events.
                && key.kind == KeyEventKind::Press
            {
                app.on_key(key);
            }
            app.tick();
        }
        Ok(())
    })();
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use router_core::ansiblex::{BridgeRules, find_hull};
    use router_core::route::Mode;

    #[test]
    fn draws_and_handles_keys() {
        let mut uni = Universe::from_sde(router_core::sde::load(&router_core::test_support::sde_dir()).unwrap());
        router_core::overlay::load_bridges(&mut uni, &router_core::test_support::fixture("ansiblex.txt")).unwrap();
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 3,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("black-ops"), max_cap: None },
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
            favourites: vec![uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()],
        };
        let mut app = App::new(
            &uni,
            settings,
            Config::default(),
            std::env::temp_dir().join("eve-router-test.json"),
            "Jita > UALX-3".into(),
            Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default()),
        );
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

    /// The text of each screen row, with no trailing spaces.
    fn screen_text(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        let rows: Vec<String> = (area.top()..area.bottom())
            .map(|y| (area.left()..area.right()).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim_end().to_string())
            .collect();
        rows.join(
            "
",
        ) + "
"
    }

    /// The first screen, with routes, the sidebar and the Shortcuts box.
    #[test]
    fn start_screen_snapshot() {
        let mut uni = Universe::from_sde(router_core::sde::load(&router_core::test_support::sde_dir()).unwrap());
        router_core::overlay::load_bridges(&mut uni, &router_core::test_support::fixture("ansiblex.txt")).unwrap();
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 3,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("black-ops"), max_cap: None },
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
            favourites: vec![uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()],
        };
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        let cfg_path = std::env::temp_dir().join("eve-router-test-snapshot.json");
        let mut app = App::new(&uni, settings, Config::default(), cfg_path, "Jita > UALX-3".into(), shortcuts);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = screen_text(terminal.backend().buffer());
        // Mask the search time in the title, for example "72.5 ms ──" gives "# ms ────".
        let end = text.find(" ms ").unwrap();
        let start = text[..end].trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').len();
        let text = format!("{}# ms {}{}", &text[..start], "─".repeat(end - start - 1), &text[end + 4..]);
        router_core::assert_snapshot!("start_screen", text);
    }

    /// The active route: the planner is hidden, and the table shows the progress.
    #[test]
    fn active_screen_snapshot() {
        use router_core::esi::active::ActiveRoute;
        let mut uni = Universe::from_sde(router_core::sde::load(&router_core::test_support::sde_dir()).unwrap());
        router_core::overlay::load_bridges(&mut uni, &router_core::test_support::fixture("ansiblex.txt")).unwrap();
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 1,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("black-ops"), max_cap: None },
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
            favourites: vec![uni.exact("Jita").unwrap()],
        };
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        let cfg_path = std::env::temp_dir().join("eve-router-test-active-snapshot.json");
        let mut app = App::new(&uni, settings, Config::default(), cfg_path, "Jita > UALX-3".into(), shortcuts);
        let mut route = ActiveRoute::new(&uni, &app.settings.rules, &app.routes[0], 1, 7, "Alice Ander", app.now);
        route.progress = 3;
        app.pilots.active = Some(route);
        app.tick();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = screen_text(terminal.backend().buffer());
        assert!(!text.contains("Route: Jita"), "the planner input is hidden");
        router_core::assert_snapshot!("active_screen", text);
    }

    #[test]
    fn active_screen_shows_a_pilot_one_time_on_a_loop_route() {
        use router_core::esi::active::ActiveRoute;
        let mut uni = Universe::from_sde(router_core::sde::load(&router_core::test_support::sde_dir()).unwrap());
        router_core::overlay::load_bridges(&mut uni, &router_core::test_support::fixture("ansiblex.txt")).unwrap();
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 1,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("black-ops"), max_cap: None },
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
            favourites: vec![uni.exact("Jita").unwrap()],
        };
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        let cfg_path = std::env::temp_dir().join("eve-router-test-loop-pilot.json");
        // A round trip: UALX-3 is the start and the destination.
        let mut app = App::new(&uni, settings, Config::default(), cfg_path, "UALX-3 > Y-ORBJ > UALX-3".into(), shortcuts);
        let ualx = uni.system(uni.exact("UALX-3").unwrap()).id;
        app.pilots.add_test_pilot(7, "Alice Ander", ualx);
        let route = ActiveRoute::new(&uni, &app.settings.rules, &app.routes[0], 1, 7, "Alice Ander", app.now);
        assert_eq!(route.steps.first().map(|s| s.system), route.steps.last().map(|s| s.system));
        app.pilots.active = Some(route);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = screen_text(terminal.backend().buffer());
        assert_eq!(text.matches(" AA ").count(), 1, "{text}");
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
        use ratatui::style::Color;
        use router_core::test_support::hole;
        use router_core::wormhole::{THERA, TURNUR};
        let mut uni = Universe::from_sde(router_core::sde::load(&router_core::test_support::sde_dir()).unwrap());
        uni.add_wormholes(&[hole(30000142, THERA), hole(30002187, THERA), hole(TURNUR, 30002053)]);
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 1,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules::default(),
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
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
        assert_eq!(app.settings.hubs, router_core::wormhole::Hubs { thera: false, turnur: true });
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

    #[test]
    fn mode_popup_toggles_optimize_order() {
        let uni = Universe::from_sde(router_core::sde::load(&router_core::test_support::sde_dir()).unwrap());
        let settings = Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 1,
            wormholes: false,
            hubs: Default::default(),
            bridges: false,
            rules: BridgeRules::default(),
            hull_source: router_core::settings::HullSource::Manual,
            min_life: 0,
            favourites: Vec::new(),
        };
        let input = "UALX-3 > Dodixie > UALX-3 > Jita > Turnur > Hek > Rens > Jita > C-J6MT > UALX-3";
        let shortcuts = Shortcuts::new(&uni, &Default::default(), &Default::default(), &Default::default());
        let cfg_path = std::env::temp_dir().join("eve-router-test-optimize.json");
        let mut app = App::new(&uni, settings, Config::default(), cfg_path, input.into(), shortcuts);
        let typed_jumps = app.routes[0].jumps;
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

        // The last row of the mode popup toggles the option, and the popup stays open.
        app.on_key(KeyEvent::from(KeyCode::Char('m')));
        for _ in 0..Mode::ALL.len() {
            app.on_key(KeyEvent::from(KeyCode::Down));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.settings.optimize);
        assert!(matches!(app.popup, Some(app::Popup::Mode(_))));
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("[x] Optimize order"));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.settings.mode, Mode::Shortest);

        // Each system gets one visit: the start, six midpoints and the destination.
        let route = &app.routes[0];
        assert!(route.jumps < typed_jumps, "{}", app.status);
        assert_eq!(route.stops.len(), 8);
        assert!(app.status.contains("Optimized order: UALX-3 > "), "{}", app.status);
        assert!(app.status.ends_with(" > UALX-3"), "{}", app.status);
    }
}
