mod ansiblex;
mod config;
mod overlay;
mod route;
mod sde;
mod sde_update;
mod ships;
mod sources;
#[cfg(test)]
mod test_support;
mod tui;
mod universe;
mod wormhole;
mod wormhole_types;

use ansiblex::{BridgeRules, find_hull};
use clap::Parser;
use config::Config;
use overlay::OverlayReport;
use petgraph::graph::NodeIndex;
use route::{Mode, Router};
use sources::{evescout, nexum};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;
use universe::{Universe, display_sec};

/// mimalloc is faster than the system allocator for the many small maps of the route searches.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
    /// The minimum time (minutes) that a wormhole must have left. The default is 60.
    #[arg(long)]
    min_life: Option<u64>,
    /// A jump bridge list in SMT format. The default is ansiblex.txt in the config directory.
    #[arg(long)]
    bridges: Option<PathBuf>,
    /// Visit each system one time, in the cheapest order. The start and the destination stay in place.
    #[arg(long)]
    optimize: bool,
    /// Print the routes and exit. Do not start the TUI.
    #[arg(long)]
    print: bool,
}

/// The settings that the TUI can change.
pub struct Settings {
    pub mode: Mode,
    /// Visit each system one time, in the cheapest order. See `Router::optimize`.
    pub optimize: bool,
    pub top: usize,
    pub wormholes: bool,
    /// The Thera and Turnur switches.
    pub hubs: wormhole::Hubs,
    pub bridges: bool,
    pub rules: BridgeRules,
    /// The minimum time (minutes) that a wormhole must have left. The router skips other wormholes.
    pub min_life: u64,
    /// The sidebar destinations.
    pub favourites: Vec<NodeIndex>,
}

impl Settings {
    /// A router for the settings. `now` (Unix seconds) and `min_life` set which wormholes are usable.
    pub fn router<'a>(&self, uni: &'a Universe, now: u64) -> Router<'a> {
        let bridges = self.bridges && self.rules.blocked_reason().is_none();
        Router::new(uni, self.mode, self.wormholes, self.hubs, bridges, self.rules, now + self.min_life * 60)
    }

    /// The waypoints in the order to route them, and a status text if the order changed.
    pub fn order(&self, router: &Router, nodes: &[NodeIndex]) -> Result<(Vec<NodeIndex>, Option<String>), String> {
        if !self.optimize {
            return Ok((nodes.to_vec(), None));
        }
        let order = router.optimize(nodes)?;
        let text = (order != nodes).then(|| {
            let names: Vec<&str> = order.iter().map(|&n| router.uni.name(n)).collect();
            format!("Optimized order: {}", names.join(" > "))
        });
        Ok((order, text))
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
    let cfg_path = config::default_path();
    let mut cfg = Config::load(&cfg_path)?;
    // The CLI flags override the config file.
    cfg.capital = cli.capital.or(cfg.capital);
    cfg.hull = cli.hull.or(cfg.hull);
    cfg.max_cap_tj = cli.max_cap.or(cfg.max_cap_tj);
    cfg.min_life_min = cli.min_life.or(cfg.min_life_min);
    cfg.mode = cli.mode.or(cfg.mode);
    cfg.optimize = cli.optimize || cfg.optimize;
    cfg.top = cli.top.or(cfg.top);

    // Start the Nexum load before the SDE update, so the fetch runs at the same time.
    // Nexum data comes only from the API.
    let cache_path = sources::cache_path(wormhole::SourceId::Nexum, &cfg_path);
    let pending = nexum::start(&cfg.nexum, cache_path, wormhole::now());
    // EVE-Scout is public, so the router always fetches it. The Thera and Turnur switches
    // act at route time, so a switch works with no restart.
    let scout_cache = sources::cache_path(wormhole::SourceId::EveScout, &cfg_path);
    let scout_pending = evescout::start(evescout::URL, scout_cache, wormhole::now());

    let sde_dir = cli.sde.unwrap_or_else(config::default_sde_dir);
    update_sde(&sde_dir)?;
    ansiblex::init(&sde_dir)?;

    let started = Instant::now();
    let mut uni = Universe::from_sde(sde::load(&sde_dir)?);
    let bridges_path = config::overlay_path(cli.bridges, &cfg_path, config::BRIDGES_FILE);
    let mut report = OverlayReport::default();
    if let Some(path) = &bridges_path {
        report = overlay::load_bridges(&mut uni, path)?;
    }
    let types = wormhole_types::load(&sde_dir)?;
    let wh = nexum::finish(pending, |id| uni.by_id.contains_key(&id), &types);
    let scout = evescout::finish(scout_pending, |id| uni.by_id.contains_key(&id), &types);
    let all: Vec<wormhole::SourceData> = wh.data.iter().chain(&scout.data).cloned().collect();
    let wormhole_count = uni.add_wormholes(&wormhole::merge(&all, wormhole::now()));
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
        optimize: cfg.optimize,
        top: cfg.top.unwrap_or(5).max(1),
        wormholes: true,
        hubs: cfg.eve_scout,
        bridges: true,
        rules: BridgeRules { capital, hull, max_cap: cfg.max_cap_tj },
        min_life: cfg.min_life_min.unwrap_or(config::DEFAULT_MIN_LIFE_MIN),
        favourites: resolve_all(&uni, &favourite_names(&cfg))?,
    };
    let systems = split_systems(&cli.systems.join(","));

    if cli.print {
        eprintln!(
            "Loaded {} systems in {} ms. Bridges: {}",
            uni.graph.node_count(),
            load_time.as_millis(),
            report.summary()
        );
        if !all.is_empty() {
            let now = wormhole::now();
            let from: Vec<String> =
                all.iter().map(|d| format!("{} ({})", d.source.label(), wormhole::age_text(d.fetched_at, now))).collect();
            eprintln!("Wormholes: {wormhole_count} from {}", from.join(", "));
        }
        for warning in wh.warning.iter().chain(&scout.warning) {
            eprintln!("{warning}");
        }
        print_routes(&mut std::io::stdout().lock(), &uni, &settings, &systems, wormhole::now())
    } else {
        let shortcuts = tui::Shortcuts::new(&uni, &report, &wh, &scout);
        tui::run(&uni, settings, cfg, cfg_path, systems.join(" > "), shortcuts)
    }
}

fn update_sde(dir: &std::path::Path) -> Result<(), String> {
    if let Some(text) = sde_update::ensure(dir)?.message(dir) {
        eprintln!("{text}");
    }
    Ok(())
}

fn favourite_names(cfg: &Config) -> Vec<String> {
    match &cfg.favourites {
        Some(names) => names.clone(),
        None => config::DEFAULT_FAVOURITES.map(String::from).to_vec(),
    }
}

/// Write the routes to `out`. The status lines go to stderr. `now` (Unix seconds) sets which
/// wormholes are usable, and the expiry text.
fn print_routes(out: &mut impl Write, uni: &Universe, settings: &Settings, names: &[String], now: u64) -> Result<(), String> {
    let nodes = resolve_all(uni, names)?;
    let router = settings.router(uni, now);
    if let Some(reason) = settings.rules.blocked_reason() {
        eprintln!("Jump bridges off: {reason}");
    }
    let io = |e: std::io::Error| e.to_string();
    if let Some(&origin) = nodes.first() {
        writeln!(out, "Shortest route from {} ({}):", uni.name(origin), settings.mode.label()).map_err(io)?;
        for (target, jumps) in router.jumps_to(origin, &settings.favourites) {
            let jumps = jumps.map_or("-".into(), |j| j.to_string());
            writeln!(out, "  {:<16} {jumps:>4}", uni.name(target)).map_err(io)?;
        }
    }
    if nodes.len() < 2 {
        return Ok(());
    }
    let started = Instant::now();
    let (nodes, changed) = settings.order(&router, &nodes)?;
    if let Some(text) = changed {
        eprintln!("{text}");
    }
    let routes = router.routes(&nodes, settings.top)?;
    eprintln!("Found {} routes in {} ms", routes.len(), started.elapsed().as_millis());
    for (i, route) in routes.iter().enumerate() {
        writeln!(out, "
#{} {}{}", i + 1, tui::jumps_label(route.jumps), tui::route_extras(route)).map_err(io)?;
        for (step, &node) in route.path.nodes.iter().enumerate() {
            let sys = uni.system(node);
            let via = match step.checked_sub(1).map(|s| route.path.edges[s]) {
                Some(e) => tui::link_label(uni, &settings.rules, e, now),
                None => String::new(),
            };
            let stop = route.stop_at(step).map(|s| s.label()).unwrap_or_default();
            writeln!(out, "  {:>3} {stop:<11} {:<20} {:>4.1} {:<20} {via}", step, sys.name, display_sec(sys.security), sys.region)
                .map_err(io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 2026-10-05T12:00:00Z.
    const FIXTURE_TIME: u64 = 1_791_201_600;

    fn overlay_universe() -> Universe {
        let mut uni = Universe::from_sde(sde::load(Path::new("sde")).unwrap());
        overlay::load_bridges(&mut uni, Path::new("tests/fixtures/ansiblex.txt")).unwrap();
        let text = std::fs::read_to_string("tests/fixtures/nexum-api.json").unwrap();
        let map = nexum::parse_map(&text).unwrap();
        let types = wormhole_types::load(Path::new("sde")).unwrap();
        let (data, _) = nexum::convert(&map, &Default::default(), |id| uni.by_id.contains_key(&id), &types, FIXTURE_TIME);
        uni.add_wormholes(&wormhole::merge(&[data], FIXTURE_TIME));
        uni
    }

    fn settings(uni: &Universe, hull: Option<&str>) -> Settings {
        Settings {
            mode: Mode::Shortest,
            optimize: false,
            top: 3,
            wormholes: true,
            hubs: Default::default(),
            bridges: true,
            rules: BridgeRules { capital: uni.exact("JK-Q77"), hull: hull.map(|h| find_hull(h).unwrap()), max_cap: None },
            min_life: 0,
            favourites: Vec::new(),
        }
    }

    #[test]
    fn overlay_reaches_jspace() {
        let uni = overlay_universe();
        let nodes = resolve_all(&uni, &["Jita".into(), "J134702".into()]).unwrap();
        let mut s = settings(&uni, None);
        assert!(s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap()[0].wormholes > 0);
        s.wormholes = false;
        assert!(s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).is_err());
    }

    #[test]
    fn bridges_need_capital_and_allowed_hull() {
        let uni = overlay_universe();
        // UALX-3 and IL-YTR have a bridge between them.
        let nodes = resolve_all(&uni, &["UALX-3".into(), "IL-YTR".into()]).unwrap();
        let uses_bridge = |s: &Settings| s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap()[0].bridges > 0;
        let mut s = settings(&uni, Some("black-ops"));
        assert!(uses_bridge(&s));
        s.rules.capital = None;
        assert!(!uses_bridge(&s));
        assert!(!uses_bridge(&settings(&uni, Some("titan"))));
    }

    /// The full `--print` text of two routes: one with jump bridges, one into J-space.
    #[test]
    fn print_routes_snapshot() {
        let uni = overlay_universe();
        let mut s = settings(&uni, Some("black-ops"));
        s.favourites = resolve_all(&uni, &["Jita".into(), "Amarr".into()]).unwrap();
        let mut out = Vec::new();
        print_routes(&mut out, &uni, &s, &["UALX-3".into(), "Jita".into()], FIXTURE_TIME).unwrap();
        print_routes(&mut out, &uni, &s, &["Jita".into(), "J134702".into()], FIXTURE_TIME).unwrap();
        crate::assert_snapshot!("print_routes", String::from_utf8(out).unwrap());
    }

    #[test]
    fn cli_help_and_version_snapshot() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let text = format!("{}
{}", cmd.render_version(), cmd.render_long_help());
        crate::assert_snapshot!("cli_help", text);
    }

    #[test]
    fn split_keeps_spaces_in_names() {
        assert_eq!(split_systems("Jita > New Caldari, Amarr"), vec!["Jita", "New Caldari", "Amarr"]);
    }
}
