//! The solar system graph and the name index.

use crate::route::RouteError;
use crate::sde::SdeData;
use crate::wormhole::Wormhole;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Meters in one light year.
pub const LIGHT_YEAR: f64 = 9_460_730_472_580_800.0;

#[derive(Clone)]
pub struct System {
    pub id: u32,
    pub name: String,
    pub region: String,
    pub region_id: u32,
    pub constellation_id: u32,
    pub security: f64,
    /// True if an alliance can hold the system: nullsec outside NPC space. The SDE gives a faction
    /// to an NPC system, constellation or region. Wormhole space, Pochven and Zarzakh are not
    /// capable. Only such a system can hold a sovereignty hub, so only there can an Ansiblex or
    /// a Pharolux work.
    pub sov_null: bool,
    /// The wormhole class from the SDE: the system, else its constellation, else its region.
    /// `None` for a system of known space.
    pub wormhole_class: Option<u32>,
    /// Position in meters.
    pub pos: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Stargate,
    Wormhole(Wormhole),
    JumpBridge,
    Filament(Filament),
}

/// One edge of a Triglavian filament jump: either the activation from a k-space system to the
/// virtual hub of a clade, or the jump from that hub into one system of the clade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filament {
    pub clade: Clade,
    /// `None` for the activation edge. `Some` for the hub-to-system edge: the mean stargate
    /// distance, in jumps, from a random landing system of the clade to this one.
    pub expected_jumps: Option<f32>,
}

/// A Pochven clade: a constellation, each with its own "Filament" items in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Clade {
    Perun,
    Svarog,
    Veles,
}

impl Clade {
    pub const ALL: [Clade; 3] = [Clade::Perun, Clade::Svarog, Clade::Veles];

    /// The constellation ID of the clade (`mapConstellations.jsonl`, region Pochven).
    fn constellation_id(self) -> u32 {
        match self {
            Clade::Perun => 20_000_787,
            Clade::Svarog => 20_000_788,
            Clade::Veles => 20_000_789,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Clade::Perun => "Krai Perun",
            Clade::Svarog => "Krai Svarog",
            Clade::Veles => "Krai Veles",
        }
    }
}

/// Pochven, the region of the three clades. A filament cannot start here: a pilot is already there.
pub const POCHVEN_REGION: u32 = 10_000_070;
/// Yasna Zakh, the region of Zarzakh (one system). A filament is unusable there.
const ZARZAKH_REGION: u32 = 10_001_000;
/// The 33 wormhole space regions. A filament needs a k-space system.
const WORMHOLE_REGIONS: std::ops::RangeInclusive<u32> = 11_000_001..=11_000_033;

/// True if a pilot can activate a filament from a system in this region: highsec, lowsec or
/// nullsec. Never a wormhole system, Pochven itself, or Zarzakh.
fn filament_source_region(region_id: u32) -> bool {
    region_id != POCHVEN_REGION && region_id != ZARZAKH_REGION && !WORMHOLE_REGIONS.contains(&region_id)
}

/// A synthetic system ID for the virtual hub of a clade, far outside the range of real IDs.
fn hub_id(clade: Clade) -> u32 {
    u32::MAX - 16 + Clade::ALL.iter().position(|&c| c == clade).unwrap() as u32
}

/// The stargate distance, in jumps, from each system of `group` to every other system of `group`,
/// using only the gates between two systems of `group`. A BFS from each system, so the result
/// holds the distance of every pair.
fn group_distances(graph: &DiGraph<System, Link>, group: &[NodeIndex]) -> HashMap<NodeIndex, HashMap<NodeIndex, u32>> {
    use petgraph::visit::EdgeRef;
    let set: HashSet<NodeIndex> = group.iter().copied().collect();
    let mut out = HashMap::with_capacity(group.len());
    for &start in group {
        let mut dist = HashMap::from([(start, 0u32)]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            let d = dist[&node];
            for edge in graph.edges(node) {
                let next = edge.target();
                if matches!(edge.weight(), Link::Stargate) && set.contains(&next) && !dist.contains_key(&next) {
                    dist.insert(next, d + 1);
                    queue.push_back(next);
                }
            }
        }
        out.insert(start, dist);
    }
    out
}

/// Add the virtual hub of each clade, with the filament edges: every eligible k-space system to
/// the hub, and the hub to each system of the clade. `source_ok` gives the eligible systems.
/// The clade systems must already have their gates among each other, else a jump counts as the
/// size of the clade (a wide, deliberately pessimistic fallback).
fn add_filaments(graph: &mut DiGraph<System, Link>, by_id: &mut HashMap<u32, NodeIndex>, source_ok: &[NodeIndex], pochven_name: &str) {
    for clade in Clade::ALL {
        let group: Vec<NodeIndex> = graph.node_indices().filter(|&n| graph[n].constellation_id == clade.constellation_id()).collect();
        if group.is_empty() {
            continue;
        }
        let distances = group_distances(graph, &group);
        let pos = {
            let sum = group.iter().fold([0.0; 3], |acc, &n| {
                let p = graph[n].pos;
                [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]]
            });
            let n = group.len() as f64;
            [sum[0] / n, sum[1] / n, sum[2] / n]
        };
        let id = hub_id(clade);
        let hub = graph.add_node(System {
            id,
            name: clade.label().to_string(),
            region: pochven_name.to_string(),
            region_id: POCHVEN_REGION,
            constellation_id: clade.constellation_id(),
            security: -1.0,
            sov_null: false,
            wormhole_class: None,
            pos,
        });
        by_id.insert(id, hub);
        for &s in source_ok {
            graph.add_edge(s, hub, Link::Filament(Filament { clade, expected_jumps: None }));
        }
        for &s in &group {
            let total: u32 = group.iter().map(|&l| distances[&l].get(&s).copied().unwrap_or(group.len() as u32)).sum();
            let expected = total as f32 / group.len() as f32;
            graph.add_edge(hub, s, Link::Filament(Filament { clade, expected_jumps: Some(expected) }));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    High,
    Low,
    Null,
}

/// The security value that the game shows. A value above 0.0 and below 0.05 shows as 0.1.
pub fn display_sec(security: f64) -> f64 {
    if security > 0.0 && security < 0.05 { 0.1 } else { (security * 10.0).round() / 10.0 }
}

pub fn band(security: f64) -> Band {
    let sec = display_sec(security);
    if sec >= 0.5 {
        Band::High
    } else if sec > 0.0 {
        Band::Low
    } else {
        Band::Null
    }
}

#[derive(Clone)]
pub struct Universe {
    pub graph: DiGraph<System, Link>,
    pub by_id: HashMap<u32, NodeIndex>,
    by_name: HashMap<String, NodeIndex>,
    /// Lowercase names in sorted order, for prefix search.
    sorted: Vec<(String, NodeIndex)>,
    pub build: Option<u32>,
}

impl Universe {
    #[tracing::instrument(level = "info", skip_all, fields(systems = data.systems.len(), stargates = data.stargates.len()))]
    pub fn from_sde(data: SdeData) -> Self {
        let npc_regions: HashSet<u32> = data.regions.iter().filter(|r| r.faction_id.is_some()).map(|r| r.id).collect();
        let npc_constellations: HashSet<u32> = data.constellations.iter().filter(|c| c.faction_id.is_some()).map(|c| c.id).collect();
        let region_class: HashMap<u32, u32> = data.regions.iter().filter_map(|r| Some((r.id, r.wormhole_class?))).collect();
        let constellation_class: HashMap<u32, u32> = data.constellations.iter().filter_map(|c| Some((c.id, c.wormhole_class?))).collect();
        let regions: HashMap<u32, String> = data.regions.into_iter().map(|r| (r.id, r.name.en)).collect();
        let mut graph = DiGraph::with_capacity(data.systems.len(), data.stargates.len());
        let mut by_id = HashMap::with_capacity(data.systems.len());
        let mut source_ok = Vec::new();
        for s in data.systems {
            let region = regions.get(&s.region_id).cloned().unwrap_or_default();
            let npc = s.faction_id.is_some() || npc_regions.contains(&s.region_id) || npc_constellations.contains(&s.constellation_id);
            let sov_null = band(s.security) == Band::Null && !WORMHOLE_REGIONS.contains(&s.region_id) && !npc;
            let wormhole_class =
                s.wormhole_class.or_else(|| constellation_class.get(&s.constellation_id).copied()).or_else(|| region_class.get(&s.region_id).copied());
            let node = graph.add_node(System {
                id: s.id,
                name: s.name.en,
                region,
                region_id: s.region_id,
                constellation_id: s.constellation_id,
                security: s.security,
                sov_null,
                wormhole_class,
                pos: [s.position.x, s.position.y, s.position.z],
            });
            by_id.insert(s.id, node);
            if filament_source_region(s.region_id) {
                source_ok.push(node);
            }
        }
        // Each stargate record is one direction of a gate pair.
        for g in data.stargates {
            if let (Some(&a), Some(&b)) = (by_id.get(&g.system_id), by_id.get(&g.destination.system_id)) {
                graph.add_edge(a, b, Link::Stargate);
            }
        }
        // The name index covers only real systems: the virtual hubs of the clades go in afterward,
        // so a search or a waypoint can never resolve to one.
        let mut sorted: Vec<(String, NodeIndex)> = graph.node_indices().map(|n| (graph[n].name.to_lowercase(), n)).collect();
        sorted.sort();
        let by_name = sorted.iter().cloned().collect();
        let pochven_name = regions.get(&POCHVEN_REGION).cloned().unwrap_or_default();
        add_filaments(&mut graph, &mut by_id, &source_ok, &pochven_name);
        Universe { graph, by_id, by_name, sorted, build: data.build }
    }

    pub fn system(&self, node: NodeIndex) -> &System {
        &self.graph[node]
    }

    /// The virtual hub of a clade, if its constellation is in the loaded SDE.
    pub fn filament_hub(&self, clade: Clade) -> Option<NodeIndex> {
        self.by_id.get(&hub_id(clade)).copied()
    }

    /// True if an alliance can hold the system. See `System::sov_null`.
    pub fn sov_null(&self, node: NodeIndex) -> bool {
        self.graph[node].sov_null
    }

    pub fn name(&self, node: NodeIndex) -> &str {
        &self.graph[node].name
    }

    /// Find a system by its exact name, case-insensitive.
    pub fn exact(&self, name: &str) -> Option<NodeIndex> {
        self.by_name.get(&name.trim().to_lowercase()).copied()
    }

    /// All system names that start with `prefix`, case-insensitive.
    pub fn complete(&self, prefix: &str) -> Vec<NodeIndex> {
        let prefix = prefix.trim().to_lowercase();
        let start = self.sorted.partition_point(|(n, _)| n.as_str() < prefix.as_str());
        self.sorted[start..].iter().take_while(|(n, _)| n.starts_with(&prefix)).map(|&(_, node)| node).collect()
    }

    /// Up to `limit` systems for a search box: the names that start with `query` first,
    /// then the names that contain it. Case-insensitive.
    pub fn search(&self, query: &str, limit: usize) -> Vec<NodeIndex> {
        self.search_where(query, limit, |_| true)
    }

    /// Like `search`, but the result holds only the systems that `keep` accepts.
    pub fn search_where(&self, query: &str, limit: usize, keep: impl Fn(NodeIndex) -> bool) -> Vec<NodeIndex> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<NodeIndex> = self.complete(&query).into_iter().filter(|&n| keep(n)).take(limit).collect();
        if found.len() < limit {
            let contains = self.sorted.iter().filter(|(n, _)| !n.starts_with(&query) && n.contains(&query));
            found.extend(contains.map(|&(_, node)| node).filter(|&n| keep(n)).take(limit - found.len()));
        }
        found
    }

    /// Find a system by its exact name, else by a unique prefix.
    /// On failure, the error holds up to 5 candidate names for the error text.
    pub fn resolve(&self, name: &str) -> Result<NodeIndex, RouteError> {
        if let Some(node) = self.exact(name) {
            return Ok(node);
        }
        let matches = self.complete(name);
        match matches.as_slice() {
            [one] => Ok(*one),
            [] => Err(RouteError::UnknownSystem(name.to_string())),
            many => Err(RouteError::AmbiguousSystem {
                name: name.to_string(),
                candidates: many.iter().take(5).map(|&n| self.name(n).to_string()).collect(),
            }),
        }
    }

    /// The number of wormholes and jump bridges. A connection in both directions counts once.
    pub fn shortcut_counts(&self) -> (usize, usize) {
        let mut wormholes = std::collections::HashSet::new();
        let mut bridges = std::collections::HashSet::new();
        for e in self.graph.raw_edges() {
            let pair = (e.source().min(e.target()), e.source().max(e.target()));
            match e.weight {
                Link::Wormhole(_) => wormholes.insert(pair),
                Link::JumpBridge => bridges.insert(pair),
                Link::Stargate | Link::Filament(_) => false,
            };
        }
        (wormholes.len(), bridges.len())
    }

    /// The number of systems with a wormhole to the system `id`, for example Thera.
    /// Two wormholes between the same pair count one time, as in `shortcut_counts`.
    pub fn hub_count(&self, id: u32) -> usize {
        use petgraph::visit::EdgeRef;
        let Some(&hub) = self.by_id.get(&id) else { return 0 };
        let ends: std::collections::HashSet<NodeIndex> =
            self.graph.edges(hub).filter(|e| matches!(e.weight(), Link::Wormhole(_))).map(|e| e.target()).collect();
        ends.len()
    }

    /// Add wormhole edges in both directions. This is the only place that adds them.
    /// Call it after the stargates and the bridge list. Return the count of wormholes added.
    #[tracing::instrument(level = "debug", skip_all, fields(holes = holes.len()))]
    pub fn add_wormholes(&mut self, holes: &[Wormhole]) -> usize {
        let mut added = 0;
        for w in holes {
            let (Some(&a), Some(&b)) = (self.by_id.get(&w.a), self.by_id.get(&w.b)) else {
                continue;
            };
            self.graph.add_edge(a, b, Link::Wormhole(w.clone()));
            self.graph.add_edge(b, a, Link::Wormhole(w.clone()));
            added += 1;
        }
        tracing::debug!(added, "wormholes added");
        added
    }

    /// Straight-line distance in light years.
    pub fn distance_ly(&self, a: NodeIndex, b: NodeIndex) -> f64 {
        let (pa, pb) = (self.graph[a].pos, self.graph[b].pos);
        let d: f64 = (0..3).map(|i| (pa[i] - pb[i]).powi(2)).sum();
        d.sqrt() / LIGHT_YEAR
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::test_support::{sde_dir, universe};

    #[test]
    fn only_player_nullsec_can_hold_sovereignty() {
        let uni = universe();
        let capable = |name: &str| uni.sov_null(uni.exact(name).unwrap_or_else(|| panic!("{name} is not on the map")));
        // Player nullsec: Tenerifis, and a Delve constellation without a faction.
        for name in ["UALX-3", "JK-Q77", "HM-XR2"] {
            assert!(capable(name), "{name}");
        }
        // Highsec, lowsec, wormhole space, Pochven and Zarzakh.
        for name in ["Jita", "Perimeter", "Amarr", "Thera", "Kuharah", "Zarzakh"] {
            assert!(!capable(name), "{name}");
        }
        // NPC regions, and NPC constellations in a region that players hold (Delve, Fountain).
        for name in ["PO4F-3", "I-YGGI", "D-SKWC", "MB4D-4", "KFIE-Z", "MN5N-X"] {
            assert!(!capable(name), "{name}");
        }
        // A virtual hub is never a system.
        assert!(uni.filament_hub(Clade::Perun).is_none_or(|hub| !uni.sov_null(hub)));
    }

    #[test]
    fn systems_get_the_wormhole_class_from_the_sde() {
        let uni = universe();
        let class = |name: &str| uni.system(uni.exact(name).unwrap_or_else(|| panic!("{name} is not on the map"))).wormhole_class;
        // A Drifter system has its own class. Thera takes the class of its region.
        assert_eq!(class("Sentinel MZ"), Some(14));
        assert_eq!(class("Azdaja Redoubt"), Some(18));
        assert_eq!(class("Thera"), Some(12));
        // A system of wormhole space takes the class of its constellation, and known space has a class too (Pochven is 25).
        assert_eq!(class("J105443"), Some(1));
        assert_eq!(class("Jita"), Some(7));
        assert_eq!(class("Tama"), Some(8));
        assert_eq!(class("Kuharah"), Some(25));
        // Exactly five systems carry a Drifter class.
        let drifters = uni.graph.node_weights().filter(|s| s.wormhole_class.is_some_and(|c| (14..=18).contains(&c))).count();
        assert_eq!(drifters, 5);
    }

    #[test]
    fn search_puts_prefix_matches_first() {
        let uni = universe();
        let names: Vec<&str> = uni.search("ama", 50).iter().map(|&n| uni.name(n)).collect();
        let prefix = names.iter().take_while(|n| n.to_lowercase().starts_with("ama")).count();
        assert!(names.contains(&"Amarr"), "{names:?}");
        assert!(prefix > 0 && prefix < names.len(), "{names:?}");
        assert!(names[prefix..].iter().all(|n| n.to_lowercase().contains("ama")), "{names:?}");
        assert_eq!(uni.search("ama", 3).len(), 3);
        assert!(uni.search("  ", 10).is_empty());
        // A filter applies before the limit, so a rare match still shows.
        let held = uni.search_where("ual", 3, |n| uni.sov_null(n));
        assert!(held.iter().all(|&n| uni.sov_null(n)) && held.iter().any(|&n| uni.name(n) == "UALX-3"), "{held:?}");
        assert!(uni.search_where("jita", 5, |n| uni.sov_null(n)).iter().all(|&n| uni.name(n) != "Jita"));
    }

    #[test]
    fn stargate_edge_count() {
        let uni = universe();
        let gates = uni.graph.edge_weights().filter(|l| **l == Link::Stargate).count();
        assert_eq!(gates, 13978);
        // One virtual hub for each clade, beside the real systems.
        assert_eq!(uni.graph.node_count(), 8490 + Clade::ALL.len());
    }

    #[test]
    fn each_clade_has_a_hub_with_nine_systems_and_filament_edges() {
        use petgraph::visit::EdgeRef;
        let uni = universe();
        for clade in Clade::ALL {
            let hub = uni.filament_hub(clade).unwrap_or_else(|| panic!("no hub for {clade:?}"));
            assert_eq!(uni.system(hub).name, clade.label());
            assert_eq!(uni.system(hub).region, "Pochven");
            let out: Vec<_> = uni.graph.edges(hub).collect();
            assert_eq!(out.len(), 9, "{clade:?} has {} systems", out.len());
            for e in &out {
                let Link::Filament(f) = e.weight() else { panic!("not a filament edge") };
                assert_eq!(f.clade, clade);
                let expected = f.expected_jumps.expect("a hub-to-system edge has an expected jump count");
                assert!((0.0..9.0).contains(&expected), "{expected} is out of range for a 9-system clade");
            }
            // A plain highsec system can reach the hub. A Pochven system cannot, because a pilot
            // is already there.
            let jita = uni.exact("Jita").unwrap();
            assert!(uni.graph.edges_connecting(jita, hub).next().is_some());
            let (_, first_system) = uni.graph.edges(hub).next().map(|e| (e.id(), e.target())).unwrap();
            assert!(uni.graph.edges_connecting(first_system, hub).next().is_none());
        }
    }

    /// A placeholder system, for a test graph where only the connectivity matters.
    fn placeholder(name: &str) -> System {
        System {
            id: 0,
            name: name.into(),
            region: String::new(),
            region_id: 0,
            constellation_id: 0,
            security: 0.0,
            sov_null: false,
            wormhole_class: None,
            pos: [0.0; 3],
        }
    }

    /// A chain of 9 systems, each linked to the next by a stargate in both directions.
    fn chain_of_nine() -> (DiGraph<System, Link>, Vec<NodeIndex>) {
        let mut graph = DiGraph::new();
        let group: Vec<NodeIndex> = (0..9).map(|i| graph.add_node(placeholder(&i.to_string()))).collect();
        for w in group.windows(2) {
            graph.add_edge(w[0], w[1], Link::Stargate);
            graph.add_edge(w[1], w[0], Link::Stargate);
        }
        (graph, group)
    }

    /// The mean distance from every system of `group` to `target`, matching `add_filaments`.
    fn mean_distance(dist: &HashMap<NodeIndex, HashMap<NodeIndex, u32>>, group: &[NodeIndex], target: NodeIndex) -> f32 {
        let total: u32 = group.iter().map(|&l| dist[&l][&target]).sum();
        total as f32 / group.len() as f32
    }

    #[test]
    fn the_mean_distance_of_a_chain_matches_the_example() {
        let (graph, group) = chain_of_nine();
        let dist = group_distances(&graph, &group);
        // An end of the chain: the mean of 0..=8 is 4.0.
        assert!((mean_distance(&dist, &group, group[0]) - 4.0).abs() < 1e-6);
        // The middle of the chain: the mean of |4-j| for j in 0..=8 is 2.222...
        assert!((mean_distance(&dist, &group, group[4]) - 20.0 / 9.0).abs() < 1e-6);
        // A system one step from the end: the mean of |1-j| is 29 / 9.
        assert!((mean_distance(&dist, &group, group[1]) - 29.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn a_gate_outside_the_group_is_not_a_shortcut() {
        // A chain of 9, plus one more system joined only to the middle. It must not shorten any
        // distance inside the chain, because `group_distances` uses gates inside the group only.
        let (mut graph, group) = chain_of_nine();
        let outside = graph.add_node(placeholder("outside"));
        graph.add_edge(group[4], outside, Link::Stargate);
        graph.add_edge(outside, group[4], Link::Stargate);
        let dist = group_distances(&graph, &group);
        assert!((mean_distance(&dist, &group, group[0]) - 4.0).abs() < 1e-6);
        assert!(!dist[&group[0]].contains_key(&outside));
    }

    #[test]
    fn security_bands() {
        assert_eq!(band(0.45), Band::High);
        assert_eq!(band(0.44), Band::Low);
        assert_eq!(band(0.01), Band::Low);
        assert_eq!(band(0.0), Band::Null);
        assert_eq!(band(-0.5), Band::Null);
    }

    #[test]
    fn resolve_names() {
        let uni = universe();
        assert_eq!(uni.system(uni.resolve("jita").unwrap()).id, 30000142);
        assert_eq!(uni.system(uni.resolve("New Caldari").unwrap()).id, uni.system(uni.exact("new caldari").unwrap()).id);
        assert!(uni.resolve("J").is_err());
        assert_eq!(uni.resolve("Nowhere"), Err(RouteError::UnknownSystem("Nowhere".into())));
        let Err(RouteError::AmbiguousSystem { name, candidates }) = uni.resolve("J") else { panic!("not ambiguous") };
        assert_eq!(name, "J");
        assert!(candidates.len() > 1 && candidates.len() <= 5);
    }

    #[test]
    fn hub_count_counts_wormhole_pairs() {
        use crate::test_support::hole;
        use crate::wormhole::{THERA, TURNUR};
        let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
        // Two Thera wormholes to Jita count as one pair, as in `shortcut_counts`.
        let holes = [hole(30000142, THERA), hole(30000142, THERA), hole(30002187, THERA)];
        assert_eq!(uni.add_wormholes(&holes), 3);
        assert_eq!(uni.hub_count(THERA), 2);
        // The gates of Turnur do not count.
        assert_eq!(uni.hub_count(TURNUR), 0);
    }
}
