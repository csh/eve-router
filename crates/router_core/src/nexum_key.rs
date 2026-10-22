//! The Nexum API key. The user sees a key one time only, so the router must never lose it.
//!
//! | Version | Where the key lives |
//! |---|---|
//! | 0.4.1 and before | `nexum.key` in `eve-router.json` |
//! | 0.4.2 and later | OS keyring: service `com.smrkn.eve-router.nexum`, user `api-key` |
//!
//! `unlock` runs at the start. It moves a key from the file to the keyring, and it loads the key
//! from the keyring. Without a keyring, a key that the user enters stays in memory and goes at exit.
//! A key that an old file holds stays in the file until a keyring is available, so no save loses it.

use crate::config::{ApiKey, Config};
use crate::esi::store::StoreError;
use std::path::Path;
use std::sync::Mutex;

/// The keyring service name of the key.
pub const KEYRING_SERVICE: &str = "com.smrkn.eve-router.nexum";
const KEYRING_USER: &str = "api-key";

/// A place for the one Nexum key.
pub trait KeyStore: Send + Sync {
    fn get(&self) -> Result<Option<ApiKey>, StoreError>;
    fn set(&self, key: &ApiKey) -> Result<(), StoreError>;
    /// Delete the key. A missing key is not an error.
    fn delete(&self) -> Result<(), StoreError>;
}

/// The OS keyring: Windows Credential Manager, macOS Keychain, or the Secret Service on Linux.
pub struct KeyringKey;

impl KeyringKey {
    /// The keyring, if the platform has one.
    pub fn open() -> Result<KeyringKey, StoreError> {
        keyring::Entry::store_status().as_ref().map(|_| KeyringKey).map_err(|e| StoreError::NoKeyring(e.to_string()))
    }

    fn entry() -> Result<keyring::Entry, StoreError> {
        Ok(keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?)
    }
}

impl KeyStore for KeyringKey {
    fn get(&self) -> Result<Option<ApiKey>, StoreError> {
        match Self::entry()?.get_password() {
            Ok(key) => Ok(Some(ApiKey(key))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, key: &ApiKey) -> Result<(), StoreError> {
        Ok(Self::entry()?.set_password(&key.0)?)
    }

    fn delete(&self) -> Result<(), StoreError> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// A key in memory only, for the tests.
#[derive(Default)]
pub struct MemoryKey(Mutex<Option<ApiKey>>);

impl KeyStore for MemoryKey {
    fn get(&self) -> Result<Option<ApiKey>, StoreError> {
        Ok(self.0.lock().map_err(|_| StoreError::Poisoned)?.clone())
    }

    fn set(&self, key: &ApiKey) -> Result<(), StoreError> {
        *self.0.lock().map_err(|_| StoreError::Poisoned)? = Some(key.clone());
        Ok(())
    }

    fn delete(&self) -> Result<(), StoreError> {
        *self.0.lock().map_err(|_| StoreError::Poisoned)? = None;
        Ok(())
    }
}

/// Make `cfg.nexum.key` right at the start. `store` is the keyring, or the reason why there is none.
/// The result holds the warnings for the user.
///
/// - The file holds a key: copy it to the keyring, then write the config without it. If the keyring
///   fails, the key stays in the file and in memory.
/// - The file holds no key: read the key from the keyring. Without a keyring there is no key.
pub fn unlock(cfg: &mut Config, cfg_path: &Path, store: Result<&dyn KeyStore, &StoreError>) -> Vec<String> {
    let mut warnings = Vec::new();
    match (cfg.nexum.key.clone().filter(|_| cfg.nexum.key_in_file), store) {
        (Some(key), Ok(store)) => match store.set(&key) {
            Ok(()) => {
                cfg.nexum.key_in_file = false;
                if let Err(e) = cfg.save(cfg_path) {
                    // The key is in the keyring. The old file still holds it, and the next save removes it.
                    warnings.push(format!("The Nexum key is in the system keyring, but the config file was not updated: {e}"));
                }
            }
            Err(e) => warnings.push(format!("The Nexum key could not move to the system keyring ({e}). It stays in the config file.")),
        },
        (Some(_), Err(e)) => warnings.push(format!("{e}. The Nexum key stays in the config file.")),
        (None, Ok(store)) => match store.get() {
            Ok(key) => cfg.nexum.key = key,
            Err(e) => warnings.push(format!("The Nexum key could not be read from the system keyring: {e}")),
        },
        // No key in the file, and no keyring: nothing to load.
        (None, Err(_)) => {}
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_migrate;

    fn key() -> ApiKey {
        ApiKey("nxm_secret".into())
    }

    /// A store that fails each call.
    struct Broken;

    impl KeyStore for Broken {
        fn get(&self) -> Result<Option<ApiKey>, StoreError> {
            Err(StoreError::NoKeyring("broken".into()))
        }
        fn set(&self, _: &ApiKey) -> Result<(), StoreError> {
            Err(StoreError::NoKeyring("broken".into()))
        }
        fn delete(&self) -> Result<(), StoreError> {
            Err(StoreError::NoKeyring("broken".into()))
        }
    }

    fn old_file(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("eve-router.json");
        std::fs::write(&path, include_str!("../tests/fixtures/config-v0.4.1.json")).unwrap();
        path
    }

    #[test]
    fn a_file_key_moves_to_the_keyring_and_leaves_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = old_file(tmp.path());
        let mut cfg = Config::load(&path).unwrap();
        assert!(cfg.nexum.key_in_file);
        let store = MemoryKey::default();
        assert!(unlock(&mut cfg, &path, Ok(&store)).is_empty());
        assert_eq!(store.get().unwrap(), Some(ApiKey("nxm_key".into())));
        // The key is in memory, and the file no longer holds it.
        assert_eq!(cfg.nexum.key, Some(ApiKey("nxm_key".into())));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("nxm_key"));
        // The next start finds the key in the keyring, with no key in the file.
        let mut again = Config::load(&path).unwrap();
        assert_eq!(again.nexum.key, None);
        assert!(unlock(&mut again, &path, Ok(&store)).is_empty());
        assert_eq!(again.nexum.key, Some(ApiKey("nxm_key".into())));
        assert_eq!(again.nexum.url.as_deref(), Some("https://nexum.example"));
    }

    /// A broken keyring must not lose the key of an old file, also not after a later save.
    #[test]
    fn a_failed_move_keeps_the_key_in_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = old_file(tmp.path());
        let mut cfg = Config::load(&path).unwrap();
        let warnings = unlock(&mut cfg, &path, Ok(&Broken));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(cfg.nexum.key_in_file && cfg.nexum.key.is_some());
        cfg.compact = true;
        cfg.save(&path).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("nxm_key"));
        // The same with no keyring at all.
        let missing = StoreError::NoKeyring("none".into());
        assert_eq!(unlock(&mut cfg, &path, Err(&missing)).len(), 1);
        assert_eq!(Config::load(&path).unwrap().nexum.key, Some(ApiKey("nxm_key".into())));
    }

    /// A key that the user enters later never reaches the file.
    #[test]
    fn a_new_key_stays_out_of_the_file() {
        let mut cfg = Config::default();
        cfg.nexum.key = Some(key());
        assert!(!serde_json::to_string(&cfg).unwrap().contains("nxm_secret"));
        cfg.nexum.key_in_file = true;
        assert!(serde_json::to_string(&cfg).unwrap().contains("nxm_secret"));
    }

    #[test]
    fn without_a_keyring_and_a_file_key_there_is_no_key_and_no_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        let missing = StoreError::NoKeyring("none".into());
        assert!(unlock(&mut cfg, &tmp.path().join("eve-router.json"), Err(&missing)).is_empty());
        assert_eq!(cfg.nexum.key, None);
    }

    /// A file of 0.4.0 holds the key too, so the chain gives it to the move step.
    #[test]
    fn a_key_of_0_4_0_reaches_the_keyring() {
        let mut cfg = config_migrate::parse(include_str!("../tests/fixtures/config-v0.4.0.json")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryKey::default();
        assert!(unlock(&mut cfg, &tmp.path().join("eve-router.json"), Ok(&store)).is_empty());
        assert_eq!(store.get().unwrap(), Some(ApiKey("nxm_key".into())));
    }

    /// Uses the real OS keyring, so it is not in the default run.
    /// Run: `cargo test -p router_core nexum_keyring_round_trip -- --ignored`
    #[test]
    #[ignore = "uses the real OS keyring"]
    fn nexum_keyring_round_trip() {
        let keyring = KeyringKey::open().unwrap();
        let backup = keyring.get().unwrap();
        keyring.set(&key()).unwrap();
        assert_eq!(keyring.get().unwrap(), Some(key()));
        keyring.delete().unwrap();
        assert_eq!(keyring.get().unwrap(), None);
        if let Some(old) = backup {
            keyring.set(&old).unwrap();
        }
    }
}
