//! The route logic of EVE Router: the SDE, the map graph, the overlays and the route search.
//! The user interfaces are separate crates.

pub mod ansiblex;
pub mod config;
pub mod esi;
pub mod labels;
pub mod overlay;
pub mod route;
pub mod sde;
pub mod sde_update;
pub mod settings;
pub mod ships;
pub mod sources;
pub mod startup;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod universe;
pub mod wormhole;
pub mod wormhole_types;
