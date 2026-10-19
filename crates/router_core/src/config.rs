//! The `eve-router.json` file.

use crate::error::FileError;
use crate::route::{Mode, RouteError};
use crate::wormhole::Hubs;
use crate::wormhole::SourceId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The config directory name inside the platform config directory.
pub const APP_DIR: &str = "com.smrkn.eve-router";
const FILE_NAME: &str = "eve-router.json";

/// The three places that the router reads and writes: the config file, the SDE directory and the
/// wormhole cache directory. `detect` finds them one time at the start. A test uses `in_dir`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub config: PathBuf,
    pub sde: PathBuf,
    pub cache: PathBuf,
}

impl AppPaths {
    /// The platform places. The cache is `com.smrkn.eve-router` in the platform cache directory,
    /// for example `%LOCALAPPDATA%\com.smrkn.eve-router` on Windows. If the platform gives no cache
    /// directory, the cache is a `cache` directory next to the config file.
    pub fn detect() -> AppPaths {
        let config = default_path();
        let cache = dirs::cache_dir().map_or_else(|| config.with_file_name("cache"), |dir| dir.join(APP_DIR));
        AppPaths { config, sde: default_sde_dir(), cache }
    }

    /// All places inside `dir`.
    pub fn in_dir(dir: &Path) -> AppPaths {
        AppPaths { config: dir.join(FILE_NAME), sde: dir.join("sde"), cache: dir.join("cache") }
    }

    /// The cache file of a wormhole source, for example `nexum.json`.
    pub fn cache_file(&self, source: SourceId) -> PathBuf {
        self.cache.join(format!("{}.json", source.file_stem()))
    }
}

/// The default config file:
/// - Windows: `%APPDATA%\com.smrkn.eve-router\eve-router.json`
/// - macOS: `~/Library/Application Support/com.smrkn.eve-router/eve-router.json`
/// - Linux: `$XDG_CONFIG_HOME/com.smrkn.eve-router/eve-router.json`, else `~/.config/...`
///
/// If the platform gives no config directory, use the working directory.
fn default_path() -> PathBuf {
    dirs::config_dir().map_or_else(|| PathBuf::from(FILE_NAME), |dir| dir.join(APP_DIR).join(FILE_NAME))
}

/// The default SDE directory. The router downloads the SDE files here.
/// - Windows: `%LOCALAPPDATA%\com.smrkn.eve-router\sde`
/// - macOS: `~/Library/Application Support/com.smrkn.eve-router/sde`
/// - Linux: `$XDG_DATA_HOME/com.smrkn.eve-router/sde`, else `~/.local/share/...`
///
/// If the platform gives no data directory, use `sde` in the working directory.
fn default_sde_dir() -> PathBuf {
    dirs::data_local_dir().map_or_else(|| PathBuf::from("sde"), |dir| dir.join(APP_DIR).join("sde"))
}

/// The default jump bridge list (SMT format), next to the config file.
pub const BRIDGES_FILE: &str = "ansiblex.txt";

/// The overlay file to load: the default file next to the config file. A missing file is not an
/// error. The router then loads no overlay.
pub fn overlay_path(cfg_path: &Path, default_name: &str) -> Option<PathBuf> {
    let path = cfg_path.with_file_name(default_name);
    path.is_file().then_some(path)
}

/// The default minimum time (minutes) that a wormhole must have left.
pub const DEFAULT_MIN_LIFE_MIN: u64 = 60;

/// The main trade hubs.
pub const DEFAULT_FAVOURITES: [&str; 5] = ["Jita", "Amarr", "Dodixie", "Hek", "Rens"];

/// An API key, for Nexum or EVE Metro. `Debug` shows only the first 4 and the last 3 characters.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct ApiKey(pub String);

impl ApiKey {
    /// For example "nxm_…xyz". A key of 7 characters or fewer shows only "…".
    pub fn masked(&self) -> String {
        let chars: Vec<char> = self.0.chars().collect();
        if chars.len() <= 7 {
            return "…".into();
        }
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 3..].iter().collect();
        format!("{head}…{tail}")
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.masked())
    }
}

/// The Nexum settings. The router fetches the map only when all three values are set.
/// A system or a region on the avoid list, by name. The file holds an object. A plain name,
/// from an older file, also loads, as an entry with `never` false.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(from = "AvoidRepr")]
pub struct AvoidName {
    pub name: String,
    /// True: no route crosses it. False: a route crosses it only if no other route exists.
    #[serde(default)]
    pub never: bool,
}

/// The two forms of an avoid entry in a file.
#[derive(Deserialize)]
#[serde(untagged)]
enum AvoidRepr {
    Name(String),
    Full {
        name: String,
        #[serde(default)]
        never: bool,
    },
}

impl From<AvoidRepr> for AvoidName {
    fn from(repr: AvoidRepr) -> Self {
        match repr {
            AvoidRepr::Name(name) => AvoidName { name, never: false },
            AvoidRepr::Full { name, never } => AvoidName { name, never },
        }
    }
}

impl AvoidName {
    pub fn new(name: &str, never: bool) -> Self {
        AvoidName { name: name.to_string(), never }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(from = "NexumFile")]
pub struct NexumConfig {
    /// For example "https://nexum.example".
    pub url: Option<String>,
    /// A key with the `read` scope is enough.
    pub key: Option<ApiKey>,
    /// The maps that the router reads. The router joins their wormholes.
    pub map_ids: Vec<String>,
    /// Keep a connection that has no known signature at either end. A file without this value
    /// skips these connections, because their data is the least reliable.
    pub keep_unsigned: bool,
}

/// The Nexum part of the config file. An old file has one `map_id`, a new file has `map_ids`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct NexumFile {
    url: Option<String>,
    key: Option<ApiKey>,
    map_id: Option<String>,
    map_ids: Vec<String>,
    keep_unsigned: bool,
}

impl From<NexumFile> for NexumConfig {
    fn from(file: NexumFile) -> Self {
        let mut map_ids = file.map_ids;
        if let Some(id) = file.map_id
            && !map_ids.contains(&id)
        {
            map_ids.insert(0, id);
        }
        NexumConfig { url: file.url, key: file.key, map_ids, keep_unsigned: file.keep_unsigned }
    }
}

impl NexumConfig {
    /// The URL, the key and the map IDs, when the URL and the key are set and at least one map is on.
    pub fn complete(&self) -> Option<(&str, &str, &[String])> {
        let ids = self.map_ids.as_slice();
        if ids.is_empty() {
            return None;
        }
        Some((self.url.as_deref()?, self.key.as_ref()?.0.as_str(), ids))
    }
}

/// The EVE Metro settings. The URL is fixed (`sources::evemetro::URL`). One user pays for the
/// service, so the settings name one linked character. The API checks that character, its
/// corporation and its alliance.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct EveMetroConfig {
    pub enabled: bool,
    pub key: Option<ApiKey>,
    /// The character that identifies the user to the API. It must be a linked character.
    pub character: Option<u64>,
}

impl EveMetroConfig {
    /// The key and the character, when the source is on and both are set.
    pub fn complete(&self) -> Option<(&str, u64)> {
        if !self.enabled {
            return None;
        }
        Some((self.key.as_ref()?.0.as_str(), self.character?))
    }

    /// The settings that the router uses. EVE Metro needs a logged-in character to identify the
    /// user, so the source is off while the app has no EVE client ID (`sso_ready` false). The file
    /// keeps the settings of the user.
    pub fn gated(&self, sso_ready: bool) -> EveMetroConfig {
        if sso_ready { self.clone() } else { EveMetroConfig::default() }
    }
}

/// A place on the desktop, in logical points: the unit of the window positions of egui. On a
/// monitor with a scale of 100% a point is a pixel.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

/// Whose position of the overlay the user sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The position of each character that has no position of its own.
    Global,
    Character(u64),
}

/// The routing overlay: a small window above the game that shows the signature of the next
/// wormhole while the client of the route character is in front. It is off until the user turns it on.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct RoutingOverlayConfig {
    pub enabled: bool,
    /// The place for each character that has no place of its own. `None`: the built-in place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Pos>,
    /// The own place of a character, by character ID.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub characters: BTreeMap<u64, Pos>,
}

impl RoutingOverlayConfig {
    /// True for the settings of a new file. The file leaves them out.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The position that the user set for this place. A character with no position of its own gives `None`.
    pub fn position_of(&self, place: Place) -> Option<Pos> {
        match place {
            Place::Global => self.position,
            Place::Character(id) => self.characters.get(&id).copied(),
        }
    }

    /// Set the position for a place. `None` resets it: a character then uses the global position.
    pub fn set_position(&mut self, place: Place, pos: Option<Pos>) {
        match (place, pos) {
            (Place::Global, pos) => self.position = pos,
            (Place::Character(id), Some(pos)) => _ = self.characters.insert(id, pos),
            (Place::Character(id), None) => _ = self.characters.remove(&id),
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// The alliance capital system. Without it, jump bridges are off.
    pub capital: Option<String>,
    /// A ship name ("Sin") or a ship group ("Black Ops" or `black-ops`).
    pub hull: Option<String>,
    /// The character whose ship sets the hull. `None`: the hull comes from `hull` only.
    pub pilot: Option<u64>,
    /// The maximum capacitor (TJ) that one bridge jump can use.
    pub max_cap_tj: Option<f32>,
    /// The minimum time (minutes) that a wormhole must have left. `None` gives `DEFAULT_MIN_LIFE_MIN`.
    pub min_life_min: Option<u64>,
    /// The sidebar destinations. `None` gives `DEFAULT_FAVOURITES`.
    pub favourites: Option<Vec<String>>,
    pub mode: Option<Mode>,
    /// Visit each system one time, in the cheapest order.
    pub optimize: bool,
    pub top: Option<usize>,
    /// The wormholes switch of the top bar. `None` gives on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wormholes: Option<bool>,
    /// The jump bridges switch of the top bar. `None` gives on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridges: Option<bool>,
    /// The cost of 1% of the Ansiblex capacitor, in jumps. `None` gives the default.
    pub cap_weight: Option<f32>,
    /// The extra cost, in jumps, of a wormhole with no known signature. `None` gives the default.
    pub unknown_sig_penalty: Option<f32>,
    /// The systems that a route avoids, by name.
    pub avoid_systems: Vec<AvoidName>,
    /// The regions that a route avoids, by name.
    pub avoid_regions: Vec<AvoidName>,
    pub nexum: NexumConfig,
    pub eve_metro: EveMetroConfig,
    /// The Thera and Turnur switches. Both are on in a file without them.
    pub eve_scout: Hubs,
    /// The Pochven switch: whether a route can go into Pochven at all, by a filament or by a
    /// wormhole (the EVE Metro map). Off by default, because the feature is new and Pochven is a
    /// long way back. One switch covers all three clades: the SDE shows one Triglavian faction
    /// only (Triglavian Collective, `500026`), and the clade corporations share it.
    pub pochven: bool,
    /// The Photon UI faction theme. A name that is not known loads as no theme.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient_theme")]
    pub theme: Option<FactionTheme>,
    /// Compact UI layout with tighter row heights and padding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compact: bool,
    #[serde(skip_serializing_if = "RoutingOverlayConfig::is_default")]
    pub routing_overlay: RoutingOverlayConfig,
}

/// The Photon UI faction theme presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FactionTheme {
    #[default]
    Caldari,
    Amarr,
    Gallente,
    Minmatar,
}

impl FactionTheme {
    pub const ALL: [FactionTheme; 4] = [FactionTheme::Caldari, FactionTheme::Amarr, FactionTheme::Gallente, FactionTheme::Minmatar];

    pub fn label(self) -> &'static str {
        match self {
            FactionTheme::Caldari => "Caldari",
            FactionTheme::Amarr => "Amarr",
            FactionTheme::Gallente => "Gallente",
            FactionTheme::Minmatar => "Minmatar",
        }
    }

    /// The theme for a name, in any case. An unknown name gives `None`.
    pub fn parse(name: &str) -> Option<FactionTheme> {
        Self::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name.trim()))
    }
}

/// An old or edited file can hold a theme name that this version does not know. The file still
/// loads, and the app uses the default theme.
fn lenient_theme<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<FactionTheme>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.and_then(|name| FactionTheme::parse(&name)))
}

/// Why a value of the config or of a settings field is not valid.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ConfigError {
    #[error("\"{0}\" is not a URL. Give a URL that starts with https://.")]
    BadUrl(String),
    #[error("Unknown hull \"{0}\"")]
    UnknownHull(String),
    #[error("\"{value}\" is not a TJ value. Give a number from 0 to {max}, or leave it empty for no limit.")]
    BadTjValue { value: String, max: f32 },
    #[error(transparent)]
    System(#[from] RouteError),
}

/// The Nexum URL from a text field. An empty text gives no URL. The URL loses a final "/".
pub fn parse_nexum_url(text: &str) -> Result<Option<String>, ConfigError> {
    let value = text.trim();
    if value.is_empty() {
        Ok(None)
    } else if value.starts_with("https://") || value.starts_with("http://") {
        Ok(Some(value.trim_end_matches('/').to_string()))
    } else {
        Err(ConfigError::BadUrl(value.to_string()))
    }
}

impl Config {
    /// Read the file. A missing file gives the default config.
    pub fn load(path: &Path) -> Result<Config, FileError> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(FileError::json(path)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(FileError::Io { path: path.to_path_buf(), source: e }),
        }
    }

    /// Write the file. Create its directory first if necessary.
    pub fn save(&self, path: &Path) -> Result<(), FileError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(FileError::io(dir))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(FileError::json(path))?;
        // The file holds the Nexum and EVE Metro keys, so only the owner can read it. The file gets its
        // mode before the router writes a key. No other user can read a key at any time.
        #[cfg(unix)]
        if self.nexum.key.is_some() || self.eve_metro.key.is_some() {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file =
                fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(FileError::io(path))?;
            // The mode above applies only to a new file. This call fixes an old file.
            file.set_permissions(fs::Permissions::from_mode(0o600)).map_err(FileError::io(path))?;
            return file.write_all((text + "\n").as_bytes()).map_err(FileError::io(path));
        }
        fs::write(path, text + "\n").map_err(FileError::io(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_is_in_app_dir() {
        let path = default_path();
        assert!(path.ends_with(Path::new(APP_DIR).join(FILE_NAME)), "{}", path.display());
    }

    /// An older file has a plain name for each avoid entry. It still loads, as a "prefer" entry.
    #[test]
    fn an_avoid_entry_reads_a_plain_name_or_an_object() {
        let text = r#"{"avoid_systems":["Uedama",{"name":"Sivala","never":true}],"avoid_regions":[{"name":"Lonetrek"}]}"#;
        let cfg: Config = serde_json::from_str(text).unwrap();
        assert_eq!(cfg.avoid_systems, [AvoidName::new("Uedama", false), AvoidName::new("Sivala", true)]);
        assert_eq!(cfg.avoid_regions, [AvoidName::new("Lonetrek", false)]);
    }

    /// The file format: the field names and the kebab-case mode names.
    #[test]
    fn config_json_snapshot() {
        let cfg = Config {
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
                map_ids: vec!["m1".into()],
                keep_unsigned: true,
            },
            eve_metro: EveMetroConfig { enabled: true, key: Some(ApiKey("metro_key".into())), character: Some(2112345678) },
            eve_scout: Hubs { thera: false, turnur: true },
            pochven: true,
            theme: None,
            compact: false,
            routing_overlay: RoutingOverlayConfig::default(),
        };
        let text = serde_json::to_string_pretty(&cfg).unwrap();
        crate::assert_snapshot!("config_json", text);
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string_pretty(&back).unwrap(), text);
    }

    /// A new file has no routing overlay entry. Set values survive a round trip, and the
    /// character IDs become text keys in the file.
    #[test]
    fn the_routing_overlay_settings_round_trip() {
        assert!(!serde_json::to_string(&Config::default()).unwrap().contains("routing_overlay"));
        let mut cfg = Config::default();
        cfg.routing_overlay.enabled = true;
        cfg.routing_overlay.position = Some(Pos { x: 40, y: -12 });
        cfg.routing_overlay.characters.insert(2112345678, Pos { x: 1, y: 2 });
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(text.contains(r#""2112345678":{"x":1,"y":2}"#), "{text}");
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.routing_overlay, cfg.routing_overlay);
    }

    #[test]
    fn a_position_is_set_and_reset_for_a_place() {
        let mut config = RoutingOverlayConfig::default();
        let a = Pos { x: 1, y: 2 };
        let b = Pos { x: 3, y: 4 };
        config.set_position(Place::Global, Some(a));
        config.set_position(Place::Character(7), Some(b));
        assert_eq!(config.position_of(Place::Global), Some(a));
        assert_eq!(config.position_of(Place::Character(7)), Some(b));
        // A character with no position of its own reads `None`. The global position is a fallback of the router.
        assert_eq!(config.position_of(Place::Character(8)), None);
        config.set_position(Place::Character(7), None);
        assert_eq!(config.position_of(Place::Character(7)), None);
        config.set_position(Place::Global, None);
        assert!(config.is_default(), "{config:?}");
    }

    #[test]
    fn overlay_path_needs_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let cfg_path = dir.join(FILE_NAME);
        let default = dir.join(BRIDGES_FILE);
        let _ = fs::remove_file(&default);
        // No default file: no overlay.
        assert_eq!(overlay_path(&cfg_path, BRIDGES_FILE), None);
        // The default file next to the config file.
        fs::write(&default, "").unwrap();
        assert_eq!(overlay_path(&cfg_path, BRIDGES_FILE), Some(default));
    }

    #[test]
    fn the_theme_loads_from_an_old_file() {
        let load = |json: &str| serde_json::from_str::<Config>(json).unwrap().theme;
        assert_eq!(load(r#"{"theme": "amarr"}"#), Some(FactionTheme::Amarr));
        assert_eq!(load(r#"{"theme": " Minmatar "}"#), Some(FactionTheme::Minmatar));
        // A name that is not known gives the default theme, and the file still loads.
        assert_eq!(load(r#"{"theme": "sansha"}"#), None);
        assert_eq!(load(r#"{}"#), None);
        let cfg = Config { theme: Some(FactionTheme::Gallente), ..Config::default() };
        assert!(serde_json::to_string(&cfg).unwrap().contains(r#""theme":"gallente""#));
    }

    #[test]
    fn save_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join(APP_DIR).join(FILE_NAME);
        Config::default().save(&path).unwrap();
        assert!(Config::load(&path).is_ok());
    }

    #[test]
    fn nexum_key_is_masked() {
        let mut cfg = Config {
            nexum: NexumConfig {
                url: Some("https://nexum.example".into()),
                key: Some(ApiKey("nxm_TESTKEY_0000000000000000xyz".into())),
                map_ids: vec!["m1".into()],
                ..NexumConfig::default()
            },
            ..Config::default()
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("TESTKEY_0000"), "{debug}");
        assert!(debug.contains("nxm_…xyz"), "{debug}");
        assert_eq!(ApiKey("abc".into()).masked(), "…");
        // A key with multi-byte characters does not split a character.
        assert_eq!(ApiKey("ééééééééé".into()).masked(), "éééé…ééé");
        assert_eq!(cfg.nexum.complete(), Some(("https://nexum.example", "nxm_TESTKEY_0000000000000000xyz", &["m1".to_string()][..])));
        cfg.nexum.map_ids.clear();
        assert_eq!(cfg.nexum.complete(), None);
        cfg.nexum.map_ids = vec!["m1".into()];
        cfg.nexum.key = None;
        assert_eq!(cfg.nexum.complete(), None);
    }

    #[test]
    fn eve_metro_is_off_without_a_client_id() {
        let cfg = EveMetroConfig { enabled: true, key: Some(ApiKey("metro_test_key".into())), character: Some(42) };
        assert_eq!(cfg.gated(true), cfg);
        assert_eq!(cfg.gated(false), EveMetroConfig::default());
        assert_eq!(cfg.gated(false).complete(), None);
        // The gate leaves the settings of the user as they are.
        assert_eq!(cfg.complete(), Some(("metro_test_key", 42)));
    }

    #[test]
    fn an_old_file_has_no_eve_metro_and_its_key_is_masked() {
        // A file from before EVE Metro: the source is off.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.eve_metro, EveMetroConfig::default());
        assert_eq!(cfg.eve_metro.complete(), None);
        let cfg = Config {
            eve_metro: EveMetroConfig { enabled: true, key: Some(ApiKey("metro_SECRET_000000000abc".into())), character: Some(7) },
            ..Config::default()
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("SECRET"), "{debug}");
        assert!(debug.contains("metr…abc"), "{debug}");
    }

    #[test]
    fn an_old_file_with_a_removed_setting_still_loads() {
        let cfg: Config = serde_json::from_str(r#"{"unknown_sig_broken": true, "unknown_sig_penalty": 2.0}"#).unwrap();
        assert_eq!(cfg.unknown_sig_penalty, Some(2.0));
    }

    #[test]
    fn an_old_file_with_one_map_id_gives_one_map() {
        let old: NexumConfig = serde_json::from_str(r#"{"url":"https://nexum.example","map_id":"m1"}"#).unwrap();
        assert_eq!(old.map_ids, vec!["m1".to_string()]);
        let new: NexumConfig = serde_json::from_str(r#"{"map_ids":["m2","m1"],"map_id":"m1"}"#).unwrap();
        assert_eq!(new.map_ids, vec!["m2".to_string(), "m1".to_string()]);
        assert_eq!(serde_json::from_str::<NexumConfig>("{}").unwrap(), NexumConfig::default());
    }

    #[test]
    fn nexum_settings_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join(FILE_NAME);
        let mut cfg = Config::default();
        cfg.nexum.url = Some("https://nexum.example".into());
        cfg.nexum.key = Some(ApiKey("nxm_secret".into()));
        cfg.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        // The file holds the full key. Only Debug masks it.
        assert!(text.contains("\"key\": \"nxm_secret\""), "{text}");
        assert_eq!(Config::load(&path).unwrap().nexum, cfg.nexum);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_broken_config_file_names_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        fs::write(&path, "{ not json").unwrap();
        let error = Config::load(&path).unwrap_err();
        assert!(matches!(error, FileError::Json { .. }), "{error:?}");
        assert!(error.to_string().starts_with(&path.display().to_string()), "{error}");
    }

    #[test]
    fn nexum_url_text() {
        assert_eq!(parse_nexum_url(""), Ok(None));
        assert_eq!(parse_nexum_url(" https://nexum.example/ "), Ok(Some("https://nexum.example".into())));
        assert_eq!(parse_nexum_url("ftp://x"), Err(ConfigError::BadUrl("ftp://x".into())));
        assert!(parse_nexum_url("ftp://x").unwrap_err().to_string().contains("is not a URL"));
    }

    #[test]
    fn hub_switches_default_on() {
        // An old file has no "eve_scout" object. A file can set one switch only.
        assert_eq!(serde_json::from_str::<Config>("{}").unwrap().eve_scout, Hubs { thera: true, turnur: true });
        let cfg: Config = serde_json::from_str(r#"{"eve_scout": {"thera": false}}"#).unwrap();
        assert_eq!(cfg.eve_scout, Hubs { thera: false, turnur: true });
    }
}
