//! Parse the SDE JSONL files that the router needs.

use crate::error::FileError;
use rayon::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Deserialize, Serialize)]
pub struct Names {
    pub en: String,
}

#[derive(Deserialize, Serialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Deserialize, Serialize)]
pub struct SdeSystem {
    #[serde(rename = "_key")]
    pub id: u32,
    pub name: Names,
    #[serde(rename = "regionID")]
    pub region_id: u32,
    #[serde(rename = "constellationID")]
    pub constellation_id: u32,
    #[serde(rename = "securityStatus")]
    pub security: f64,
    pub position: Position,
    /// Set on a few systems that an NPC faction holds. See `SdeRegion::faction_id`.
    #[serde(rename = "factionID", default, skip_serializing_if = "Option::is_none")]
    pub faction_id: Option<u32>,
}

#[derive(Deserialize, Serialize)]
pub struct Destination {
    #[serde(rename = "solarSystemID")]
    pub system_id: u32,
}

#[derive(Deserialize, Serialize)]
pub struct SdeStargate {
    #[serde(rename = "solarSystemID")]
    pub system_id: u32,
    pub destination: Destination,
}

#[derive(Deserialize, Serialize)]
pub struct SdeRegion {
    #[serde(rename = "_key")]
    pub id: u32,
    pub name: Names,
    /// Set for a region that an NPC faction holds. The SDE has no flag for NPC nullsec. A
    /// region, a constellation or a system with a faction is NPC space. The ESI sovereignty map
    /// agreed with this rule for every nullsec system (3538 on 2026-10-20).
    #[serde(rename = "factionID", default, skip_serializing_if = "Option::is_none")]
    pub faction_id: Option<u32>,
}

/// A constellation. A sovereignty region can hold an NPC constellation (Delve, Fountain).
#[derive(Deserialize, Serialize)]
pub struct SdeConstellation {
    #[serde(rename = "_key")]
    pub id: u32,
    /// See `SdeRegion::faction_id`.
    #[serde(rename = "factionID", default, skip_serializing_if = "Option::is_none")]
    pub faction_id: Option<u32>,
}

#[derive(Deserialize)]
struct SdeInfo {
    #[serde(rename = "buildNumber")]
    build_number: u32,
}

pub struct SdeData {
    pub systems: Vec<SdeSystem>,
    pub stargates: Vec<SdeStargate>,
    pub regions: Vec<SdeRegion>,
    pub constellations: Vec<SdeConstellation>,
    pub build: Option<u32>,
}

/// Read a JSONL file and parse its lines in parallel.
pub fn read_jsonl<T: DeserializeOwned + Send>(path: &Path) -> Result<Vec<T>, FileError> {
    let text = fs::read_to_string(path).map_err(FileError::io(path))?;
    text.par_lines().filter(|line| !line.trim().is_empty()).map(|line| serde_json::from_str(line).map_err(FileError::json(path))).collect()
}

/// Keep only the fields of `T` in each line of a JSONL file, and write the lines again.
/// The router reads only these fields, so the rest is dead weight on the disk.
pub fn slim<T: DeserializeOwned + Serialize + Send>(data: &[u8]) -> Result<Vec<u8>, serde_json::Error> {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<String> = text
        .par_lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<T>(line).and_then(|v| serde_json::to_string(&v)))
        .collect::<Result<_, _>>()?;
    Ok(lines
        .iter()
        .flat_map(|l| {
            [
                l.as_str(),
                "
",
            ]
        })
        .collect::<String>()
        .into_bytes())
}

/// Load the map files from an SDE directory. The four files load at the same time.
pub fn load(dir: &Path) -> Result<SdeData, FileError> {
    let ((systems, stargates), (regions, constellations)) = rayon::join(
        || {
            rayon::join(
                || read_jsonl::<SdeSystem>(&dir.join("mapSolarSystems.jsonl")),
                || read_jsonl::<SdeStargate>(&dir.join("mapStargates.jsonl")),
            )
        },
        || {
            rayon::join(
                || read_jsonl::<SdeRegion>(&dir.join("mapRegions.jsonl")),
                || read_jsonl::<SdeConstellation>(&dir.join("mapConstellations.jsonl")),
            )
        },
    );
    Ok(SdeData { systems: systems?, stargates: stargates?, regions: regions?, constellations: constellations?, build: build_number(dir) })
}

/// The build number from `_sde.jsonl`. A missing file gives `None`.
pub fn build_number(dir: &Path) -> Option<u32> {
    read_jsonl::<SdeInfo>(&dir.join("_sde.jsonl")).ok().and_then(|v| v.first().map(|i| i.build_number))
}
