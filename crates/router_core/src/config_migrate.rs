//! Read the config file of an older version.
//!
//! A new version can change the shape of `eve-router.json`. A file that the new version cannot
//! read would reset the settings, and the Nexum API key with them. The user sees that key one time
//! only. So each shape that a release wrote stays here, and `magic_migrate` moves its data to the
//! next shape:
//!
//! `ConfigV1` (0.4.0) -> `ConfigV2` (0.4.1) -> `Config` (0.4.2 and later)
//!
//! `parse` is the only entry. When a release changes the file shape:
//! 1. Copy the current `Config` here as a new `ConfigVn`. Make it strict, like the others.
//! 2. Make it the head of the chain: `parse` starts there.
//! 3. Add `From<ConfigVn> for Config`.
//! 4. Add the file of that release to `tests/fixtures` and a test.
//!
//! The old shapes are strict (`deny_unknown_fields`). A file with a field that the shape does not
//! know belongs to a newer shape, and the chain does not claim it. `Config` reads that file. The
//! types that did not change between the releases (`Mode`, `AvoidName` and others) are shared.

use crate::config::{AvoidName, Config, EveMetroConfig, FactionTheme, NexumConfig, RoutingOverlayConfig};
use crate::route::Mode;
use crate::wormhole::Hubs;
use crate::wormhole_space::{Stance, WormholeSpace};
use magic_migrate::Migrate;
use serde::Deserialize;

/// Read the text of a config file. A file of 0.4.0 or 0.4.1 moves to the current shape. Any other
/// file goes to `Config` as it is. The error is the one of `Config`, so a damaged file still gives
/// a JSON error.
pub fn parse(text: &str) -> Result<Config, serde_json::Error> {
    match ConfigV2::from_str_migrations(text) {
        Some(old) => Ok(old.into()),
        None => serde_json::from_str(text),
    }
}

/// `magic_migrate` gives each step the text. A text that is not JSON becomes `null`, which no
/// struct accepts, so the chain ends with `None` and `parse` reports the real error.
fn deserializer<'de>(input: &'de str) -> impl serde::de::Deserializer<'de> {
    serde_json::from_str::<serde_json::Value>(input).unwrap_or(serde_json::Value::Null)
}

/// The hubs of 0.4.0 and 0.4.1: two switches.
#[derive(Deserialize, Debug)]
#[serde(default)]
struct HubsV1 {
    thera: bool,
    turnur: bool,
}

impl Default for HubsV1 {
    fn default() -> Self {
        HubsV1 { thera: true, turnur: true }
    }
}

impl From<HubsV1> for Hubs {
    fn from(old: HubsV1) -> Self {
        let stance = |on| if on { Stance::Allow } else { Stance::Block };
        Hubs { thera: stance(old.thera), turnur: stance(old.turnur) }
    }
}

/// The file of 0.4.0. It has `pochven`, and the file always holds it.
#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
struct ConfigV1 {
    capital: Option<String>,
    hull: Option<String>,
    pilot: Option<u64>,
    max_cap_tj: Option<f32>,
    min_life_min: Option<u64>,
    favourites: Option<Vec<String>>,
    mode: Option<Mode>,
    optimize: bool,
    top: Option<usize>,
    wormholes: Option<bool>,
    bridges: Option<bool>,
    cap_weight: Option<f32>,
    unknown_sig_penalty: Option<f32>,
    avoid_systems: Vec<AvoidName>,
    avoid_regions: Vec<AvoidName>,
    nexum: NexumConfig,
    eve_metro: EveMetroConfig,
    eve_scout: HubsV1,
    /// Whether a route can enter Pochven. 0.4.0 turned it off for a file without the key.
    pochven: bool,
    #[serde(deserialize_with = "crate::config::lenient_theme")]
    theme: Option<FactionTheme>,
    compact: bool,
    check_for_updates: Option<bool>,
    auto_update: bool,
    routing_overlay: RoutingOverlayConfig,
}

/// The file of 0.4.1. The key `pochven` became `filaments`, and `None` means on.
#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
struct ConfigV2 {
    capital: Option<String>,
    hull: Option<String>,
    pilot: Option<u64>,
    max_cap_tj: Option<f32>,
    min_life_min: Option<u64>,
    favourites: Option<Vec<String>>,
    mode: Option<Mode>,
    optimize: bool,
    top: Option<usize>,
    wormholes: Option<bool>,
    bridges: Option<bool>,
    cap_weight: Option<f32>,
    unknown_sig_penalty: Option<f32>,
    avoid_systems: Vec<AvoidName>,
    avoid_regions: Vec<AvoidName>,
    nexum: NexumConfig,
    eve_metro: EveMetroConfig,
    eve_scout: HubsV1,
    filaments: Option<bool>,
    #[serde(deserialize_with = "crate::config::lenient_theme")]
    theme: Option<FactionTheme>,
    compact: bool,
    check_for_updates: Option<bool>,
    auto_update: bool,
    routing_overlay: RoutingOverlayConfig,
}

impl From<ConfigV1> for ConfigV2 {
    fn from(old: ConfigV1) -> Self {
        ConfigV2 {
            capital: old.capital,
            hull: old.hull,
            pilot: old.pilot,
            max_cap_tj: old.max_cap_tj,
            min_life_min: old.min_life_min,
            favourites: old.favourites,
            mode: old.mode,
            optimize: old.optimize,
            top: old.top,
            wormholes: old.wormholes,
            bridges: old.bridges,
            cap_weight: old.cap_weight,
            unknown_sig_penalty: old.unknown_sig_penalty,
            avoid_systems: old.avoid_systems,
            avoid_regions: old.avoid_regions,
            nexum: old.nexum,
            eve_metro: old.eve_metro,
            eve_scout: old.eve_scout,
            filaments: Some(old.pochven),
            theme: old.theme,
            compact: old.compact,
            check_for_updates: old.check_for_updates,
            auto_update: old.auto_update,
            routing_overlay: old.routing_overlay,
        }
    }
}

impl From<ConfigV2> for Config {
    fn from(old: ConfigV2) -> Self {
        Config {
            capital: old.capital,
            hull: old.hull,
            pilot: old.pilot,
            max_cap_tj: old.max_cap_tj,
            min_life_min: old.min_life_min,
            favourites: old.favourites,
            mode: old.mode,
            optimize: old.optimize,
            top: old.top,
            wormholes: old.wormholes,
            bridges: old.bridges,
            cap_weight: old.cap_weight,
            unknown_sig_penalty: old.unknown_sig_penalty,
            avoid_systems: old.avoid_systems,
            avoid_regions: old.avoid_regions,
            nexum: old.nexum,
            eve_metro: old.eve_metro,
            eve_scout: old.eve_scout.into(),
            wormhole_space: WormholeSpace::default(),
            filaments: old.filaments.map(|on| if on { Stance::Allow } else { Stance::Block }),
            theme: old.theme,
            compact: old.compact,
            check_for_updates: old.check_for_updates,
            auto_update: old.auto_update,
            routing_overlay: old.routing_overlay,
        }
    }
}

impl Migrate for ConfigV1 {
    type From = Self;

    fn deserializer<'de>(input: &'de str) -> impl serde::de::Deserializer<'de> {
        deserializer(input)
    }
}

impl Migrate for ConfigV2 {
    type From = ConfigV1;

    fn deserializer<'de>(input: &'de str) -> impl serde::de::Deserializer<'de> {
        deserializer(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiKey;

    /// The file that 0.4.0 wrote. The router wrote it from the snapshot test of that release.
    const V0_4_0: &str = include_str!("../tests/fixtures/config-v0.4.0.json");
    /// The file that 0.4.1 wrote.
    const V0_4_1: &str = include_str!("../tests/fixtures/config-v0.4.1.json");

    /// What both files say, after the move to the current shape.
    fn expected(filaments: Stance) -> Config {
        Config {
            capital: Some("JK-Q77".into()),
            hull: Some("black-ops".into()),
            pilot: Some(2112345678),
            max_cap_tj: Some(36.5),
            min_life_min: Some(30),
            favourites: Some(vec!["Jita".into()]),
            mode: Some(Mode::PreferHighsec),
            optimize: true,
            top: Some(3),
            wormholes: Some(false),
            bridges: Some(true),
            cap_weight: Some(0.6),
            unknown_sig_penalty: Some(4.0),
            avoid_systems: vec![AvoidName::new("Rens", true)],
            avoid_regions: vec![AvoidName::new("Lonetrek", false)],
            nexum: NexumConfig {
                url: Some("https://nexum.example".into()),
                key: Some(ApiKey("nxm_key".into())),
                key_in_file: true,
                map_ids: vec!["m1".into()],
                keep_unsigned: true,
            },
            eve_metro: EveMetroConfig { enabled: true, character: Some(2112345678), ..Default::default() },
            eve_scout: Hubs { thera: Stance::Block, turnur: Stance::Allow },
            filaments: Some(filaments),
            check_for_updates: Some(false),
            auto_update: true,
            ..Default::default()
        }
    }

    fn json_of(cfg: &Config) -> String {
        serde_json::to_string_pretty(cfg).unwrap()
    }

    #[test]
    fn a_file_of_0_4_0_moves_to_the_current_shape_and_keeps_the_key() {
        let cfg = parse(V0_4_0).unwrap();
        assert_eq!(json_of(&cfg), json_of(&expected(Stance::Allow)));
        assert_eq!(cfg.nexum.key, Some(ApiKey("nxm_key".into())));
    }

    #[test]
    fn a_file_of_0_4_1_moves_to_the_current_shape_and_keeps_the_key() {
        let cfg = parse(V0_4_1).unwrap();
        assert_eq!(json_of(&cfg), json_of(&expected(Stance::Allow)));
        assert_eq!(cfg.nexum.key, Some(ApiKey("nxm_key".into())));
    }

    /// The old `pochven: false` blocks the filaments. The chain must not drop it.
    #[test]
    fn the_pochven_switch_of_0_4_0_becomes_the_filaments_stance() {
        let off = parse(&V0_4_0.replace(r#""pochven": true"#, r#""pochven": false"#)).unwrap();
        assert_eq!(off.filaments, Some(Stance::Block));
        assert_eq!(parse(r#"{"pochven": true}"#).unwrap().filaments, Some(Stance::Allow));
    }

    /// An old shape reads an unknown theme name as Caldari, like `Config`, and still moves the key.
    #[test]
    fn an_unknown_theme_in_an_old_file_becomes_caldari() {
        use crate::config::FactionTheme;
        let cfg = parse(r#"{"theme": "sansha", "nexum": {"key": "nxm_key"}}"#).unwrap();
        assert_eq!(cfg.theme, Some(FactionTheme::Caldari));
        assert_eq!(cfg.nexum.key, Some(ApiKey("nxm_key".into())));
    }

    /// Both old files read the hub switches as Allow and Block, and a file with no hubs as Allow.
    #[test]
    fn the_hub_switches_become_stances() {
        let cfg = parse(r#"{"eve_scout": {"thera": false}}"#).unwrap();
        assert_eq!(cfg.eve_scout, Hubs { thera: Stance::Block, turnur: Stance::Allow });
        assert_eq!(parse("{}").unwrap().eve_scout, Hubs::default());
    }

    /// A file of 0.4.1 has no `pochven` key, so an absent `filaments` stays on.
    #[test]
    fn an_empty_file_is_the_default_config() {
        assert_eq!(json_of(&parse("{}").unwrap()), json_of(&Config::default()));
    }

    /// A file that the current version wrote has fields that the old shapes do not know. The chain
    /// must leave it alone, or it would lose the new stances.
    #[test]
    fn a_current_file_is_not_claimed_by_an_old_shape() {
        let text = r#"{"eve_scout":{"thera":"avoid"},"filaments":"avoid","wormhole_space":{"pochven":"block"}}"#;
        assert!(ConfigV2::from_str_migrations(text).is_none());
        let cfg = parse(text).unwrap();
        assert_eq!(cfg.eve_scout.thera, Stance::Avoid);
        assert_eq!(cfg.filaments, Some(Stance::Avoid));
        assert_eq!(cfg.wormhole_space.pochven, Stance::Block);
        // And the file that this version writes reads back to the same value.
        assert_eq!(json_of(&parse(&json_of(&cfg)).unwrap()), json_of(&cfg));
    }

    /// A file that is not JSON still gives a JSON error, so the app moves it to `.bad`.
    #[test]
    fn a_damaged_file_gives_a_json_error() {
        assert!(parse("{ not json").is_err());
        assert!(parse("").is_err());
    }
}
