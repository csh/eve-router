//! The route settings, and the system names of the user input.

use crate::ansiblex::{BridgeRules, find_hull, hull_by_type, same_hull, table};
use crate::config::{self, Config};
use crate::route::{Mode, Router};
use crate::universe::Universe;
use crate::wormhole;
use petgraph::graph::NodeIndex;

/// Where the hull comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HullSource {
    /// The hull picker.
    Manual,
    /// The live ship of this character.
    Pilot(u64),
}

/// The settings that a user interface can change.
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
    /// With `Pilot`, `follow` sets `rules.hull`.
    pub hull_source: HullSource,
    /// The minimum time (minutes) that a wormhole must have left. The router skips other wormholes.
    pub min_life: u64,
    /// The sidebar destinations.
    pub favourites: Vec<NodeIndex>,
}

impl Settings {
    /// The settings from the config file. The CLI flags are already in `cfg`.
    pub fn from_config(cfg: &Config, uni: &Universe) -> Result<Settings, String> {
        let capital = match &cfg.capital {
            Some(name) => Some(resolve_all(uni, std::slice::from_ref(name))?[0]),
            None => None,
        };
        let hull = match &cfg.hull {
            Some(name) => Some(find_hull(name).ok_or_else(|| format!("Unknown hull \"{name}\""))?),
            None => None,
        };
        Ok(Settings {
            mode: cfg.mode.unwrap_or(Mode::Shortest),
            optimize: cfg.optimize,
            top: cfg.top.unwrap_or(5).max(1),
            wormholes: true,
            hubs: cfg.eve_scout,
            bridges: true,
            rules: BridgeRules { capital, hull, max_cap: cfg.max_cap_tj },
            hull_source: cfg.pilot.map_or(HullSource::Manual, HullSource::Pilot),
            min_life: cfg.min_life_min.unwrap_or(config::DEFAULT_MIN_LIFE_MIN),
            favourites: resolve_all(uni, &favourite_names(cfg))?,
        })
    }

    /// A router for the settings. `now` (Unix seconds) and `min_life` set which wormholes are usable.
    pub fn router<'a>(&self, uni: &'a Universe, now: u64) -> Router<'a> {
        let bridges = self.bridges && self.rules.blocked_reason().is_none();
        Router::new(uni, self.mode, self.wormholes, self.hubs, bridges, self.rules, now + self.min_life * 60)
    }

    /// Set the hull from the ship of the followed pilot. `ship_type` is `None` while the ship is
    /// not known, and then the hull stays. A type that is not in `ships.json` gives no hull.
    /// True if the hull changed.
    pub fn follow(&mut self, ship_type: Option<u32>) -> bool {
        let (HullSource::Pilot(_), Some(type_id)) = (self.hull_source, ship_type) else { return false };
        let hull = hull_by_type(type_id);
        let changed = !same_hull(hull, self.rules.hull);
        self.rules.hull = hull;
        changed
    }

    /// Copy the settings to `cfg`, for `Config::save`. The other fields of `cfg` stay.
    pub fn store(&self, uni: &Universe, cfg: &mut Config) {
        cfg.capital = self.rules.capital.map(|n| uni.name(n).to_string());
        cfg.hull = self.rules.hull.map(|h| h.name.clone());
        cfg.pilot = match self.hull_source {
            HullSource::Pilot(id) => Some(id),
            HullSource::Manual => None,
        };
        cfg.max_cap_tj = self.rules.max_cap;
        cfg.mode = Some(self.mode);
        cfg.optimize = self.optimize;
        cfg.top = Some(self.top);
        cfg.eve_scout = self.hubs;
        cfg.favourites = Some(self.favourites.iter().map(|&n| uni.name(n).to_string()).collect());
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

/// Split the system arguments, or a pasted list. The separators are ">", ",", ";", a tab and a
/// line break. A name can hold a space, for example "New Caldari".
pub fn split_systems(input: &str) -> Vec<String> {
    input.split(['>', ',', ';', '\t', '\n', '\r']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// The max TJ for one bridge jump, from a text field. An empty text gives no limit.
pub fn parse_max_cap(text: &str) -> Result<Option<f32>, String> {
    let value = text.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let max = table().gate_capacitor_tj;
    match value.parse::<f32>() {
        Ok(tj) if (0.0..=max).contains(&tj) => Ok(Some(tj)),
        _ => Err(format!("\"{value}\" is not a TJ value. Give a number from 0 to {max}, or leave it empty for no limit.")),
    }
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

fn favourite_names(cfg: &Config) -> Vec<String> {
    match &cfg.favourites {
        Some(names) => names.clone(),
        None => config::DEFAULT_FAVOURITES.map(String::from).to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FIXTURE_TIME, overlay_universe, settings};

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

    #[test]
    fn follow_sets_the_hull_from_the_ship() {
        let uni = overlay_universe();
        let sin = find_hull("Sin").unwrap();
        let sin_type = sin.type_id;
        let mut s = settings(&uni, Some("Rorqual"));
        // The manual source ignores the ship.
        assert!(!s.follow(sin_type));
        assert_eq!(s.rules.hull.unwrap().name, "Rorqual");
        s.hull_source = HullSource::Pilot(1);
        // The ship is not known yet: the hull stays.
        assert!(!s.follow(None));
        assert_eq!(s.rules.hull.unwrap().name, "Rorqual");
        assert!(s.follow(sin_type));
        assert!(std::ptr::eq(s.rules.hull.unwrap(), sin));
        // The same ship again is no change.
        assert!(!s.follow(sin_type));
        // A type that is not in ships.json gives no hull.
        assert!(s.follow(Some(u32::MAX)));
        assert!(s.rules.hull.is_none());
    }

    #[test]
    fn hull_source_round_trip() {
        let uni = overlay_universe();
        let mut s = settings(&uni, Some("Sin"));
        s.hull_source = HullSource::Pilot(42);
        let mut cfg = Config::default();
        s.store(&uni, &mut cfg);
        assert_eq!(cfg.pilot, Some(42));
        // The last known hull stays in the file for the next startup.
        assert_eq!(cfg.hull.as_deref(), Some("Sin"));
        let back = Settings::from_config(&cfg, &uni).unwrap();
        assert_eq!(back.hull_source, HullSource::Pilot(42));
        s.hull_source = HullSource::Manual;
        s.store(&uni, &mut cfg);
        assert_eq!(cfg.pilot, None);
    }

    #[test]
    fn max_cap_text() {
        assert_eq!(parse_max_cap(" "), Ok(None));
        assert_eq!(parse_max_cap("36.5"), Ok(Some(36.5)));
        assert!(parse_max_cap("-1").unwrap_err().contains("is not a TJ value"));
        assert!(parse_max_cap("lots").is_err());
    }

    #[test]
    fn store_copies_settings_to_config() {
        let uni = overlay_universe();
        let mut s = settings(&uni, Some("black-ops"));
        s.favourites = vec![uni.exact("Jita").unwrap()];
        s.rules.max_cap = Some(36.5);
        let mut cfg = Config { min_life_min: Some(30), ..Config::default() };
        s.store(&uni, &mut cfg);
        assert_eq!(cfg.capital.as_deref(), Some("JK-Q77"));
        assert_eq!(cfg.hull.as_deref(), Some("Black Ops"));
        assert_eq!(cfg.max_cap_tj, Some(36.5));
        assert_eq!(cfg.mode, Some(Mode::Shortest));
        assert_eq!(cfg.top, Some(3));
        assert_eq!(cfg.favourites, Some(vec!["Jita".to_string()]));
        // A field that the settings do not hold stays.
        assert_eq!(cfg.min_life_min, Some(30));
    }

    #[test]
    fn split_keeps_spaces_in_names() {
        assert_eq!(split_systems("Jita > New Caldari, Amarr"), vec!["Jita", "New Caldari", "Amarr"]);
        assert_eq!(split_systems("Jita\r\n New Caldari \n\nAmarr;Rens\tHek\n"), vec!["Jita", "New Caldari", "Amarr", "Rens", "Hek"]);
    }
}
