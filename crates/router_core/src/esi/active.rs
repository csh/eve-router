//! The active route: the route that the router sent to the game, and the progress of the pilot.
//!
//! The route is a frozen copy. A change of the settings does not change it, so the in-game
//! waypoints and the app stay the same. The copy holds system IDs, not graph indices, because
//! the graph can change between two runs (the wormholes load at each start).
//!
//! The in-game autopilot follows gates, and it takes an Ansiblex when the pilot is on the access
//! list of the gate. It cannot take a wormhole. Thus the route goes to the game in segments. A
//! segment ends before each wormhole, the manual hop. When the pilot makes that hop, `observe`
//! gives the waypoints of the next segment.

use super::client::Location;
use crate::ansiblex::BridgeRules;
use crate::labels::link_label;
use crate::route::{Route, Stop};
use crate::universe::{Link, Universe};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The most waypoints that one segment sends. Not measured in game yet: phase 6 of the plan
/// measures the in-game cap. Above it, a segment sends its stops and its last system only.
pub const WAYPOINT_CAP: usize = 100;
/// The off-route state needs this many polls in a row off the path. One odd poll (a pod kill,
/// a cyno, a wormhole exit) does not give an alarm.
pub const OFF_ROUTE_POLLS: u8 = 2;
const FILE_NAME: &str = "active-route.json";

/// The active route file, next to the config file.
pub fn active_path(cfg_path: &Path) -> PathBuf {
    cfg_path.with_file_name(FILE_NAME)
}

/// How the pilot gets to a step.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hop {
    /// The first step.
    Start,
    Gate,
    /// A manual hop. The autopilot stops before it: the pilot warps to the signature and jumps.
    Wormhole,
    /// An Ansiblex. The autopilot takes it when the pilot is on the access list of the gate.
    Bridge,
}

impl Hop {
    /// True for a hop that the autopilot cannot make. Only a wormhole is manual.
    pub fn is_manual(self) -> bool {
        self == Hop::Wormhole
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Step {
    pub system: u32,
    pub hop: Hop,
    /// The link label at the start of the route, for example "Ansiblex · Zone 1 → 2 · 36 TJ".
    pub via: String,
}

/// What a location means for the route.
#[derive(Debug, PartialEq)]
pub enum Observation {
    /// No change of the progress.
    Same,
    /// The pilot is at this step, or passed it.
    Progress(usize),
    /// The pilot made a manual hop. Send these waypoints, with `clear_other_waypoints` on the first.
    NextSegment { step: usize, waypoints: Vec<u32> },
    /// The pilot is at the destination.
    Arrived,
    /// The pilot is off the path for `OFF_ROUTE_POLLS` polls in a row.
    OffRoute { system: u32 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActiveRoute {
    /// The route number in the list, for "Route #1".
    pub number: usize,
    pub character: u64,
    pub character_name: String,
    pub steps: Vec<Step>,
    /// The step index of the start, of each midpoint and of the destination.
    pub stops: Vec<usize>,
    /// The highest step that the pilot reached. It never goes down.
    pub progress: usize,
    /// The segment whose waypoints are in the game.
    pub sent_segment: usize,
    /// The polls in a row with the pilot off the path.
    #[serde(skip)]
    off_polls: u8,
}

impl ActiveRoute {
    /// Freeze a route. `now` (Unix seconds) sets the wormhole labels.
    pub fn new(
        uni: &Universe,
        rules: &BridgeRules,
        route: &Route,
        number: usize,
        character: u64,
        character_name: &str,
        now: u64,
    ) -> ActiveRoute {
        let steps = route
            .path
            .nodes
            .iter()
            .enumerate()
            .map(|(i, &node)| {
                let system = uni.system(node).id;
                let Some(&edge) = i.checked_sub(1).and_then(|e| route.path.edges.get(e)) else {
                    return Step { system, hop: Hop::Start, via: String::new() };
                };
                let hop = match &uni.graph[edge] {
                    Link::Stargate => Hop::Gate,
                    Link::Wormhole(_) => Hop::Wormhole,
                    Link::JumpBridge => Hop::Bridge,
                };
                Step { system, hop, via: link_label(uni, rules, edge, now) }
            })
            .collect();
        Self::from_steps(steps, route.stops.clone(), number, character, character_name)
    }

    pub fn from_steps(steps: Vec<Step>, stops: Vec<usize>, number: usize, character: u64, character_name: &str) -> ActiveRoute {
        ActiveRoute {
            number,
            character,
            character_name: character_name.to_owned(),
            steps,
            stops,
            progress: 0,
            sent_segment: 0,
            off_polls: 0,
        }
    }

    pub fn jumps(&self) -> usize {
        self.steps.len().saturating_sub(1)
    }

    pub fn arrived(&self) -> bool {
        self.progress + 1 >= self.steps.len()
    }

    /// The count of manual hops.
    pub fn manual_hops(&self) -> usize {
        self.steps.iter().filter(|s| s.hop.is_manual()).count()
    }

    /// True after `OFF_ROUTE_POLLS` polls off the path, until the pilot is on the path again.
    pub fn is_off_route(&self) -> bool {
        self.off_polls >= OFF_ROUTE_POLLS
    }

    /// The part that a step plays in the route.
    pub fn stop_at(&self, step: usize) -> Option<Stop> {
        let k = self.stops.iter().position(|&s| s == step)?;
        Some(match k {
            0 => Stop::Start,
            k if k + 1 == self.stops.len() => Stop::Destination,
            k => Stop::Midpoint(k),
        })
    }

    /// The first step of each segment: step 0, and each step after a manual hop.
    fn segment_starts(&self) -> Vec<usize> {
        std::iter::once(0).chain((1..self.steps.len()).filter(|&i| self.steps[i].hop.is_manual())).collect()
    }

    fn segment_of(&self, step: usize) -> usize {
        self.segment_starts().iter().rposition(|&s| s <= step).unwrap_or(0)
    }

    /// The waypoints of a segment: each system after its first step, up to the system before
    /// the next manual hop. A segment over `WAYPOINT_CAP` gives its stops and its last system.
    pub fn segment_waypoints(&self, segment: usize) -> Vec<u32> {
        let starts = self.segment_starts();
        let Some(&first) = starts.get(segment) else { return Vec::new() };
        let end = starts.get(segment + 1).copied().unwrap_or(self.steps.len());
        let range = first + 1..end;
        if range.len() <= WAYPOINT_CAP {
            return range.map(|i| self.steps[i].system).collect();
        }
        let last = end - 1;
        range.filter(|&i| i == last || self.stops.contains(&i)).map(|i| self.steps[i].system).collect()
    }

    /// The waypoints to send at the start.
    pub fn first_waypoints(&self) -> Vec<u32> {
        self.segment_waypoints(0)
    }

    /// The count of waypoints in all segments, for the confirm text.
    pub fn waypoint_count(&self) -> usize {
        (0..self.segment_starts().len()).map(|k| self.segment_waypoints(k).len()).sum()
    }

    /// Update the progress with a new location of the pilot.
    pub fn observe(&mut self, location: &Location) -> Observation {
        let system = location.solar_system_id;
        // The first step at or after the progress. A step before it does not move the progress
        // back, and is not off the path.
        let ahead = (self.progress..self.steps.len()).find(|&i| self.steps[i].system == system);
        let behind = self.steps[..self.progress].iter().any(|s| s.system == system);
        let Some(step) = ahead else {
            if behind {
                self.off_polls = 0;
                return Observation::Same;
            }
            self.off_polls = self.off_polls.saturating_add(1);
            return if self.off_polls == OFF_ROUTE_POLLS { Observation::OffRoute { system } } else { Observation::Same };
        };
        self.off_polls = 0;
        if step == self.progress {
            return Observation::Same;
        }
        self.progress = step;
        if self.arrived() {
            return Observation::Arrived;
        }
        let segment = self.segment_of(step);
        if segment > self.sent_segment {
            self.sent_segment = segment;
            let waypoints = self.segment_waypoints(segment);
            if !waypoints.is_empty() {
                return Observation::NextSegment { step, waypoints };
            }
        }
        Observation::Progress(step)
    }

    /// Read the file. A missing or bad file gives `None`: the route is gone, and the planner shows.
    pub fn load(path: &Path) -> Option<ActiveRoute> {
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
    }

    /// Write a temporary file, then rename it, so a reader never sees half a file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.part");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Delete the file after a stop or an arrival. A missing file is not an error.
    pub fn clear(path: &Path) -> Result<(), String> {
        match fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FIXTURE_TIME, overlay_universe, settings};

    /// A route of gates, with a manual hop of `hop` into each system in `manual`.
    fn route(systems: &[u32], manual: &[(u32, Hop)]) -> ActiveRoute {
        let steps = systems
            .iter()
            .enumerate()
            .map(|(i, &system)| {
                let hop = match manual.iter().find(|(s, _)| *s == system) {
                    _ if i == 0 => Hop::Start,
                    Some((_, hop)) => *hop,
                    None => Hop::Gate,
                };
                Step { system, hop, via: String::new() }
            })
            .collect();
        ActiveRoute::from_steps(steps, vec![0, systems.len() - 1], 1, 7, "Alice")
    }

    fn at(system: u32) -> Location {
        Location { solar_system_id: system, station_id: None, structure_id: None }
    }

    #[test]
    fn gate_route_is_one_segment() {
        let r = route(&[1, 2, 3, 4], &[]);
        assert_eq!(r.first_waypoints(), [2, 3, 4]);
        assert_eq!(r.waypoint_count(), 3);
        assert_eq!(r.manual_hops(), 0);
    }

    #[test]
    fn segments_split_at_wormholes_only() {
        // 1 gate 2 bridge 3 gate 4 wormhole 5 gate 6. The autopilot takes a bridge when the
        // pilot is on its access list, so a bridge does not end a segment.
        let r = route(&[1, 2, 3, 4, 5, 6], &[(3, Hop::Bridge), (5, Hop::Wormhole)]);
        assert_eq!(r.segment_waypoints(0), [2, 3, 4]);
        assert_eq!(r.segment_waypoints(1), [6]);
        assert_eq!(r.waypoint_count(), 4);
        assert_eq!(r.manual_hops(), 1);
    }

    #[test]
    fn progress_goes_up_and_skips_rows() {
        let mut r = route(&[1, 2, 3, 4, 5], &[]);
        assert_eq!(r.observe(&at(1)), Observation::Same);
        // Two jumps between two polls.
        assert_eq!(r.observe(&at(3)), Observation::Progress(2));
        // Back one jump: the progress stays, and the pilot is not off the path.
        assert_eq!(r.observe(&at(2)), Observation::Same);
        assert_eq!(r.progress, 2);
        assert_eq!(r.observe(&at(5)), Observation::Arrived);
        assert!(r.arrived());
    }

    #[test]
    fn manual_hop_gives_the_next_segment() {
        let mut r = route(&[1, 2, 3, 4, 5], &[(3, Hop::Wormhole)]);
        assert_eq!(r.observe(&at(2)), Observation::Progress(1));
        assert_eq!(r.observe(&at(3)), Observation::NextSegment { step: 2, waypoints: vec![4, 5] });
        assert_eq!(r.sent_segment, 1);
        // The segment goes out one time only.
        assert_eq!(r.observe(&at(4)), Observation::Progress(3));
    }

    #[test]
    fn off_route_needs_two_polls() {
        let mut r = route(&[1, 2, 3], &[]);
        assert_eq!(r.observe(&at(99)), Observation::Same);
        assert!(!r.is_off_route());
        assert_eq!(r.observe(&at(99)), Observation::OffRoute { system: 99 });
        assert!(r.is_off_route());
        // The alarm goes out one time.
        assert_eq!(r.observe(&at(99)), Observation::Same);
        // Back on the path.
        assert_eq!(r.observe(&at(2)), Observation::Progress(1));
        assert!(!r.is_off_route());
        // One odd poll between two good polls gives no alarm.
        assert_eq!(r.observe(&at(98)), Observation::Same);
        assert_eq!(r.observe(&at(2)), Observation::Same);
        assert_eq!(r.observe(&at(98)), Observation::Same);
        assert!(!r.is_off_route());
    }

    #[test]
    fn long_segment_sends_stops_only() {
        let systems: Vec<u32> = (1..=WAYPOINT_CAP as u32 + 10).collect();
        let mut r = route(&systems, &[]);
        r.stops = vec![0, 50, systems.len() - 1];
        assert_eq!(r.first_waypoints(), [51, systems.len() as u32]);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join("eve-router-test-active");
        let _ = fs::remove_dir_all(&dir);
        let path = active_path(&dir.join("eve-router.json"));
        let mut r = route(&[1, 2, 3], &[(3, Hop::Wormhole)]);
        r.observe(&at(2));
        r.save(&path).unwrap();
        assert_eq!(ActiveRoute::load(&path), Some(r));
        ActiveRoute::clear(&path).unwrap();
        ActiveRoute::clear(&path).unwrap();
        assert_eq!(ActiveRoute::load(&path), None);
    }

    #[test]
    fn real_route_with_jump_bridges() {
        let uni = overlay_universe();
        let s = settings(&uni, Some("Sin"));
        let nodes = [uni.exact("Jita").unwrap(), uni.exact("UALX-3").unwrap()];
        let routes = s.router(&uni, FIXTURE_TIME).routes(&nodes, 1).unwrap();
        let route = &routes[0];
        let r = ActiveRoute::new(&uni, &s.rules, route, 1, 7, "Alice", FIXTURE_TIME);
        assert_eq!(r.jumps(), route.jumps);
        assert_eq!(r.manual_hops(), route.wormholes);
        assert_eq!(r.steps[0].hop, Hop::Start);
        assert_eq!(r.steps[0].system, uni.system(nodes[0]).id);
        assert_eq!(r.stop_at(r.jumps()), Some(Stop::Destination));
        for step in r.steps.iter().filter(|s| s.hop == Hop::Bridge) {
            assert!(step.via.starts_with("Ansiblex"), "{}", step.via);
        }
        // Each system is in exactly one segment, but the first step and each manual hop exit.
        assert_eq!(r.waypoint_count(), r.steps.len() - 1 - r.manual_hops());
    }
}
