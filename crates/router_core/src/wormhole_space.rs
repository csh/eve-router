//! The controls for the closed regions of the map: one stance for each class of J-space, one for
//! the Drifter systems and one for Pochven. The stances work like the avoid list, but they stay out of it.

use crate::route::PENALTY;
use crate::universe::{System, Universe};
use crate::wormhole_types::DRIFTER_SYSTEMS;
use serde::{Deserialize, Serialize};

/// What a route does with a group of systems. The order runs from mild to strict, so `max` of two
/// stances is the strictest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

    /// The word of a switch that shows the stance, for example "Filaments: avoid".
    pub fn state(self) -> &'static str {
        match self {
            Stance::Allow => "on",
            Stance::Avoid => "avoid",
            Stance::Block => "off",
        }
    }

    /// The stance that a click on an on and off switch gives: Block turns on, any other stance turns off.
    pub fn toggled(self) -> Stance {
        match self {
            Stance::Block => Stance::Allow,
            Stance::Allow | Stance::Avoid => Stance::Block,
        }
    }
}

/// The file form of a stance that used to be a switch. An old file holds `true` or `false`. Both
/// read as Allow and Block. A new file writes `true` and `false` for those two, so an old version
/// still reads it, and the word "avoid" for Avoid. Use it with `#[serde(with = "...::switch")]`.
pub mod switch {
    use super::Stance;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum File {
        Switch(bool),
        Word(Stance),
    }

    fn to_file(stance: Stance) -> File {
        match stance {
            Stance::Allow => File::Switch(true),
            Stance::Block => File::Switch(false),
            Stance::Avoid => File::Word(Stance::Avoid),
        }
    }

    fn from_file(file: File) -> Stance {
        match file {
            File::Switch(true) => Stance::Allow,
            File::Switch(false) => Stance::Block,
            File::Word(stance) => stance,
        }
    }

    pub fn serialize<S: Serializer>(stance: &Stance, ser: S) -> Result<S::Ok, S::Error> {
        to_file(*stance).serialize(ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Stance, D::Error> {
        File::deserialize(de).map(from_file)
    }

    /// The same for an `Option` field: `None` stays a missing value.
    pub mod option {
        use super::{File, Stance, from_file, to_file};
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        pub fn serialize<S: Serializer>(stance: &Option<Stance>, ser: S) -> Result<S::Ok, S::Error> {
            stance.map(to_file).serialize(ser)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Stance>, D::Error> {
            Ok(Option::<File>::deserialize(de)?.map(from_file))
        }
    }
}

/// The class that the SDE gives to the systems of Pochven.
const POCHVEN_CLASS: u32 = 25;

/// A class of J-space with a stance. The class comes from the `wormholeClassID` of the SDE. A new
/// class of space needs a new variant here, a field in `WormholeSpace` and an entry in `Group::ALL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WormholeClass {
    C1,
    C2,
    C3,
    C4,
    C5,
    C6,
}

impl WormholeClass {
    /// The class for a `wormholeClassID`, or `None` for an ID that is not a class with a stance.
    pub fn from_id(id: u32) -> Option<WormholeClass> {
        match id {
            1 => Some(WormholeClass::C1),
            2 => Some(WormholeClass::C2),
            3 => Some(WormholeClass::C3),
            4 => Some(WormholeClass::C4),
            5 => Some(WormholeClass::C5),
            6 => Some(WormholeClass::C6),
            _ => None,
        }
    }

    /// The number of the class, from 1 to 6.
    pub fn number(self) -> u8 {
        match self {
            WormholeClass::C1 => 1,
            WormholeClass::C2 => 2,
            WormholeClass::C3 => 3,
            WormholeClass::C4 => 4,
            WormholeClass::C5 => 5,
            WormholeClass::C6 => 6,
        }
    }
}

/// A group of systems with one stance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// A class of J-space.
    Class(WormholeClass),
    /// The five Drifter systems, by system ID. The SDE does not mark them, so `DRIFTER_SYSTEMS` lists them.
    Drifter,
    /// The systems of Pochven. The SDE gives them the class 25.
    Pochven,
}

impl Group {
    /// The groups in the order of the settings page.
    pub const ALL: [Group; 8] = [
        Group::Class(WormholeClass::C1),
        Group::Class(WormholeClass::C2),
        Group::Class(WormholeClass::C3),
        Group::Class(WormholeClass::C4),
        Group::Class(WormholeClass::C5),
        Group::Class(WormholeClass::C6),
        Group::Drifter,
        Group::Pochven,
    ];

    /// True if a route reaches the group only through a wormhole. Pochven has the filaments too.
    pub fn is_wormhole(self) -> bool {
        self != Group::Pochven
    }

    pub fn label(self) -> String {
        match self {
            Group::Class(c) => format!("Class {}", c.number()),
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
            Group::Class(WormholeClass::C1) => &self.c1,
            Group::Class(WormholeClass::C2) => &self.c2,
            Group::Class(WormholeClass::C3) => &self.c3,
            Group::Class(WormholeClass::C4) => &self.c4,
            Group::Class(WormholeClass::C5) => &self.c5,
            Group::Class(WormholeClass::C6) => &self.c6,
            Group::Drifter => &self.drifter,
            Group::Pochven => &self.pochven,
        }
    }

    fn slot(&mut self, group: Group) -> &mut Stance {
        match group {
            Group::Class(WormholeClass::C1) => &mut self.c1,
            Group::Class(WormholeClass::C2) => &mut self.c2,
            Group::Class(WormholeClass::C3) => &mut self.c3,
            Group::Class(WormholeClass::C4) => &mut self.c4,
            Group::Class(WormholeClass::C5) => &mut self.c5,
            Group::Class(WormholeClass::C6) => &mut self.c6,
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
            Some(POCHVEN_CLASS) => self.pochven,
            Some(id) => WormholeClass::from_id(id).map_or(Stance::Allow, |c| self.get(Group::Class(c))),
            None => Stance::Allow,
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

    #[test]
    fn only_the_classes_1_to_6_have_a_stance() {
        for id in 1..=6 {
            assert_eq!(WormholeClass::from_id(id).map(|c| u32::from(c.number())), Some(id));
        }
        // 0 is no class, 7 to 18 are the special classes, and 25 is Pochven, which has its own group.
        for id in [0, 7, 12, 18, 25, 99] {
            assert_eq!(WormholeClass::from_id(id), None);
        }
    }
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
        space.set(Group::Class(WormholeClass::C1), Stance::Block);
        space.set(Group::Class(WormholeClass::C2), Stance::Avoid);
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
        space.set(Group::Class(WormholeClass::C1), Stance::Avoid);
        assert_eq!(space.danger(uni)[node.index()], PENALTY);
        assert!(!space.never(uni)[node.index()]);
        space.set(Group::Class(WormholeClass::C1), Stance::Block);
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
        s.space.set(Group::Class(WormholeClass::C1), Stance::Avoid);
        let routes = found(&s).unwrap();
        assert_eq!((routes[0].path.nodes.len(), routes[0].avoided), (2, 1));
        s.space.set(Group::Class(WormholeClass::C1), Stance::Block);
        assert!(found(&s).is_err());
        // Another class stance leaves the hole open.
        s.space.set(Group::Class(WormholeClass::C1), Stance::Allow);
        s.space.set(Group::Class(WormholeClass::C2), Stance::Block);
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
        s.filaments = Stance::Allow;
        let found = |s: &crate::settings::Settings| s.router(uni, FIXTURE_TIME).routes(&[jita, target], 1);
        assert!(found(&s).is_ok());
        s.space.set(Group::Pochven, Stance::Avoid);
        assert!(found(&s).is_ok());
        s.space.set(Group::Pochven, Stance::Block);
        assert!(found(&s).is_err());
        // A blocked class of wormhole space leaves the filament alone.
        s.space.set(Group::Pochven, Stance::Allow);
        s.space.set(Group::Class(WormholeClass::C1), Stance::Block);
        assert!(found(&s).is_ok());
    }

    /// A blocked wormhole group has no effect while wormholes are off. A blocked Pochven does.
    #[test]
    fn a_wormhole_group_blocks_only_while_wormholes_are_on() {
        let mut space = WormholeSpace::default();
        space.set(Group::Class(WormholeClass::C4), Stance::Block);
        space.set(Group::Drifter, Stance::Block);
        assert!(space.blocks(true) && !space.blocks(false));
        space.set(Group::Pochven, Stance::Block);
        assert!(space.blocks(false));
    }

    /// A switch of an old file reads as Allow or Block. A new file keeps `true` and `false`.
    #[test]
    fn a_switch_file_form_reads_old_and_new_text() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Holder {
            #[serde(with = "switch")]
            hub: Stance,
            #[serde(default, with = "switch::option", skip_serializing_if = "Option::is_none")]
            maybe: Option<Stance>,
        }
        let read = |text: &str| serde_json::from_str::<Holder>(text).unwrap();
        assert_eq!(read(r#"{"hub": true}"#), Holder { hub: Stance::Allow, maybe: None });
        assert_eq!(read(r#"{"hub": false, "maybe": true}"#), Holder { hub: Stance::Block, maybe: Some(Stance::Allow) });
        assert_eq!(read(r#"{"hub": "avoid", "maybe": "block"}"#), Holder { hub: Stance::Avoid, maybe: Some(Stance::Block) });
        assert!(serde_json::from_str::<Holder>(r#"{"hub": 3}"#).is_err());
        let text = |hub, maybe| serde_json::to_string(&Holder { hub, maybe }).unwrap();
        assert_eq!(text(Stance::Allow, None), r#"{"hub":true}"#);
        assert_eq!(text(Stance::Block, Some(Stance::Allow)), r#"{"hub":false,"maybe":true}"#);
        assert_eq!(text(Stance::Avoid, Some(Stance::Avoid)), r#"{"hub":"avoid","maybe":"avoid"}"#);
    }

    #[test]
    fn a_click_turns_a_stance_on_or_off() {
        assert_eq!(Stance::ALL.map(Stance::toggled), [Stance::Block, Stance::Block, Stance::Allow]);
        assert_eq!(Stance::ALL.map(Stance::state), ["on", "avoid", "off"]);
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
