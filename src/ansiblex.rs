//! Ansiblex jump bridge rules from the September 2026 capacitor update.
//! Source: https://www.eveonline.com/news/view/force-projection-ansiblex-capacitor-update
//! The hull costs and the gate capacitor come from the SDE. The zones are not in the SDE.

use crate::ships::ShipData;
use crate::universe::Universe;
use petgraph::graph::NodeIndex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// A ship, or a ship group that stands for all of its ships.
pub struct Hull {
    /// `None` for a group.
    pub type_id: Option<u32>,
    pub name: String,
    pub group: String,
    /// The base cost of one jump in TJ. `None` means the hull cannot use a bridge.
    pub base_tj: Option<f32>,
    /// Why the hull cannot use a bridge.
    pub ban: Option<String>,
}

impl Hull {
    /// The kebab-case name for the CLI and the config file, for example `black-ops`.
    pub fn key(&self) -> String {
        self.name.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
    }
}

/// A `&'static Hull` lets the later ESI step map a ship type to a hull.
pub type HullClass = &'static Hull;

/// The hulls and the Ansiblex values from the SDE (`ships.json`).
pub struct HullTable {
    /// Sorted by group, then name.
    pub ships: Vec<Hull>,
    pub groups: Vec<Hull>,
    /// The capacitor of one Ansiblex gate.
    pub gate_capacitor_tj: f32,
}

static TABLE: OnceLock<HullTable> = OnceLock::new();

/// GJ in one TJ. The SDE gives capacitor values in GJ.
const GJ_PER_TJ: f64 = 1000.0;

impl HullTable {
    fn new(data: ShipData) -> Self {
        let max_mass = data.ansiblex.max_jump_mass_kg;
        // A ship needs a bridge cost, and its mass must not be more than the gate limit.
        let check = |cost_gj: Option<f64>, mass: Option<f64>| match (cost_gj, mass) {
            (None, _) => Err("has no Ansiblex activation cost".to_string()),
            (Some(_), Some(m)) if m > max_mass => Err(format!("is heavier than the {max_mass:.0} kg gate limit")),
            (Some(c), _) => Ok((c / GJ_PER_TJ) as f32),
        };
        let mut groups: BTreeMap<String, (Option<f64>, Option<f64>)> = BTreeMap::new();
        let ships = data
            .ships
            .into_iter()
            .map(|s| {
                // A group gets the highest cost and the highest mass of its ships.
                let g = groups.entry(s.group.clone()).or_insert((s.bridge_cost_gj, s.mass_kg));
                g.0 = g.0.zip(s.bridge_cost_gj).map(|(a, b)| a.max(b));
                g.1 = match (g.1, s.mass_kg) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                let result = check(s.bridge_cost_gj, s.mass_kg);
                Hull { type_id: Some(s.type_id), name: s.name, group: s.group, base_tj: result.clone().ok(), ban: result.err() }
            })
            .collect();
        let groups = groups
            .into_iter()
            .map(|(name, (cost, mass))| {
                let result = check(cost, mass);
                Hull { type_id: None, group: name.clone(), name, base_tj: result.clone().ok(), ban: result.err() }
            })
            .collect();
        HullTable { ships, groups, gate_capacitor_tj: (data.ansiblex.capacitor_gj / GJ_PER_TJ) as f32 }
    }
}

/// Load the hull table from `ships.json` in the SDE directory. Later calls return the first table.
pub fn init(sde_dir: &std::path::Path) -> Result<&'static HullTable, String> {
    if let Some(table) = TABLE.get() {
        return Ok(table);
    }
    let data = crate::ships::load(sde_dir)?;
    Ok(TABLE.get_or_init(|| HullTable::new(data)))
}

pub fn table() -> &'static HullTable {
    // The tests read the repository SDE.
    #[cfg(test)]
    init(std::path::Path::new("sde")).unwrap();
    TABLE.get().expect("ansiblex::init must run before ansiblex::table")
}

/// Find a ship by its name, else a group by its name or key, case-insensitive.
pub fn find_hull(name: &str) -> Option<HullClass> {
    let name = name.trim();
    let t = table();
    t.ships
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .or_else(|| t.groups.iter().find(|h| h.name.eq_ignore_ascii_case(name) || h.key().eq_ignore_ascii_case(name)))
}

/// The zone and the cost multiplier for a distance from the capital.
pub fn zone(ly: f64) -> (u8, f32) {
    match ly {
        d if d <= 5.0 => (1, 0.0),
        d if d <= 10.0 => (2, 2.0),
        d if d <= 15.0 => (3, 6.0),
        d if d <= 20.0 => (4, 9.0),
        _ => (5, 15.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BridgeCost {
    pub zone: u8,
    pub multiplier: f32,
    /// `None` when no hull is set.
    pub tj: Option<f32>,
}

#[derive(Clone, Copy, Default)]
pub struct BridgeRules {
    pub capital: Option<NodeIndex>,
    pub hull: Option<HullClass>,
    pub max_cap: Option<f32>,
}

impl BridgeRules {
    /// The cost of a bridge jump. The departure system sets the zone.
    pub fn cost(&self, uni: &Universe, from: NodeIndex) -> Option<BridgeCost> {
        let capital = self.capital?;
        let (zone, multiplier) = zone(uni.distance_ly(capital, from));
        let tj = self.hull.and_then(|h| h.base_tj).map(|base| base * multiplier);
        Some(BridgeCost { zone, multiplier, tj })
    }

    /// A bridge jump needs a capital and a hull that can use bridges.
    /// With a cap limit, the jump cost must not be more than the limit.
    pub fn allowed(&self, uni: &Universe, from: NodeIndex) -> bool {
        if self.hull.is_some_and(|h| h.base_tj.is_none()) {
            return false;
        }
        let Some(cost) = self.cost(uni, from) else {
            return false;
        };
        match (self.max_cap, cost.tj) {
            (Some(max), Some(tj)) => tj <= max,
            _ => true,
        }
    }

    /// Why bridges are off for every jump, if they are.
    pub fn blocked_reason(&self) -> Option<String> {
        if self.capital.is_none() {
            return Some("Set a capital to use jump bridges".into());
        }
        let hull = self.hull?;
        hull.ban.as_ref().map(|ban| format!("The {} {ban}", hull.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::tests::universe;

    fn rules(hull: Option<&str>, max_cap: Option<f32>) -> BridgeRules {
        let uni = universe();
        BridgeRules {
            capital: uni.exact("JK-Q77"),
            hull: hull.map(|h| find_hull(h).unwrap()),
            max_cap,
        }
    }

    #[test]
    fn zones_from_capital() {
        let uni = universe();
        let r = rules(None, None);
        let zone_of = |name: &str| r.cost(uni, uni.exact(name).unwrap()).unwrap().zone;
        assert_eq!(zone_of("KW-OAM"), 1);
        assert_eq!(zone_of("QLU-P0"), 2);
        assert_eq!(zone_of("T6GY-Y"), 3);
    }

    #[test]
    fn black_ops_cost() {
        let uni = universe();
        let r = rules(Some("black-ops"), None);
        let cost = r.cost(uni, uni.exact("QLU-P0").unwrap()).unwrap();
        assert_eq!(cost.tj, Some(36.0));
    }

    #[test]
    fn cap_limit_and_bans() {
        let uni = universe();
        let qlu = uni.exact("QLU-P0").unwrap();
        let kw = uni.exact("KW-OAM").unwrap();
        let r = rules(Some("black-ops"), Some(30.0));
        assert!(!r.allowed(uni, qlu));
        assert!(r.allowed(uni, kw));
        assert!(!rules(Some("titan"), None).allowed(uni, kw));
        let no_capital = BridgeRules { capital: None, ..rules(None, None) };
        assert!(!no_capital.allowed(uni, kw));
    }

    /// The table from the blog post, in TJ. The SDE values must match it.
    const BLOG: &[(&str, f32)] = &[
        ("Capsule", 1.0), ("Exhumer", 1.0), ("Expedition Frigate", 1.0), ("Freighter", 1.0),
        ("Hauler", 1.0), ("Jump Freighter", 1.0), ("Mining Barge", 1.0), ("Prototype Exploration Ship", 1.0),
        ("Shuttle", 1.0), ("Special Edition Yachts", 1.0), ("Corvette", 1.0), ("Frigate", 4.0),
        ("Covert Ops", 5.0), ("Destroyer", 6.0), ("Assault Frigate", 7.0), ("Blockade Runner", 7.0),
        ("Deep Space Transport", 7.0), ("Interceptor", 8.0), ("Interdictor", 8.5), ("Command Destroyer", 9.0),
        ("Electronic Attack Ship", 9.0), ("Logistics Frigate", 9.0), ("Tactical Destroyer", 9.0),
        ("Industrial Command Ship", 10.0), ("Cruiser", 10.0), ("Heavy Interdiction Cruiser", 10.5),
        ("Stealth Bomber", 11.0), ("Force Recon Ship", 11.5), ("Heavy Assault Cruiser", 11.5),
        ("Combat Recon Ship", 11.5), ("Flag Cruiser", 12.0), ("Logistics", 12.5), ("Strategic Cruiser", 13.0),
        ("Attack Battlecruiser", 14.0), ("Combat Battlecruiser", 14.5), ("Expedition Command Ship", 15.0),
        ("Command Ship", 16.5), ("Battleship", 16.5), ("Black Ops", 18.0), ("Marauder", 19.0),
        ("Capital Industrial Ship", 19.0),
    ];

    #[test]
    fn sde_costs_match_blog() {
        for &(group, tj) in BLOG {
            let hull = find_hull(group).unwrap_or_else(|| panic!("no group {group}"));
            assert_eq!(hull.base_tj, Some(tj), "{group}");
        }
        assert_eq!(table().gate_capacitor_tj, 1250.0);
    }

    #[test]
    fn sde_bans() {
        let mut banned: Vec<&str> =
            table().groups.iter().filter(|h| h.base_tj.is_none()).map(|h| h.name.as_str()).collect();
        banned.sort();
        assert_eq!(
            banned,
            ["Carrier", "Command Carrier", "Dreadnought", "Force Auxiliary", "Lancer Dreadnought", "Supercarrier", "Titan"]
        );
        // The Rorqual can use a bridge, but the Avatar cannot.
        assert_eq!(find_hull("Rorqual").unwrap().base_tj, Some(19.0));
        assert!(find_hull("Avatar").unwrap().ban.is_some());
        // A ship name and a group key both work.
        assert_eq!(find_hull("sin").unwrap().base_tj, Some(18.0));
        assert_eq!(find_hull("heavy-assault-cruiser").unwrap().base_tj, Some(11.5));
    }
}
