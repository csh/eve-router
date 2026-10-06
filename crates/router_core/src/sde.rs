//! Parse the SDE JSONL files that the router needs.

use rayon::prelude::*;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;

#[derive(Deserialize)]
pub struct Names {
    pub en: String,
}

#[derive(Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Deserialize)]
pub struct SdeSystem {
    #[serde(rename = "_key")]
    pub id: u32,
    pub name: Names,
    #[serde(rename = "regionID")]
    pub region_id: u32,
    #[serde(rename = "securityStatus")]
    pub security: f64,
    pub position: Position,
}

#[derive(Deserialize)]
pub struct Destination {
    #[serde(rename = "solarSystemID")]
    pub system_id: u32,
}

#[derive(Deserialize)]
pub struct SdeStargate {
    #[serde(rename = "solarSystemID")]
    pub system_id: u32,
    pub destination: Destination,
}

#[derive(Deserialize)]
pub struct SdeRegion {
    #[serde(rename = "_key")]
    pub id: u32,
    pub name: Names,
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
    pub build: Option<u32>,
}

/// Read a JSONL file and parse its lines in parallel.
pub fn read_jsonl<T: DeserializeOwned + Send>(path: &Path) -> Result<Vec<T>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.par_lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|e| format!("{}: {e}", path.display())))
        .collect()
}

/// Load the map files from an SDE directory. The three files load at the same time.
pub fn load(dir: &Path) -> Result<SdeData, String> {
    let (systems, (stargates, regions)) = rayon::join(
        || read_jsonl::<SdeSystem>(&dir.join("mapSolarSystems.jsonl")),
        || {
            rayon::join(
                || read_jsonl::<SdeStargate>(&dir.join("mapStargates.jsonl")),
                || read_jsonl::<SdeRegion>(&dir.join("mapRegions.jsonl")),
            )
        },
    );
    Ok(SdeData { systems: systems?, stargates: stargates?, regions: regions?, build: build_number(dir) })
}

/// The build number from `_sde.jsonl`. A missing file gives `None`.
pub fn build_number(dir: &Path) -> Option<u32> {
    read_jsonl::<SdeInfo>(&dir.join("_sde.jsonl")).ok().and_then(|v| v.first().map(|i| i.build_number))
}
