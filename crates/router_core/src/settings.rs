//! The route settings, and the system names of the user input.

use crate::ansiblex::{BridgeRules, find_hull, hull_by_type, same_hull, table};
use crate::config::{self, AvoidName, Config};
use crate::route::{DEFAULT_CAP_WEIGHT, DEFAULT_UNKNOWN_SIG_PENALTY, JUMP, Mode, PENALTY, Router, RouterOptions};
use crate::universe::Universe;
use crate::wormhole;
use petgraph::graph::NodeIndex;
use std::collections::HashSet;

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
    /// Drop a wormhole with no known signature, instead of the penalty.
    pub unknown_sig_broken: bool,
}

impl Default for RouteCosts {
    fn default() -> Self {
        RouteCosts {
            cap_weight: DEFAULT_CAP_WEIGHT as f32 / JUMP as f32,
            unknown_sig_penalty: DEFAULT_UNKNOWN_SIG_PENALTY as f32 / JUMP as f32,
            unknown_sig_broken: false,
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
            unknown_sig_broken: cfg.unknown_sig_broken,
        }
    }

    /// A cost in jumps as milli-jumps.
    fn milli(jumps: f32) -> u32 {
        (jumps.clamp(0.0, Self::MAX) * JUMP as f32).round() as u32
    }
}

/// The sentence that follows a "No route" error while a "Never" entry could cause it.
pub const NO_ROUTE_HINT: &str = "Your avoid list might be in the way.";

/// One entry of the avoid list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry<T> {
    pub item: T,
    /// True: no route crosses it, and a trip to it finds no route. False: a route crosses it
    /// only if no other route exists, or the other routes are much longer.
    pub never: bool,
}

/// The systems and the regions that a route avoids. A jump into a "prefer" entry costs as much
/// as `PENALTY`. A jump into a "never" entry is not possible.
#[derive(Clone, Debug, Default, PartialEq)]
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
    pub mode: Mode,
    /// Visit each system one time, in the cheapest order. See `Router::optimize`.
    pub optimize: bool,
    pub top: usize,
    pub wormholes: bool,
    /// The Thera and Turnur switches.
    pub hubs: wormhole::Hubs,
    pub bridges: bool,
    pub rules: BridgeRules,
    pub costs: RouteCosts,
    /// With `Pilot`, `follow` sets `rules.hull`.
    pub hull_source: HullSource,
    /// The minimum time (minutes) that a wormhole must have left. The router skips other wormholes.
    pub min_life: u64,
    /// The sidebar destinations.
    pub favourites: Vec<NodeIndex>,
    pub avoid: Avoid,
}

impl Settings {
    /// The settings from the config. The TUI applies its CLI flags to `cfg` first.
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
            costs: RouteCosts::from_config(cfg),
            hull_source: cfg.pilot.map_or(HullSource::Manual, HullSource::Pilot),
            min_life: cfg.min_life_min.unwrap_or(config::DEFAULT_MIN_LIFE_MIN),
            favourites: resolve_all(uni, &favourite_names(cfg))?,
            avoid: avoid_from_config(cfg, uni),
        })
    }

    /// A router for the settings. `now` (Unix seconds) and `min_life` set which wormholes are usable.
    pub fn router<'a>(&self, uni: &'a Universe, now: u64) -> Router<'a> {
        let bridges = self.bridges && self.rules.blocked_reason().is_none();
        let options = RouterOptions {
            mode: self.mode,
            wormholes: self.wormholes,
            hubs: self.hubs,
            bridges,
            rules: self.rules,
            cap_weight: RouteCosts::milli(self.costs.cap_weight),
            unknown_sig_penalty: RouteCosts::milli(self.costs.unknown_sig_penalty),
            unknown_sig_broken: self.costs.unknown_sig_broken,
            now: now + self.min_life * 60,
        };
        let mut router = Router::new(uni, options);
        if !self.avoid.is_empty() {
            router.set_danger(self.avoid.danger(uni));
            router.set_never(self.avoid.never(uni));
        }
        router
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
        cfg.cap_weight = Some(self.costs.cap_weight);
        cfg.unknown_sig_penalty = Some(self.costs.unknown_sig_penalty);
        cfg.unknown_sig_broken = self.costs.unknown_sig_broken;
        cfg.eve_scout = self.hubs;
        cfg.favourites = Some(self.favourites.iter().map(|&n| uni.name(n).to_string()).collect());
        cfg.avoid_systems = self.avoid.systems.iter().map(|e| AvoidName::new(uni.name(e.item), e.never)).collect();
        cfg.avoid_regions = self.avoid.regions.iter().map(|e| AvoidName::new(&e.item, e.never)).collect();
    }

    /// The text of a route error. A "No route" error gets a hint while a "Never" entry is on the
    /// avoid list, because only such an entry can block every route.
    pub fn explain(&self, error: String) -> String {
        let never = self.avoid.systems.iter().any(|e| e.never) || self.avoid.regions.iter().any(|e| e.never);
        if never && error.starts_with("No route") { format!("{error}. {NO_ROUTE_HINT}") } else { error }
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
    fn route_costs_round_trip_and_default() {
        let uni = overlay_universe();
        let mut s = settings(&uni, None);
        assert_eq!(s.costs, RouteCosts::default());
        assert_eq!((s.costs.cap_weight, s.costs.unknown_sig_penalty), (0.6, 4.0));
        s.costs = RouteCosts { cap_weight: 1.5, unknown_sig_penalty: 2.0, unknown_sig_broken: true };
        let mut cfg = Config::default();
        s.store(&uni, &mut cfg);
        assert_eq!(Settings::from_config(&cfg, &uni).unwrap().costs, s.costs);
        // A value that is negative, too large or not a number gives the default.
        cfg.cap_weight = Some(-1.0);
        cfg.unknown_sig_penalty = Some(f32::NAN);
        let costs = Settings::from_config(&cfg, &uni).unwrap().costs;
        assert_eq!((costs.cap_weight, costs.unknown_sig_penalty), (0.6, 4.0));
        cfg.cap_weight = Some(RouteCosts::MAX + 1.0);
        assert_eq!(Settings::from_config(&cfg, &uni).unwrap().costs.cap_weight, 0.6);
    }

    #[test]
    fn route_costs_reach_the_router() {
        let uni = overlay_universe();
        // Jita to J134702 uses a Nexum wormhole with no signature.
        let nodes = resolve_all(&uni, &["Jita".into(), "J134702".into()]).unwrap();
        let mut s = settings(&uni, None);
        let cost = |s: &Settings| s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).map(|r| r[0].path.cost);
        assert_eq!(cost(&s), Ok(1000 + 4000));
        s.costs.unknown_sig_penalty = 1.5;
        assert_eq!(cost(&s), Ok(1000 + 1500));
        s.costs.unknown_sig_broken = true;
        assert!(cost(&s).is_err());
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
        let error = "No route from Jita to Perimeter".to_string();
        assert_eq!(s.explain(error.clone()), error);
        // A "prefer" entry never blocks a route, so it gets no hint.
        let perimeter = uni.exact("Perimeter").unwrap();
        s.avoid.add_system(perimeter);
        assert_eq!(s.explain(error.clone()), error);
        s.avoid.set_system_never(perimeter, true);
        assert_eq!(s.explain(error.clone()), format!("{error}. {NO_ROUTE_HINT}"));
        // Another error stays as it is.
        assert_eq!(s.explain("Give two or more systems".into()), "Give two or more systems");
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
        let back = Settings::from_config(&cfg, &uni).unwrap();
        assert_eq!(back.avoid, s.avoid);
        // A name that the SDE does not know is dropped, and the other entries stay.
        cfg.avoid_systems.push(AvoidName::new("Nowhere", false));
        cfg.avoid_regions.push(AvoidName::new("Nowhere", false));
        let back = Settings::from_config(&cfg, &uni).unwrap();
        assert_eq!((back.avoid.systems.len(), back.avoid.regions.len()), (1, 1));
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
