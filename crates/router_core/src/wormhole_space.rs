//! The controls for the closed regions of the map: one stance for each class of J-space, one for
//! the Drifter systems and one for Pochven. The stances work like the avoid list, but they stay out of it.

use crate::route::PENALTY;
use crate::universe::{System, Universe};
use crate::wormhole_types::DRIFTER_SYSTEMS;
use serde::{Deserialize, Serialize};

/// What a route does with a group of systems.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    /// The route uses the systems like any other system.
    #[default]
    Allow,
    /// A jump into the systems costs as much as `PENALTY`, like a "prefer" entry of the avoid list.
    Avoid,
    /// No route enters the systems, like a "never" entry of the avoid list.
    Block,
}

impl Stance {
    pub const ALL: [Stance; 3] = [Stance::Allow, Stance::Avoid, Stance::Block];

    pub fn label(self) -> &'static str {
        match self {
            Stance::Allow => "Allow",
            Stance::Avoid => "Avoid",
            Stance::Block => "Block",
        }
    }
}

/// The class that the SDE gives to the systems of Pochven.
const POCHVEN_CLASS: u32 = 25;

/// A group of systems with one stance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// A class of J-space from 1 to 6. The class comes from the `wormholeClassID` of the SDE.
    Class(u8),
    /// The five Drifter systems, by system ID. The SDE does not mark them, so `DRIFTER_SYSTEMS` lists them.
    Drifter,
    /// The systems of Pochven. The SDE gives them the class 25.
    Pochven,
}

impl Group {
    /// The groups in the order of the settings page.
    pub const ALL: [Group; 8] = [
        Group::Class(1),
        Group::Class(2),
        Group::Class(3),
        Group::Class(4),
        Group::Class(5),
        Group::Class(6),
        Group::Drifter,
        Group::Pochven,
    ];

    /// True if a route reaches the group only through a wormhole. Pochven has the filaments too.
    pub fn is_wormhole(self) -> bool {
        self != Group::Pochven
    }

    pub fn label(self) -> String {
        match self {
            Group::Class(c) => format!("Class {c}"),
            Group::Drifter => "Drifter".to_string(),
            Group::Pochven => "Pochven".to_string(),
        }
    }
}

/// The stance of each group. A missing key in a config file gives `Allow`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WormholeSpace {
    pub c1: Stance,
    pub c2: Stance,
    pub c3: Stance,
    pub c4: Stance,
    pub c5: Stance,
    pub c6: Stance,
    pub drifter: Stance,
    pub pochven: Stance,
}

impl WormholeSpace {
    pub fn get(&self, group: Group) -> Stance {
        *self.slot_ref(group)
    }

    pub fn set(&mut self, group: Group, stance: Stance) {
        *self.slot(group) = stance;
    }

    fn slot_ref(&self, group: Group) -> &Stance {
        match group {
            Group::Class(1) => &self.c1,
            Group::Class(2) => &self.c2,
            Group::Class(3) => &self.c3,
            Group::Class(4) => &self.c4,
            Group::Class(5) => &self.c5,
            Group::Class(6) => &self.c6,
            Group::Class(_) => &Stance::Allow,
            Group::Drifter => &self.drifter,
            Group::Pochven => &self.pochven,
        }
    }

    fn slot(&mut self, group: Group) -> &mut Stance {
        match group {
            Group::Class(1) => &mut self.c1,
            Group::Class(2) => &mut self.c2,
            Group::Class(3) => &mut self.c3,
            Group::Class(4) => &mut self.c4,
            Group::Class(5) => &mut self.c5,
            Group::Class(6) => &mut self.c6,
            Group::Class(_) => panic!("only the classes 1 to 6 have a stance"),
            Group::Drifter => &mut self.drifter,
            Group::Pochven => &mut self.pochven,
        }
    }

    /// True if every group is allowed.
    pub fn is_default(&self) -> bool {
        *self == WormholeSpace::default()
    }

    /// True if a group is blocked, so a route can find no way. With `wormholes` off, a wormhole
    /// group has no effect, so only Pochven can block.
    pub fn blocks(&self, wormholes: bool) -> bool {
        Group::ALL.iter().any(|&g| (wormholes || !g.is_wormhole()) && self.get(g) == Stance::Block)
    }

    /// The stance for one system. A Drifter system follows the Drifter stance only.
    pub fn stance(&self, system: &System) -> Stance {
        if DRIFTER_SYSTEMS.contains(&system.id) {
            return self.drifter;
        }
        match system.wormhole_class {
            Some(c @ 1..=6) => self.get(Group::Class(c as u8)),
            Some(POCHVEN_CLASS) => self.pochven,
            _ => Stance::Allow,
        }
    }

    /// The cost of a jump into each system, by `NodeIndex`: `PENALTY` for an avoided system.
    /// The result is for `Router::set_danger`.
    pub fn danger(&self, uni: &Universe) -> Vec<u64> {
        uni.graph.node_weights().map(|s| if self.stance(s) == Stance::Avoid { PENALTY } else { 0 }).collect()
    }

    /// True for each blocked system, by `NodeIndex`. The result is for `Router::set_never`.
    pub fn never(&self, uni: &Universe) -> Vec<bool> {
        uni.graph.node_weights().map(|s| self.stance(s) == Stance::Block).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::universe;

    fn system(name: &str) -> &'static System {
        let uni = universe();
        uni.system(uni.exact(name).unwrap_or_else(|| panic!("{name} is not on the map")))
    }

    #[test]
    fn all_allowed_by_default() {
        let space = WormholeSpace::default();
        assert!(space.is_default() && !space.blocks(true));
        let uni = universe();
        assert!(space.danger(uni).iter().all(|&d| d == 0));
        assert!(space.never(uni).iter().all(|&n| !n));
    }

    #[test]
    fn a_class_stance_acts_on_the_systems_of_that_class_only() {
        let mut space = WormholeSpace::default();
        space.set(Group::Class(1), Stance::Block);
        space.set(Group::Class(2), Stance::Avoid);
        assert!(space.blocks(true) && !space.is_default());
        // J105443 is class 1. Find a class 2 system by the class itself.
        assert_eq!(space.stance(system("J105443")), Stance::Block);
        let c2 = universe().graph.node_weights().find(|s| s.wormhole_class == Some(2)).unwrap();
        assert_eq!(space.stance(c2), Stance::Avoid);
        // Known space, Thera and Pochven are never touched by a class stance.
        for name in ["Jita", "Tama", "Thera", "Kuharah"] {
            assert_eq!(space.stance(system(name)), Stance::Allow, "{name}");
        }
    }

    #[test]
    fn the_drifter_stance_acts_by_system_id() {
        let mut space = WormholeSpace::default();
        space.set(Group::Drifter, Stance::Block);
        for name in ["Sentinel MZ", "Liberated Barbican", "Sanctified Vidette", "Conflux Eyrie", "Azdaja Redoubt"] {
            assert_eq!(space.stance(system(name)), Stance::Block, "{name}");
        }
        assert_eq!(space.never(universe()).iter().filter(|&&n| n).count(), 5);
        // Another Drifter stance leaves the class systems alone.
        assert_eq!(space.stance(system("J105443")), Stance::Allow);
    }

    #[test]
    fn avoid_costs_a_penalty_and_block_stops_the_route() {
        let uni = universe();
        let node = uni.exact("J105443").unwrap();
        let mut space = WormholeSpace::default();
        space.set(Group::Class(1), Stance::Avoid);
        assert_eq!(space.danger(uni)[node.index()], PENALTY);
        assert!(!space.never(uni)[node.index()]);
        space.set(Group::Class(1), Stance::Block);
        assert!(space.never(uni)[node.index()]);
    }

    /// A route from Jita to a class 1 system, through one wormhole. Avoid still crosses the hole
    /// and counts one avoided system. Block finds no route.
    #[test]
    fn a_blocked_class_refuses_the_wormhole_into_it() {
        use crate::test_support::{FIXTURE_TIME, hole, settings};
        let mut uni = universe().clone();
        let (jita, c1) = (uni.exact("Jita").unwrap(), uni.exact("J105443").unwrap());
        uni.add_wormholes(&[hole(uni.system(jita).id, uni.system(c1).id)]);
        let mut s = settings(&uni, None);
        let found = |s: &crate::settings::Settings| s.router(&uni, FIXTURE_TIME).routes(&[jita, c1], 1);
        assert_eq!(found(&s).unwrap()[0].path.nodes.len(), 2);
        s.space.set(Group::Class(1), Stance::Avoid);
        let routes = found(&s).unwrap();
        assert_eq!((routes[0].path.nodes.len(), routes[0].avoided), (2, 1));
        s.space.set(Group::Class(1), Stance::Block);
        assert!(found(&s).is_err());
        // Another class stance leaves the hole open.
        s.space.set(Group::Class(1), Stance::Allow);
        s.space.set(Group::Class(2), Stance::Block);
        assert!(found(&s).is_ok());
    }

    /// The SDE gives the classes 14 to 18 to the five systems of `DRIFTER_SYSTEMS` and to no other
    /// system. The SDE has no label for a class, so the list stays a list of IDs. This test fails
    /// if a new SDE adds a system to these classes, or drops one.
    #[test]
    fn the_drifter_list_matches_the_sde_classes() {
        let uni = universe();
        let mut from_sde: Vec<u32> =
            uni.graph.node_weights().filter(|s| s.wormhole_class.is_some_and(|c| (14..=18).contains(&c))).map(|s| s.id).collect();
        from_sde.sort_unstable();
        let mut listed = DRIFTER_SYSTEMS.to_vec();
        listed.sort_unstable();
        assert_eq!(from_sde, listed);
    }

    #[test]
    fn the_pochven_stance_acts_on_the_systems_of_class_25() {
        let uni = universe();
        let mut space = WormholeSpace::default();
        space.set(Group::Pochven, Stance::Block);
        assert_eq!(space.stance(system("Kuharah")), Stance::Block);
        for name in ["Jita", "Thera", "J105443", "Sentinel MZ"] {
            assert_eq!(space.stance(system(name)), Stance::Allow, "{name}");
        }
        let blocked = space.never(uni).iter().filter(|&&n| n).count();
        let class_25 = uni.graph.node_weights().filter(|s| s.wormhole_class == Some(25)).count();
        assert_eq!(blocked, class_25);
        assert!(blocked > 0);
        space.set(Group::Pochven, Stance::Avoid);
        assert_eq!(space.danger(uni).iter().filter(|&&d| d == PENALTY).count(), class_25);
    }

    /// A filament jump ends in a system of Pochven, so a blocked Pochven leaves the filament no use.
    /// Avoid still lets the filament run, because a trip that ends in Pochven has no other way.
    #[test]
    fn a_blocked_pochven_stops_the_filament_route() {
        use crate::test_support::{FIXTURE_TIME, settings};
        use crate::universe::Clade;
        use petgraph::visit::EdgeRef;
        let uni = universe();
        let hub = uni.filament_hub(Clade::Perun).unwrap();
        let target = uni.graph.edges(hub).next().unwrap().target();
        let jita = uni.exact("Jita").unwrap();
        let mut s = settings(uni, None);
        s.filaments = true;
        let found = |s: &crate::settings::Settings| s.router(uni, FIXTURE_TIME).routes(&[jita, target], 1);
        assert!(found(&s).is_ok());
        s.space.set(Group::Pochven, Stance::Avoid);
        assert!(found(&s).is_ok());
        s.space.set(Group::Pochven, Stance::Block);
        assert!(found(&s).is_err());
        // A blocked class of wormhole space leaves the filament alone.
        s.space.set(Group::Pochven, Stance::Allow);
        s.space.set(Group::Class(1), Stance::Block);
        assert!(found(&s).is_ok());
    }

    /// A blocked wormhole group has no effect while wormholes are off. A blocked Pochven does.
    #[test]
    fn a_wormhole_group_blocks_only_while_wormholes_are_on() {
        let mut space = WormholeSpace::default();
        space.set(Group::Class(4), Stance::Block);
        space.set(Group::Drifter, Stance::Block);
        assert!(space.blocks(true) && !space.blocks(false));
        space.set(Group::Pochven, Stance::Block);
        assert!(space.blocks(false));
    }

    #[test]
    fn config_text_reads_with_missing_keys() {
        let space: WormholeSpace = serde_json::from_str(r#"{"c5":"block","drifter":"avoid"}"#).unwrap();
        assert_eq!((space.c5, space.drifter, space.c1), (Stance::Block, Stance::Avoid, Stance::Allow));
        assert_eq!(serde_json::from_str::<WormholeSpace>("{}").unwrap(), WormholeSpace::default());
        let text = serde_json::to_string(&space).unwrap();
        assert_eq!(serde_json::from_str::<WormholeSpace>(&text).unwrap(), space);
    }
}
