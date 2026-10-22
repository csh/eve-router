//! The route settings, and the system names of the user input.

use crate::ansiblex::{BridgeRules, HullTable, same_hull};
use crate::config::{self, AvoidName, Config, ConfigError};
use crate::route::{DEFAULT_CAP_WEIGHT, DEFAULT_UNKNOWN_SIG_PENALTY, JUMP, Mode, PENALTY, RouteError, Router, RouterOptions};
use crate::universe::Universe;
use crate::wormhole;
use crate::wormhole_space::{Stance, WormholeSpace};
use petgraph::graph::NodeIndex;
use std::collections::HashSet;
use std::sync::Arc;

/// Where the hull comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HullSource {
    /// The hull picker.
    Manual,
    /// The live ship of this character.
    Pilot(u64),
}

/// The soft costs of a route, in jumps. A cost of 1 makes a link as dear as one more gate jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteCosts {
    /// The cost of 1% of the Ansiblex capacitor, for a bridge jump with a hull.
    pub cap_weight: f32,
    /// The extra cost of a wormhole with no known signature at its departure system.
    pub unknown_sig_penalty: f32,
}

impl Default for RouteCosts {
    fn default() -> Self {
        RouteCosts {
            cap_weight: DEFAULT_CAP_WEIGHT as f32 / JUMP as f32,
            unknown_sig_penalty: DEFAULT_UNKNOWN_SIG_PENALTY as f32 / JUMP as f32,
        }
    }
}

impl RouteCosts {
    /// The most that a cost can be, in jumps. A larger cost makes the link unusable in effect.
    pub const MAX: f32 = 1000.0;

    /// The costs from the config. A missing or invalid value gives the default.
    pub fn from_config(cfg: &Config) -> Self {
        let default = RouteCosts::default();
        let ok = |v: Option<f32>, default: f32| v.filter(|v| v.is_finite() && (0.0..=Self::MAX).contains(v)).unwrap_or(default);
        RouteCosts {
            cap_weight: ok(cfg.cap_weight, default.cap_weight),
            unknown_sig_penalty: ok(cfg.unknown_sig_penalty, default.unknown_sig_penalty),
        }
    }

    /// A cost in jumps as milli-jumps.
    fn milli(jumps: f32) -> u32 {
        (jumps.clamp(0.0, Self::MAX) * JUMP as f32).round() as u32
    }
}

/// The most routes that the planner shows. The search runs on the UI thread, and its cost grows with
/// the count: a release build needed 5.5 ms for 5 routes and 37 ms for 20 (Jita to Amarr).
pub const MAX_TOP: usize = 10;

/// The sentence that follows a "No route" error while a "Never" entry could cause it.
pub const NO_ROUTE_HINT: &str = "Your avoid list might be in the way.";

/// The sentence that follows a "No route" error while a class of wormhole space is blocked. Many
/// settings can cause such an error, so it names none of them.
pub const NO_ROUTE_STRICT_HINT: &str = "Your settings may be too strict.";

/// One entry of the avoid list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<T> {
    pub item: T,
    /// True: no route crosses it, and a trip to it finds no route. False: a route crosses it
    /// only if no other route exists, or the other routes are much longer.
    pub never: bool,
}

/// The systems and the regions that a route avoids. A jump into a "prefer" entry costs as much
/// as `PENALTY`. A jump into a "never" entry is not possible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Avoid {
    pub systems: Vec<Entry<NodeIndex>>,
    pub regions: Vec<Entry<String>>,
}

impl Avoid {
    /// The number of entries.
    pub fn len(&self) -> usize {
        self.systems.len() + self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.systems.clear();
        self.regions.clear();
    }

    /// Add a "prefer" entry for a system. A system that is in the list already does not count again.
    pub fn add_system(&mut self, node: NodeIndex) {
        if !self.has_system(node) {
            self.systems.push(Entry { item: node, never: false });
        }
    }

    /// Add a "prefer" entry for a region. A region that is in the list already does not count again.
    pub fn add_region(&mut self, region: &str) {
        if !self.has_region(region) {
            self.regions.push(Entry { item: region.to_string(), never: false });
        }
    }

    pub fn set_system_never(&mut self, node: NodeIndex, never: bool) {
        self.systems.iter_mut().filter(|e| e.item == node).for_each(|e| e.never = never);
    }

    pub fn set_region_never(&mut self, region: &str, never: bool) {
        self.regions.iter_mut().filter(|e| e.item == region).for_each(|e| e.never = never);
    }

    pub fn remove_system(&mut self, node: NodeIndex) {
        self.systems.retain(|e| e.item != node);
    }

    pub fn remove_region(&mut self, region: &str) {
        self.regions.retain(|e| e.item != region);
    }

    pub fn has_system(&self, node: NodeIndex) -> bool {
        self.systems.iter().any(|e| e.item == node)
    }

    pub fn has_region(&self, region: &str) -> bool {
        self.regions.iter().any(|e| e.item == region)
    }

    /// True if an entry covers the system: the system itself, or its region.
    pub fn covers(&self, uni: &Universe, node: NodeIndex) -> bool {
        self.has_system(node) || self.has_region(&uni.system(node).region)
    }

    /// The cost of a jump into each system, by `NodeIndex`: `PENALTY` for a "prefer" entry.
    /// The result is for `Router::set_danger`.
    pub fn danger(&self, uni: &Universe) -> Vec<u64> {
        self.mark(uni, false).into_iter().map(|hit| if hit { PENALTY } else { 0 }).collect()
    }

    /// True for each system with a "never" entry, by `NodeIndex`. The result is for `Router::set_never`.
    pub fn never(&self, uni: &Universe) -> Vec<bool> {
        self.mark(uni, true)
    }

    /// True for each system that has an entry with this `never` value, by `NodeIndex`.
    fn mark(&self, uni: &Universe, never: bool) -> Vec<bool> {
        let systems: HashSet<NodeIndex> = self.systems.iter().filter(|e| e.never == never).map(|e| e.item).collect();
        let regions: HashSet<&str> = self.regions.iter().filter(|e| e.never == never).map(|e| e.item.as_str()).collect();
        uni.graph.node_indices().map(|n| systems.contains(&n) || regions.contains(uni.system(n).region.as_str())).collect()
    }
}

/// The settings that a user interface can change.
pub struct Settings {
    /// The ships and the groups. `follow` and the hull picker read it.
    pub hulls: Arc<HullTable>,
    pub mode: Mode,
    /// Visit each system one time, in the cheapest order. See `Router::optimize`.
    pub optimize: bool,
    pub top: usize,
    pub wormholes: bool,
    /// The Thera and Turnur switches.
    pub hubs: wormhole::Hubs,
    /// The stance of each class of wormhole space, of the Drifter systems and of Pochven. It acts like
    /// the avoid list, but stays apart from it.
    pub space: WormholeSpace,
    pub bridges: bool,
    pub rules: BridgeRules,
    pub costs: RouteCosts,
    /// The stance for the filaments.
    pub filaments: Stance,
    /// With `Pilot`, `follow` sets `rules.hull`.
    pub hull_source: HullSource,
    /// The minimum time (minutes) that a wormhole must have left. The router skips other wormholes.
    pub min_life: u64,
    /// The sidebar destinations.
    pub favourites: Vec<NodeIndex>,
    pub avoid: Avoid,
}

impl Settings {
    /// The settings from the config, and a warning for each value that the map or the ship list does
    /// not know. A renamed system or hull must not stop the start, so the function drops such a value
    /// and loads the rest, as `avoid_from_config` does. The warnings are for the user.
    pub fn from_config(cfg: &Config, uni: &Universe, hulls: Arc<HullTable>) -> (Settings, Vec<String>) {
        let mut warnings = Vec::new();
        let capital = cfg.capital.as_ref().and_then(|name| match uni.resolve(name) {
            Ok(node) if uni.sov_null(node) => Some(node),
            Ok(_) | Err(_) => {
                warnings.push("The capital in the config was invalid, jump bridge calculations are disabled.".into());
                None
            }
        });
        let hull = cfg.hull.as_ref().and_then(|name| {
            let hull = hulls.find(name);
            if hull.is_none() {
                warnings.push(format!("Hull \"{name}\" is unknown, so no hull is set."));
            }
            hull
        });
        let mut favourites = Vec::new();
        for name in favourite_names(cfg) {
            match uni.resolve(&name) {
                Ok(node) => favourites.push(node),
                Err(e) => warnings.push(format!("A favourite is not usable and left the list. {e}.")),
            }
        }
        let settings = Settings {
            hulls,
            mode: cfg.mode.unwrap_or(Mode::Shortest),
            optimize: cfg.optimize,
            top: cfg.top.unwrap_or(5).clamp(1, MAX_TOP),
            wormholes: cfg.wormholes.unwrap_or(true),
            hubs: cfg.eve_scout,
            space: cfg.wormhole_space,
            bridges: cfg.bridges.unwrap_or(true),
            rules: BridgeRules { capital, hull, max_cap: cfg.max_cap_tj },
            costs: RouteCosts::from_config(cfg),
            filaments: cfg.filaments.unwrap_or(Stance::Allow),
            hull_source: cfg.pilot.map_or(HullSource::Manual, HullSource::Pilot),
            min_life: cfg.min_life_min.unwrap_or(config::DEFAULT_MIN_LIFE_MIN),
            favourites,
            avoid: avoid_from_config(cfg, uni),
        };
        (settings, warnings)
    }

    /// True if Pochven is blocked. A filament jump ends in Pochven, so the filaments have no use then.
    pub fn pochven_blocked(&self) -> bool {
        self.space.pochven == Stance::Block
    }

    /// A router for the settings. `now` (Unix seconds) and `min_life` set which wormholes are usable.
    pub fn router<'a>(&self, uni: &'a Universe, now: u64) -> Router<'a> {
        let bridges = self.bridges && self.rules.blocked_reason().is_none();
        let options = RouterOptions {
            mode: self.mode,
            wormholes: self.wormholes,
            hubs: self.hubs,
            bridges,
            rules: self.rules.clone(),
            cap_weight: RouteCosts::milli(self.costs.cap_weight),
            unknown_sig_penalty: RouteCosts::milli(self.costs.unknown_sig_penalty),
            filaments: self.filaments,
            now: now + self.min_life * 60,
        };
        let mut router = Router::new(uni, options);
        if !self.avoid.is_empty() || !self.space.is_default() {
            let (danger, never) = (self.avoid.danger(uni), self.avoid.never(uni));
            let (space_danger, space_never) = (self.space.danger(uni), self.space.never(uni));
            router.set_danger(danger.iter().zip(&space_danger).map(|(&a, &b)| a.max(b)).collect());
            router.set_never(never.iter().zip(&space_never).map(|(&a, &b)| a || b).collect());
        }
        router
    }

    /// Set the hull from the ship of the followed pilot. `ship_type` is `None` while the ship is
    /// not known, and then the hull stays. A type that is not in `ships.json` gives no hull.
    /// True if the hull changed.
    pub fn follow(&mut self, ship_type: Option<u32>) -> bool {
        let (HullSource::Pilot(_), Some(type_id)) = (self.hull_source, ship_type) else { return false };
        let hull = self.hulls.by_type(type_id);
        let changed = !same_hull(hull.as_ref(), self.rules.hull.as_ref());
        self.rules.hull = hull;
        changed
    }

    /// Copy the settings to `cfg`, for `Config::save`. The other fields of `cfg` stay.
    pub fn store(&self, uni: &Universe, cfg: &mut Config) {
        cfg.capital = self.rules.capital.map(|n| uni.name(n).to_string());
        cfg.hull = self.rules.hull.as_ref().map(|h| h.name.clone());
        cfg.pilot = match self.hull_source {
            HullSource::Pilot(id) => Some(id),
            HullSource::Manual => None,
        };
        cfg.max_cap_tj = self.rules.max_cap;
        cfg.min_life_min = Some(self.min_life);
        cfg.mode = Some(self.mode);
        cfg.optimize = self.optimize;
        cfg.top = Some(self.top);
        cfg.wormholes = Some(self.wormholes);
        cfg.bridges = Some(self.bridges);
        cfg.cap_weight = Some(self.costs.cap_weight);
        cfg.unknown_sig_penalty = Some(self.costs.unknown_sig_penalty);
        cfg.eve_scout = self.hubs;
        cfg.wormhole_space = self.space;
        cfg.filaments = Some(self.filaments);
        cfg.favourites = Some(self.favourites.iter().map(|&n| uni.name(n).to_string()).collect());
        cfg.avoid_systems = self.avoid.systems.iter().map(|e| AvoidName::new(uni.name(e.item), e.never)).collect();
        cfg.avoid_regions = self.avoid.regions.iter().map(|e| AvoidName::new(&e.item, e.never)).collect();
    }

    /// The text of a route error. A "No route" error gets a hint while a "Never" entry is on the
    /// avoid list, or a group of the space settings is blocked, because only such a rule can block every route.
    /// A blocked wormhole group counts only while wormholes are on.
    pub fn explain(&self, error: &RouteError) -> String {
        let never = self.avoid.systems.iter().any(|e| e.never) || self.avoid.regions.iter().any(|e| e.never);
        match (error, self.space.blocks(self.wormholes), never) {
            (RouteError::NoRoute { .. }, true, _) => format!("{error}. {NO_ROUTE_STRICT_HINT}"),
            (RouteError::NoRoute { .. }, false, true) => format!("{error}. {NO_ROUTE_HINT}"),
            _ => error.to_string(),
        }
    }

    /// The waypoints in the order to route them.
    pub fn order(&self, router: &Router, nodes: &[NodeIndex]) -> Result<Vec<NodeIndex>, RouteError> {
        if self.optimize { router.optimize(nodes) } else { Ok(nodes.to_vec()) }
    }
}

/// Split the system arguments, or a pasted list. The separators are ">", ",", ";", a tab and a
/// line break. A name can hold a space, for example "New Caldari".
pub fn split_systems(input: &str) -> Vec<String> {
    input.split(['>', ',', ';', '\t', '\n', '\r']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// The max TJ for one bridge jump, from a text field. An empty text gives no limit.
pub fn parse_max_cap(text: &str, hulls: &HullTable) -> Result<Option<f32>, ConfigError> {
    let value = text.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let max = hulls.gate_capacitor_tj;
    match value.parse::<f32>() {
        Ok(tj) if (0.0..=max).contains(&tj) => Ok(Some(tj)),
        _ => Err(ConfigError::BadTjValue { value: value.to_string(), max }),
    }
}

pub fn resolve_all(uni: &Universe, names: &[String]) -> Result<Vec<NodeIndex>, RouteError> {
    names.iter().map(|name| uni.resolve(name)).collect()
}

/// The avoid list of the config. A name that the SDE does not know is dropped.
fn avoid_from_config(cfg: &Config, uni: &Universe) -> Avoid {
    let known: std::collections::HashSet<&str> = uni.graph.node_weights().map(|s| s.region.as_str()).collect();
    let mut avoid = Avoid::default();
    for entry in &cfg.avoid_systems {
        if let Some(node) = uni.exact(&entry.name) {
            avoid.add_system(node);
            avoid.set_system_never(node, entry.never);
        }
    }
    for entry in cfg.avoid_regions.iter().filter(|r| known.contains(r.name.as_str())) {
        avoid.add_region(&entry.name);
        avoid.set_region_never(&entry.name, entry.never);
    }
    avoid
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
    use crate::route::WORMHOLE_TIE;
    use crate::test_support::{FIXTURE_TIME, hulls, overlay_universe, settings};

    #[test]
    fn a_config_that_resolves_gives_no_warning() {
        let uni = overlay_universe();
        let cfg = Config { capital: Some("JK-Q77".into()), hull: Some("Sin".into()), ..Config::default() };
        let (s, warnings) = Settings::from_config(&cfg, &uni, hulls());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(s.rules.capital.is_some() && s.rules.hull.is_some());
        assert_eq!(s.favourites.len(), config::DEFAULT_FAVOURITES.len());
    }

    #[test]
    fn an_unknown_or_non_sovnull_capital_is_dropped_with_a_warning() {
        let uni = overlay_universe();
        for name in ["Jita", "PO4F-3", "Thera", "Nowhere"] {
            let cfg = Config { capital: Some(name.into()), ..Config::default() };
            let (s, warnings) = Settings::from_config(&cfg, &uni, hulls());
            assert_eq!(s.rules.capital, None, "{name}");
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert_eq!(warnings[0], "The capital in the config was invalid, jump bridge calculations are disabled.");
        }
    }

    #[test]
    fn an_unknown_hull_is_dropped_with_a_warning() {
        let uni = overlay_universe();
        let cfg = Config { hull: Some("No Such Ship".into()), ..Config::default() };
        let (s, warnings) = Settings::from_config(&cfg, &uni, hulls());
        assert!(s.rules.hull.is_none());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("No Such Ship"), "{warnings:?}");
    }

    #[test]
    fn an_unknown_favourite_is_dropped_and_the_others_stay() {
        let uni = overlay_universe();
        let cfg = Config { favourites: Some(vec!["Jita".into(), "Nowhere".into(), "Amarr".into()]), ..Config::default() };
        let (s, warnings) = Settings::from_config(&cfg, &uni, hulls());
        let names: Vec<&str> = s.favourites.iter().map(|&n| uni.name(n)).collect();
        assert_eq!(names, ["Jita", "Amarr"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Nowhere"), "{warnings:?}");
    }

    #[test]
    fn an_ambiguous_prefix_is_dropped_with_a_warning() {
        // A hand-typed prefix can match more than one system, for example after an SDE update.
        let uni = overlay_universe();
        let cfg = Config { favourites: Some(vec!["J".into()]), ..Config::default() };
        let (s, warnings) = Settings::from_config(&cfg, &uni, hulls());
        assert!(s.favourites.is_empty());
        assert!(warnings[0].contains("more than one system"), "{warnings:?}");
    }

    #[test]
    fn the_route_count_stays_between_one_and_the_maximum() {
        let uni = overlay_universe();
        let top = |top| Settings::from_config(&Config { top: Some(top), ..Config::default() }, &uni, hulls()).0.top;
        assert_eq!(MAX_TOP, 10);
        assert_eq!(top(0), 1);
        assert_eq!(top(7), 7);
        assert_eq!(top(MAX_TOP), MAX_TOP);
        assert_eq!(top(11), MAX_TOP);
        assert_eq!(top(10_000), MAX_TOP);
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

    #[test]
    fn follow_sets_the_hull_from_the_ship() {
        let uni = overlay_universe();
        let sin = hulls().find("Sin").unwrap();
        let sin_type = sin.type_id;
        let mut s = settings(&uni, Some("Rorqual"));
        // The manual source ignores the ship.
        assert!(!s.follow(sin_type));
        assert_eq!(s.rules.hull.as_ref().unwrap().name, "Rorqual");
        s.hull_source = HullSource::Pilot(1);
        // The ship is not known yet: the hull stays.
        assert!(!s.follow(None));
        assert_eq!(s.rules.hull.as_ref().unwrap().name, "Rorqual");
        assert!(s.follow(sin_type));
        assert!(same_hull(s.rules.hull.as_ref(), Some(&sin)));
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
        let back = Settings::from_config(&cfg, &uni, hulls()).0;
        assert_eq!(back.hull_source, HullSource::Pilot(42));
        s.hull_source = HullSource::Manual;
        s.store(&uni, &mut cfg);
        assert_eq!(cfg.pilot, None);
    }

    #[test]
    fn the_wormhole_and_bridge_switches_round_trip_and_default_to_on() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        assert!(Settings::from_config(&Config::default(), &uni, hulls()).0.wormholes);
        (s.wormholes, s.bridges) = (false, false);
        let mut cfg = Config::default();
        s.store(&uni, &mut cfg);
        let back = Settings::from_config(&cfg, &uni, hulls()).0;
        assert_eq!((back.wormholes, back.bridges), (false, false));
    }

    #[test]
    fn filaments_default_to_on_and_the_choice_round_trips() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        assert_eq!(Settings::from_config(&Config::default(), &uni, hulls()).0.filaments, Stance::Allow);
        assert!(!serde_json::to_string(&Config::default()).unwrap().contains("filaments"));
        for stance in [Stance::Block, Stance::Avoid] {
            s.filaments = stance;
            let mut cfg = Config::default();
            s.store(&uni, &mut cfg);
            assert_eq!(Settings::from_config(&cfg, &uni, hulls()).0.filaments, stance);
        }
    }

    /// A file from before the rename keeps its choice: the old key is `pochven`.
    #[test]
    fn the_old_pochven_key_sets_the_filaments_switch() {
        let uni = overlay_universe();
        let cfg = crate::config_migrate::parse(r#"{"pochven": false}"#).unwrap();
        assert_eq!(Settings::from_config(&cfg, &uni, hulls()).0.filaments, Stance::Block);
    }

    #[test]
    fn route_costs_round_trip_and_default() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        assert_eq!(s.costs, RouteCosts::default());
        assert_eq!((s.costs.cap_weight, s.costs.unknown_sig_penalty), (0.6, 4.0));
        s.costs = RouteCosts { cap_weight: 1.5, unknown_sig_penalty: 2.0 };
        let mut cfg = Config::default();
        s.store(&uni, &mut cfg);
        assert_eq!(Settings::from_config(&cfg, &uni, hulls()).0.costs, s.costs);
        // A value that is negative, too large or not a number gives the default.
        cfg.cap_weight = Some(-1.0);
        cfg.unknown_sig_penalty = Some(f32::NAN);
        let costs = Settings::from_config(&cfg, &uni, hulls()).0.costs;
        assert_eq!((costs.cap_weight, costs.unknown_sig_penalty), (0.6, 4.0));
        cfg.cap_weight = Some(RouteCosts::MAX + 1.0);
        assert_eq!(Settings::from_config(&cfg, &uni, hulls()).0.costs.cap_weight, 0.6);
    }

    #[test]
    fn route_costs_reach_the_router() {
        let uni = overlay_universe();
        // Jita to J134702 uses a Nexum wormhole with no signature.
        let nodes = resolve_all(&uni, &["Jita".into(), "J134702".into()]).unwrap();
        let mut s = settings(&uni, None);
        let cost = |s: &Settings| s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).map(|r| r[0].path.cost);
        assert_eq!(cost(&s), Ok(1000 + WORMHOLE_TIE + 4000));
        s.costs.unknown_sig_penalty = 1.5;
        assert_eq!(cost(&s), Ok(1000 + WORMHOLE_TIE + 1500));
    }

    /// The route of Jita to Dodixie, and the first system on it that is in another region than both ends.
    fn route_and_middle(uni: &Universe, s: &Settings) -> (Vec<NodeIndex>, NodeIndex) {
        let nodes = resolve_all(uni, &["Jita".into(), "Dodixie".into()]).unwrap();
        let route = s.router(uni, FIXTURE_TIME).routes(&nodes, 1).unwrap().remove(0);
        let ends = [uni.system(nodes[0]).region.clone(), uni.system(nodes[1]).region.clone()];
        let middle = *route.path.nodes.iter().find(|&&n| !ends.contains(&uni.system(n).region)).expect("a system in a third region");
        (nodes, middle)
    }

    #[test]
    fn an_avoided_system_leaves_the_route() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let (nodes, middle) = route_and_middle(&uni, &s);
        let route = |s: &Settings| s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap().remove(0).path;
        let first = route(&s);
        s.avoid.add_system(middle);
        s.avoid.add_system(middle);
        assert_eq!(s.avoid.len(), 1);
        assert!(!route(&s).nodes.contains(&middle));
        s.avoid.clear();
        assert_eq!(route(&s), first);
    }

    #[test]
    fn an_avoided_region_leaves_the_route() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let (nodes, middle) = route_and_middle(&uni, &s);
        let region = uni.system(middle).region.clone();
        s.avoid.add_region(&region);
        let path = s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap().remove(0).path;
        assert!(path.nodes.iter().all(|&n| uni.system(n).region != region), "the route still crosses {region}");
    }

    #[test]
    fn a_never_system_is_not_crossed_even_when_it_is_the_only_way() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let nodes = resolve_all(&uni, &["Jita".into(), "Perimeter".into()]).unwrap();
        s.avoid.add_system(nodes[1]);
        assert!(s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).is_ok(), "prefer still crosses it");
        s.avoid.set_system_never(nodes[1], true);
        assert!(s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).is_err());
        s.avoid.set_system_never(nodes[1], false);
        assert!(s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).is_ok());
    }

    #[test]
    fn a_no_route_error_points_at_a_never_entry() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let error = RouteError::NoRoute { from: "Jita".into(), to: "Perimeter".into() };
        let text = error.to_string();
        assert_eq!(text, "No route from Jita to Perimeter");
        assert_eq!(s.explain(&error), text);
        // A "prefer" entry never blocks a route, so it gets no hint.
        let perimeter = uni.exact("Perimeter").unwrap();
        s.avoid.add_system(perimeter);
        assert_eq!(s.explain(&error), text);
        s.avoid.set_system_never(perimeter, true);
        assert_eq!(s.explain(&error), format!("{text}. {NO_ROUTE_HINT}"));
        // Another error stays as it is.
        assert_eq!(s.explain(&RouteError::TooFewSystems), "Give two or more systems");
    }

    #[test]
    fn a_blocked_class_gets_the_general_hint_and_a_stance_is_stored() {
        use crate::wormhole_space::{Group, Stance, WormholeClass};
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let error = RouteError::NoRoute { from: "Jita".into(), to: "Perimeter".into() };
        let text = error.to_string();
        // Avoid does not block a route, so it gets no hint.
        s.space.set(Group::Class(WormholeClass::C3), Stance::Avoid);
        assert_eq!(s.explain(&error), text);
        s.space.set(Group::Class(WormholeClass::C3), Stance::Block);
        assert_eq!(s.explain(&error), format!("{text}. {NO_ROUTE_STRICT_HINT}"));
        // A blocked class and a never entry give the one general hint.
        let perimeter = uni.exact("Perimeter").unwrap();
        s.avoid.add_system(perimeter);
        s.avoid.set_system_never(perimeter, true);
        assert_eq!(s.explain(&error), format!("{text}. {NO_ROUTE_STRICT_HINT}"));
        // With wormholes off, a blocked wormhole class has no effect, so it gives no hint. Pochven still does.
        s.avoid.set_system_never(perimeter, false);
        s.wormholes = false;
        assert_eq!(s.explain(&error), text);
        s.space.set(Group::Pochven, Stance::Block);
        assert_eq!(s.explain(&error), format!("{text}. {NO_ROUTE_STRICT_HINT}"));
        s.wormholes = true;
        s.space.set(Group::Pochven, Stance::Allow);
        s.avoid.set_system_never(perimeter, true);
        // The stance goes to the config and comes back.
        let mut cfg = Config::default();
        s.store(&uni, &mut cfg);
        assert_eq!(cfg.wormhole_space.c3, Stance::Block);
        let hulls = crate::test_support::hulls();
        let (back, _) = Settings::from_config(&cfg, &uni, hulls);
        assert_eq!(back.space, s.space);
    }

    #[test]
    fn an_entry_covers_its_system_and_the_systems_of_its_region() {
        let uni = overlay_universe();
        let mut avoid = Avoid::default();
        let (jita, perimeter, amarr) = (uni.exact("Jita").unwrap(), uni.exact("Perimeter").unwrap(), uni.exact("Amarr").unwrap());
        assert!(!avoid.covers(&uni, jita));
        avoid.add_system(jita);
        avoid.add_region(&uni.system(amarr).region);
        assert!(avoid.covers(&uni, jita) && avoid.covers(&uni, amarr));
        assert!(!avoid.covers(&uni, perimeter) || uni.system(perimeter).region == uni.system(amarr).region);
    }

    /// New Eden to Dead End has one way only, through Central Point.
    #[test]
    fn a_choke_point_blocks_a_route_in_never_mode_and_not_in_prefer_mode() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let nodes = resolve_all(&uni, &["New Eden".into(), "Dead End".into()]).unwrap();
        let point = uni.exact("Central Point").unwrap();
        let routes = |s: &Settings| s.router(&uni, FIXTURE_TIME).routes(&nodes, 3);

        // With no avoid entry, the route goes through Central Point.
        assert!(routes(&s).unwrap().iter().all(|r| r.path.nodes.contains(&point)));

        // Prefer: the route still succeeds, and it counts the system that it had to enter.
        s.avoid.add_system(point);
        let found = routes(&s).unwrap();
        assert!(found.iter().all(|r| r.path.nodes.contains(&point) && r.avoided == 1));

        // Never: no route exists, and the error points at the avoid list.
        s.avoid.set_system_never(point, true);
        let error = routes(&s).err().expect("no route");
        assert_eq!(s.explain(&error), "No route from New Eden to Dead End. Your avoid list might be in the way.");

        // Back to Prefer: the route is there again.
        s.avoid.set_system_never(point, false);
        assert!(routes(&s).is_ok());
    }

    #[test]
    fn a_never_region_is_not_crossed() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let (nodes, middle) = route_and_middle(&uni, &s);
        let region = uni.system(middle).region.clone();
        s.avoid.add_region(&region);
        s.avoid.set_region_never(&region, true);
        let routes = s.router(&uni, FIXTURE_TIME).routes(&nodes, 3).unwrap();
        assert!(routes.iter().all(|r| r.path.nodes.iter().all(|&n| uni.system(n).region != region)));
    }

    #[test]
    fn an_avoided_system_stays_when_no_other_route_exists() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let nodes = resolve_all(&uni, &["Jita".into(), "Perimeter".into()]).unwrap();
        s.avoid.add_system(nodes[1]);
        let routes = s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap();
        assert_eq!(routes[0].path.nodes.last(), Some(&nodes[1]));
    }

    #[test]
    fn the_avoid_list_round_trips_and_drops_unknown_names() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        let rens = uni.exact("Rens").unwrap();
        s.avoid.add_system(rens);
        s.avoid.set_system_never(rens, true);
        s.avoid.add_region("Lonetrek");
        let mut cfg = Config::default();
        s.store(&uni, &mut cfg);
        assert_eq!(
            (cfg.avoid_systems.as_slice(), cfg.avoid_regions.as_slice()),
            (&[AvoidName::new("Rens", true)][..], &[AvoidName::new("Lonetrek", false)][..])
        );
        let back = Settings::from_config(&cfg, &uni, hulls()).0;
        assert_eq!(back.avoid, s.avoid);
        // A name that the SDE does not know is dropped, and the other entries stay.
        cfg.avoid_systems.push(AvoidName::new("Nowhere", false));
        cfg.avoid_regions.push(AvoidName::new("Nowhere", false));
        let back = Settings::from_config(&cfg, &uni, hulls()).0;
        assert_eq!((back.avoid.systems.len(), back.avoid.regions.len()), (1, 1));
    }

    #[test]
    fn a_system_name_gives_a_typed_error() {
        let uni = overlay_universe();
        let names = |list: &[&str]| list.iter().map(std::string::ToString::to_string).collect::<Vec<_>>();
        assert_eq!(resolve_all(&uni, &names(&["Nowhere"])), Err(RouteError::UnknownSystem("Nowhere".into())));
        assert_eq!(resolve_all(&uni, &names(&["Nowhere"])).unwrap_err().to_string(), "Unknown system \"Nowhere\"");
        // "A" is the first letter of many system names.
        let Err(RouteError::AmbiguousSystem { name, candidates }) = resolve_all(&uni, &names(&["A"])) else { panic!("not ambiguous") };
        assert_eq!(name, "A");
        assert!(candidates.len() > 1, "{candidates:?}");
    }

    #[test]
    fn max_cap_text() {
        assert_eq!(parse_max_cap(" ", &hulls()), Ok(None));
        assert_eq!(parse_max_cap("36.5", &hulls()), Ok(Some(36.5)));
        assert_eq!(parse_max_cap("-1", &hulls()), Err(ConfigError::BadTjValue { value: "-1".into(), max: 1250.0 }));
        assert!(parse_max_cap("-1", &hulls()).unwrap_err().to_string().contains("is not a TJ value"));
        assert!(parse_max_cap("lots", &hulls()).is_err());
    }

    #[test]
    fn store_copies_settings_to_config() {
        let uni = overlay_universe();
        let mut s = settings(&uni, Some("black-ops"));
        s.favourites = vec![uni.exact("Jita").unwrap()];
        s.rules.max_cap = Some(36.5);
        s.min_life = 240;
        let mut cfg = Config { min_life_min: Some(30), ..Config::default() };
        cfg.nexum.url = Some("https://nexum.example".into());
        s.store(&uni, &mut cfg);
        assert_eq!(cfg.capital.as_deref(), Some("JK-Q77"));
        assert_eq!(cfg.hull.as_deref(), Some("Black Ops"));
        assert_eq!(cfg.max_cap_tj, Some(36.5));
        assert_eq!(cfg.mode, Some(Mode::Shortest));
        assert_eq!(cfg.top, Some(3));
        assert_eq!(cfg.favourites, Some(vec!["Jita".to_string()]));
        assert_eq!(cfg.min_life_min, Some(240));
        // A field that the settings do not hold stays.
        assert_eq!(cfg.nexum.url.as_deref(), Some("https://nexum.example"));
    }

    /// A wormhole with 45 minutes left is usable at a minimum life of 30 minutes, and not at 60.
    #[test]
    fn min_life_sets_which_wormholes_a_route_uses() {
        use crate::test_support::{hole, universe};
        use crate::wormhole::{Expiry, Wormhole};
        let base = universe();
        let (jita_id, far_id) = (base.system(base.exact("Jita").unwrap()).id, base.system(base.exact("D-3GIQ").unwrap()).id);
        let mut uni = base.clone();
        let ends_in_45 = Wormhole { expiry: Some(Expiry { at: FIXTURE_TIME + 45 * 60 }), ..hole(jita_id, far_id) };
        uni.add_wormholes(&[ends_in_45]);
        let (jita, far) = (uni.exact("Jita").unwrap(), uni.exact("D-3GIQ").unwrap());
        let mut s = settings(&uni, None);
        // No signature cost: the test is about the life of the hole, not its price.
        s.costs.unknown_sig_penalty = 0.0;
        let wormholes = |s: &Settings| s.router(&uni, FIXTURE_TIME).routes(&[jita, far], 1).unwrap().remove(0).wormholes;
        s.min_life = 30;
        assert_eq!(wormholes(&s), 1);
        s.min_life = 60;
        assert_eq!(wormholes(&s), 0);
    }

    #[test]
    fn split_keeps_spaces_in_names() {
        assert_eq!(split_systems("Jita > New Caldari, Amarr"), vec!["Jita", "New Caldari", "Amarr"]);
        assert_eq!(split_systems("Jita\r\n New Caldari \n\nAmarr;Rens\tHek\n"), vec!["Jita", "New Caldari", "Amarr", "Rens", "Hek"]);
    }
}
