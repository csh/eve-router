//! The `eve-router.json` file. CLI flags override its values.

use crate::route::Mode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The config directory name inside the platform config directory.
const APP_DIR: &str = "com.smrkn.eve-router";
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
/// The default nexum map export, next to the config file.
pub const NEXUM_FILE: &str = "nexum.json";

/// The overlay file to load: the CLI flag if given, else the default file next to the
/// config file. A missing default file is not an error. The router then loads no overlay.
pub fn overlay_path(flag: Option<PathBuf>, cfg_path: &Path, default_name: &str) -> Option<PathBuf> {
    flag.or_else(|| {
        let path = cfg_path.with_file_name(default_name);
        path.is_file().then_some(path)
    })
}

/// The main trade hubs.
pub const DEFAULT_FAVOURITES: [&str; 5] = ["Jita", "Amarr", "Dodixie", "Hek", "Rens"];

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// The alliance capital system. Without it, jump bridges are off.
    pub capital: Option<String>,
    /// A ship name ("Sin") or a ship group ("Black Ops" or `black-ops`).
    pub hull: Option<String>,
    /// The maximum capacitor (TJ) that one bridge jump can use.
    pub max_cap_tj: Option<f32>,
    /// The sidebar destinations. `None` gives `DEFAULT_FAVOURITES`.
    pub favourites: Option<Vec<String>>,
    pub mode: Option<Mode>,
    pub top: Option<usize>,
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
        fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
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
}
