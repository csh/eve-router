//! Overlay edges: wormholes and jump bridges from nexum, and jump bridges from an SMT list.

use crate::universe::{Link, Universe};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Default, Debug)]
pub struct OverlayReport {
    pub wormholes: usize,
    pub bridges: usize,
    pub skipped_broken: usize,
    pub skipped_expired: usize,
    pub unknown: Vec<String>,
}

impl OverlayReport {
    pub fn summary(&self) -> String {
        let mut s = format!("{} wormholes, {} jump bridge directions", self.wormholes, self.bridges);
        if self.skipped_broken + self.skipped_expired > 0 {
            s += &format!(
                ", skipped {} broken, {} expired",
                self.skipped_broken, self.skipped_expired
            );
        }
        if !self.unknown.is_empty() {
            s += &format!(", unknown: {}", self.unknown.join(" "));
        }
        s
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexumMap {
    systems: Vec<NexumSystem>,
    connections: Vec<NexumConnection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexumSystem {
    id: String,
    eve_system_id: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexumConnection {
    source_id: String,
    target_id: String,
    connection_type: String,
    size: Option<String>,
    #[serde(rename = "type")]
    sig_type: Option<String>,
    time_status: Option<String>,
    #[serde(default)]
    broken: bool,
}

/// Add the nexum wormholes. Add the nexum jump bridges only if `with_bridges` is true.
pub fn load_nexum(uni: &mut Universe, path: &Path, with_bridges: bool) -> Result<OverlayReport, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let map: NexumMap = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut report = OverlayReport::default();
    let nodes: HashMap<&str, _> = map
        .systems
        .iter()
        .filter_map(|s| match uni.by_id.get(&s.eve_system_id) {
            Some(&node) => Some((s.id.as_str(), node)),
            None => {
                report.unknown.push(s.eve_system_id.to_string());
                None
            }
        })
        .collect();

    for c in &map.connections {
        let link = match c.connection_type.as_str() {
            "standard" => Link::Wormhole {
                sig_type: c.sig_type.clone(),
                size: c.size.clone(),
                time: c.time_status.clone(),
            },
            "jumpgate" if with_bridges => Link::JumpBridge,
            // "gate" is a stargate. The SDE already holds it.
            _ => continue,
        };
        if c.broken {
            report.skipped_broken += 1;
            continue;
        }
        if c.time_status.as_deref() == Some("expired") {
            report.skipped_expired += 1;
            continue;
        }
        let (Some(&a), Some(&b)) = (nodes.get(c.source_id.as_str()), nodes.get(c.target_id.as_str())) else {
            continue;
        };
        match link {
            Link::JumpBridge => report.bridges += 2,
            _ => report.wormholes += 1,
        }
        uni.graph.add_edge(a, b, link.clone());
        uni.graph.add_edge(b, a, link);
    }
    Ok(report)
}

/// Add jump bridges from an SMT list. Each line `<id> <FROM> --> <TO>` is one direction.
pub fn load_bridges(uni: &mut Universe, path: &Path) -> Result<OverlayReport, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(add_bridges(uni, &text))
}

pub fn add_bridges(uni: &mut Universe, text: &str) -> OverlayReport {
    let mut report = OverlayReport::default();
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
            (Some(a), Some(b)) => {
                uni.graph.add_edge(a, b, Link::JumpBridge);
                report.bridges += 1;
            }
            (a, _) => report.unknown.push(if a.is_none() { from } else { to }.to_string()),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::tests::universe;

    #[test]
    fn smt_list_parses() {
        let text = fs::read_to_string("tests/fixtures/ansiblex.txt").unwrap();
        let base = universe();
        // Work on a copy of the graph, because the shared test universe must stay unchanged.
        let mut uni = Universe::from_sde(crate::sde::load(Path::new("sde")).unwrap());
        let report = add_bridges(&mut uni, &text);
        assert!(report.unknown.is_empty(), "{:?}", report.unknown);
        let lines = text.lines().filter(|l| l.contains("-->") && !l.trim_start().starts_with('#')).count();
        assert_eq!(report.bridges, lines);
        assert_eq!(uni.graph.edge_count(), base.graph.edge_count() + lines);
    }
}
