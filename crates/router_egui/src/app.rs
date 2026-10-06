//! The window state, the startup load and the route search.

use router_core::config::{self, Config};
use router_core::labels::{jumps_label, route_extras};
use router_core::settings::{Settings, resolve_all, split_systems};
use router_core::startup::{self, Loaded};
use router_core::universe::Universe;
use router_core::wormhole;
use std::sync::mpsc::{self, Receiver};

/// The result of the startup load: the map, the settings and the status lines.
type Startup = Result<(Loaded, Settings, Vec<String>), String>;

enum State {
    /// The startup load runs on a thread, because it can take up to 35 s.
    Loading(Receiver<Startup>),
    Ready { loaded: Box<Loaded>, settings: Settings },
    Failed(String),
}

pub struct RouterApp {
    state: State,
    input: String,
    /// The route summaries of the last search, or its error.
    routes: Result<Vec<String>, String>,
    status: Vec<String>,
}

impl RouterApp {
    /// Start the load. `ctx` gets a repaint request when the load ends.
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load());
            ctx.request_repaint();
        });
        RouterApp { state: State::Loading(rx), input: "Jita > Amarr".into(), routes: Ok(Vec::new()), status: Vec::new() }
    }

    /// Take the load result, if the thread sent it.
    fn poll(&mut self) {
        let State::Loading(rx) = &self.state else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.state = match result {
            Ok((loaded, settings, status)) => {
                self.status = status;
                State::Ready { loaded: Box::new(loaded), settings }
            }
            Err(e) => State::Failed(e),
        };
        self.search();
    }

    fn search(&mut self) {
        if let State::Ready { loaded, settings } = &self.state {
            self.routes = route_summaries(&loaded.uni, settings, &self.input, wormhole::now());
        }
    }
}

/// The same load as the TUI, with the config file and the default SDE directory.
fn load() -> Startup {
    let cfg_path = config::default_path();
    let cfg = Config::load(&cfg_path)?;
    let pending = startup::begin(&cfg, &cfg_path);
    let sde_dir = config::default_sde_dir();
    let mut status = Vec::new();
    let loaded = startup::finish(pending, &sde_dir, None, |outcome| status.extend(outcome.message(&sde_dir)))?;
    status.extend(loaded.wh.warning.iter().chain(&loaded.scout.warning).cloned());
    let settings = Settings::from_config(&cfg, &loaded.uni)?;
    Ok((loaded, settings, status))
}

/// One line for each route, for example "#1 30 jumps (2 wormholes)".
pub fn route_summaries(uni: &Universe, settings: &Settings, input: &str, now: u64) -> Result<Vec<String>, String> {
    let nodes = resolve_all(uni, &split_systems(input))?;
    if nodes.len() < 2 {
        return Ok(Vec::new());
    }
    let router = settings.router(uni, now);
    let (nodes, _) = settings.order(&router, &nodes)?;
    let routes = router.routes(&nodes, settings.top)?;
    Ok(routes.iter().enumerate().map(|(i, r)| format!("#{} {}{}", i + 1, jumps_label(r.jumps), route_extras(r))).collect())
}

impl eframe::App for RouterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("EVE Router");
            match &self.state {
                State::Loading(_) => {
                    ui.label("Loading…");
                    return;
                }
                State::Failed(e) => {
                    ui.colored_label(egui::Color32::RED, e);
                    return;
                }
                State::Ready { .. } => {}
            }
            let response = ui.horizontal(|ui| {
                ui.label("Route:");
                ui.text_edit_singleline(&mut self.input)
            });
            if response.inner.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.search();
            }
            match &self.routes {
                Ok(lines) => lines.iter().for_each(|line| _ = ui.label(line)),
                Err(e) => _ = ui.colored_label(egui::Color32::RED, e),
            }
            for line in &self.status {
                ui.small(line);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::test_support::{FIXTURE_TIME, overlay_universe, settings};

    /// The same routes as the `--print` snapshot of the TUI crate.
    #[test]
    fn summaries_match_the_print_output() {
        let uni = overlay_universe();
        let lines = route_summaries(&uni, &settings(&uni, Some("black-ops")), "UALX-3 > Jita", FIXTURE_TIME).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "#1 30 jumps");
        assert_eq!(lines[1], "#2 30 jumps (2 wormholes)");
        assert_eq!(route_summaries(&uni, &settings(&uni, None), "Jita", FIXTURE_TIME).unwrap(), Vec::<String>::new());
        assert!(route_summaries(&uni, &settings(&uni, None), "Jita > Nowhere", FIXTURE_TIME).is_err());
    }
}
