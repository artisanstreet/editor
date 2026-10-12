//! The vocabulary Forge and the Editor share.
//!
//! Everything here is plain data with serde. Forge owns every value; the
//! Editor holds copies it receives from `GET` endpoints and the event stream
//! and replaces them by id. See `ARCHITECTURE.md` for the protocol.

mod agent_names;
mod engine;
mod event;
mod extensions;
mod ids;
mod item;
mod request;
mod thread;
#[cfg(feature = "tls")]
pub mod tls;

pub use agent_names::*;
pub use engine::*;
pub use event::*;
pub use extensions::*;
pub use ids::*;
pub use item::*;
pub use request::*;
pub use thread::*;

/// Milliseconds since the Unix epoch.
pub type Timestamp = i64;

/// The current time as a [`Timestamp`].
#[must_use]
pub fn now() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The build both sides must match exactly; a mismatch means "update Forge".
pub const PROTOCOL_VERSION: u32 = 1;
