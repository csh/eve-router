//! Route costs, k-shortest paths (Yen's algorithm) and waypoint chains.

use crate::ansiblex::BridgeRules;
use crate::universe::{Band, Link, Universe, band};
use crate::wormhole::{Hubs, Wormhole};
use petgraph::algo::{astar, dijkstra};
use petgraph::graph::{EdgeIndex, EdgeReference, NodeIndex};
use petgraph::visit::{EdgeFiltered, EdgeRef};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// The cost of one jump, in milli-jumps. Soft costs (a bridge capacitor, an unknown signature)
/// add fractions of a jump to it.
pub const JUMP: u64 = 1000;

/// The cost of one jump into an unwanted system. It is larger than any jump cost,
/// so a route first has the fewest unwanted jumps, then the lowest cost.
pub const PENALTY: u64 = 1_000_000 * JUMP;

/// The default cost of 1% of the gate capacitor, in milli-jumps. A bridge jump of 5% costs 3 jumps more.
pub const DEFAULT_CAP_WEIGHT: u32 = 600;

/// The default extra cost of a wormhole with no known signature at its departure system, in
/// milli-jumps. The pilot must scan the hole down first.
pub const DEFAULT_UNKNOWN_SIG_PENALTY: u32 = 4 * JUMP as u32;

/// The most midpoints that "optimize order" takes. Held-Karp keeps m × 2^m states for m midpoints.
pub const MAX_MIDPOINTS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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
    /// The sum of the shares of the gate capacitor that the bridge jumps use, in percent.
    /// `None` when no hull is set.
    pub bridge_cap_pct: Option<f32>,
    /// The number of wormhole jumps with no known signature at the departure system.
    pub unknown_sigs: usize,
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

/// True if the wormhole has no known signature in the system `from`.
fn sig_unknown(uni: &Universe, w: &Wormhole, from: NodeIndex) -> bool {
    w.sig_at(uni.system(from).id).is_none()
}

/// What a `Router` needs to price the links of a universe.
#[derive(Clone, Copy)]
pub struct RouterOptions {
    pub mode: Mode,
    pub wormholes: bool,
    /// Switches the wormholes of each hub (Thera, Turnur) on and off.
    pub hubs: Hubs,
    pub bridges: bool,
    pub rules: BridgeRules,
    /// The cost of 1% of the gate capacitor, in milli-jumps. It prices a bridge jump with a hull.
    pub cap_weight: u32,
    /// The extra cost of a wormhole with no known signature at its departure system, in milli-jumps.
    pub unknown_sig_penalty: u32,
    /// Drop the wormholes with no known signature at their departure system, instead of a penalty.
    pub unknown_sig_broken: bool,
    /// Unix seconds. It sets which wormholes are expired.
    pub now: u64,
}

impl Default for RouterOptions {
    fn default() -> Self {
        RouterOptions {
            mode: Mode::Shortest,
            wormholes: true,
            hubs: Hubs::default(),
            bridges: true,
            rules: BridgeRules::default(),
            cap_weight: DEFAULT_CAP_WEIGHT,
            unknown_sig_penalty: DEFAULT_UNKNOWN_SIG_PENALTY,
            unknown_sig_broken: false,
            now: 0,
        }
    }
}

pub struct Router<'a> {
    pub uni: &'a Universe,
    pub rules: BridgeRules,
    /// The cost of a jump into each node, by `NodeIndex`. It includes `node_danger`.
    node_cost: Vec<u64>,
    /// The danger cost of each node, in milli-jumps, by `NodeIndex`. It is zero until `set_danger`.
    node_danger: Vec<u64>,
    /// True if the current settings allow the edge, by `EdgeIndex`.
    edge_ok: Vec<bool>,
    /// The cost that an edge adds to the cost of a jump into its target, by `EdgeIndex`.
    edge_extra: Vec<u64>,
}

/// A Yen candidate path: the cost, the nodes and the edges, cheapest first.
type Candidate = Reverse<(u64, Vec<NodeIndex>, Vec<EdgeIndex>)>;

struct Bans {
    /// Banned nodes, by `NodeIndex`.
    nodes: Vec<bool>,
    edges: HashSet<EdgeIndex>,
}

impl Bans {
    fn new(node_count: usize) -> Self {
        Bans { nodes: vec![false; node_count], edges: HashSet::new() }
    }
}

impl<'a> Router<'a> {
    /// Calculate the node costs and the edge permissions one time, in parallel.
    /// Each search then reads them instead of calculating them again.
    pub fn new(uni: &'a Universe, options: RouterOptions) -> Self {
        let RouterOptions { mode, wormholes, hubs, bridges, rules, cap_weight, unknown_sig_penalty, unknown_sig_broken, now } = options;
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
                if unwanted { JUMP + PENALTY } else { JUMP }
            })
            .collect();
        let hull_kg = rules.hull.and_then(|h| h.mass_kg);
        let edge_ok = graph
            .raw_edges()
            .par_iter()
            .map(|e| match &e.weight {
                Link::Stargate => true,
                Link::Wormhole(w) => {
                    wormholes && hubs.allows(w) && w.usable(now, hull_kg) && !(unknown_sig_broken && sig_unknown(uni, w, e.source()))
                }
                Link::JumpBridge => bridges && rules.allowed(uni, e.source()),
            })
            .collect();
        // A bridge jump with a hull costs more as it uses more of the gate capacitor.
        let edge_extra = graph
            .raw_edges()
            .par_iter()
            .map(|e| match e.weight {
                Link::JumpBridge if bridges => {
                    let pct = rules.cost(uni, e.source()).and_then(|c| c.cap_pct).unwrap_or(0.0);
                    (f64::from(pct) * f64::from(cap_weight)).round() as u64
                }
                Link::Wormhole(ref w) if wormholes && sig_unknown(uni, w, e.source()) => u64::from(unknown_sig_penalty),
                _ => 0,
            })
            .collect();
        let node_danger = vec![0; graph.node_count()];
        Router { uni, rules, node_cost, node_danger, edge_ok, edge_extra }
    }

    fn link_allowed(&self, e: EdgeReference<'_, Link>) -> bool {
        self.edge_ok[e.id().index()]
    }

    /// Set the danger cost of each system, in milli-jumps, by `NodeIndex`. A jump into a system pays
    /// its danger cost. A later call replaces the earlier danger costs. This is the place for data
    /// such as the recent deaths in a system, or a gate camp.
    pub fn set_danger(&mut self, danger: Vec<u64>) {
        assert_eq!(danger.len(), self.node_cost.len(), "one danger cost for each system");
        for ((cost, old), new) in self.node_cost.iter_mut().zip(&self.node_danger).zip(&danger) {
            *cost = *cost - old + new;
        }
        self.node_danger = danger;
    }

    /// The cost of a jump along an edge: the cost of the target plus the extra cost of the edge.
    fn step_cost(&self, e: EdgeReference<'_, Link>) -> u64 {
        self.node_cost[e.target().index()] + self.edge_extra[e.id().index()]
    }

    /// The cost of a jump along the edge `e`.
    fn edge_cost(&self, e: EdgeIndex) -> u64 {
        let (_, target) = self.uni.graph.edge_endpoints(e).expect("an edge of the graph");
        self.node_cost[target.index()] + self.edge_extra[e.index()]
    }

    fn usable(&self, e: EdgeReference<'_, Link>, bans: &Bans) -> bool {
        self.link_allowed(e) && !bans.nodes[e.target().index()] && !bans.edges.contains(&e.id())
    }

    /// The edge for one step of a path: the cheapest edge between the two systems. At equal cost,
    /// a stargate comes first, because it needs no overlay.
    fn pick_edge(&self, a: NodeIndex, b: NodeIndex, bans: &Bans) -> EdgeIndex {
        self.uni
            .graph
            .edges_connecting(a, b)
            .filter(|e| self.usable(*e, bans))
            .min_by_key(|e| {
                let rank = match e.weight() {
                    Link::Stargate => 0,
                    Link::JumpBridge => 1,
                    Link::Wormhole(_) => 2,
                };
                (self.step_cost(*e), rank)
            })
            .map(|e| e.id())
            .expect("astar returned a step with no usable edge")
    }

    fn shortest(&self, from: NodeIndex, to: NodeIndex, bans: &Bans) -> Option<Path> {
        let graph = EdgeFiltered::from_fn(&self.uni.graph, |e: EdgeReference<'_, Link>| self.usable(e, bans));
        let (cost, nodes) = astar(&graph, from, |n| n == to, |e| self.step_cost(e), |_| 0)?;
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
        let mut seen: HashSet<Vec<EdgeIndex>> = HashSet::from([found[0].edges.clone()]);
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
                    // A found path with the same root edges gives up the edge that follows the root.
                    for p in &found {
                        if p.edges.len() > i && p.edges[..i] == prev.edges[..i] {
                            bans.edges.insert(p.edges[i]);
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
                    let root_cost: u64 = prev.edges[..i].iter().map(|&e| self.edge_cost(e)).sum();
                    Some(Reverse((root_cost + spur_path.cost, nodes, edges)))
                })
                .collect();
            for candidate in spurs {
                if seen.insert(candidate.0.2.clone()) {
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
        let legs: Vec<Vec<Path>> = waypoints.par_windows(2).map(|w| self.k_shortest(w[0], w[1], n)).collect();
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
        let mut seen_edges = HashSet::new();
        let mut routes = Vec::new();
        while let Some(Reverse((cost, idx))) = heap.pop() {
            let mut path = Path { nodes: vec![waypoints[0]], edges: Vec::new(), cost };
            let mut stops = vec![0];
            for (&i, leg) in idx.iter().zip(&legs) {
                path.nodes.extend(&leg[i].nodes[1..]);
                path.edges.extend(&leg[i].edges);
                stops.push(path.edges.len());
            }
            if seen_edges.insert(path.edges.clone()) {
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

    /// The waypoints in the cheapest order, as a classic TSP. Each system gets one visit: a repeated
    /// midpoint merges, and a midpoint equal to the start or the destination drops out. The start and
    /// the destination stay in place. A tie keeps the given order.
    pub fn optimize(&self, waypoints: &[NodeIndex]) -> Result<Vec<NodeIndex>, String> {
        let [from, inner @ .., to] = waypoints else {
            return Ok(waypoints.to_vec());
        };
        let mut midpoints = Vec::new();
        for &w in inner {
            if w != *from && w != *to && !midpoints.contains(&w) {
                midpoints.push(w);
            }
        }
        if midpoints.len() > MAX_MIDPOINTS {
            return Err(format!("Optimize order takes {MAX_MIDPOINTS} midpoints at most. This route has {}", midpoints.len()));
        }
        if midpoints.len() >= 2 {
            midpoints = self.best_order(*from, &midpoints, *to);
        }
        Ok(std::iter::once(*from).chain(midpoints).chain([*to]).collect())
    }

    /// The cheapest order of `free` on a route from `from` to `to`, with Held-Karp.
    fn best_order(&self, from: NodeIndex, free: &[NodeIndex], to: NodeIndex) -> Vec<NodeIndex> {
        // Node 0 is `from`, nodes 1..=m are the midpoints, and node m + 1 is `to`.
        let m = free.len();
        let nodes: Vec<NodeIndex> = std::iter::once(from).chain(free.iter().copied()).chain([to]).collect();
        let graph = EdgeFiltered::from_fn(&self.uni.graph, |e: EdgeReference<'_, Link>| self.link_allowed(e));
        // dist[i][j]: the cost from node i to node j, or u64::MAX if no route exists.
        // Each source has its own search, so the searches run in parallel.
        let dist: Vec<Vec<u64>> = nodes[..=m]
            .par_iter()
            .map(|&src| {
                let costs = dijkstra(&graph, src, None, |e| self.step_cost(e));
                nodes.iter().map(|n| costs.get(n).copied().unwrap_or(u64::MAX)).collect()
            })
            .collect();

        // A state is a set of visited midpoints (a bit mask) and the last midpoint j. A state reads
        // only the states with one midpoint fewer. Thus each layer of equal set size runs in parallel.
        // layers[s] holds the masks with s bits, and pos[mask] is the index of the mask in its layer.
        let full = (1usize << m) - 1;
        let mut layers: Vec<Vec<usize>> = vec![Vec::new(); m + 1];
        let mut pos = vec![0u32; full + 1];
        for mask in 1..=full {
            let layer = &mut layers[mask.count_ones() as usize];
            pos[mask] = layer.len() as u32;
            layer.push(mask);
        }
        // cost[s][pos[mask] * m + j]: the cheapest cost from `from` through `mask`, ending at j.
        // prev[s][...]: the midpoint before j, or NONE.
        const NONE: u8 = u8::MAX;
        // Only the layer below is necessary, so the cost keeps one layer. At 20 midpoints, the largest
        // layer holds about 30 MB of costs. prev keeps all layers, for the order at the end.
        let mut cost = vec![u64::MAX; m * m];
        let mut prev: Vec<Vec<u8>> = vec![Vec::new(), vec![NONE; m * m]];
        for j in 0..m {
            cost[pos[1 << j] as usize * m + j] = dist[0][j + 1];
        }
        for layer in &layers[2..] {
            let below = &cost;
            let mut layer_cost = vec![u64::MAX; layer.len() * m];
            let mut layer_prev = vec![NONE; layer.len() * m];
            layer_cost.par_chunks_mut(m).zip(layer_prev.par_chunks_mut(m)).zip(layer).for_each(|((c, p), &mask)| {
                for k in (0..m).filter(|&k| mask & (1 << k) != 0) {
                    let rest = mask & !(1 << k);
                    for j in (0..m).filter(|&j| rest & (1 << j) != 0) {
                        let v = below[pos[rest] as usize * m + j].saturating_add(dist[j + 1][k + 1]);
                        if v < c[k] {
                            c[k] = v;
                            p[k] = j as u8;
                        }
                    }
                }
            });
            cost = layer_cost;
            prev.push(layer_prev);
        }

        let end = |j: usize| cost[j].saturating_add(dist[j + 1][m + 1]);
        let given = (0..=m).map(|i| dist[i][i + 1]).fold(0, u64::saturating_add);
        let mut j = (0..m).min_by_key(|&j| end(j)).unwrap();
        if end(j) >= given {
            return free.to_vec();
        }
        let mut order = Vec::with_capacity(m);
        let (mut mask, mut s) = (full, m);
        loop {
            order.push(free[j]);
            let p = prev[s][pos[mask] as usize * m + j];
            if p == NONE {
                break;
            }
            mask &= !(1 << j);
            s -= 1;
            j = p as usize;
        }
        order.reverse();
        order
    }

    fn summarize(&self, path: Path) -> Route {
        let mut route = Route {
            jumps: path.edges.len(),
            wormholes: 0,
            bridges: 0,
            bridge_tj: None,
            bridge_cap_pct: None,
            unknown_sigs: 0,
            stops: Vec::new(),
            path,
        };
        let mut tj = Some(0.0);
        let mut cap_pct = Some(0.0);
        for &e in &route.path.edges {
            match &self.uni.graph[e] {
                Link::Wormhole(w) => {
                    route.wormholes += 1;
                    let (from, _) = self.uni.graph.edge_endpoints(e).unwrap();
                    route.unknown_sigs += usize::from(sig_unknown(self.uni, w, from));
                }
                Link::JumpBridge => {
                    route.bridges += 1;
                    let (from, _) = self.uni.graph.edge_endpoints(e).unwrap();
                    let cost = self.rules.cost(self.uni, from);
                    tj = tj.zip(cost.and_then(|c| c.tj)).map(|(a, b)| a + b);
                    cap_pct = cap_pct.zip(cost.and_then(|c| c.cap_pct)).map(|(a, b)| a + b);
                }
                Link::Stargate => {}
            }
        }
        route.bridge_tj = if route.bridges > 0 { tj } else { None };
        route.bridge_cap_pct = if route.bridges > 0 { cap_pct } else { None };
        route
    }

    /// The jump count from `origin` to each target, on the cheapest route in the current mode.
    /// One search covers all targets.
    pub fn jumps_to(&self, origin: NodeIndex, targets: &[NodeIndex]) -> Vec<(NodeIndex, Option<u64>)> {
        let graph = &self.uni.graph;
        // The best (cost, jumps) for each node. A tie in cost keeps the lower jump count.
        let mut best: Vec<Option<(u64, u64)>> = vec![None; graph.node_count()];
        best[origin.index()] = Some((0, 0));
        let mut heap = BinaryHeap::from([Reverse((0u64, 0u64, origin))]);
        while let Some(Reverse((cost, jumps, node))) = heap.pop() {
            if best[node.index()] != Some((cost, jumps)) {
                continue;
            }
            for e in graph.edges(node).filter(|e| self.link_allowed(*e)) {
                let next = (cost + self.step_cost(e), jumps + 1);
                let slot = &mut best[e.target().index()];
                if slot.is_none_or(|b| next < b) {
                    *slot = Some(next);
                    heap.push(Reverse((next.0, next.1, e.target())));
                }
            }
        }
        targets.iter().map(|&t| (t, best[t.index()].map(|(_, jumps)| jumps))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{hole, sde_dir, universe};

    fn router(mode: Mode) -> Router<'static> {
        Router::new(universe(), RouterOptions { mode, ..Default::default() })
    }

    use crate::ansiblex::find_hull;
    use crate::wormhole::{Expiry, Hubs, MassStatus, Size, THERA, TURNUR, Wormhole};
    use std::sync::OnceLock;

    /// The expiry of the Hek-Perimeter wormhole.
    const ENDS: u64 = 2_000_000_000;

    /// The SDE, plus one test wormhole for each rule. Each pair is many gate jumps apart,
    /// so a route of 1 jump uses the wormhole.
    fn holes_universe() -> &'static Universe {
        static UNI: OnceLock<Universe> = OnceLock::new();
        UNI.get_or_init(|| {
            let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
            let id = |name: &str| uni.system(uni.exact(name).unwrap()).id;
            let holes = [
                Wormhole { size: Some(Size::Medium), ..hole(id("Jita"), id("Amarr")) },
                Wormhole { size: Some(Size::Small), ..hole(id("Dodixie"), id("Hek")) },
                Wormhole { mass: Some(MassStatus::Critical), ..hole(id("Rens"), id("Amarr")) },
                Wormhole { expiry: Some(Expiry { at: ENDS, exact: true }), ..hole(id("Hek"), id("Perimeter")) },
                // Ashab to Pator through Thera, and Turnur to Oursulaert.
                hole(id("Ashab"), THERA),
                hole(THERA, id("Pator")),
                hole(TURNUR, id("Oursulaert")),
            ];
            assert_eq!(uni.add_wormholes(&holes), 7);
            uni
        })
    }

    fn jumps(hull: Option<&str>, from: &str, to: &str, now: u64) -> usize {
        hub_jumps(Hubs::default(), hull, from, to, now)
    }

    fn hub_jumps(hubs: Hubs, hull: Option<&str>, from: &str, to: &str, now: u64) -> usize {
        let uni = holes_universe();
        let rules = BridgeRules { capital: None, hull: hull.map(|h| find_hull(h).unwrap()), max_cap: None };
        let router = Router::new(uni, RouterOptions { hubs, bridges: false, rules, now, ..Default::default() });
        let nodes = [uni.exact(from).unwrap(), uni.exact(to).unwrap()];
        router.routes(&nodes, 1).unwrap()[0].jumps
    }

    #[test]
    fn hull_mass_against_size() {
        // Sin: 106,300,000 kg. A Medium wormhole allows 62,000,000 kg.
        assert!(jumps(Some("Sin"), "Jita", "Amarr", 0) > 1);
        assert_eq!(jumps(Some("Rifter"), "Jita", "Amarr", 0), 1);
    }

    #[test]
    fn no_hull_skips_size_check() {
        assert_eq!(jumps(None, "Dodixie", "Hek", 0), 1);
    }

    #[test]
    fn critical_is_blocked() {
        assert!(jumps(None, "Rens", "Amarr", 0) > 1);
    }

    #[test]
    fn expired_is_blocked() {
        assert_eq!(jumps(None, "Hek", "Perimeter", ENDS - 1), 1);
        assert!(jumps(None, "Hek", "Perimeter", ENDS) > 1);
    }

    #[test]
    fn wormholes_off() {
        let uni = holes_universe();
        let router = Router::new(uni, RouterOptions { wormholes: false, bridges: false, ..Default::default() });
        let nodes = [uni.exact("Dodixie").unwrap(), uni.exact("Hek").unwrap()];
        assert!(router.routes(&nodes, 1).unwrap()[0].jumps > 1);
    }

    #[test]
    fn thera_off_blocks_thera_wormholes() {
        let on = Hubs::default();
        assert_eq!(hub_jumps(on, None, "Ashab", "Pator", 0), 2);
        assert!(hub_jumps(Hubs { thera: false, ..on }, None, "Ashab", "Pator", 0) > 2);
        // The Turnur switch does not act on Thera.
        assert_eq!(hub_jumps(Hubs { turnur: false, ..on }, None, "Ashab", "Pator", 0), 2);
    }

    #[test]
    fn turnur_off_keeps_turnur_gates() {
        let on = Hubs::default();
        assert_eq!(hub_jumps(on, None, "Turnur", "Oursulaert", 0), 1);
        // The wormhole is off, but the gates into Turnur stay, so a longer route exists.
        assert!(hub_jumps(Hubs { turnur: false, ..on }, None, "Turnur", "Oursulaert", 0) > 1);
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
    fn optimize_reorders_midpoints() {
        let r = router(Mode::Shortest);
        let typed = [node("Jita"), node("Amarr"), node("Perimeter"), node("Dodixie")];
        let order = r.optimize(&typed).unwrap();
        // Perimeter is one jump from Jita, so the route visits it before Amarr.
        assert_eq!(order, [node("Jita"), node("Perimeter"), node("Amarr"), node("Dodixie")]);
        // A shortest order stays as typed.
        assert_eq!(r.optimize(&order).unwrap(), order);
    }

    #[test]
    fn optimize_visits_each_system_once() {
        let r = router(Mode::Shortest);
        let names = ["UALX-3", "Dodixie", "UALX-3", "Jita", "Turnur", "Hek", "Rens", "Jita", "C-J6MT", "UALX-3"];
        let typed: Vec<NodeIndex> = names.iter().map(|n| node(n)).collect();
        let order = r.optimize(&typed).unwrap();
        assert_eq!((order[0], order[7]), (node("UALX-3"), node("UALX-3")));
        let mut midpoints = order[1..7].to_vec();
        midpoints.sort();
        let mut expected: Vec<NodeIndex> = ["Dodixie", "Jita", "Turnur", "Hek", "Rens", "C-J6MT"].map(node).to_vec();
        expected.sort();
        assert_eq!(midpoints, expected);
    }

    #[test]
    fn optimize_matches_brute_force() {
        let r = router(Mode::Shortest);
        let start = node("Jita");
        let end = node("Amarr");
        let free = ["Rens", "Hek", "Dodixie", "Perimeter", "Tash-Murkon Prime"].map(node);
        let jumps = |mid: &[NodeIndex]| {
            let w: Vec<NodeIndex> = std::iter::once(start).chain(mid.iter().copied()).chain([end]).collect();
            r.routes(&w, 1).unwrap()[0].jumps
        };
        // Every order of the five midpoints, with Heap's algorithm.
        let mut perm = free.to_vec();
        let mut c = vec![0; perm.len()];
        let mut best = jumps(&perm);
        let mut i = 0;
        while i < perm.len() {
            if c[i] < i {
                perm.swap(if i % 2 == 0 { 0 } else { c[i] }, i);
                best = best.min(jumps(&perm));
                c[i] += 1;
                i = 0;
            } else {
                c[i] = 0;
                i += 1;
            }
        }
        let typed: Vec<NodeIndex> = std::iter::once(start).chain(free).chain([end]).collect();
        let order = r.optimize(&typed).unwrap();
        assert_eq!(r.routes(&order, 1).unwrap()[0].jumps, best);
    }

    #[test]
    fn optimize_limits_midpoints() {
        let r = router(Mode::Shortest);
        let waypoints: Vec<NodeIndex> = (0..MAX_MIDPOINTS + 3).map(NodeIndex::new).collect();
        assert!(r.optimize(&waypoints).is_err());
    }

    #[test]
    fn hub_sidebar() {
        let targets = ["Jita", "Amarr", "Dodixie", "Hek", "Rens"].map(node);
        let hubs = router(Mode::Shortest).jumps_to(node("Jita"), &targets);
        assert_eq!(hubs[0], (node("Jita"), Some(0)));
        assert!(hubs.iter().all(|(_, j)| j.is_some()));
    }

    /// The SDE, plus a wormhole between Jita and Perimeter, which also share a stargate.
    fn parallel_universe() -> &'static Universe {
        static UNI: OnceLock<Universe> = OnceLock::new();
        UNI.get_or_init(|| {
            let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
            let id = |name: &str| uni.system(uni.exact(name).unwrap()).id;
            let holes = [hole(id("Jita"), id("Perimeter"))];
            assert_eq!(uni.add_wormholes(&holes), 1);
            uni
        })
    }

    /// The edge from Jita to Perimeter of each link kind: (stargate, wormhole).
    fn parallel_edges(r: &Router) -> (EdgeIndex, EdgeIndex) {
        let (jita, perimeter) = (r.uni.exact("Jita").unwrap(), r.uni.exact("Perimeter").unwrap());
        let find = |want: fn(&Link) -> bool| r.uni.graph.edges_connecting(jita, perimeter).find(|e| want(e.weight())).unwrap().id();
        (find(|l| matches!(l, Link::Stargate)), find(|l| matches!(l, Link::Wormhole(_))))
    }

    #[test]
    fn a_dearer_edge_loses_to_a_parallel_edge() {
        let uni = parallel_universe();
        // The test wormhole has no signature, so the penalty for it is off.
        let mut r = Router::new(uni, RouterOptions { unknown_sig_penalty: 0, ..Default::default() });
        let (gate, hole) = parallel_edges(&r);
        let (jita, perimeter) = (uni.exact("Jita").unwrap(), uni.exact("Perimeter").unwrap());

        // At equal cost, the stargate comes first. The wormhole route follows it as its own route.
        let paths = r.k_shortest(jita, perimeter, 2);
        assert_eq!(paths.iter().map(|p| p.edges.clone()).collect::<Vec<_>>(), [vec![gate], vec![hole]]);

        // A dear stargate makes the wormhole the first path, at the lower cost.
        r.edge_extra[gate.index()] = JUMP / 2;
        let paths = r.k_shortest(jita, perimeter, 2);
        assert_eq!(
            paths.iter().map(|p| (p.edges.clone(), p.cost)).collect::<Vec<_>>(),
            [(vec![hole], JUMP), (vec![gate], JUMP + JUMP / 2)]
        );
    }

    #[test]
    fn an_extra_cost_changes_the_order_of_routes() {
        let uni = parallel_universe();
        let mut r = Router::new(uni, RouterOptions::default());
        let (_, hole) = parallel_edges(&r);
        r.edge_extra[hole.index()] = JUMP / 2;
        let routes = r.routes(&[uni.exact("Jita").unwrap(), uni.exact("Perimeter").unwrap()], 2).unwrap();
        assert_eq!(routes.iter().map(|r| (r.wormholes, r.path.cost)).collect::<Vec<_>>(), [(0, JUMP), (1, JUMP + JUMP / 2)]);
    }

    #[test]
    fn jumps_to_counts_jumps_of_the_cheapest_route() {
        let uni = parallel_universe();
        let mut r = Router::new(uni, RouterOptions::default());
        let (jita, perimeter) = (uni.exact("Jita").unwrap(), uni.exact("Perimeter").unwrap());
        let targets = [jita, perimeter];
        // Both edges cost one jump, whatever the extra cost of a dear edge is.
        for e in uni.graph.edge_indices() {
            r.edge_extra[e.index()] = 0;
        }
        let (gate, hole) = parallel_edges(&r);
        r.edge_extra[gate.index()] = 700;
        r.edge_extra[hole.index()] = 300;
        assert_eq!(r.jumps_to(jita, &targets), [(jita, Some(0)), (perimeter, Some(1))]);
    }

    #[test]
    fn a_bridge_costs_by_the_share_of_the_gate_capacitor() {
        let uni = crate::test_support::overlay_universe();
        let rules = BridgeRules { capital: uni.exact("JK-Q77"), hull: find_hull("Battleship"), max_cap: None };
        let (from, to) = (uni.exact("QLU-P0").unwrap(), uni.exact("Q5KZ-W").unwrap());
        // QLU-P0 is in zone 2, so a Battleship uses 33 TJ of 1250 TJ.
        let pct = rules.cost(&uni, from).unwrap().cap_pct.unwrap();
        assert!(pct > 0.0);
        let route = |cap_weight| {
            let r = Router::new(&uni, RouterOptions { rules, cap_weight, ..Default::default() });
            r.routes(&[from, to], 1).unwrap().remove(0)
        };

        // A weight of 0 makes the bridge free of extra cost: one jump.
        let free = route(0);
        assert_eq!((free.bridges, free.path.cost), (1, JUMP));
        assert!((free.bridge_cap_pct.unwrap() - pct).abs() < 1e-4);

        // The default weight adds the share of the capacitor to the cost of the jump.
        let priced = route(DEFAULT_CAP_WEIGHT);
        let extra = (f64::from(pct) * f64::from(DEFAULT_CAP_WEIGHT)).round() as u64;
        assert!(extra > 0);
        assert!(priced.path.cost > free.path.cost && priced.path.cost <= JUMP + extra, "the bridge, or a cheaper gate route");

        // A very high weight sends the route over the gates.
        let gates = route(u32::MAX);
        assert_eq!(gates.bridges, 0);
        assert_eq!(gates.bridge_cap_pct, None);
        assert!(gates.jumps > 1);
    }

    /// The Jita-Amarr wormhole of the test universe has no signature. The gate route is 10 jumps or more.
    fn jita_amarr(options: RouterOptions) -> Route {
        let uni = holes_universe();
        let r = Router::new(uni, options);
        r.routes(&[uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap()], 1).unwrap().remove(0)
    }

    #[test]
    fn an_unknown_signature_costs_extra() {
        let free = jita_amarr(RouterOptions { unknown_sig_penalty: 0, ..Default::default() });
        assert_eq!((free.wormholes, free.unknown_sigs, free.path.cost), (1, 1, JUMP));
        let priced = jita_amarr(RouterOptions::default());
        assert_eq!((priced.wormholes, priced.unknown_sigs), (1, 1));
        assert_eq!(priced.path.cost, JUMP + u64::from(DEFAULT_UNKNOWN_SIG_PENALTY));
    }

    #[test]
    fn a_big_penalty_sends_the_route_over_the_gates() {
        let route = jita_amarr(RouterOptions { unknown_sig_penalty: u32::MAX, ..Default::default() });
        assert_eq!((route.wormholes, route.unknown_sigs), (0, 0));
        assert!(route.jumps > 1);
    }

    #[test]
    fn broken_drops_a_wormhole_with_no_signature() {
        let route = jita_amarr(RouterOptions { unknown_sig_broken: true, ..Default::default() });
        assert_eq!(route.wormholes, 0);
        assert!(route.jumps > 1);
    }

    /// A wormhole route of 1 jump, with an unknown signature, ranks after a gate route of 2 jumps.
    #[test]
    fn a_longer_cheaper_route_ranks_first() {
        let uni = parallel_universe();
        let nodes = [uni.exact("Jita").unwrap(), uni.exact("Perimeter").unwrap()];
        let ranked = |options: RouterOptions| -> Vec<(usize, usize)> {
            let routes = Router::new(uni, options).routes(&nodes, 6).unwrap();
            routes.iter().map(|r| (r.jumps, r.wormholes)).collect()
        };

        // With no penalty, the wormhole route is as cheap as the gate route.
        let free = ranked(RouterOptions { unknown_sig_penalty: 0, ..Default::default() });
        assert_eq!(free[..2], [(1, 0), (1, 1)]);

        // With the default penalty, a 2-jump gate route is first, and then the wormhole route follows.
        let priced = ranked(RouterOptions::default());
        let gate_two = priced.iter().position(|&r| r == (2, 0)).expect("a 2-jump gate route");
        let hole = priced.iter().position(|&r| r == (1, 1)).unwrap_or(priced.len());
        assert_eq!(priced[0], (1, 0));
        assert!(gate_two < hole, "{priced:?}");
    }

    #[test]
    fn a_danger_cost_steers_the_route() {
        let uni = holes_universe();
        let nodes = [uni.exact("Jita").unwrap(), uni.exact("Dodixie").unwrap()];
        let mut r = Router::new(uni, RouterOptions { wormholes: false, ..Default::default() });
        let best = r.routes(&nodes, 1).unwrap().remove(0);
        let middle = best.path.nodes[best.jumps / 2];
        let mut danger = vec![0; uni.graph.node_count()];
        danger[middle.index()] = 10 * JUMP;
        r.set_danger(danger);
        let safer = r.routes(&nodes, 1).unwrap().remove(0);
        assert!(!safer.path.nodes.contains(&middle));
        assert!(safer.path.cost > best.path.cost);
        // A later call replaces the earlier danger: zero danger gives the first route back.
        r.set_danger(vec![0; uni.graph.node_count()]);
        assert_eq!(r.routes(&nodes, 1).unwrap()[0].path, best.path);
    }
}
