//! The token store.
//!
//! | Item | Where |
//! |---|---|
//! | Refresh token | OS keyring: service `com.smrkn.eve-router`, user = character ID |
//! | Access token | Memory only. It expires after 20 minutes |
//! | Character list (ID, name, scopes, last use) | `characters.json` next to `eve-router.json` |
//!
//! Without a keyring, the front end can offer `Accounts::session_only`. The tokens then stay in
//! memory and go at exit. The router never writes a token to a file.

use super::sso::Tokens;
use super::{Character, Secret};
use crate::config::APP_DIR;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The keyring service name of each refresh token.
pub const KEYRING_SERVICE: &str = APP_DIR;
const FILE_NAME: &str = "characters.json";

/// The character list file, next to the config file.
pub fn characters_path(cfg_path: &Path) -> PathBuf {
    cfg_path.with_file_name(FILE_NAME)
}

/// A place for refresh tokens, by character ID.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: u64) -> Result<Option<Secret>, String>;
    fn set(&self, id: u64, secret: &Secret) -> Result<(), String>;
    /// Delete the token. A missing token is not an error.
    fn delete(&self, id: u64) -> Result<(), String>;
}

/// The OS keyring: Windows Credential Manager, macOS Keychain, or the Secret Service on Linux.
pub struct Keyring;

impl Keyring {
    /// The keyring, if the platform has one. The error text says why not, for the
    /// "Session only" prompt.
    pub fn open() -> Result<Keyring, String> {
        keyring::Entry::store_status().as_ref().map(|_| Keyring).map_err(|e| format!("No system keyring: {e}"))
    }

    fn entry(id: u64) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, &id.to_string()).map_err(|e| e.to_string())
    }
}

impl SecretStore for Keyring {
    fn get(&self, id: u64) -> Result<Option<Secret>, String> {
        match Self::entry(id)?.get_password() {
            Ok(token) => Ok(Some(Secret::new(token))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, id: u64, secret: &Secret) -> Result<(), String> {
        Self::entry(id)?.set_password(secret.expose()).map_err(|e| e.to_string())
    }

    fn delete(&self, id: u64) -> Result<(), String> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Tokens in memory only, for "Session only" and for the tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<u64, Secret>>);

impl SecretStore for MemoryStore {
    fn get(&self, id: u64) -> Result<Option<Secret>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.get(&id).cloned())
    }

    fn set(&self, id: u64, secret: &Secret) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.insert(id, secret.clone());
        Ok(())
    }

    fn delete(&self, id: u64) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(&id);
        Ok(())
    }
}

/// One row of `characters.json`. It holds no token.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CharacterEntry {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The last start of a route with this character, in Unix seconds.
    #[serde(default)]
    pub last_used: Option<u64>,
}

impl CharacterEntry {
    pub fn character(&self) -> Character {
        Character { id: self.id, name: self.name.clone(), scopes: self.scopes.clone() }
    }
}

/// The logged-in characters and their refresh tokens.
pub struct Accounts {
    pub characters: Vec<CharacterEntry>,
    /// `None` for "Session only": the list is not written.
    path: Option<PathBuf>,
    secrets: Box<dyn SecretStore>,
}

impl Accounts {
    /// The OS keyring and `characters.json`. An error means no keyring, or a bad list file.
    pub fn open(cfg_path: &Path) -> Result<Accounts, String> {
        Self::with_store(Some(characters_path(cfg_path)), Box::new(Keyring::open()?))
    }

    /// Tokens in memory only. Nothing goes to disk.
    pub fn session_only() -> Accounts {
        Accounts { characters: Vec::new(), path: None, secrets: Box::new(MemoryStore::default()) }
    }

    /// Read the list file, if `path` is set. A missing file gives an empty list.
    pub fn with_store(path: Option<PathBuf>, secrets: Box<dyn SecretStore>) -> Result<Accounts, String> {
        let characters = match &path {
            Some(path) => load(path)?,
            None => Vec::new(),
        };
        Ok(Accounts { characters, path, secrets })
    }

    pub fn is_session_only(&self) -> bool {
        self.path.is_none()
    }

    /// Store the tokens of a login or a refresh. The refresh token goes to the store before the
    /// list changes, because SSO can rotate it: the old token can be invalid after this call.
    pub fn store(&mut self, tokens: &Tokens) -> Result<(), String> {
        let c = &tokens.character;
        self.secrets.set(c.id, &tokens.refresh)?;
        match self.characters.iter_mut().find(|e| e.id == c.id) {
            Some(entry) => {
                entry.name.clone_from(&c.name);
                entry.scopes.clone_from(&c.scopes);
            }
            None => self.characters.push(CharacterEntry { id: c.id, name: c.name.clone(), scopes: c.scopes.clone(), last_used: None }),
        }
        self.save()
    }

    /// The refresh token of a character. `None` means the character must log in again.
    pub fn refresh_token(&self, id: u64) -> Result<Option<Secret>, String> {
        self.secrets.get(id)
    }

    /// Delete the refresh token and the list entry. Revoke the token at SSO before this call,
    /// with `sso::revoke`.
    pub fn remove(&mut self, id: u64) -> Result<(), String> {
        self.secrets.delete(id)?;
        self.characters.retain(|e| e.id != id);
        self.save()
    }

    /// Record a route start with this character, for the preselection in the picker.
    pub fn mark_used(&mut self, id: u64, now: u64) -> Result<(), String> {
        if let Some(entry) = self.characters.iter_mut().find(|e| e.id == id) {
            entry.last_used = Some(now);
        }
        self.save()
    }

    /// The character of the last route start.
    pub fn last_used(&self) -> Option<u64> {
        self.characters.iter().filter(|e| e.last_used.is_some()).max_by_key(|e| e.last_used).map(|e| e.id)
    }

    fn save(&self) -> Result<(), String> {
        match &self.path {
            Some(path) => save(path, &self.characters),
            None => Ok(()),
        }
    }
}

fn load(path: &Path) -> Result<Vec<CharacterEntry>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write a temporary file, then rename it, so a reader never sees half a file.
fn save(path: &Path, characters: &[CharacterEntry]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.part");
    let text = serde_json::to_string_pretty(characters).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(id: u64, name: &str, refresh: &str) -> Tokens {
        Tokens {
            character: Character { id, name: name.into(), scopes: vec!["esi-ui.write_waypoint.v1".into()] },
            access: Secret::new("access"),
            refresh: Secret::new(refresh),
            expires_at: 0,
        }
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn store_refresh_and_remove() {
        let path = dir("eve-router-test-accounts").join("characters.json");
        let mut accounts = Accounts::with_store(Some(path.clone()), Box::new(MemoryStore::default())).unwrap();
        accounts.store(&tokens(1, "Alice", "r1")).unwrap();
        accounts.store(&tokens(2, "Bob", "r2")).unwrap();
        // A refresh rotates the token and keeps one entry.
        accounts.store(&tokens(1, "Alice Ander", "r1b")).unwrap();
        assert_eq!(accounts.refresh_token(1).unwrap(), Some(Secret::new("r1b")));
        assert_eq!(accounts.characters.len(), 2);

        // The file holds the list, with no token.
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Alice Ander") && !text.contains("r1b") && !text.contains("access"), "{text}");
        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded, accounts.characters);

        accounts.remove(1).unwrap();
        assert_eq!(accounts.refresh_token(1).unwrap(), None);
        assert_eq!(load(&path).unwrap().iter().map(|e| e.id).collect::<Vec<_>>(), [2]);
        // A second remove is not an error.
        accounts.remove(1).unwrap();
    }

    #[test]
    fn last_used_character() {
        let mut accounts = Accounts::session_only();
        accounts.store(&tokens(1, "Alice", "r1")).unwrap();
        accounts.store(&tokens(2, "Bob", "r2")).unwrap();
        assert_eq!(accounts.last_used(), None);
        accounts.mark_used(2, 100).unwrap();
        accounts.mark_used(1, 200).unwrap();
        assert_eq!(accounts.last_used(), Some(1));
    }

    #[test]
    fn session_only_writes_nothing() {
        let accounts = Accounts::session_only();
        assert!(accounts.is_session_only());
        assert!(accounts.characters.is_empty());
    }

    #[test]
    fn missing_file_is_an_empty_list_and_a_bad_file_is_an_error() {
        let dir = dir("eve-router-test-accounts-bad");
        assert_eq!(load(&dir.join("characters.json")).unwrap(), []);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("characters.json"), "not json").unwrap();
        assert!(load(&dir.join("characters.json")).is_err());
    }

    #[test]
    fn old_file_without_optional_fields() {
        let entries: Vec<CharacterEntry> = serde_json::from_str(r#"[{"id": 5, "name": "Cid"}]"#).unwrap();
        assert_eq!(entries[0], CharacterEntry { id: 5, name: "Cid".into(), scopes: vec![], last_used: None });
    }

    /// Uses the real OS keyring, so it is not in the default run.
    /// Run: `cargo test -p router_core keyring_round_trip -- --ignored`
    #[test]
    #[ignore]
    fn keyring_round_trip() {
        let keyring = Keyring::open().unwrap();
        let id = 1;
        keyring.set(id, &Secret::new("test-token")).unwrap();
        assert_eq!(keyring.get(id).unwrap(), Some(Secret::new("test-token")));
        keyring.delete(id).unwrap();
        assert_eq!(keyring.get(id).unwrap(), None);
    }
}
