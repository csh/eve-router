//! Jump bridges from an SMT list. A bridge works in both directions, so the router keeps one
//! pair of systems for each bridge and adds the two edges itself.

use crate::error::{FileError, write_atomic};
use crate::universe::{Link, Universe};
use petgraph::graph::NodeIndex;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// One bridge: the two systems, in the order of their names. The router treats A <-> B and
/// B <-> A as the same bridge.
pub type Pair = (NodeIndex, NodeIndex);

#[derive(Default, Debug, Clone)]
pub struct OverlayReport {
    /// The number of edges that the bridges add: two for each pair.
    pub bridges: usize,
    /// The bridges, one pair for each bridge.
    pub pairs: Vec<Pair>,
    /// The names that are not a system, and the lines that are not a bridge.
    pub unknown: Vec<String>,
    /// The lines that name the same system at both ends.
    pub same: Vec<String>,
}

impl OverlayReport {
    pub fn summary(&self) -> String {
        let mut s = format!("{} jump bridges", self.pairs.len());
        if !self.unknown.is_empty() {
            s += &format!(", unknown: {}", self.unknown.join(" "));
        }
        if !self.same.is_empty() {
            s += &format!(", same system at both ends: {}", self.same.join(" "));
        }
        s
    }
}

/// Read the bridges of an SMT list. Each line `<id> <FROM> --> <TO>` is one direction. The line
/// of the other direction, and a line that repeats, add nothing new.
pub fn parse_bridges(uni: &Universe, text: &str) -> OverlayReport {
    let mut report = OverlayReport::default();
    let mut seen = HashSet::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((from, to)) = line.split_once("-->") else {
            report.unknown.push(line.to_string());
            continue;
        };
        // The first token on the left side is the structure ID.
        let from = from.trim().split_once(char::is_whitespace).map_or(from, |(_, name)| name);
        let (from, to) = (from.trim(), to.trim());
        match (uni.exact(from), uni.exact(to)) {
            (Some(a), Some(b)) if a == b => report.same.push(from.to_string()),
            (Some(a), Some(b)) => {
                let pair = ordered(uni, a, b);
                if seen.insert(pair) {
                    report.pairs.push(pair);
                }
            }
            (a, _) => report.unknown.push(if a.is_none() { from } else { to }.to_string()),
        }
    }
    report.bridges = report.pairs.len() * 2;
    report
}

/// The pair with the system of the lower name first.
pub fn ordered(uni: &Universe, a: NodeIndex, b: NodeIndex) -> Pair {
    if uni.name(a).to_lowercase() <= uni.name(b).to_lowercase() { (a, b) } else { (b, a) }
}

/// Make `pairs` the bridges of the map: remove each old bridge, then add the two edges of each pair.
pub fn set_bridges(uni: &mut Universe, pairs: &[Pair]) {
    uni.graph.retain_edges(|g, e| !matches!(g[e], Link::JumpBridge));
    for &(a, b) in pairs {
        uni.graph.add_edge(a, b, Link::JumpBridge);
        uni.graph.add_edge(b, a, Link::JumpBridge);
    }
}

/// Add the bridges of an SMT list to a map with no bridges.
pub fn add_bridges(uni: &mut Universe, text: &str) -> OverlayReport {
    let report = parse_bridges(uni, text);
    set_bridges(uni, &report.pairs);
    report
}

pub fn load_bridges(uni: &mut Universe, path: &Path) -> Result<OverlayReport, FileError> {
    let text = fs::read_to_string(path).map_err(FileError::io(path))?;
    Ok(add_bridges(uni, &text))
}

/// The SMT text of `pairs`: one line for each direction.
pub fn bridge_text(uni: &Universe, pairs: &[Pair]) -> String {
    let mut text = String::from("# Jump bridges, one line for each direction. The router writes this file.\n");
    for &(a, b) in pairs {
        text += &format!("0 {} --> {}\n0 {} --> {}\n", uni.name(a), uni.name(b), uni.name(b), uni.name(a));
    }
    text
}

/// Write `pairs` to the bridge file, through a temporary file and a rename. A crash during the
/// write leaves the old list whole.
pub fn save_bridges(path: &Path, uni: &Universe, pairs: &[Pair]) -> Result<(), FileError> {
    write_atomic(path, &path.with_extension("txt.part"), &bridge_text(uni, pairs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fixture, sde_dir, universe};

    #[test]
    fn save_bridges_failure_keeps_the_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ansiblex.txt");
        fs::write(&path, "old\n").unwrap();
        // A directory blocks the temporary file.
        fs::create_dir(path.with_extension("txt.part")).unwrap();
        assert!(save_bridges(&path, universe(), &[]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
    }

    #[test]
    fn save_bridges_writes_the_list_and_leaves_no_part_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ansiblex.txt");
        let uni = universe();
        let pair = ordered(uni, uni.exact("Jita").unwrap(), uni.exact("Amarr").unwrap());
        save_bridges(&path, uni, &[pair]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), bridge_text(uni, &[pair]));
        assert!(!path.with_extension("txt.part").exists());
    }

    #[test]
    fn smt_list_parses() {
        let text = fs::read_to_string(fixture("ansiblex.txt")).unwrap();
        let base = universe();
        // Work on a copy of the graph, because the shared test universe must stay unchanged.
        let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
        let report = add_bridges(&mut uni, &text);
        assert!(report.unknown.is_empty() && report.same.is_empty(), "{:?}", report);
        let lines = text.lines().filter(|l| l.contains("-->") && !l.trim_start().starts_with('#')).count();
        assert_eq!(report.bridges, lines);
        assert_eq!(report.pairs.len() * 2, lines);
        assert_eq!(uni.graph.edge_count(), base.graph.edge_count() + lines);
    }

    #[test]
    fn one_line_makes_both_directions_and_a_repeat_adds_nothing() {
        let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
        let before = uni.graph.edge_count();
        let report = add_bridges(&mut uni, "0 Jita --> Perimeter\n0 Perimeter --> Jita\n0 jita --> perimeter\n");
        assert_eq!(report.pairs.len(), 1);
        assert_eq!(uni.graph.edge_count(), before + 2);
    }

    #[test]
    fn a_bridge_needs_two_different_systems() {
        let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
        let before = uni.graph.edge_count();
        let report = add_bridges(&mut uni, "0 Jita --> Jita\n0 jita --> Jita\n");
        assert!(report.pairs.is_empty());
        assert_eq!(report.same.len(), 2);
        assert_eq!(uni.graph.edge_count(), before);
    }

    #[test]
    fn set_bridges_replaces_the_old_bridges() {
        let mut uni = Universe::from_sde(crate::sde::load(&sde_dir()).unwrap());
        let before = uni.graph.edge_count();
        let first = add_bridges(&mut uni, "0 Jita --> Perimeter\n");
        let other = parse_bridges(&uni, "0 Amarr --> Dodixie\n");
        set_bridges(&mut uni, &other.pairs);
        assert_eq!(uni.graph.edge_count(), before + 2);
        assert_ne!(first.pairs, other.pairs);
        let text = bridge_text(&uni, &other.pairs);
        assert_eq!(parse_bridges(&uni, &text).pairs, other.pairs);
    }
}
