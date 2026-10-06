//! EVE SSO and ESI. The login and the token store are here. The front ends hold no SSO code.

pub mod sso;
pub mod store;

use std::fmt;

/// The scopes that the router requests at each login.
pub const SCOPES: [&str; 4] =
    ["esi-ui.write_waypoint.v1", "esi-location.read_location.v1", "esi-location.read_ship_type.v1", "esi-location.read_online.v1"];

/// The env var that gives the client ID of the EVE developer app.
pub const CLIENT_ID_VAR: &str = "EVE_ROUTER_CLIENT_ID";

/// The client ID of the EVE developer app. The env var at run time wins over the value at build
/// time. The repository holds no client ID. Without one, the router cannot log in.
pub fn client_id() -> Option<String> {
    let runtime = std::env::var(CLIENT_ID_VAR).ok();
    runtime.or_else(|| option_env!("EVE_ROUTER_CLIENT_ID").map(str::to_owned)).map(|id| id.trim().to_owned()).filter(|id| !id.is_empty())
}

/// An access token or a refresh token. `Debug` shows no part of the token, and there is no
/// `Display`, so a token cannot go into a log or a status line by mistake.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The token text, for a request header or the keyring only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// A character from the SSO token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Character {
    pub id: u64,
    pub name: String,
    pub scopes: Vec<String>,
}

impl Character {
    /// The scopes of `SCOPES` that the token does not have. A character with a missing scope
    /// must log in again.
    pub fn missing_scopes(&self) -> Vec<&'static str> {
        SCOPES.iter().copied().filter(|s| !self.scopes.iter().any(|have| have == s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_hides_the_token() {
        let secret = Secret::new("abcdefghijklmnop");
        assert_eq!(format!("{secret:?}"), "Secret(…)");
        assert_eq!(secret.expose(), "abcdefghijklmnop");
    }

    #[test]
    fn missing_scopes() {
        let mut character = Character { id: 1, name: "A".into(), scopes: SCOPES.iter().map(|s| s.to_string()).collect() };
        assert!(character.missing_scopes().is_empty());
        character.scopes.retain(|s| s != "esi-ui.write_waypoint.v1");
        assert_eq!(character.missing_scopes(), ["esi-ui.write_waypoint.v1"]);
    }
}
