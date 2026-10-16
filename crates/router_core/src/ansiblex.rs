//! Ansiblex jump bridge rules from the September 2026 capacitor update.
//! Source: https://www.eveonline.com/news/view/force-projection-ansiblex-capacitor-update
//! The hull costs and the gate capacitor come from the SDE. The zones are not in the SDE.

use crate::error::SdeError;
use crate::ships::ShipData;
use crate::universe::Universe;
use petgraph::graph::NodeIndex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Why a hull cannot use an Ansiblex bridge. The text completes "The <hull> ...".
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Ban {
    #[error("has no Ansiblex activation cost")]
    NoCost,
    #[error("is heavier than the {max_mass_kg:.0} kg gate limit")]
    TooHeavy { max_mass_kg: f64 },
}

/// A ship, or a ship group that stands for all of its ships.
pub struct Hull {
    /// `None` for a group.
    pub type_id: Option<u32>,
    pub name: String,
    pub group: String,
    /// The base cost of one jump in TJ. `None` means the hull cannot use a bridge.
    pub base_tj: Option<f32>,
    /// The share of the gate capacitor that one base jump uses, in percent. `None` with `base_tj`.
    pub base_cap_pct: Option<f32>,
    /// Why the hull cannot use a bridge.
    pub ban: Option<Ban>,
    /// The mass in kg, for the wormhole size check. A group has the mass of its heaviest ship.
    pub mass_kg: Option<f64>,
}

impl Hull {
    /// The kebab-case name for the config file, for example `black-ops`.
    pub fn key(&self) -> String {
        self.name.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
    }
}

/// A shared table entry. Compare two entries with `same_hull`, not with `==`.
pub type HullClass = Arc<Hull>;

/// The hulls and the Ansiblex values from the SDE (`ships.json`).
pub struct HullTable {
    /// Sorted by group, then name.
    pub ships: Vec<HullClass>,
    pub groups: Vec<HullClass>,
    /// The capacitor of one Ansiblex gate.
    pub gate_capacitor_tj: f32,
}

/// GJ in one TJ. The SDE gives capacitor values in GJ.
const GJ_PER_TJ: f64 = 1000.0;

impl HullTable {
    fn new(data: ShipData) -> Self {
        let max_mass = data.ansiblex.max_jump_mass_kg;
        let gate_capacitor_tj = (data.ansiblex.capacitor_gj / GJ_PER_TJ) as f32;
        let hull = |type_id: Option<u32>, name: String, group: String, result: Result<f32, Ban>, mass_kg: Option<f64>| {
            let base_tj = result.as_ref().ok().copied();
            Arc::new(Hull {
                type_id,
                name,
                group,
                base_tj,
                base_cap_pct: base_tj.map(|tj| tj / gate_capacitor_tj * 100.0),
                ban: result.err(),
                mass_kg,
            })
        };
        // A ship needs a bridge cost, and its mass must not be more than the gate limit.
        let check = |cost_gj: Option<f64>, mass: Option<f64>| match (cost_gj, mass) {
            (None, _) => Err(Ban::NoCost),
            (Some(_), Some(m)) if m > max_mass => Err(Ban::TooHeavy { max_mass_kg: max_mass }),
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
                hull(Some(s.type_id), s.name, s.group, check(s.bridge_cost_gj, s.mass_kg), s.mass_kg)
            })
            .collect();
        let groups = groups.into_iter().map(|(name, (cost, mass))| hull(None, name.clone(), name, check(cost, mass), mass)).collect();
        HullTable { ships, groups, gate_capacitor_tj }
    }
}

impl HullTable {
    /// Load the hull table from `ships.json` in the SDE directory.
    pub fn load(sde_dir: &Path) -> Result<Arc<HullTable>, SdeError> {
        Ok(Arc::new(HullTable::new(crate::ships::load(sde_dir)?)))
    }

    /// Find a ship by its name, else a group by its name or key, case-insensitive.
    pub fn find(&self, name: &str) -> Option<HullClass> {
        let name = name.trim();
        self.ships
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .or_else(|| self.groups.iter().find(|h| h.name.eq_ignore_ascii_case(name) || h.key().eq_ignore_ascii_case(name)))
            .cloned()
    }

    /// The ship with this type ID. `None` for a type that is not in `ships.json`.
    pub fn by_type(&self, type_id: u32) -> Option<HullClass> {
        self.ships.iter().find(|h| h.type_id == Some(type_id)).cloned()
    }

    /// The hull picker rows: "none", then each ship whose name or group contains `filter`.
    pub fn rows(&self, filter: &str) -> Vec<Option<HullClass>> {
        let filter = filter.to_lowercase();
        std::iter::once(None)
            .chain(
                self.ships
                    .iter()
                    .filter(|h| h.name.to_lowercase().contains(&filter) || h.group.to_lowercase().contains(&filter))
                    .cloned()
                    .map(Some),
            )
            .collect()
    }
}

/// True if `a` and `b` are the same table entry, or both are "none".
pub fn same_hull(a: Option<&HullClass>, b: Option<&HullClass>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (a, b) => a.is_none() && b.is_none(),
    }
}

/// True if a route for `planned` suits `ship`: the same ship, or `planned` is the group of `ship`.
pub fn same_class(planned: &HullClass, ship: &HullClass) -> bool {
    Arc::ptr_eq(planned, ship) || (planned.type_id.is_none() && planned.group == ship.group)
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
    /// The share of the gate capacitor that the jump uses, in percent. `None` when no hull is set.
    pub cap_pct: Option<f32>,
}

#[derive(Clone, Default)]
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
        let tj = self.hull.as_ref().and_then(|h| h.base_tj).map(|base| base * multiplier);
        let cap_pct = self.hull.as_ref().and_then(|h| h.base_cap_pct).map(|pct| pct * multiplier);
        Some(BridgeCost { zone, multiplier, tj, cap_pct })
    }

    /// A bridge jump needs a capital and a hull that can use bridges.
    /// With a cap limit, the jump cost must not be more than the limit.
    pub fn allowed(&self, uni: &Universe, from: NodeIndex) -> bool {
        if self.hull.as_ref().is_some_and(|h| h.base_tj.is_none()) {
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
        let hull = self.hull.as_ref()?;
        hull.ban.as_ref().map(|ban| format!("The {} {ban}", hull.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{hulls, universe};

    fn rules(hull: Option<&str>, max_cap: Option<f32>) -> BridgeRules {
        let uni = universe();
        BridgeRules { capital: uni.exact("JK-Q77"), hull: hull.map(|h| hulls().find(h).unwrap()), max_cap }
    }

    #[test]
    fn a_missing_table_names_the_file_and_the_way_out() {
        let tmp = tempfile::tempdir().unwrap();
        let Err(error) = HullTable::load(tmp.path()) else { panic!("loaded from an empty directory") };
        assert!(matches!(error, SdeError::Missing { .. }), "{error:?}");
        let text = error.to_string();
        assert!(text.contains("ships.json") && text.contains("Unset EVE_ROUTER_SKIP_SDE_CHECK to download it."), "{text}");
    }

    #[test]
    fn a_ban_completes_the_sentence_about_the_hull() {
        let avatar = hulls().find("Avatar").unwrap();
        assert!(matches!(avatar.ban, Some(Ban::NoCost)));
        let rules = BridgeRules { capital: None, hull: Some(avatar), max_cap: None };
        // A capital is the first rule, so set one to see the ban text.
        let uni = universe();
        let rules = BridgeRules { capital: uni.exact("JK-Q77"), ..rules };
        assert_eq!(rules.blocked_reason().as_deref(), Some("The Avatar has no Ansiblex activation cost"));
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
        // The gate holds 1250 TJ, so 36 TJ is 2.88%.
        assert!((cost.cap_pct.unwrap() - 2.88).abs() < 1e-4);
        assert_eq!(rules(None, None).cost(uni, uni.exact("QLU-P0").unwrap()).unwrap().cap_pct, None);
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
        ("Capsule", 1.0),
        ("Exhumer", 1.0),
        ("Expedition Frigate", 1.0),
        ("Freighter", 1.0),
        ("Hauler", 1.0),
        ("Jump Freighter", 1.0),
        ("Mining Barge", 1.0),
        ("Prototype Exploration Ship", 1.0),
        ("Shuttle", 1.0),
        ("Special Edition Yachts", 1.0),
        ("Corvette", 1.0),
        ("Frigate", 4.0),
        ("Covert Ops", 5.0),
        ("Destroyer", 6.0),
        ("Assault Frigate", 7.0),
        ("Blockade Runner", 7.0),
        ("Deep Space Transport", 7.0),
        ("Interceptor", 8.0),
        ("Interdictor", 8.5),
        ("Command Destroyer", 9.0),
        ("Electronic Attack Ship", 9.0),
        ("Logistics Frigate", 9.0),
        ("Tactical Destroyer", 9.0),
        ("Industrial Command Ship", 10.0),
        ("Cruiser", 10.0),
        ("Heavy Interdiction Cruiser", 10.5),
        ("Stealth Bomber", 11.0),
        ("Force Recon Ship", 11.5),
        ("Heavy Assault Cruiser", 11.5),
        ("Combat Recon Ship", 11.5),
        ("Flag Cruiser", 12.0),
        ("Logistics", 12.5),
        ("Strategic Cruiser", 13.0),
        ("Attack Battlecruiser", 14.0),
        ("Combat Battlecruiser", 14.5),
        ("Expedition Command Ship", 15.0),
        ("Command Ship", 16.5),
        ("Battleship", 16.5),
        ("Black Ops", 18.0),
        ("Marauder", 19.0),
        ("Capital Industrial Ship", 19.0),
    ];

    #[test]
    fn sde_costs_match_blog() {
        for &(group, tj) in BLOG {
            let hull = hulls().find(group).unwrap_or_else(|| panic!("no group {group}"));
            assert_eq!(hull.base_tj, Some(tj), "{group}");
        }
        assert_eq!(hulls().gate_capacitor_tj, 1250.0);
    }

    #[test]
    fn sde_bans() {
        let table = hulls();
        let mut banned: Vec<&str> = table.groups.iter().filter(|h| h.base_tj.is_none()).map(|h| h.name.as_str()).collect();
        banned.sort();
        assert_eq!(banned, ["Carrier", "Command Carrier", "Dreadnought", "Force Auxiliary", "Lancer Dreadnought", "Supercarrier", "Titan"]);
        // The Rorqual can use a bridge, but the Avatar cannot.
        assert_eq!(hulls().find("Rorqual").unwrap().base_tj, Some(19.0));
        assert!(hulls().find("Avatar").unwrap().ban.is_some());
        // A ship name and a group key both work.
        assert_eq!(hulls().find("sin").unwrap().base_tj, Some(18.0));
        assert_eq!(hulls().find("heavy-assault-cruiser").unwrap().base_tj, Some(11.5));
    }

    #[test]
    fn hull_by_type_id() {
        let sin = hulls().find("Sin").unwrap();
        assert!(same_hull(hulls().by_type(sin.type_id.unwrap()).as_ref(), Some(&sin)));
        assert_eq!(hulls().by_type(670).unwrap().name, "Capsule");
        assert!(hulls().by_type(u32::MAX).is_none());
    }

    #[test]
    fn hull_compare() {
        let sin = hulls().find("Sin").unwrap();
        let black_ops = hulls().find("black-ops").unwrap();
        let rorqual = hulls().find("Rorqual").unwrap();
        assert!(same_hull(Some(&sin), Some(&sin)));
        assert!(same_hull(None, None));
        assert!(!same_hull(Some(&sin), None));
        assert!(!same_hull(Some(&black_ops), Some(&sin)));
        // A planned group covers each ship of the group.
        assert!(same_class(&black_ops, &sin));
        assert!(same_class(&sin, &sin));
        assert!(!same_class(&rorqual, &sin));
        assert!(!same_class(&sin, &black_ops));
    }

    #[test]
    fn hull_mass() {
        assert_eq!(hulls().find("Sin").unwrap().mass_kg, Some(106_300_000.0));
        assert_eq!(hulls().find("Rifter").unwrap().mass_kg, Some(1_067_000.0));
        // A group has the mass of its heaviest ship.
        let group = hulls().find("black-ops").unwrap().mass_kg.unwrap();
        assert!(group >= 106_300_000.0);
        let heaviest = hulls().ships.iter().filter(|h| h.group == "Black Ops").filter_map(|h| h.mass_kg).fold(0.0, f64::max);
        assert_eq!(group, heaviest);
    }
}
