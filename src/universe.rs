//! The solar system graph and the name index.

use crate::sde::SdeData;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

/// Meters in one light year.
pub const LIGHT_YEAR: f64 = 9_460_730_472_580_800.0;

pub struct System {
    pub id: u32,
    pub name: String,
    pub region: String,
    pub security: f64,
    /// Position in meters.
    pub pos: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Stargate,
    Wormhole {
        sig_type: Option<String>,
        size: Option<String>,
        time: Option<String>,
    },
    JumpBridge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    High,
    Low,
    Null,
}

/// The security value that the game shows. A value above 0.0 and below 0.05 shows as 0.1.
pub fn display_sec(security: f64) -> f64 {
    if security > 0.0 && security < 0.05 {
        0.1
    } else {
        (security * 10.0).round() / 10.0
    }
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

pub struct Universe {
    pub graph: DiGraph<System, Link>,
    pub by_id: HashMap<u32, NodeIndex>,
    by_name: HashMap<String, NodeIndex>,
    /// Lowercase names in sorted order, for prefix search.
    sorted: Vec<(String, NodeIndex)>,
    pub build: Option<u32>,
}

impl Universe {
    pub fn from_sde(data: SdeData) -> Self {
        let regions: HashMap<u32, String> =
            data.regions.into_iter().map(|r| (r.id, r.name.en)).collect();
        let mut graph = DiGraph::with_capacity(data.systems.len(), data.stargates.len());
        let mut by_id = HashMap::with_capacity(data.systems.len());
        for s in data.systems {
            let region = regions.get(&s.region_id).cloned().unwrap_or_default();
            let node = graph.add_node(System {
                id: s.id,
                name: s.name.en,
                region,
                security: s.security,
                pos: [s.position.x, s.position.y, s.position.z],
            });
            by_id.insert(s.id, node);
        }
        // Each stargate record is one direction of a gate pair.
        for g in data.stargates {
            if let (Some(&a), Some(&b)) = (by_id.get(&g.system_id), by_id.get(&g.destination.system_id)) {
                graph.add_edge(a, b, Link::Stargate);
            }
        }
        let mut sorted: Vec<(String, NodeIndex)> = graph
            .node_indices()
            .map(|n| (graph[n].name.to_lowercase(), n))
            .collect();
        sorted.sort();
        let by_name = sorted.iter().cloned().collect();
        Universe { graph, by_id, by_name, sorted, build: data.build }
    }

    pub fn system(&self, node: NodeIndex) -> &System {
        &self.graph[node]
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
        self.sorted[start..]
            .iter()
            .take_while(|(n, _)| n.starts_with(&prefix))
            .map(|&(_, node)| node)
            .collect()
    }

    /// Find a system by its exact name, else by a unique prefix.
    /// On failure, return up to 5 candidate names for the error text.
    pub fn resolve(&self, name: &str) -> Result<NodeIndex, Vec<String>> {
        if let Some(node) = self.exact(name) {
            return Ok(node);
        }
        let matches = self.complete(name);
        match matches.as_slice() {
            [one] => Ok(*one),
            many => Err(many.iter().take(5).map(|&n| self.name(n).to_string()).collect()),
        }
    }

    /// The number of wormholes and jump bridges. A connection in both directions counts once.
    pub fn shortcut_counts(&self) -> (usize, usize) {
        let mut wormholes = std::collections::HashSet::new();
        let mut bridges = std::collections::HashSet::new();
        for e in self.graph.raw_edges() {
            let pair = (e.source().min(e.target()), e.source().max(e.target()));
            match e.weight {
                Link::Wormhole { .. } => wormholes.insert(pair),
                Link::JumpBridge => bridges.insert(pair),
                Link::Stargate => false,
            };
        }
        (wormholes.len(), bridges.len())
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
    use std::path::Path;
    use std::sync::OnceLock;

    /// The real SDE, loaded one time for all tests.
    pub fn universe() -> &'static Universe {
        static UNI: OnceLock<Universe> = OnceLock::new();
        UNI.get_or_init(|| Universe::from_sde(crate::sde::load(Path::new("sde")).unwrap()))
    }

    #[test]
    fn stargate_edge_count() {
        let uni = universe();
        let gates = uni.graph.edge_weights().filter(|l| **l == Link::Stargate).count();
        assert_eq!(gates, 13978);
        assert_eq!(uni.graph.node_count(), 8490);
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
    }
}
