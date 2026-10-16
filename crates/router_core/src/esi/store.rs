//! The token store.
//!
//! | Item | Where |
//! |---|---|
//! | Refresh token | OS keyring: service `com.smrkn.eve-router`, user = character ID |
//! | Access token | Memory only. It expires after 20 minutes |
//! | Character list (ID, name, scopes, last use) | `characters.json` next to `eve-router.json` |
//!
//! Without a keyring, the GUI can offer `Accounts::session_only`. The tokens then stay in
//! memory and go at exit. The router never writes a token to a file.

use super::Character;
use super::sso::Tokens;
use crate::config::APP_DIR;
use crate::error::{FileError, write_atomic};
use oauth2::{RefreshToken, Scope};
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

/// Why the router cannot read or write a refresh token or the character list.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The platform has no keyring.
    #[error("No system keyring: {0}")]
    NoKeyring(String),
    #[error("{0}")]
    Keyring(#[from] keyring::Error),
    #[error("The token store is locked by a failed thread")]
    Poisoned,
    #[error(transparent)]
    File(#[from] FileError),
}

/// A place for refresh tokens, by character ID.
pub trait TokenStore: Send + Sync {
    fn get(&self, id: u64) -> Result<Option<RefreshToken>, StoreError>;
    fn set(&self, id: u64, token: &RefreshToken) -> Result<(), StoreError>;
    /// Delete the token. A missing token is not an error.
    fn delete(&self, id: u64) -> Result<(), StoreError>;
}

/// The OS keyring: Windows Credential Manager, macOS Keychain, or the Secret Service on Linux.
pub struct Keyring;

impl Keyring {
    /// The keyring, if the platform has one. The error text says why not, for the
    /// "Session only" prompt.
    pub fn open() -> Result<Keyring, StoreError> {
        keyring::Entry::store_status().as_ref().map(|_| Keyring).map_err(|e| StoreError::NoKeyring(e.to_string()))
    }

    fn entry(id: u64) -> Result<keyring::Entry, StoreError> {
        Ok(keyring::Entry::new(KEYRING_SERVICE, &id.to_string())?)
    }
}

impl TokenStore for Keyring {
    fn get(&self, id: u64) -> Result<Option<RefreshToken>, StoreError> {
        match Self::entry(id)?.get_password() {
            Ok(token) => Ok(Some(RefreshToken::new(token))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, id: u64, token: &RefreshToken) -> Result<(), StoreError> {
        Ok(Self::entry(id)?.set_password(token.secret())?)
    }

    fn delete(&self, id: u64) -> Result<(), StoreError> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Tokens in memory only, for "Session only" and for the tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<u64, RefreshToken>>);

impl TokenStore for MemoryStore {
    fn get(&self, id: u64) -> Result<Option<RefreshToken>, StoreError> {
        Ok(self.0.lock().map_err(|_| StoreError::Poisoned)?.get(&id).cloned())
    }

    fn set(&self, id: u64, token: &RefreshToken) -> Result<(), StoreError> {
        self.0.lock().map_err(|_| StoreError::Poisoned)?.insert(id, token.clone());
        Ok(())
    }

    fn delete(&self, id: u64) -> Result<(), StoreError> {
        self.0.lock().map_err(|_| StoreError::Poisoned)?.remove(&id);
        Ok(())
    }
}

/// One row of `characters.json`. It holds no token.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CharacterEntry {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<Scope>,
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
    secrets: Box<dyn TokenStore>,
}

impl Accounts {
    /// Tokens in memory only. Nothing goes to disk.
    pub fn session_only() -> Accounts {
        Accounts { characters: Vec::new(), path: None, secrets: Box::new(MemoryStore::default()) }
    }

    /// Read the list file, if `path` is set. A missing file gives an empty list.
    pub fn with_store(path: Option<PathBuf>, secrets: Box<dyn TokenStore>) -> Result<Accounts, StoreError> {
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
    pub fn store(&mut self, tokens: &Tokens) -> Result<(), StoreError> {
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
    pub fn refresh_token(&self, id: u64) -> Result<Option<RefreshToken>, StoreError> {
        self.secrets.get(id)
    }

    /// Delete the refresh token and the list entry. Revoke the token at SSO before this call,
    /// with `sso::revoke`.
    pub fn remove(&mut self, id: u64) -> Result<(), StoreError> {
        self.secrets.delete(id)?;
        self.characters.retain(|e| e.id != id);
        self.save()
    }

    /// Record a route start with this character, for the preselection in the picker.
    pub fn mark_used(&mut self, id: u64, now: u64) -> Result<(), StoreError> {
        if let Some(entry) = self.characters.iter_mut().find(|e| e.id == id) {
            entry.last_used = Some(now);
        }
        self.save()
    }

    /// The character of the last route start.
    pub fn last_used(&self) -> Option<u64> {
        self.characters.iter().filter(|e| e.last_used.is_some()).max_by_key(|e| e.last_used).map(|e| e.id)
    }

    fn save(&self) -> Result<(), StoreError> {
        match &self.path {
            Some(path) => Ok(save(path, &self.characters)?),
            None => Ok(()),
        }
    }
}

fn load(path: &Path) -> Result<Vec<CharacterEntry>, FileError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(FileError::json(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(FileError::Io { path: path.to_path_buf(), source: e }),
    }
}

/// Write a temporary file, then rename it, so a reader never sees half a file.
fn save(path: &Path, characters: &[CharacterEntry]) -> Result<(), FileError> {
    let text = serde_json::to_string_pretty(characters).map_err(FileError::json(path))?;
    write_atomic(path, &path.with_extension("json.part"), &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(id: u64, name: &str, refresh: &str) -> Tokens {
        Tokens {
            character: Character { id, name: name.into(), scopes: vec![Scope::new("esi-ui.write_waypoint.v1".into())] },
            access: oauth2::AccessToken::new("access".into()),
            refresh: RefreshToken::new(refresh.into()),
            expires_at: 0,
        }
    }

    /// The token text. `RefreshToken` has no `PartialEq`.
    fn secret(result: Result<Option<RefreshToken>, StoreError>) -> Option<String> {
        result.unwrap().map(|t| t.secret().clone())
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn store_refresh_and_remove() {
        let tmp = dir();
        let path = tmp.path().join("characters.json");
        let mut accounts = Accounts::with_store(Some(path.clone()), Box::new(MemoryStore::default())).unwrap();
        accounts.store(&tokens(1, "Alice", "r1")).unwrap();
        accounts.store(&tokens(2, "Bob", "r2")).unwrap();
        // A refresh rotates the token and keeps one entry.
        accounts.store(&tokens(1, "Alice Ander", "r1b")).unwrap();
        assert_eq!(secret(accounts.refresh_token(1)).as_deref(), Some("r1b"));
        assert_eq!(accounts.characters.len(), 2);

        // The file holds the list, with no token.
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Alice Ander") && !text.contains("r1b") && !text.contains("access"), "{text}");
        // A `Scope` is a plain string in the file.
        assert!(
            text.contains(
                r#""scopes": [
      "esi-ui.write_waypoint.v1"
    ]"#
            ),
            "{text}"
        );
        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded, accounts.characters);

        accounts.remove(1).unwrap();
        assert_eq!(secret(accounts.refresh_token(1)), None);
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
        let tmp = dir();
        let dir = tmp.path().to_path_buf();
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
        keyring.set(id, &RefreshToken::new("test-token".into())).unwrap();
        assert_eq!(secret(keyring.get(id)).as_deref(), Some("test-token"));
        keyring.delete(id).unwrap();
        assert_eq!(secret(keyring.get(id)), None);
    }
}
