//! EVE SSO and ESI. The login and the token store are here. The front ends hold no SSO code.

pub mod active;
pub mod client;
pub mod pilots;
pub mod sso;
pub mod store;
pub mod tracker;

use oauth2::Scope;

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

/// A character from the SSO token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Character {
    pub id: u64,
    pub name: String,
    pub scopes: Vec<Scope>,
}

impl Character {
    /// The scopes of `SCOPES` that the token does not have. A character with a missing scope
    /// must log in again.
    pub fn missing_scopes(&self) -> Vec<&'static str> {
        SCOPES.iter().copied().filter(|s| !self.scopes.iter().any(|have| have.as_str() == *s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_scopes() {
        let mut character = Character { id: 1, name: "A".into(), scopes: SCOPES.iter().map(|s| Scope::new(s.to_string())).collect() };
        assert!(character.missing_scopes().is_empty());
        character.scopes.retain(|s| s.as_str() != "esi-ui.write_waypoint.v1");
        assert_eq!(character.missing_scopes(), ["esi-ui.write_waypoint.v1"]);
    }
}
