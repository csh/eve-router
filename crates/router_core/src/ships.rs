//! The ship table: the Ansiblex cost of each ship, from the SDE dogma data.
//!
//! The SDE update parses `types.jsonl` (153 MB), `typeDogma.jsonl` (28 MB), `groups.jsonl` and
//! `typeLists.jsonl` one time. It keeps only the results: this table in `ships.json`, and
//! the wormhole type table in `wormholes.json` (see `wormhole_types`).

use crate::error::{FileError, SdeError, write_atomic};
use rayon::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub const FILE: &str = "ships.json";

/// The shape of `ships.json`. Bump this when a field's meaning changes, so `sde_update` makes
/// the file again even when the SDE build number does not change. A ship with no bridge cost
/// or no mass still deserializes, so only a true shape change needs a bump.
pub const SCHEMA: u32 = 1;

/// The SDE files that the table comes from.
pub const SOURCES: [&str; 4] = ["types.jsonl", "typeDogma.jsonl", "groups.jsonl", "typeLists.jsonl"];

const SHIP_CATEGORY: u32 = 6;
pub(crate) const ANSIBLEX_TYPE: u32 = 35841;
/// `baseJumpBridgeCapacitorCost`, in GJ. A ship without it cannot use a bridge.
const ATTR_BRIDGE_COST: u32 = 6364;
/// `capacitorCapacity`, in GJ.
const ATTR_CAPACITOR: u32 = 482;
/// `rechargeRate`, in ms.
const ATTR_RECHARGE: u32 = 55;
/// `gateMaxJumpMass`, in kg.
const ATTR_MAX_JUMP_MASS: u32 = 2798;
/// The `typeLists.jsonl` entry of the ship groups that cannot activate a Triglavian filament.
const FILAMENT_BAN_LIST: &str = "InvalidShipGroupsForRandomJump";

#[derive(Serialize, Deserialize, Debug)]
pub struct ShipData {
    /// The schema of this file. `0` for a file from before this field: it has no `filament_ok`.
    #[serde(default)]
    pub schema: u32,
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
    /// False: the SDE lists the ship's group as unable to activate a Triglavian filament.
    /// A `ships.json` from before this field defaults to false: safe until the next SDE update.
    #[serde(default)]
    pub filament_ok: bool,
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

#[derive(Deserialize)]
struct SdeTypeList {
    #[serde(default)]
    name: String,
    #[serde(rename = "includedGroupIDs", default)]
    included_group_ids: Vec<u32>,
}

fn parse_lines<T: DeserializeOwned + Send>(name: &str, data: &[u8]) -> Result<Vec<T>, SdeError> {
    let text = std::str::from_utf8(data).map_err(|source| SdeError::Utf8 { name: name.into(), source })?;
    text.par_lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|source| SdeError::Line { name: name.into(), source }))
        .collect()
}

/// The 4 SDE source files, parsed. The ship table and the wormhole table both read it.
pub struct Sources {
    types: Vec<SdeType>,
    attrs: HashMap<u32, HashMap<u32, f64>>,
    groups: Vec<SdeGroup>,
    type_lists: Vec<SdeTypeList>,
}

impl Sources {
    /// A dogma attribute value of a type.
    pub fn attr(&self, type_id: u32, attr_id: u32) -> Option<f64> {
        self.attrs.get(&type_id).and_then(|a| a.get(&attr_id)).copied()
    }

    /// The type ID and the English name of each type that has a name.
    pub fn names(&self) -> impl Iterator<Item = (u32, &str)> {
        self.types.iter().filter_map(|t| Some((t.id, t.name.as_ref()?.en.as_deref()?)))
    }

    /// The group IDs of the SDE type list named `name`. `None` when no list has that name.
    fn type_list(&self, name: &str) -> Option<&[u32]> {
        self.type_lists.iter().find(|l| l.name == name).map(|l| l.included_group_ids.as_slice())
    }
}

/// Parse the 4 SDE source files, in the order of `SOURCES`.
pub fn parse(types: &[u8], dogma: &[u8], groups: &[u8], type_lists: &[u8]) -> Result<Sources, SdeError> {
    let ((types, dogma), (groups, type_lists)) = rayon::join(
        || rayon::join(|| parse_lines::<SdeType>("types.jsonl", types), || parse_lines::<SdeTypeDogma>("typeDogma.jsonl", dogma)),
        || rayon::join(|| parse_lines::<SdeGroup>("groups.jsonl", groups), || parse_lines::<SdeTypeList>("typeLists.jsonl", type_lists)),
    );
    let (types, dogma, groups, type_lists) = (types?, dogma?, groups?, type_lists?);
    let attrs = dogma.into_iter().map(|d| (d.id, d.attributes.into_iter().map(|a| (a.id, a.value)).collect())).collect();
    Ok(Sources { types, attrs, groups, type_lists })
}

/// Make the ship table from the parsed SDE files.
pub fn derive(build: Option<u32>, src: &Sources) -> Result<ShipData, SdeError> {
    let ship_groups: HashMap<u32, &str> = src
        .groups
        .iter()
        .filter(|g| g.category_id == SHIP_CATEGORY)
        .map(|g| (g.id, g.name.as_ref().and_then(|n| n.en.as_deref()).unwrap_or_default()))
        .collect();

    let need = |attr_id: u32, what: &'static str| src.attr(ANSIBLEX_TYPE, attr_id).ok_or(SdeError::NoAnsiblexAttribute(what));
    let ansiblex = AnsiblexData {
        capacitor_gj: need(ATTR_CAPACITOR, "capacitorCapacity")?,
        recharge_ms: need(ATTR_RECHARGE, "rechargeRate")?,
        max_jump_mass_kg: need(ATTR_MAX_JUMP_MASS, "gateMaxJumpMass")?,
    };
    let filament_banned_groups: HashSet<u32> =
        src.type_list(FILAMENT_BAN_LIST).ok_or(SdeError::NoTypeList(FILAMENT_BAN_LIST))?.iter().copied().collect();

    let mut ships: Vec<ShipRow> = src
        .types
        .iter()
        .filter(|t| t.published)
        .filter_map(|t| {
            let group = ship_groups.get(&t.group_id)?.to_string();
            Some(ShipRow {
                type_id: t.id,
                name: t.name.as_ref()?.en.clone()?,
                group_id: t.group_id,
                group,
                mass_kg: t.mass,
                bridge_cost_gj: src.attr(t.id, ATTR_BRIDGE_COST),
                filament_ok: !filament_banned_groups.contains(&t.group_id),
            })
        })
        .collect();
    if ships.is_empty() {
        return Err(SdeError::NoShips);
    }
    ships.sort_by(|a, b| a.group.cmp(&b.group).then(a.name.cmp(&b.name)));
    Ok(ShipData { schema: SCHEMA, build, ansiblex, ships })
}

pub fn save(dir: &Path, data: &ShipData) -> Result<(), FileError> {
    let path = dir.join(FILE);
    let text = serde_json::to_string(data).map_err(FileError::json(&path))?;
    write_atomic(&path, &path.with_extension("json.part"), &text)
}

pub fn load(dir: &Path) -> Result<ShipData, SdeError> {
    let path = dir.join(FILE);
    let text = fs::read_to_string(&path).map_err(|source| SdeError::Missing { path: path.clone(), source })?;
    Ok(serde_json::from_str(&text).map_err(FileError::json(&path))?)
}
