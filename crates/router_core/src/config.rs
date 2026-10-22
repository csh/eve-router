//! The `eve-router.json` file.

use crate::error::{FileError, write_atomic_with};
use crate::esi::client::Affiliation;
use crate::route::{Mode, RouteError};
use crate::wormhole::Hubs;
use crate::wormhole::SourceId;
use crate::wormhole_space::{Stance, WormholeSpace};
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

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(from = "NexumFile")]
pub struct NexumConfig {
    /// For example "https://nexum.example".
    pub url: Option<String>,
    /// A key with the `read` scope is enough. The key lives in the OS keyring
    /// (`nexum_key::KEYRING_SERVICE`). The file holds it only while `key_in_file` is true.
    pub key: Option<ApiKey>,
    /// True while the key is in the file: the file came from a version before the keyring, and the
    /// router has not moved the key yet, or the keyring is not available. `save` then writes the
    /// key, so no save loses it. The router sets this to false after the key is in the keyring.
    pub key_in_file: bool,
    /// The maps that the router reads. The router joins their wormholes.
    pub map_ids: Vec<String>,
    /// Keep a connection that has no known signature at either end. A file without this value
    /// skips these connections, because their data is the least reliable.
    pub keep_unsigned: bool,
}

impl Serialize for NexumConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            url: &'a Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            key: Option<&'a ApiKey>,
            map_ids: &'a [String],
            keep_unsigned: bool,
        }
        let key = self.key.as_ref().filter(|_| self.key_in_file);
        Out { url: &self.url, key, map_ids: &self.map_ids, keep_unsigned: self.keep_unsigned }.serialize(serializer)
    }
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
        let key_in_file = file.key.is_some();
        NexumConfig { url: file.url, key: file.key, key_in_file, map_ids, keep_unsigned: file.keep_unsigned }
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
    /// The API key. The file never holds it. The app sets it at start from the build or from the
    /// env var (`sources::evemetro::api_key`).
    #[serde(skip)]
    pub key: Option<ApiKey>,
    /// The character that identifies the user to the API. It must be a linked character.
    pub character: Option<u64>,
    /// The corporation and the alliance of `character`. The file never holds them. The app sets
    /// them from the ESI poll of a character with a valid login (`Pilots::affiliation`), because
    /// the service grants access by these IDs.
    #[serde(skip)]
    pub affiliation: Option<Affiliation>,
}

impl EveMetroConfig {
    /// The key and the character, when the source is on and both are set.
    pub fn complete(&self) -> Option<(&str, u64)> {
        if !self.enabled {
            return None;
        }
        Some((self.key.as_ref()?.0.as_str(), self.character?))
    }

    /// What a request needs: the key, the character and its verified affiliation. `None` while the
    /// settings are not complete or ESI has not given the affiliation.
    pub fn request(&self) -> Option<(&str, u64, &Affiliation)> {
        let (key, character) = self.complete()?;
        Some((key, character, self.affiliation.as_ref()?))
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
    /// The stance (allow, avoid, block) of each class of wormhole space. The default stays out of a file.
    #[serde(skip_serializing_if = "WormholeSpace::is_default")]
    pub wormhole_space: WormholeSpace,
    /// The filaments stance: whether a route can use a filament. `None` gives Allow. One stance covers
    /// all three clades: the SDE shows one Triglavian faction only (Triglavian Collective,
    /// `500026`), and the clade corporations share it. A file from before the rename has the
    /// key `pochven`. `config_migrate` reads that file.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::wormhole_space::switch::option")]
    pub filaments: Option<Stance>,
    /// The Photon UI faction theme. A name that is not known loads as Caldari.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient_theme")]
    pub theme: Option<FactionTheme>,
    /// Compact UI layout with tighter row heights and padding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compact: bool,
    /// Whether the app looks for a new version at the start. `None` gives on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_for_updates: Option<bool>,
    /// Install a new version at the start with no question. It has effect only while
    /// `check_for_updates` is on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_update: bool,
    #[serde(skip_serializing_if = "RoutingOverlayConfig::is_default")]
    pub routing_overlay: RoutingOverlayConfig,
}

impl Config {
    /// Whether the app looks for a new version at the start.
    pub fn checks_for_updates(&self) -> bool {
        self.check_for_updates.unwrap_or(true)
    }
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
/// loads, and the theme is Caldari, the default. A missing or null theme stays `None`. Each shape
/// of the config file in `config_migrate` uses this function.
pub(crate) fn lenient_theme<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<FactionTheme>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.map(|name| FactionTheme::parse(&name).unwrap_or_default()))
}

/// Why a value of the config or of a settings field is not valid.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ConfigError {
    #[error("\"{0}\" is not a URL. Give a URL that starts with https:// or http://.")]
    BadUrl(String),
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

/// The warning for a Nexum URL that uses plain `http://` to a remote host.
const HTTP_WARNING: &str = "This URL uses http://, so the API key crosses the network in clear text.";

/// The warning for a Nexum URL, or `None`. Both `https://` and `http://` are valid. The API key goes
/// in the header of each request, so `http://` to a remote host sends it in clear text. A URL for
/// this machine or for a private network address (a Nexum on the same computer or on a LAN) gets
/// no warning. A text that is not a URL also gets none, because `parse_nexum_url` rejects it.
pub fn url_warning(text: &str) -> Option<&'static str> {
    use oauth2::url::{Host, Url};
    let url = Url::parse(text.trim()).ok()?;
    if url.scheme() != "http" {
        return None;
    }
    let local = match url.host()? {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback() || ip.is_private(),
        Host::Ipv6(ip) => ip.is_loopback(),
    };
    (!local).then_some(HTTP_WARNING)
}

impl Config {
    /// Read the file. A missing file gives the default config. A file of an older version moves to
    /// the current shape (`config_migrate`).
    pub fn load(path: &Path) -> Result<Config, FileError> {
        match fs::read_to_string(path) {
            Ok(text) => crate::config_migrate::parse(&text).map_err(FileError::json(path)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(FileError::Io { path: path.to_path_buf(), source: e }),
        }
    }

    /// As `load`, and a file that is not valid JSON moves to `eve-router.json.bad`. The result is
    /// the default config and the new path of the file. An I/O error is not a damaged file, because
    /// a lock or a permission can cause it. Such an error passes through, and the file stays.
    pub fn load_recovering(path: &Path) -> Result<(Config, Option<PathBuf>), FileError> {
        match Config::load(path) {
            Err(error @ FileError::Json { .. }) => {
                let bad = path.with_extension("json.bad");
                match fs::rename(path, &bad) {
                    Ok(()) => Ok((Config::default(), Some(bad))),
                    Err(_) => Err(error),
                }
            }
            other => other.map(|cfg| (cfg, None)),
        }
    }

    /// Write the file through a temporary file and a rename, so a crash never leaves half a file.
    /// Create its directory first if necessary. A file that still holds the Nexum key (`key_in_file`) is private: only the owner can
    /// read it on Unix. The temporary file gets that mode before it holds a key.
    pub fn save(&self, path: &Path) -> Result<(), FileError> {
        let text = serde_json::to_string_pretty(self).map_err(FileError::json(path))?;
        let private = self.nexum.key_in_file && self.nexum.key.is_some();
        write_atomic_with(path, &path.with_extension("json.part"), &(text + "\n"), private)
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
                key_in_file: false,
                map_ids: vec!["m1".into()],
                keep_unsigned: true,
            },
            eve_metro: EveMetroConfig {
                enabled: true,
                key: Some(ApiKey("metro_key".into())),
                character: Some(2112345678),
                ..Default::default()
            },
            eve_scout: Hubs { thera: Stance::Block, turnur: Stance::Allow },
            wormhole_space: WormholeSpace::default(),
            filaments: Some(Stance::Allow),
            theme: None,
            compact: false,
            check_for_updates: Some(false),
            auto_update: true,
            routing_overlay: RoutingOverlayConfig::default(),
        };
        let text = serde_json::to_string_pretty(&cfg).unwrap();
        crate::assert_snapshot!("config_json", text);
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string_pretty(&back).unwrap(), text);
    }

    /// An old file has no update fields: the check is on and the install asks. The defaults
    /// stay out of a new file.
    #[test]
    fn the_update_settings_default_to_check_and_ask() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.checks_for_updates());
        assert!(!cfg.auto_update);
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(!text.contains("check_for_updates") && !text.contains("auto_update"), "{text}");
        let off: Config = serde_json::from_str(r#"{"check_for_updates":false}"#).unwrap();
        assert!(!off.checks_for_updates());
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
        // A name that is not known gives Caldari, and the file still loads.
        assert_eq!(load(r#"{"theme": "sansha"}"#), Some(FactionTheme::Caldari));
        assert_eq!(load(r#"{"theme": null}"#), None);
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
        let cfg = EveMetroConfig { enabled: true, key: Some(ApiKey("metro_test_key".into())), character: Some(42), ..Default::default() };
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
            eve_metro: EveMetroConfig {
                enabled: true,
                key: Some(ApiKey("metro_SECRET_000000000abc".into())),
                character: Some(7),
                ..Default::default()
            },
            ..Config::default()
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("SECRET"), "{debug}");
        assert!(debug.contains("metr…abc"), "{debug}");
    }

    #[test]
    fn the_eve_metro_key_never_reaches_the_file() {
        let cfg = Config {
            eve_metro: EveMetroConfig { enabled: true, key: Some(ApiKey("metro_SECRET".into())), character: Some(7), ..Default::default() },
            ..Config::default()
        };
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(!text.contains("SECRET"), "{text}");
        // The file cannot supply the affiliation either: a pilot could name a paying corporation.
        let forged: EveMetroConfig =
            serde_json::from_str(r#"{"enabled":true,"character":7,"affiliation":{"corporation_id":1,"alliance_id":2}}"#).unwrap();
        assert_eq!(forged.affiliation, None);
        let with = EveMetroConfig { affiliation: Some(Affiliation { corporation_id: 1, alliance_id: Some(2) }), ..forged };
        assert!(!serde_json::to_string(&with).unwrap().contains("corporation"));
        // A key from an older file is ignored.
        let old: EveMetroConfig = serde_json::from_str(r#"{"enabled":true,"key":"metro_OLD","character":7}"#).unwrap();
        assert_eq!(old, EveMetroConfig { enabled: true, key: None, character: Some(7), ..Default::default() });
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

    /// A key that the file holds (`key_in_file`) is written in full. Only Debug masks it. A key that
    /// the keyring holds never reaches the file.
    #[test]
    fn nexum_settings_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join(FILE_NAME);
        let mut cfg = Config::default();
        cfg.nexum.url = Some("https://nexum.example".into());
        cfg.nexum.key = Some(ApiKey("nxm_secret".into()));
        cfg.save(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("nxm_secret"));
        assert_eq!(Config::load(&path).unwrap().nexum.key, None);
        cfg.nexum.key_in_file = true;
        cfg.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
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
    fn save_creates_the_folder_and_leaves_no_part_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new").join(FILE_NAME);
        Config::default().save(&path).unwrap();
        assert!(path.is_file());
        assert!(!path.with_extension("json.part").exists());
    }

    #[test]
    fn a_failed_save_keeps_the_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        Config { top: Some(3), ..Config::default() }.save(&path).unwrap();
        // A directory blocks the temporary file.
        fs::create_dir(path.with_extension("json.part")).unwrap();
        assert!(Config { top: Some(9), ..Config::default() }.save(&path).is_err());
        assert_eq!(Config::load(&path).unwrap().top, Some(3));
    }

    #[test]
    fn a_damaged_config_is_moved_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        fs::write(&path, r#"{"top": "#).unwrap();
        let (cfg, moved) = Config::load_recovering(&path).unwrap();
        let bad = moved.unwrap();
        assert_eq!(bad, path.with_extension("json.bad"));
        assert_eq!(fs::read_to_string(&bad).unwrap(), r#"{"top": "#);
        assert!(!path.exists());
        assert_eq!(cfg.top, None);
    }

    #[test]
    fn a_good_or_missing_config_is_not_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let (_, moved) = Config::load_recovering(&path).unwrap();
        assert_eq!(moved, None);
        Config { top: Some(4), ..Config::default() }.save(&path).unwrap();
        let (cfg, moved) = Config::load_recovering(&path).unwrap();
        assert_eq!((cfg.top, moved), (Some(4), None));
        assert!(path.is_file());
    }

    #[test]
    fn an_unreadable_config_is_not_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        // Reading a directory is an I/O error, not a JSON error. A locked file acts the same way.
        fs::create_dir(&path).unwrap();
        assert!(matches!(Config::load_recovering(&path), Err(FileError::Io { .. })));
        assert!(path.is_dir());
    }

    #[test]
    fn nexum_url_text() {
        assert_eq!(parse_nexum_url(""), Ok(None));
        assert_eq!(parse_nexum_url(" https://nexum.example/ "), Ok(Some("https://nexum.example".into())));
        assert_eq!(parse_nexum_url("ftp://x"), Err(ConfigError::BadUrl("ftp://x".into())));
        assert!(parse_nexum_url("ftp://x").unwrap_err().to_string().contains("is not a URL"));
    }

    #[test]
    fn http_to_a_remote_host_gets_a_warning() {
        assert!(url_warning("http://203.0.113.5").is_some());
        assert!(url_warning("http://nexum.example").is_some());
        assert!(url_warning(" HTTP://Nexum.Example:3000/ ").is_some());
    }

    #[test]
    fn http_to_this_machine_or_a_private_address_gets_none() {
        for url in [
            "http://localhost:3000",
            "http://LOCALHOST",
            "http://127.0.0.1",
            "http://192.168.1.5",
            "http://10.0.0.2:8080",
            "http://[::1]:3000",
        ] {
            assert_eq!(url_warning(url), None, "{url}");
        }
    }

    #[test]
    fn https_and_a_bad_url_get_no_warning() {
        assert_eq!(url_warning("https://nexum.example"), None);
        assert_eq!(url_warning("https://203.0.113.5"), None);
        assert_eq!(url_warning("not a url"), None);
        assert_eq!(url_warning(""), None);
    }

    #[test]
    fn both_schemes_are_valid_urls() {
        assert_eq!(parse_nexum_url("http://localhost:3000/"), Ok(Some("http://localhost:3000".into())));
        assert_eq!(parse_nexum_url("https://nexum.example"), Ok(Some("https://nexum.example".into())));
    }

    #[test]
    fn the_url_error_names_both_schemes() {
        let text = parse_nexum_url("ftp://x").unwrap_err().to_string();
        assert!(text.contains("https://") && text.contains("http://"), "{text}");
    }

    #[test]
    fn hub_stances_default_to_allow() {
        // An old file has no "eve_scout" object. A file can set one hub only.
        assert_eq!(serde_json::from_str::<Config>("{}").unwrap().eve_scout, Hubs::default());
        let cfg: Config = serde_json::from_str(r#"{"eve_scout": {"thera": false}}"#).unwrap();
        assert_eq!(cfg.eve_scout, Hubs { thera: Stance::Block, turnur: Stance::Allow });
    }

    /// An old file holds `true` and `false`. A new file writes those two for Allow and Block, so an
    /// old version reads it, and writes the word "avoid" for Avoid.
    #[test]
    fn hub_and_filament_stances_read_old_switches_and_write_them_back() {
        let cfg: Config = serde_json::from_str(r#"{"eve_scout": {"thera": "avoid", "turnur": false}, "filaments": "avoid"}"#).unwrap();
        assert_eq!(cfg.eve_scout, Hubs { thera: Stance::Avoid, turnur: Stance::Block });
        assert_eq!(cfg.filaments, Some(Stance::Avoid));
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(
            text.contains(r#""thera":"avoid""#) && text.contains(r#""turnur":false"#) && text.contains(r#""filaments":"avoid""#),
            "{text}"
        );
        let old: Config = serde_json::from_str(r#"{"eve_scout": {"thera": true}, "filaments": false}"#).unwrap();
        assert_eq!((old.eve_scout.thera, old.filaments), (Stance::Allow, Some(Stance::Block)));
        assert_eq!(serde_json::from_str::<Config>("{}").unwrap().filaments, None);
    }
}
