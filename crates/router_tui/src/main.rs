mod tui;

use clap::Parser;
use router_core::config::{self, Config};
use router_core::labels::{Shortcuts, route_text};
use router_core::route::Mode;
use router_core::settings::{Settings, resolve_all, split_systems};
use router_core::universe::Universe;
use router_core::{startup, wormhole};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

/// mimalloc is faster than the system allocator for the many small maps of the route searches.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Find the top-n routes between EVE Online solar systems.
#[derive(Parser)]
#[command(name = "eve-router", version)]
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
    // A hull on the command line is a manual hull for this run.
    if cli.hull.is_some() {
        cfg.pilot = None;
    }
    cfg.hull = cli.hull.or(cfg.hull);
    cfg.max_cap_tj = cli.max_cap.or(cfg.max_cap_tj);
    cfg.min_life_min = cli.min_life.or(cfg.min_life_min);
    cfg.mode = cli.mode.or(cfg.mode);
    cfg.optimize = cli.optimize || cfg.optimize;
    cfg.top = cli.top.or(cfg.top);

    // Start the wormhole fetches before the SDE update, so they run at the same time.
    let pending = startup::begin(&cfg, &cfg_path);
    let sde_dir = cli.sde.unwrap_or_else(config::default_sde_dir);
    let startup::Loaded { uni, report, wh, scout, all, wormhole_count, load_time } =
        startup::finish(pending, &sde_dir, cli.bridges, |outcome| {
            if let Some(text) = outcome.message(&sde_dir) {
                eprintln!("{text}");
            }
        })?;

    let settings = Settings::from_config(&cfg, &uni)?;
    let systems = split_systems(&cli.systems.join(","));

    if cli.print {
        eprintln!("Loaded {} systems in {} ms. Bridges: {}", uni.graph.node_count(), load_time.as_millis(), report.summary());
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
        let shortcuts = Shortcuts::new(&uni, &report, &wh, &scout);
        tui::run(&uni, settings, cfg, cfg_path, systems.join(" > "), shortcuts)
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
        writeln!(out, "\n{}", route_text(uni, &settings.rules, i, route, now)).map_err(io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::test_support::{FIXTURE_TIME, overlay_universe, settings};

    /// The full `--print` text of two routes: one with jump bridges, one into J-space.
    #[test]
    fn print_routes_snapshot() {
        let uni = overlay_universe();
        let mut s = settings(&uni, Some("black-ops"));
        s.favourites = resolve_all(&uni, &["Jita".into(), "Amarr".into()]).unwrap();
        let mut out = Vec::new();
        print_routes(&mut out, &uni, &s, &["UALX-3".into(), "Jita".into()], FIXTURE_TIME).unwrap();
        print_routes(&mut out, &uni, &s, &["Jita".into(), "J134702".into()], FIXTURE_TIME).unwrap();
        router_core::assert_snapshot!("print_routes", String::from_utf8(out).unwrap());
    }

    #[test]
    fn cli_help_and_version_snapshot() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let text = format!(
            "{}
{}",
            cmd.render_version(),
            cmd.render_long_help()
        );
        router_core::assert_snapshot!("cli_help", text);
    }
}
