//! The route logic of EVE Router: the SDE, the map graph, the overlays and the route search.
//! The user interfaces are separate crates.

pub mod ansiblex;
pub mod config;
pub mod config_migrate;
pub mod error;
pub mod esi;
pub mod labels;
pub mod life_scale;
pub mod log;
pub mod nexum_key;
pub mod overlay;
pub mod refresh;
pub mod route;
pub mod routing_overlay;
pub mod sde;
pub mod sde_update;
pub mod settings;
pub mod ships;
pub mod sources;
pub mod startup;
/// Fixtures and fakes for tests. The feature `test-support` turns it on for other crates. No
/// other code in this crate depends on the feature.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod universe;
pub mod wormhole;
pub mod wormhole_space;
pub mod wormhole_types;
