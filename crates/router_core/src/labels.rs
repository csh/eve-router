//! The text of the route summaries, the route steps and the Shortcuts box, for each user interface.

use crate::ansiblex::BridgeRules;
use crate::overlay::OverlayReport;
use crate::route::Route;
use crate::sources::{evescout, nexum};
use crate::universe::{Link, Universe};
use crate::wormhole::{MassStatus, SourceId, THERA, TURNUR, Wormhole, expiry_text};
use petgraph::graph::EdgeIndex;

/// The overlay counts for the "Shortcuts" sidebar box.
pub struct Shortcuts {
    pub wormholes: usize,
    pub bridges: usize,
    /// Broken or expired wormholes that the router did not load.
    pub skipped: usize,
    /// The wormholes with an end in Thera, and in Turnur.
    pub thera: usize,
    pub turnur: usize,
    /// The source and the fetch time of each set of wormhole data.
    pub sources: Vec<(SourceId, u64)>,
    /// A problem with the overlay data, for the status line at startup.
    pub warning: Option<String>,
}

impl Shortcuts {
    pub fn new(uni: &Universe, report: &OverlayReport, wh: &nexum::Load, scout: &evescout::Load) -> Self {
        let (wormholes, bridges) = uni.shortcut_counts();
        let mut warnings = Vec::new();
        if !report.unknown.is_empty() {
            warnings.push(format!("Bridges: unknown systems: {}", report.unknown.join(", ")));
        }
        for (source, unknown) in [(SourceId::Nexum, &wh.report.unknown), (SourceId::EveScout, &scout.report.unknown)] {
            if !unknown.is_empty() {
                let ids: Vec<String> = unknown.iter().map(u32::to_string).collect();
                warnings.push(format!("{}: unknown system IDs: {}", source.label(), ids.join(", ")));
            }
        }
        warnings.extend(wh.warning.clone());
        warnings.extend(scout.warning.clone());
        Shortcuts {
            wormholes,
            bridges,
            skipped: wh.report.skipped() + scout.report.skipped(),
            thera: uni.hub_count(THERA),
            turnur: uni.hub_count(TURNUR),
            sources: wh.data.iter().chain(&scout.data).map(|d| (d.source, d.fetched_at)).collect(),
            warning: (!warnings.is_empty()).then(|| warnings.join(" · ")),
        }
    }
}

/// "1 jump" or "N jumps".
pub fn jumps_label(jumps: usize) -> String {
    if jumps == 1 { "1 jump".into() } else { format!("{jumps} jumps") }
}

/// The overlay part of a route summary, for example " (2 wormholes, 1 jump bridge, 36 TJ)".
pub fn route_extras(route: &Route) -> String {
    let mut parts = Vec::new();
    if route.wormholes > 0 {
        let s = if route.wormholes == 1 { "" } else { "s" };
        parts.push(format!("{} wormhole{s}", route.wormholes));
    }
    if route.bridges > 0 {
        let s = if route.bridges == 1 { "" } else { "s" };
        parts.push(format!("{} jump bridge{s}", route.bridges));
    }
    if let Some(tj) = route.bridge_tj {
        parts.push(format!("{tj} TJ"));
    }
    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) }
}

/// How a route enters a system: a gate, a wormhole or a bridge.
pub fn link_label(uni: &Universe, rules: &BridgeRules, edge: EdgeIndex, now: u64) -> String {
    match &uni.graph[edge] {
        Link::Stargate => "gate".into(),
        Link::Wormhole(w) => {
            let (from, _) = uni.graph.edge_endpoints(edge).unwrap();
            wormhole_label(w, uni.system(from).id, now)
        }
        Link::JumpBridge => {
            // "Ansiblex · Zone 1 → 2 · 36 TJ". The departure zone sets the cost.
            let (from, to) = uni.graph.edge_endpoints(edge).unwrap();
            let mut parts = vec!["Ansiblex".to_string()];
            if let (Some(a), Some(b)) = (rules.cost(uni, from), rules.cost(uni, to)) {
                parts.push(format!("Zone {} → {}", a.zone, b.zone));
                parts.extend(a.tj.map(|tj| format!("{tj} TJ")));
            }
            parts.join(" · ")
        }
    }
}

/// For example "Wormhole · Sig. ABC · Large · Less than 3h 10m remaining · Critical".
/// "ABC" is the signature in `from`, the system that the jump leaves. With no signature, the
/// label shows "Sig. Unknown", so the pilot knows to scan. A stable mass shows no text.
/// The label does not show the type. The hull check uses the type.
pub fn wormhole_label(w: &Wormhole, from: u32, now: u64) -> String {
    let mut parts = vec!["Wormhole".to_string()];
    parts.push(format!("Sig. {}", w.sig_at(from).as_deref().unwrap_or("Unknown")));
    parts.extend(w.size.map(|s| s.label().to_string()));
    parts.extend(w.expiry.map(|e| expiry_text(e, now)));
    parts.extend(w.mass.filter(|&m| m != MassStatus::Stable).map(|m| m.label().to_string()));
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wormhole_text() {
        use crate::test_support::hole;
        use crate::wormhole::{Expiry, MassStatus, Size, Wormhole};
        let now = 1_000_000;
        let w = Wormhole {
            wh_type: Some("K162".into()),
            size: Some(Size::Large),
            expiry: Some(Expiry { at: now + 3 * 3600 + 600, exact: false }),
            mass: Some(MassStatus::Critical),
            ..hole(1, 2)
        };
        assert_eq!(wormhole_label(&w, 1, now), "Wormhole · Sig. Unknown · Large · Less than 3h 10m remaining · Critical");
        // The signature in the system that the jump leaves: "ABC" from system 1, "DEF" from system 2.
        let signed = Wormhole { sig_a: Some("ABC-123".into()), sig_b: Some("def".into()), ..w.clone() };
        assert_eq!(wormhole_label(&signed, 1, now), "Wormhole · Sig. ABC · Large · Less than 3h 10m remaining · Critical");
        assert_eq!(wormhole_label(&signed, 2, now), "Wormhole · Sig. DEF · Large · Less than 3h 10m remaining · Critical");
        // A stable mass shows no text. The label never shows the type: the hull check uses it.
        let stable = Wormhole { mass: Some(MassStatus::Stable), size: Some(Size::XLarge), ..hole(1, 2) };
        assert_eq!(wormhole_label(&stable, 1, now), "Wormhole · Sig. Unknown · XL");
        let destabilized = Wormhole { mass: Some(MassStatus::Destabilized), ..hole(1, 2) };
        assert_eq!(wormhole_label(&destabilized, 1, now), "Wormhole · Sig. Unknown · Destabilized");
    }

    #[test]
    fn shortcuts_warning_text() {
        use crate::sources::{evescout, nexum};
        let uni = crate::test_support::universe();
        let report = OverlayReport { bridges: 0, unknown: vec!["Nowhere".into()] };
        let mut wh = nexum::Load { warning: Some("Nexum offline, map from 14:02".into()), ..Default::default() };
        wh.report.unknown = vec![1, 2];
        let mut scout = evescout::Load { warning: Some("EVE-Scout offline, feed from 14:02".into()), ..Default::default() };
        scout.report.unknown = vec![3];
        let warning = Shortcuts::new(uni, &report, &wh, &scout).warning.unwrap();
        assert_eq!(
            warning,
            "Bridges: unknown systems: Nowhere · Nexum: unknown system IDs: 1, 2 · EVE-Scout: unknown system IDs: 3 · Nexum offline, map from 14:02 · EVE-Scout offline, feed from 14:02"
        );
    }
}
