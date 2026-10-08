//! The `eve-router.json` file. CLI flags override its values.

use crate::route::Mode;
use crate::wormhole::Hubs;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The config directory name inside the platform config directory.
pub const APP_DIR: &str = "com.smrkn.eve-router";
const FILE_NAME: &str = "eve-router.json";

/// The default config file:
/// - Windows: `%APPDATA%\com.smrkn.eve-router\eve-router.json`
/// - macOS: `~/Library/Application Support/com.smrkn.eve-router/eve-router.json`
/// - Linux: `$XDG_CONFIG_HOME/com.smrkn.eve-router/eve-router.json`, else `~/.config/...`
///
/// If the platform gives no config directory, use the working directory.
pub fn default_path() -> PathBuf {
    dirs::config_dir().map_or_else(|| PathBuf::from(FILE_NAME), |dir| dir.join(APP_DIR).join(FILE_NAME))
}

/// The default SDE directory. The router downloads the SDE files here.
/// - Windows: `%LOCALAPPDATA%\com.smrkn.eve-router\sde`
/// - macOS: `~/Library/Application Support/com.smrkn.eve-router/sde`
/// - Linux: `$XDG_DATA_HOME/com.smrkn.eve-router/sde`, else `~/.local/share/...`
///
/// If the platform gives no data directory, use `sde` in the working directory.
pub fn default_sde_dir() -> PathBuf {
    dirs::data_local_dir().map_or_else(|| PathBuf::from("sde"), |dir| dir.join(APP_DIR).join("sde"))
}

/// The default jump bridge list (SMT format), next to the config file.
pub const BRIDGES_FILE: &str = "ansiblex.txt";

/// The overlay file to load: the CLI flag if given, else the default file next to the
/// config file. A missing default file is not an error. The router then loads no overlay.
pub fn overlay_path(flag: Option<PathBuf>, cfg_path: &Path, default_name: &str) -> Option<PathBuf> {
    flag.or_else(|| {
        let path = cfg_path.with_file_name(default_name);
        path.is_file().then_some(path)
    })
}

/// The default minimum time (minutes) that a wormhole must have left.
pub const DEFAULT_MIN_LIFE_MIN: u64 = 60;

/// The main trade hubs.
pub const DEFAULT_FAVOURITES: [&str; 5] = ["Jita", "Amarr", "Dodixie", "Hek", "Rens"];

/// A Nexum API key. `Debug` shows only the first 4 and the last 3 characters.
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
/// A system or a region on the avoid list, by name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AvoidName {
    pub name: String,
    /// True: no route crosses it. False: a route crosses it only if no other route exists.
    #[serde(default)]
    pub never: bool,
}

impl AvoidName {
    pub fn new(name: &str, never: bool) -> Self {
        AvoidName { name: name.to_string(), never }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NexumConfig {
    /// For example "https://nexum.example".
    pub url: Option<String>,
    /// A key with the `read` scope is enough.
    pub key: Option<ApiKey>,
    pub map_id: Option<String>,
}

impl NexumConfig {
    /// The URL, the key and the map ID, when all three are set.
    pub fn complete(&self) -> Option<(&str, &str, &str)> {
        Some((self.url.as_deref()?, self.key.as_ref()?.0.as_str(), self.map_id.as_deref()?))
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
    /// The cost of 1% of the Ansiblex capacitor, in jumps. `None` gives the default.
    pub cap_weight: Option<f32>,
    /// The extra cost, in jumps, of a wormhole with no known signature. `None` gives the default.
    pub unknown_sig_penalty: Option<f32>,
    /// Drop a wormhole with no known signature, instead of a penalty.
    pub unknown_sig_broken: bool,
    /// The systems that a route avoids, by name.
    pub avoid_systems: Vec<AvoidName>,
    /// The regions that a route avoids, by name.
    pub avoid_regions: Vec<AvoidName>,
    pub nexum: NexumConfig,
    /// The Thera and Turnur switches. Both are on in a file without them.
    pub eve_scout: Hubs,
}

/// The config before and after the CLI flags. A save puts back the file value of each field
/// that a flag set, so a flag lasts one run. A field that the user changes in the app keeps the
/// new value. A change back to the flag value counts as no change.
#[derive(Default)]
pub struct RunOverrides {
    file: Config,
    run: Config,
}

impl RunOverrides {
    /// `file`: the config file as it loaded. `run`: the config with the flags, as
    /// `Settings::store` writes it, so `restore` compares the same spellings.
    pub fn new(file: Config, run: Config) -> Self {
        Self { file, run }
    }

    /// Put back the file value of each flagged field that still has its flag value. A change
    /// back to the flag value counts as no change. The hull and the pilot are one field:
    /// together they set the hull source.
    pub fn restore(&self, cfg: &mut Config) {
        fn keep<T: PartialEq + Clone>(slot: &mut T, file: &T, run: &T) {
            if slot == run && run != file {
                slot.clone_from(file);
            }
        }
        let (f, r) = (&self.file, &self.run);
        keep(&mut cfg.capital, &f.capital, &r.capital);
        let mut hull = (cfg.hull.take(), cfg.pilot.take());
        keep(&mut hull, &(f.hull.clone(), f.pilot), &(r.hull.clone(), r.pilot));
        (cfg.hull, cfg.pilot) = hull;
        keep(&mut cfg.max_cap_tj, &f.max_cap_tj, &r.max_cap_tj);
        keep(&mut cfg.min_life_min, &f.min_life_min, &r.min_life_min);
        keep(&mut cfg.mode, &f.mode, &r.mode);
        keep(&mut cfg.optimize, &f.optimize, &r.optimize);
        keep(&mut cfg.top, &f.top, &r.top);
        keep(&mut cfg.cap_weight, &f.cap_weight, &r.cap_weight);
        keep(&mut cfg.unknown_sig_penalty, &f.unknown_sig_penalty, &r.unknown_sig_penalty);
        keep(&mut cfg.unknown_sig_broken, &f.unknown_sig_broken, &r.unknown_sig_broken);
    }
}

/// The Nexum URL from a text field. An empty text gives no URL. The URL loses a final "/".
pub fn parse_nexum_url(text: &str) -> Result<Option<String>, String> {
    let value = text.trim();
    if value.is_empty() {
        Ok(None)
    } else if value.starts_with("https://") || value.starts_with("http://") {
        Ok(Some(value.trim_end_matches('/').to_string()))
    } else {
        Err(format!("\"{value}\" is not a URL. Give a URL that starts with https://."))
    }
}

impl Config {
    /// Read the file. A missing file gives the default config.
    pub fn load(path: &Path) -> Result<Config, String> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Write the file. Create its directory first if necessary.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // The file holds the Nexum key, so only the owner can read it. The file gets its
        // mode before the router writes the key. No other user can read the key at any time.
        #[cfg(unix)]
        if self.nexum.key.is_some() {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let err = |e: std::io::Error| format!("{}: {e}", path.display());
            let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(err)?;
            // The mode above applies only to a new file. This call fixes an old file.
            file.set_permissions(fs::Permissions::from_mode(0o600)).map_err(err)?;
            return file.write_all((text + "\n").as_bytes()).map_err(err);
        }
        fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_overrides_keep_the_hull_and_pilot_together() {
        // The file follows pilot 9. `--hull Paladin` gives a manual hull for this run.
        let file = Config { hull: Some("Sin".into()), pilot: Some(9), ..Config::default() };
        let run = Config { hull: Some("Paladin".into()), pilot: None, ..file.clone() };
        let overrides = RunOverrides::new(file, run.clone());
        // The user picks the manual hull Rorqual in the app: the pilot stays off.
        let mut cfg = Config { hull: Some("Rorqual".into()), ..run };
        overrides.restore(&mut cfg);
        assert_eq!((cfg.hull.as_deref(), cfg.pilot), (Some("Rorqual"), None));
    }

    #[test]
    fn run_overrides_last_one_run() {
        let file = Config { hull: Some("Sin".into()), pilot: Some(9), top: Some(3), ..Config::default() };
        let run = Config { hull: Some("Paladin".into()), pilot: None, top: Some(5), ..file.clone() };
        let overrides = RunOverrides::new(file, run.clone());
        // No change in the app: the file values come back.
        let mut cfg = run.clone();
        overrides.restore(&mut cfg);
        assert_eq!((cfg.hull.as_deref(), cfg.pilot, cfg.top), (Some("Sin"), Some(9), Some(3)));
        // The user changed the route count in the app: the new value stays.
        let mut cfg = Config { top: Some(7), ..run };
        overrides.restore(&mut cfg);
        assert_eq!((cfg.hull.as_deref(), cfg.top), (Some("Sin"), Some(7)));
    }

    #[test]
    fn default_path_is_in_app_dir() {
        let path = default_path();
        assert!(path.ends_with(Path::new(APP_DIR).join(FILE_NAME)), "{}", path.display());
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
            cap_weight: Some(0.6),
            unknown_sig_penalty: Some(4.0),
            unknown_sig_broken: true,
            avoid_systems: vec![AvoidName::new("Rens", true)],
            avoid_regions: vec![AvoidName::new("Lonetrek", false)],
            nexum: NexumConfig {
                url: Some("https://nexum.example".into()),
                key: Some(ApiKey("nxm_key".into())),
                map_id: Some("m1".into()),
            },
            eve_scout: Hubs { thera: false, turnur: true },
        };
        let text = serde_json::to_string_pretty(&cfg).unwrap();
        crate::assert_snapshot!("config_json", text);
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string_pretty(&back).unwrap(), text);
    }

    #[test]
    fn overlay_path_order() {
        let dir = std::env::temp_dir().join("eve-router-test-overlay");
        fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join(FILE_NAME);
        let default = dir.join(BRIDGES_FILE);
        let _ = fs::remove_file(&default);
        // No flag and no default file: no overlay.
        assert_eq!(overlay_path(None, &cfg_path, BRIDGES_FILE), None);
        // No flag: the default file next to the config file.
        fs::write(&default, "").unwrap();
        assert_eq!(overlay_path(None, &cfg_path, BRIDGES_FILE), Some(default));
        // The flag wins.
        let flag = PathBuf::from("other.txt");
        assert_eq!(overlay_path(Some(flag.clone()), &cfg_path, BRIDGES_FILE), Some(flag));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_creates_directory() {
        let dir = std::env::temp_dir().join("eve-router-test-dir");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join(APP_DIR).join(FILE_NAME);
        Config::default().save(&path).unwrap();
        assert!(Config::load(&path).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nexum_key_is_masked() {
        let mut cfg = Config {
            nexum: NexumConfig {
                url: Some("https://nexum.example".into()),
                key: Some(ApiKey("nxm_TESTKEY_0000000000000000xyz".into())),
                map_id: Some("m1".into()),
            },
            ..Config::default()
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("TESTKEY_0000"), "{debug}");
        assert!(debug.contains("nxm_…xyz"), "{debug}");
        assert_eq!(ApiKey("abc".into()).masked(), "…");
        // A key with multi-byte characters does not split a character.
        assert_eq!(ApiKey("ééééééééé".into()).masked(), "éééé…ééé");
        assert_eq!(cfg.nexum.complete(), Some(("https://nexum.example", "nxm_TESTKEY_0000000000000000xyz", "m1")));
        cfg.nexum.key = None;
        assert_eq!(cfg.nexum.complete(), None);
    }

    #[test]
    fn nexum_settings_round_trip() {
        let dir = std::env::temp_dir().join("eve-router-test-nexum-cfg");
        let _ = fs::remove_dir_all(&dir);
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
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nexum_url_text() {
        assert_eq!(parse_nexum_url(""), Ok(None));
        assert_eq!(parse_nexum_url(" https://nexum.example/ "), Ok(Some("https://nexum.example".into())));
        assert!(parse_nexum_url("ftp://x").unwrap_err().contains("is not a URL"));
    }

    #[test]
    fn hub_switches_default_on() {
        // An old file has no "eve_scout" object. A file can set one switch only.
        assert_eq!(serde_json::from_str::<Config>("{}").unwrap().eve_scout, Hubs { thera: true, turnur: true });
        let cfg: Config = serde_json::from_str(r#"{"eve_scout": {"thera": false}}"#).unwrap();
        assert_eq!(cfg.eve_scout, Hubs { thera: false, turnur: true });
    }
}
