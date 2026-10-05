//! The ship table: the Ansiblex cost of each ship, from the SDE dogma data.
//!
//! The SDE update extracts this table from `types.jsonl` (153 MB), `typeDogma.jsonl`
//! (28 MB) and `groups.jsonl`, and keeps only the result in `ships.json`.

use rayon::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub const FILE: &str = "ships.json";

/// The SDE files that the table comes from.
pub const SOURCES: [&str; 3] = ["types.jsonl", "typeDogma.jsonl", "groups.jsonl"];

const SHIP_CATEGORY: u32 = 6;
const ANSIBLEX_TYPE: u32 = 35841;
/// `baseJumpBridgeCapacitorCost`, in GJ. A ship without it cannot use a bridge.
const ATTR_BRIDGE_COST: u32 = 6364;
/// `capacitorCapacity`, in GJ.
const ATTR_CAPACITOR: u32 = 482;
/// `rechargeRate`, in ms.
const ATTR_RECHARGE: u32 = 55;
/// `gateMaxJumpMass`, in kg.
const ATTR_MAX_JUMP_MASS: u32 = 2798;

#[derive(Serialize, Deserialize, Debug)]
pub struct ShipData {
    pub build: Option<u32>,
    pub ansiblex: AnsiblexData,
    pub ships: Vec<ShipRow>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AnsiblexData {
    pub capacitor_gj: f64,
    pub recharge_ms: f64,
    pub max_jump_mass_kg: f64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ShipRow {
    pub type_id: u32,
    pub name: String,
    pub group_id: u32,
    pub group: String,
    pub mass_kg: Option<f64>,
    /// `None`: the ship cannot use a bridge.
    pub bridge_cost_gj: Option<f64>,
}

#[derive(Deserialize)]
struct Names {
    en: Option<String>,
}

#[derive(Deserialize)]
struct SdeType {
    #[serde(rename = "_key")]
    id: u32,
    #[serde(rename = "groupID")]
    group_id: u32,
    name: Option<Names>,
    #[serde(default)]
    published: bool,
    mass: Option<f64>,
}

#[derive(Deserialize)]
struct SdeGroup {
    #[serde(rename = "_key")]
    id: u32,
    #[serde(rename = "categoryID")]
    category_id: u32,
    name: Option<Names>,
}

#[derive(Deserialize)]
struct SdeTypeDogma {
    #[serde(rename = "_key")]
    id: u32,
    #[serde(rename = "dogmaAttributes", default)]
    attributes: Vec<SdeAttribute>,
}

#[derive(Deserialize)]
struct SdeAttribute {
    #[serde(rename = "attributeID")]
    id: u32,
    value: f64,
}

fn parse_lines<T: DeserializeOwned + Send>(name: &str, data: &[u8]) -> Result<Vec<T>, String> {
    let text = std::str::from_utf8(data).map_err(|e| format!("{name}: {e}"))?;
    text.par_lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{name}: {e}")))
        .collect()
}

/// Make the ship table from the 3 SDE source files, in the order of `SOURCES`.
pub fn derive(build: Option<u32>, types: &[u8], dogma: &[u8], groups: &[u8]) -> Result<ShipData, String> {
    let (types, (dogma, groups)) = rayon::join(
        || parse_lines::<SdeType>("types.jsonl", types),
        || rayon::join(|| parse_lines::<SdeTypeDogma>("typeDogma.jsonl", dogma), || parse_lines::<SdeGroup>("groups.jsonl", groups)),
    );
    let (types, dogma, groups) = (types?, dogma?, groups?);

    let ship_groups: HashMap<u32, String> = groups
        .into_iter()
        .filter(|g| g.category_id == SHIP_CATEGORY)
        .map(|g| (g.id, g.name.and_then(|n| n.en).unwrap_or_default()))
        .collect();
    let attrs: HashMap<u32, HashMap<u32, f64>> =
        dogma.into_iter().map(|d| (d.id, d.attributes.into_iter().map(|a| (a.id, a.value)).collect())).collect();
    let attr = |type_id: u32, attr_id: u32| attrs.get(&type_id).and_then(|a| a.get(&attr_id)).copied();

    let need = |attr_id: u32, what: &str| {
        attr(ANSIBLEX_TYPE, attr_id).ok_or(format!("typeDogma.jsonl: the Ansiblex ({ANSIBLEX_TYPE}) has no {what}"))
    };
    let ansiblex = AnsiblexData {
        capacitor_gj: need(ATTR_CAPACITOR, "capacitorCapacity")?,
        recharge_ms: need(ATTR_RECHARGE, "rechargeRate")?,
        max_jump_mass_kg: need(ATTR_MAX_JUMP_MASS, "gateMaxJumpMass")?,
    };

    let mut ships: Vec<ShipRow> = types
        .into_iter()
        .filter(|t| t.published)
        .filter_map(|t| {
            let group = ship_groups.get(&t.group_id)?.clone();
            Some(ShipRow {
                type_id: t.id,
                name: t.name.and_then(|n| n.en)?,
                group_id: t.group_id,
                group,
                mass_kg: t.mass,
                bridge_cost_gj: attr(t.id, ATTR_BRIDGE_COST),
            })
        })
        .collect();
    if ships.is_empty() {
        return Err("types.jsonl: no published ships".into());
    }
    ships.sort_by(|a, b| a.group.cmp(&b.group).then(a.name.cmp(&b.name)));
    Ok(ShipData { build, ansiblex, ships })
}

pub fn save(dir: &Path, data: &ShipData) -> Result<(), String> {
    let path = dir.join(FILE);
    let tmp = path.with_extension("json.part");
    let text = serde_json::to_string(data).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(dir: &Path) -> Result<ShipData, String> {
    let path = dir.join(FILE);
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e}. Unset {} to download it.",
            path.display(),
            crate::sde_update::SKIP_ENV
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}
