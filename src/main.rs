mod ansiblex;
mod config;
mod overlay;
mod route;
mod sde;
mod sde_update;
mod ships;
mod tui;
mod universe;

use ansiblex::{BridgeRules, find_hull};
use clap::Parser;
use config::Config;
use overlay::OverlayReport;
use petgraph::graph::NodeIndex;
use route::{Mode, Router};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;
use universe::{Universe, display_sec};

/// Find the top-n routes between EVE Online solar systems.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The origin, the waypoints and the destination. Split them with spaces, ">" or ",".
    systems: Vec<String>,
    /// The SDE directory. The default is sde in the platform data directory, for example
    /// %LOCALAPPDATA%\com.smrkn.eve-router\sde on Windows. The router keeps it up to date.
    /// Set EVE_ROUTER_SKIP_SDE_CHECK=1 to skip the update check.
    #[arg(long)]
    sde: Option<PathBuf>,
    /// The config file. The default is eve-router.json in the platform config
    /// directory, for example %APPDATA%\com.smrkn.eve-router on Windows.
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, value_enum)]
    mode: Option<Mode>,
    /// The number of routes.
    #[arg(short = 'n', long)]
    top: Option<usize>,
    /// The alliance capital system. Jump bridges need it.
    #[arg(long)]
    capital: Option<String>,
    /// The ship ("Sin") or ship group ("black-ops"). It sets the bridge capacitor cost.
    #[arg(long)]
    hull: Option<String>,
    /// The maximum capacitor (TJ) for one bridge jump.
    #[arg(long)]
    max_cap: Option<f32>,
    /// A jump bridge list in SMT format. The default is ansiblex.txt in the config directory.
    #[arg(long)]
    bridges: Option<PathBuf>,
    /// A nexum map export with wormholes. The default is nexum.json in the config directory.
    #[arg(long)]
    nexum: Option<PathBuf>,
    /// Print the routes and exit. Do not start the TUI.
    #[arg(long)]
    print: bool,
}

/// The settings that the TUI can change.
pub struct Settings {
    pub mode: Mode,
    pub top: usize,
    pub wormholes: bool,
    pub bridges: bool,
    pub rules: BridgeRules,
    /// The sidebar destinations.
    pub favourites: Vec<NodeIndex>,
}

impl Settings {
    pub fn router<'a>(&self, uni: &'a Universe) -> Router<'a> {
        let bridges = self.bridges && self.rules.blocked_reason().is_none();
        Router::new(uni, self.mode, self.wormholes, bridges, self.rules)
    }
}

/// Split the system arguments. A name can hold a space, for example "New Caldari".
pub fn split_systems(input: &str) -> Vec<String> {
    input
        .split(['>', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

pub fn resolve_all(uni: &Universe, names: &[String]) -> Result<Vec<NodeIndex>, String> {
    names
        .iter()
        .map(|name| {
            uni.resolve(name).map_err(|candidates| match candidates.as_slice() {
                [] => format!("Unknown system \"{name}\""),
                c => format!("\"{name}\" matches more than one system: {}", c.join(", ")),
            })
        })
        .collect()
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let cfg_path = cli.config.unwrap_or_else(config::default_path);
    let mut cfg = Config::load(&cfg_path)?;
    // The CLI flags override the config file.
    cfg.capital = cli.capital.or(cfg.capital);
    cfg.hull = cli.hull.or(cfg.hull);
    cfg.max_cap_tj = cli.max_cap.or(cfg.max_cap_tj);
    cfg.mode = cli.mode.or(cfg.mode);
    cfg.top = cli.top.or(cfg.top);

    let sde_dir = cli.sde.unwrap_or_else(config::default_sde_dir);
    update_sde(&sde_dir)?;
    ansiblex::init(&sde_dir)?;

    let started = Instant::now();
    let mut uni = Universe::from_sde(sde::load(&sde_dir)?);
    let bridges_path = config::overlay_path(cli.bridges, &cfg_path, config::BRIDGES_FILE);
    let nexum_path = config::overlay_path(cli.nexum, &cfg_path, config::NEXUM_FILE);
    let mut report = OverlayReport::default();
    if let Some(path) = &nexum_path {
        // The bridge list is authoritative. Use the nexum jump bridges only without it.
        report = overlay::load_nexum(&mut uni, path, bridges_path.is_none())?;
    }
    if let Some(path) = &bridges_path {
        let r = overlay::load_bridges(&mut uni, path)?;
        report.bridges += r.bridges;
        report.unknown.extend(r.unknown);
    }
    let load_time = started.elapsed();

    let capital = match &cfg.capital {
        Some(name) => Some(resolve_all(&uni, std::slice::from_ref(name))?[0]),
        None => None,
    };
    let hull = match &cfg.hull {
        Some(name) => Some(find_hull(name).ok_or_else(|| format!("Unknown hull \"{name}\""))?),
        None => None,
    };
    let settings = Settings {
        mode: cfg.mode.unwrap_or(Mode::Shortest),
        top: cfg.top.unwrap_or(5).max(1),
        wormholes: true,
        bridges: true,
        rules: BridgeRules { capital, hull, max_cap: cfg.max_cap_tj },
        favourites: resolve_all(&uni, &favourite_names(&cfg))?,
    };
    let systems = split_systems(&cli.systems.join(","));

    if cli.print {
        eprintln!(
            "Loaded {} systems in {} ms. Overlay: {}",
            uni.graph.node_count(),
            load_time.as_millis(),
            report.summary()
        );
        print_routes(&uni, &settings, &systems)
    } else {
        let shortcuts = tui::Shortcuts::new(&uni, &report);
        tui::run(&uni, settings, cfg, cfg_path, systems.join(" > "), shortcuts)
    }
}

fn update_sde(dir: &std::path::Path) -> Result<(), String> {
    use sde_update::Outcome;
    match sde_update::ensure(dir)? {
        Outcome::Updated { from, to, bytes } => {
            let mb = bytes as f64 / 1e6;
            match from {
                // Same build: only a file was missing, for example ships.json from an older version.
                Some(b) if b == to => eprintln!("Completed SDE build {to} ({mb:.1} MB) in {}", dir.display()),
                Some(b) => eprintln!("Updated the SDE from build {b} to build {to} ({mb:.1} MB) in {}", dir.display()),
                None => eprintln!("Downloaded SDE build {to} ({mb:.1} MB) to {}", dir.display()),
            }
        }
        Outcome::Offline { local, error } => {
            eprintln!("Cannot check for a new SDE. Using local build {local}. Cause: {error}");
        }
        Outcome::Skipped | Outcome::UpToDate => {}
    }
    Ok(())
}

fn favourite_names(cfg: &Config) -> Vec<String> {
    match &cfg.favourites {
        Some(names) => names.clone(),
        None => config::DEFAULT_FAVOURITES.map(String::from).to_vec(),
    }
}

fn print_routes(uni: &Universe, settings: &Settings, names: &[String]) -> Result<(), String> {
    let nodes = resolve_all(uni, names)?;
    let router = settings.router(uni);
    if let Some(reason) = settings.rules.blocked_reason() {
        eprintln!("Jump bridges off: {reason}");
    }
    if let Some(&origin) = nodes.first() {
        println!("Shortest route from {} ({}):", uni.name(origin), settings.mode.label());
        for (target, jumps) in router.jumps_to(origin, &settings.favourites) {
            let jumps = jumps.map_or("-".into(), |j| j.to_string());
            println!("  {:<16} {jumps:>4}", uni.name(target));
        }
    }
    if nodes.len() < 2 {
        return Ok(());
    }
    let started = Instant::now();
    let routes = router.routes(&nodes, settings.top)?;
    eprintln!("Found {} routes in {} ms", routes.len(), started.elapsed().as_millis());
    for (i, route) in routes.iter().enumerate() {
        println!("\n#{} {}{}", i + 1, tui::jumps_label(route.jumps), tui::route_extras(route));
        for (step, &node) in route.path.nodes.iter().enumerate() {
            let sys = uni.system(node);
            let via = match step.checked_sub(1).map(|s| route.path.edges[s]) {
                Some(e) => tui::link_label(uni, &settings.rules, e),
                None => String::new(),
            };
            let stop = route.stop_at(step).map(|s| s.label()).unwrap_or_default();
            println!("  {:>3} {stop:<11} {:<20} {:>4.1} {:<20} {via}", step, sys.name, display_sec(sys.security), sys.region);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn overlay_universe() -> Universe {
        let mut uni = Universe::from_sde(sde::load(Path::new("sde")).unwrap());
        overlay::load_nexum(&mut uni, Path::new("tests/fixtures/nexum.json"), false).unwrap();
        overlay::load_bridges(&mut uni, Path::new("tests/fixtures/ansiblex.txt")).unwrap();
        uni
    }

    fn settings(uni: &Universe, hull: Option<&str>) -> Settings {
        Settings {
            mode: Mode::Shortest,
            top: 3,
            wormholes: true,
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: hull.map(|h| find_hull(h).unwrap()), max_cap: None },
            favourites: Vec::new(),
        }
    }

    #[test]
    fn overlay_reaches_jspace() {
        let uni = overlay_universe();
        let nodes = resolve_all(&uni, &["Jita".into(), "J134702".into()]).unwrap();
        let mut s = settings(&uni, None);
        assert!(s.router(&uni).routes(&nodes, 1).unwrap()[0].wormholes > 0);
        s.wormholes = false;
        assert!(s.router(&uni).routes(&nodes, 1).is_err());
    }

    #[test]
    fn bridges_need_capital_and_allowed_hull() {
        let uni = overlay_universe();
        // UALX-3 and IL-YTR have a bridge between them.
        let nodes = resolve_all(&uni, &["UALX-3".into(), "IL-YTR".into()]).unwrap();
        let uses_bridge = |s: &Settings| s.router(&uni).routes(&nodes, 1).unwrap()[0].bridges > 0;
        let mut s = settings(&uni, Some("black-ops"));
        assert!(uses_bridge(&s));
        s.rules.capital = None;
        assert!(!uses_bridge(&s));
        assert!(!uses_bridge(&settings(&uni, Some("titan"))));
    }

    #[test]
    fn split_keeps_spaces_in_names() {
        assert_eq!(split_systems("Jita > New Caldari, Amarr"), vec!["Jita", "New Caldari", "Amarr"]);
    }
}
