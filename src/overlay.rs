//! Jump bridges from an SMT list.

use crate::universe::{Link, Universe};
use std::fs;
use std::path::Path;

#[derive(Default, Debug)]
pub struct OverlayReport {
    pub bridges: usize,
    pub unknown: Vec<String>,
}

impl OverlayReport {
    pub fn summary(&self) -> String {
        let mut s = format!("{} jump bridge directions", self.bridges);
        if !self.unknown.is_empty() {
            s += &format!(", unknown: {}", self.unknown.join(" "));
        }
        s
    }
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
