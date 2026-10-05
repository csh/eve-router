//! Route costs, k-shortest paths (Yen's algorithm) and waypoint chains.

use crate::ansiblex::BridgeRules;
use crate::universe::{Band, Link, Universe, band};
use petgraph::algo::{astar, dijkstra};
use petgraph::graph::{EdgeIndex, EdgeReference, NodeIndex};
use petgraph::visit::{EdgeFiltered, EdgeRef};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// The cost of one jump into an unwanted system. It is larger than any jump count,
/// so a route first has the fewest unwanted jumps, then the fewest jumps.
pub const PENALTY: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Shortest,
    PreferHighsec,
    LessSecure,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Shortest, Mode::PreferHighsec, Mode::LessSecure];

    pub fn title(self) -> &'static str {
        match self {
            Mode::Shortest => "Shortest",
            Mode::PreferHighsec => "Prefer highsec",
            Mode::LessSecure => "Less secure",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Mode::Shortest => "The fewest jumps, at any security",
            Mode::PreferHighsec => "Stay in highsec where a route exists",
            Mode::LessSecure => "Stay in lowsec and nullsec where a route exists",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Shortest => "shortest",
            Mode::PreferHighsec => "prefer-highsec",
            Mode::LessSecure => "less-secure",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub nodes: Vec<NodeIndex>,
    pub edges: Vec<EdgeIndex>,
    pub cost: u64,
}

pub struct Route {
    pub path: Path,
    pub jumps: usize,
    pub wormholes: usize,
    pub bridges: usize,
    /// The sum of the bridge jump costs. `None` when no hull is set.
    pub bridge_tj: Option<f32>,
    /// The step index of each given system: the start, the midpoints, the destination.
    pub stops: Vec<usize>,
}

/// The part that a step plays in a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Start,
    /// The midpoint number, from 1.
    Midpoint(usize),
    Destination,
}

impl Stop {
    pub fn label(self) -> String {
        match self {
            Stop::Start => "Start".into(),
            Stop::Midpoint(n) => format!("Midpoint {n}"),
            Stop::Destination => "Destination".into(),
        }
    }
}

impl Route {
    /// The stop at a step, if the step is one of the given systems.
    pub fn stop_at(&self, step: usize) -> Option<Stop> {
        let k = self.stops.iter().position(|&s| s == step)?;
        Some(match k {
            0 => Stop::Start,
            k if k + 1 == self.stops.len() => Stop::Destination,
            k => Stop::Midpoint(k),
        })
    }
}

pub struct Router<'a> {
    pub uni: &'a Universe,
    pub rules: BridgeRules,
    /// The cost of a jump into each node, by `NodeIndex`.
    node_cost: Vec<u64>,
    /// True if the current settings allow the edge, by `EdgeIndex`.
    edge_ok: Vec<bool>,
}

/// A Yen candidate path: the cost, the nodes and the edges, cheapest first.
type Candidate = Reverse<(u64, Vec<NodeIndex>, Vec<EdgeIndex>)>;

struct Bans {
    /// Banned nodes, by `NodeIndex`.
    nodes: Vec<bool>,
    pairs: HashSet<(NodeIndex, NodeIndex)>,
}

impl Bans {
    fn new(node_count: usize) -> Self {
        Bans { nodes: vec![false; node_count], pairs: HashSet::new() }
    }
}

impl<'a> Router<'a> {
    /// Calculate the node costs and the edge permissions one time, in parallel.
    /// Each search then reads them instead of calculating them again.
    pub fn new(uni: &'a Universe, mode: Mode, wormholes: bool, bridges: bool, rules: BridgeRules) -> Self {
        let graph = &uni.graph;
        let node_cost = graph
            .raw_nodes()
            .par_iter()
            .map(|n| {
                let b = band(n.weight.security);
                let unwanted = match mode {
                    Mode::Shortest => false,
                    Mode::PreferHighsec => b != Band::High,
                    Mode::LessSecure => b == Band::High,
                };
                if unwanted { 1 + PENALTY } else { 1 }
            })
            .collect();
        let edge_ok = graph
            .raw_edges()
            .par_iter()
            .map(|e| match e.weight {
                Link::Stargate => true,
                Link::Wormhole { .. } => wormholes,
                Link::JumpBridge => bridges && rules.allowed(uni, e.source()),
            })
            .collect();
        Router { uni, rules, node_cost, edge_ok }
    }

    fn link_allowed(&self, e: EdgeReference<'_, Link>) -> bool {
        self.edge_ok[e.id().index()]
    }

    /// The cost of a jump into `node`.
    fn enter_cost(&self, node: NodeIndex) -> u64 {
        self.node_cost[node.index()]
    }

    fn usable(&self, e: EdgeReference<'_, Link>, bans: &Bans) -> bool {
        self.link_allowed(e) && !bans.nodes[e.target().index()] && !bans.pairs.contains(&(e.source(), e.target()))
    }

    /// The edge for one step of a path. A stargate comes first, because it needs no overlay.
    fn pick_edge(&self, a: NodeIndex, b: NodeIndex, bans: &Bans) -> EdgeIndex {
        self.uni
            .graph
            .edges_connecting(a, b)
            .filter(|e| self.usable(*e, bans))
            .min_by_key(|e| match e.weight() {
                Link::Stargate => 0,
                Link::JumpBridge => 1,
                Link::Wormhole { .. } => 2,
            })
            .map(|e| e.id())
            .expect("astar returned a step with no usable edge")
    }

    fn shortest(&self, from: NodeIndex, to: NodeIndex, bans: &Bans) -> Option<Path> {
        let graph = EdgeFiltered::from_fn(&self.uni.graph, |e: EdgeReference<'_, Link>| self.usable(e, bans));
        let (cost, nodes) = astar(&graph, from, |n| n == to, |e| self.enter_cost(e.target()), |_| 0)?;
        let edges = nodes.windows(2).map(|w| self.pick_edge(w[0], w[1], bans)).collect();
        Some(Path { nodes, edges, cost })
    }

    /// The `k` cheapest loopless paths, with Yen's algorithm.
    pub fn k_shortest(&self, from: NodeIndex, to: NodeIndex, k: usize) -> Vec<Path> {
        let node_count = self.uni.graph.node_count();
        let Some(first) = self.shortest(from, to, &Bans::new(node_count)) else {
            return Vec::new();
        };
        let mut found = vec![first];
        let mut seen: HashSet<Vec<NodeIndex>> = HashSet::from([found[0].nodes.clone()]);
        let mut candidates: BinaryHeap<Candidate> = BinaryHeap::new();

        while found.len() < k {
            let prev = found.last().unwrap();
            // Each spur search is independent of the others, so they run in parallel.
            let spurs: Vec<Candidate> = (0..prev.nodes.len() - 1)
                .into_par_iter()
                .filter_map(|i| {
                    let spur = prev.nodes[i];
                    let root = &prev.nodes[..=i];
                    let mut bans = Bans::new(node_count);
                    for p in &found {
                        if p.nodes.len() > i + 1 && p.nodes[..=i] == *root {
                            bans.pairs.insert((p.nodes[i], p.nodes[i + 1]));
                        }
                    }
                    for n in &root[..i] {
                        bans.nodes[n.index()] = true;
                    }
                    let spur_path = self.shortest(spur, to, &bans)?;
                    let mut nodes = root.to_vec();
                    nodes.extend(&spur_path.nodes[1..]);
                    let mut edges = prev.edges[..i].to_vec();
                    edges.extend(spur_path.edges);
                    let root_cost: u64 = root[1..].iter().map(|&n| self.enter_cost(n)).sum();
                    Some(Reverse((root_cost + spur_path.cost, nodes, edges)))
                })
                .collect();
            for candidate in spurs {
                if seen.insert(candidate.0.1.clone()) {
                    candidates.push(candidate);
                }
            }
            let Some(Reverse((cost, nodes, edges))) = candidates.pop() else {
                break;
            };
            found.push(Path { nodes, edges, cost });
        }
        found
    }

    /// The `n` cheapest routes through all waypoints, in order.
    pub fn routes(&self, waypoints: &[NodeIndex], n: usize) -> Result<Vec<Route>, String> {
        if waypoints.len() < 2 {
            return Err("Give two or more systems".into());
        }
        let legs: Vec<Vec<Path>> = waypoints
            .par_windows(2)
            .map(|w| self.k_shortest(w[0], w[1], n))
            .collect();
        for (w, leg) in waypoints.windows(2).zip(&legs) {
            if leg.is_empty() {
                return Err(format!("No route from {} to {}", self.uni.name(w[0]), self.uni.name(w[1])));
            }
        }

        // Best-first search over one path index for each leg.
        let cost_of = |idx: &[usize]| idx.iter().zip(&legs).map(|(&i, leg)| leg[i].cost).sum::<u64>();
        let start = vec![0; legs.len()];
        let mut heap = BinaryHeap::from([Reverse((cost_of(&start), start.clone()))]);
        let mut queued = HashSet::from([start]);
        let mut seen_nodes = HashSet::new();
        let mut routes = Vec::new();
        while let Some(Reverse((cost, idx))) = heap.pop() {
            let mut path = Path { nodes: vec![waypoints[0]], edges: Vec::new(), cost };
            let mut stops = vec![0];
            for (&i, leg) in idx.iter().zip(&legs) {
                path.nodes.extend(&leg[i].nodes[1..]);
                path.edges.extend(&leg[i].edges);
                stops.push(path.edges.len());
            }
            if seen_nodes.insert(path.nodes.clone()) {
                let mut route = self.summarize(path);
                route.stops = stops;
                routes.push(route);
                if routes.len() == n {
                    break;
                }
            }
            for leg in 0..idx.len() {
                if idx[leg] + 1 < legs[leg].len() {
                    let mut next = idx.clone();
                    next[leg] += 1;
                    if queued.insert(next.clone()) {
                        heap.push(Reverse((cost_of(&next), next)));
                    }
                }
            }
        }
        Ok(routes)
    }

    fn summarize(&self, path: Path) -> Route {
        let mut route =
            Route { jumps: path.edges.len(), wormholes: 0, bridges: 0, bridge_tj: None, stops: Vec::new(), path };
        let mut tj = Some(0.0);
        for &e in &route.path.edges {
            match &self.uni.graph[e] {
                Link::Wormhole { .. } => route.wormholes += 1,
                Link::JumpBridge => {
                    route.bridges += 1;
                    let (from, _) = self.uni.graph.edge_endpoints(e).unwrap();
                    let cost = self.rules.cost(self.uni, from).and_then(|c| c.tj);
                    tj = tj.zip(cost).map(|(a, b)| a + b);
                }
                Link::Stargate => {}
            }
        }
        route.bridge_tj = if route.bridges > 0 { tj } else { None };
        route
    }

    /// The jump count from `origin` to each target, in the current mode. One search covers all targets.
    pub fn jumps_to(&self, origin: NodeIndex, targets: &[NodeIndex]) -> Vec<(NodeIndex, Option<u64>)> {
        let graph = EdgeFiltered::from_fn(&self.uni.graph, |e: EdgeReference<'_, Link>| self.link_allowed(e));
        let costs = dijkstra(&graph, origin, None, |e| self.enter_cost(e.target()));
        // Each jump costs 1, and each unwanted jump adds PENALTY. Thus the remainder is the jump count.
        targets.iter().map(|&t| (t, costs.get(&t).map(|c| c % PENALTY))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::tests::universe;

    fn router(mode: Mode) -> Router<'static> {
        Router::new(universe(), mode, true, true, BridgeRules::default())
    }

    fn node(name: &str) -> NodeIndex {
        universe().exact(name).unwrap()
    }

    #[test]
    fn jita_perimeter_is_one_jump() {
        for mode in [Mode::Shortest, Mode::PreferHighsec, Mode::LessSecure] {
            let routes = router(mode).routes(&[node("Jita"), node("Perimeter")], 1).unwrap();
            assert_eq!(routes[0].jumps, 1);
        }
    }

    #[test]
    fn prefer_highsec_stays_in_highsec() {
        let uni = universe();
        let short = router(Mode::Shortest).routes(&[node("Jita"), node("Amarr")], 1).unwrap();
        let safe = router(Mode::PreferHighsec).routes(&[node("Jita"), node("Amarr")], 1).unwrap();
        assert!(short[0].jumps <= safe[0].jumps);
        assert!(safe[0].path.nodes.iter().all(|&n| band(uni.system(n).security) == Band::High));
    }

    #[test]
    fn yen_gives_unique_sorted_paths() {
        let paths = router(Mode::Shortest).k_shortest(node("Jita"), node("Dodixie"), 8);
        assert_eq!(paths.len(), 8);
        assert!(paths.windows(2).all(|w| w[0].cost <= w[1].cost));
        let unique: HashSet<_> = paths.iter().map(|p| p.nodes.clone()).collect();
        assert_eq!(unique.len(), 8);
        for p in &paths {
            let set: HashSet<_> = p.nodes.iter().collect();
            assert_eq!(set.len(), p.nodes.len(), "path has a loop");
        }
    }

    #[test]
    fn waypoints_chain() {
        let r = router(Mode::Shortest);
        let direct = r.routes(&[node("Jita"), node("Amarr")], 1).unwrap()[0].jumps;
        let via = r.routes(&[node("Jita"), node("Rens"), node("Amarr")], 3).unwrap();
        assert_eq!(via.len(), 3);
        assert!(via[0].jumps >= direct);
        assert!(via[0].path.nodes.contains(&node("Rens")));
        // The stops mark Jita, Rens and Amarr.
        let route = &via[0];
        let at = |stop: usize| route.path.nodes[route.stops[stop]];
        assert_eq!([at(0), at(1), at(2)], [node("Jita"), node("Rens"), node("Amarr")]);
        assert_eq!(route.stop_at(0), Some(Stop::Start));
        assert_eq!(route.stop_at(route.stops[1]), Some(Stop::Midpoint(1)));
        assert_eq!(route.stop_at(route.jumps), Some(Stop::Destination));
        assert_eq!(route.stop_at(1), None);
    }

    #[test]
    fn hub_sidebar() {
        let targets = ["Jita", "Amarr", "Dodixie", "Hek", "Rens"].map(node);
        let hubs = router(Mode::Shortest).jumps_to(node("Jita"), &targets);
        assert_eq!(hubs[0], (node("Jita"), Some(0)));
        assert!(hubs.iter().all(|(_, j)| j.is_some()));
    }
}
