//! The wormhole type table: the per-jump mass and the maximum life of each wormhole type.
//!
//! The SDE update makes it from `types.jsonl` and `typeDogma.jsonl`, and keeps it in
//! `wormholes.json`. A type is a name `Wormhole X000` with the attributes 1382 and 1385.

use crate::ships::Sources;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub const FILE: &str = "wormholes.json";

/// The exit side of a wormhole. Its type gives no limits, and the SDE has no dogma for it.
pub const K162: &str = "K162";
/// The longest life of any wormhole. Nexum uses it for a K162, so the router does the same.
const K162_LIFE_HOURS: f64 = 48.0;

/// `wormholeTargetSystemClass`.
const ATTR_TARGET_CLASS: u32 = 1381;
/// `wormholeMaxStableTime`, in minutes.
const ATTR_MAX_LIFE_MIN: u32 = 1382;
/// `wormholeMaxStableMass`, in kg.
const ATTR_TOTAL_MASS: u32 = 1383;
/// `wormholeMaxJumpMass`, in kg.
const ATTR_MAX_JUMP_MASS: u32 = 1385;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WormholeType {
    /// For example "B274".
    pub code: String,
    pub max_jump_kg: f64,
    pub max_life_h: f64,
    pub total_mass_kg: Option<f64>,
    pub target_class: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct WormholeTypes {
    pub build: Option<u32>,
    /// Sorted by code.
    pub types: Vec<WormholeType>,
}

impl WormholeTypes {
    /// The type for a code, case-insensitive. K162 and an unknown code give `None`.
    pub fn get(&self, code: &str) -> Option<&WormholeType> {
        let code = code.trim();
        self.types.iter().find(|t| t.code.eq_ignore_ascii_case(code))
    }

    /// The maximum life in hours. K162 gives 48 h. An unknown code gives `None`.
    pub fn life_hours(&self, code: &str) -> Option<f64> {
        if code.trim().eq_ignore_ascii_case(K162) {
            return Some(K162_LIFE_HOURS);
        }
        self.get(code).map(|t| t.max_life_h)
    }
}

/// The code in a name such as "Wormhole B274": one capital letter and three digits.
fn wormhole_code(name: &str) -> Option<&str> {
    let code = name.strip_prefix("Wormhole ")?;
    let b = code.as_bytes();
    let valid = b.len() == 4 && b[0].is_ascii_uppercase() && b[1..].iter().all(u8::is_ascii_digit);
    valid.then_some(code)
}

/// Make the table from the parsed SDE files.
pub fn derive(build: Option<u32>, src: &Sources) -> WormholeTypes {
    let mut types: Vec<WormholeType> = src
        .names()
        .filter_map(|(id, name)| {
            let code = wormhole_code(name)?;
            Some(WormholeType {
                code: code.to_string(),
                max_jump_kg: src.attr(id, ATTR_MAX_JUMP_MASS)?,
                max_life_h: src.attr(id, ATTR_MAX_LIFE_MIN)? / 60.0,
                total_mass_kg: src.attr(id, ATTR_TOTAL_MASS),
                target_class: src.attr(id, ATTR_TARGET_CLASS).map(|v| v as u32),
            })
        })
        .collect();
    types.sort_by(|a, b| a.code.cmp(&b.code));
    // The SDE has several type IDs with the same code and the same values, for example C729.
    types.dedup();
    WormholeTypes { build, types }
}

pub fn save(dir: &Path, data: &WormholeTypes) -> Result<(), String> {
    let path = dir.join(FILE);
    let tmp = path.with_extension("json.part");
    let text = serde_json::to_string(data).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(dir: &Path) -> Result<WormholeTypes, String> {
    let path = dir.join(FILE);
    let text =
        fs::read_to_string(&path).map_err(|e| format!("{}: {e}. Unset {} to download it.", path.display(), crate::sde_update::SKIP_ENV))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPES: &str = concat!(
        r#"{"_key": 30677, "groupID": 988, "name": {"en": "Wormhole B274"}, "published": false}"#,
        "\n",
        r#"{"_key": 30831, "groupID": 988, "name": {"en": "Wormhole K162"}, "published": false}"#,
        "\n",
        r#"{"_key": 587, "groupID": 25, "name": {"en": "Rifter"}, "published": true, "mass": 1067000.0}"#,
        "\n",
        r#"{"_key": 99999, "groupID": 988, "name": {"en": "Wormhole Beacon"}, "published": false}"#,
        "\n",
    );
    const DOGMA: &str = concat!(
        r#"{"_key": 30677, "dogmaAttributes": [{"attributeID": 1381, "value": 7.0}, {"attributeID": 1382, "value": 1440.0}, {"attributeID": 1383, "value": 2000000000.0}, {"attributeID": 1384, "value": 0.0}, {"attributeID": 1385, "value": 375000000.0}]}"#,
        "\n",
        r#"{"_key": 99999, "dogmaAttributes": [{"attributeID": 1385, "value": 5.0}]}"#,
        "\n",
    );

    #[test]
    fn derive_from_sde_lines() {
        let src = crate::ships::parse(TYPES.as_bytes(), DOGMA.as_bytes(), b"").unwrap();
        let table = derive(Some(1), &src);
        let b274 = WormholeType {
            code: "B274".into(),
            max_jump_kg: 375_000_000.0,
            max_life_h: 24.0,
            total_mass_kg: Some(2_000_000_000.0),
            target_class: Some(7),
        };
        // K162 has no dogma. "Wormhole Beacon" is not a wormhole code.
        assert_eq!(table.types, vec![b274]);
        assert_eq!(table.build, Some(1));
        assert_eq!(table.get("b274").unwrap().max_jump_kg, 375_000_000.0);
        assert_eq!(table.life_hours("B274"), Some(24.0));
        assert_eq!(table.life_hours("k162"), Some(48.0));
        assert_eq!(table.life_hours("Z999"), None);
        assert!(table.get(K162).is_none());
    }

    #[test]
    fn repo_table_matches_sde() {
        let table = load(&crate::test_support::sde_dir()).unwrap();
        assert_eq!(table.types.len(), 99);
        assert_eq!(table.get("B274").unwrap().max_jump_kg, 375_000_000.0);
        // The SDE value, not the Nexum chart value (24 h).
        assert_eq!(table.life_hours("C248"), Some(16.0));
    }
}
