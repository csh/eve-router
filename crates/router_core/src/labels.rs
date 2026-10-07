//! The text of the route summaries, the route steps and the Shortcuts box, for each user interface.

use crate::ansiblex::{BridgeRules, HullClass, hull_by_type};
use crate::esi::active::ActiveRoute;
use crate::esi::client::Ship;
use crate::esi::pilots::Pilots;
use crate::overlay::OverlayReport;
use crate::route::Route;
use crate::settings::{HullSource, Settings};
use crate::sources::{evescout, nexum};
use crate::universe::{Link, Universe, display_sec};
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

/// The in-game security colors, as `0xRRGGBB`.
pub fn sec_rgb(security: f64) -> u32 {
    match (display_sec(security) * 10.0).round() as i32 {
        10.. => 0x2FEFEF,
        9 => 0x48F0C0,
        8 => 0x00EF47,
        7 => 0x00F000,
        6 => 0x8FEF2F,
        5 => 0xEFEF00,
        4 => 0xD77700,
        3 => 0xF06000,
        2 => 0xF04800,
        1 => 0xD73000,
        _ => 0xF00000,
    }
}

pub fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// A ship shows its group, for example "Sin (Black Ops)". A group shows only its name.
pub fn hull_label(h: HullClass) -> String {
    match h.type_id {
        Some(_) => format!("{} ({})", h.name, h.group),
        None => h.name.clone(),
    }
}

/// "1 jump" or "N jumps".
/// The ship of a pilot, for the pilot rows. For example `Apocalypse · "apoc"`.
pub struct ShipText {
    /// The ship type. The ship name, for a type that is not in `ships.json`.
    pub kind: String,
    /// The name that the player gave the ship. `None` for a default name.
    pub name: Option<String>,
    /// True if the ship cannot use a jump bridge.
    pub no_bridges: bool,
}

pub fn ship_text(ship: &Ship, character: &str) -> ShipText {
    let Some(hull) = hull_by_type(ship.ship_type_id) else {
        return ShipText { kind: ship.ship_name.clone(), name: None, no_bridges: false };
    };
    let name = ship.ship_name.trim().to_lowercase();
    let kind = hull.name.to_lowercase();
    // EVE names a new ship after its type and its pilot, for example "Capsule - Cyrene Hawthorne".
    let default = name.is_empty() || name == kind || (name.contains(&kind) && name.contains(&character.to_lowercase()));
    ShipText { kind: hull.name.clone(), name: (!default).then(|| ship.ship_name.trim().to_string()), no_bridges: hull.base_tj.is_none() }
}

impl std::fmt::Display for ShipText {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(name) = &self.name {
            write!(f, " · \"{name}\"")?;
        }
        if self.no_bridges {
            write!(f, " · no bridges")?;
        }
        Ok(())
    }
}

/// The text of the Pilot button, for example `smrkn · Apocalypse` or `Hull: Sin (Black Ops)`.
pub fn pilot_label(settings: &Settings, pilots: &Pilots) -> String {
    match settings.hull_source {
        HullSource::Pilot(id) => {
            let name = pilots.characters().into_iter().find(|c| c.id == id).map_or_else(|| id.to_string(), |c| c.name);
            let kind = settings.rules.hull.map_or("ship unknown".into(), |h| h.name.clone());
            format!("{name} · {kind}")
        }
        HullSource::Manual => format!("Hull: {}", settings.rules.hull.map_or("none".into(), hull_label)),
    }
}

/// The banner text for the next wormhole of the active route. `None` when no wormhole is ahead.
/// For example "In Perimeter: warp to signature ABC and jump to Amarr."
pub fn wormhole_hint(uni: &Universe, route: &ActiveRoute) -> Option<String> {
    let step = route.next_wormhole()?;
    let name = |id: u32| uni.by_id.get(&id).map_or_else(|| id.to_string(), |&n| uni.name(n).to_string());
    let from = name(route.steps[step - 1].system);
    let to = name(route.steps[step].system);
    let sig = route.steps[step].sig.as_deref();
    Some(match (route.progress + 1 == step, sig) {
        (true, Some(sig)) => format!("In {from}: warp to signature {sig} and jump to {to}."),
        (true, None) => format!("In {from}: scan the wormhole to {to}. The signature is not known."),
        (false, Some(sig)) => format!("Next wormhole: {from} to {to}, signature {sig}."),
        (false, None) => format!("Next wormhole: {from} to {to}, signature not known."),
    })
}

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
    if let Some(pct) = route.bridge_cap_pct {
        parts.push(format!("{pct:.1}% of a gate"));
    }
    if route.unknown_sigs > 0 {
        parts.push(format!("{} unknown sig{}", route.unknown_sigs, if route.unknown_sigs == 1 { "" } else { "s" }));
    }
    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) }
}

/// The text of route number `index` (from 0), as `--print` writes it: the summary line, then
/// one line for each step. The text has no final line break.
pub fn route_text(uni: &Universe, rules: &BridgeRules, index: usize, route: &Route, now: u64) -> String {
    let mut lines = vec![format!("#{} {}{}", index + 1, jumps_label(route.jumps), route_extras(route))];
    for (step, &node) in route.path.nodes.iter().enumerate() {
        let sys = uni.system(node);
        let via = match step.checked_sub(1).map(|s| route.path.edges[s]) {
            Some(e) => link_label(uni, rules, e, now),
            None => String::new(),
        };
        let stop = route.stop_at(step).map(|s| s.label()).unwrap_or_default();
        lines.push(format!("  {:>3} {stop:<11} {:<20} {:>4.1} {:<20} {via}", step, sys.name, display_sec(sys.security), sys.region));
    }
    lines.join("\n")
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
    fn pilot_label_names_the_source() {
        use crate::settings::HullSource;
        use crate::test_support::{overlay_universe, settings};
        let uni = overlay_universe();
        let pilots = Pilots::offline(std::env::temp_dir().join("eve-router-test-pilot-label").join("active-route.json"));
        let mut s = settings(&uni, Some("Sin"));
        assert_eq!(pilot_label(&s, &pilots), "Hull: Sin (Black Ops)");
        // A character that is not in the list shows its ID.
        s.hull_source = HullSource::Pilot(7);
        assert_eq!(pilot_label(&s, &pilots), "7 · Sin");
        s.rules.hull = None;
        assert_eq!(pilot_label(&s, &pilots), "7 · ship unknown");
    }

    #[test]
    fn wormhole_hint_names_the_signature() {
        use crate::esi::active::{ActiveRoute, Hop, Step};
        let uni = crate::test_support::overlay_universe();
        let id = |name: &str| uni.system(uni.exact(name).unwrap()).id;
        let step = |name: &str, hop, sig: Option<&str>| Step { system: id(name), hop, via: String::new(), sig: sig.map(String::from) };
        let steps = vec![step("Jita", Hop::Start, None), step("Perimeter", Hop::Gate, None), step("Amarr", Hop::Wormhole, Some("ABC"))];
        let mut r = ActiveRoute::from_steps(steps, vec![0, 2], 1, 7, "Alice");
        assert_eq!(wormhole_hint(&uni, &r).as_deref(), Some("Next wormhole: Perimeter to Amarr, signature ABC."));
        // The pilot is in the system of the wormhole.
        r.progress = 1;
        assert_eq!(wormhole_hint(&uni, &r).as_deref(), Some("In Perimeter: warp to signature ABC and jump to Amarr."));
        r.steps[2].sig = None;
        assert_eq!(wormhole_hint(&uni, &r).as_deref(), Some("In Perimeter: scan the wormhole to Amarr. The signature is not known."));
        r.progress = 2;
        assert_eq!(wormhole_hint(&uni, &r), None);
    }

    #[test]
    fn ship_text_shows_the_type_first() {
        use crate::ansiblex::find_hull;
        use crate::esi::client::Ship;
        let ship = |name: &str, type_id: u32| Ship { ship_type_id: type_id, ship_item_id: 1, ship_name: name.into() };
        let apoc = find_hull("Apocalypse").unwrap().type_id.unwrap();
        // A name from the player shows after the type.
        assert_eq!(ship_text(&ship("apoc", apoc), "smrkn").to_string(), "Apocalypse · \"apoc\"");
        // A default name does not show. EVE uses both forms.
        assert_eq!(ship_text(&ship("Capsule - Cyrene Hawthorne", 670), "Cyrene Hawthorne").to_string(), "Capsule");
        assert_eq!(ship_text(&ship("smrkn's Apocalypse", apoc), "smrkn").to_string(), "Apocalypse");
        assert_eq!(ship_text(&ship("Apocalypse", apoc), "smrkn").to_string(), "Apocalypse");
        // A ship that cannot use bridges says so.
        let avatar = find_hull("Avatar").unwrap().type_id.unwrap();
        assert_eq!(ship_text(&ship("Big", avatar), "smrkn").to_string(), "Avatar · \"Big\" · no bridges");
        // A type that is not in ships.json shows the ship name only.
        let unknown = ship_text(&ship("New hull", u32::MAX), "smrkn");
        assert_eq!(unknown.to_string(), "New hull");
        assert!(!unknown.no_bridges);
    }

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
