#![expect(
    clippy::ref_option,
    clippy::renamed_function_params,
    clippy::trivially_copy_pass_by_ref,
    clippy::unused_self,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::allow_attributes,
    clippy::assertions_on_result_states,
    clippy::wildcard_imports,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
mod agent_names;
#[cfg(feature = "test-support")]
pub mod clock;
mod content;
mod engine;
mod event;
mod extensions;
mod failure;
mod ids;
mod item;
mod model;
mod reassembly;
mod request;
mod search;
mod secret;
mod settings;
mod source_index;
mod thread;
#[cfg(feature = "tls")]
pub mod tls;
pub mod transport;
mod tunnel;

pub use agent_names::*;
pub use content::*;
pub use engine::*;
pub use event::*;
pub use extensions::*;
pub use failure::*;
pub use ids::*;
pub use item::*;
pub use model::*;
pub use reassembly::*;
pub use request::*;
pub use search::*;
pub use secret::Secret;
pub use settings::*;
pub use source_index::*;
pub use thread::*;
pub use transport::*;
pub use tunnel::*;

pub type Timestamp = i64;

#[must_use]
pub fn now() -> Timestamp {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    #[cfg(feature = "test-support")]
    let wall = wall.saturating_add(clock::offset());
    wall
}

pub const PROTOCOL_VERSION: u32 = 7;

mod collection;
pub use collection::*;
