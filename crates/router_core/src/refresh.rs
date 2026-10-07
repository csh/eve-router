//! The live refresh of the wormhole sources. Each refresh builds a full new map from the base
//! map. Thus a route of the old map keeps correct edge indices until the app swaps the map.

use crate::labels::Shortcuts;
use crate::overlay::OverlayReport;
use crate::sources::{evescout, nexum};
use crate::universe::Universe;
use crate::wormhole::{self, SourceData};
use std::sync::Arc;

/// A map with the wormholes of one load.
pub struct Snapshot {
    pub uni: Arc<Universe>,
    /// The counts of the Shortcuts box, and the fetch warnings for the status line.
    pub shortcuts: Shortcuts,
    /// The wormhole data of each source.
    pub all: Vec<SourceData>,
}

/// Build a map: a copy of `base` with the merged wormholes of `wh` and `scout` at `now`.
/// The startup and each refresh use this function, so both build the same map. The copy keeps
/// the `NodeIndex` of each system. Return the snapshot, and the count of wormholes that the map got.
pub fn build(base: &Universe, report: &OverlayReport, wh: &nexum::Load, scout: &evescout::Load, now: u64) -> (Snapshot, usize) {
    let mut uni = base.clone();
    let all: Vec<SourceData> = wh.data.iter().chain(&scout.data).cloned().collect();
    let count = uni.add_wormholes(&wormhole::merge(&all, now));
    let shortcuts = Shortcuts::new(&uni, report, wh, scout);
    (Snapshot { uni: Arc::new(uni), shortcuts, all }, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{hole, universe};
    use crate::wormhole::{SourceId, THERA};

    const JITA: u32 = 30000142;

    /// EVE-Scout data with one wormhole from Thera to Jita.
    fn scout_load() -> evescout::Load {
        // Read the record from JSON, so the test does not depend on each field of `SourceData`.
        let json = serde_json::json!({ "source": SourceId::EveScout, "fetched_at": 0, "holes": [hole(THERA, JITA)] });
        let data: SourceData = serde_json::from_value(json).unwrap();
        evescout::Load { data: Some(data), ..Default::default() }
    }

    #[test]
    fn build_keeps_the_node_of_each_system() {
        let base = universe();
        let (snap, count) = build(base, &OverlayReport::default(), &nexum::Load::default(), &scout_load(), 0);
        assert_eq!(count, 1);
        assert_eq!(snap.uni.graph.node_count(), base.graph.node_count());
        for (id, &node) in &base.by_id {
            assert_eq!(snap.uni.by_id[id], node);
            assert_eq!(snap.uni.system(node).id, *id);
        }
        // The copy has the two directions of the wormhole. The base map does not change.
        assert_eq!(snap.uni.graph.edge_count(), base.graph.edge_count() + 2);
        assert_eq!((snap.shortcuts.wormholes, snap.shortcuts.thera), (1, 1));
        assert_eq!(snap.all.len(), 1);
    }
}
