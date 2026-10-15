//! The vocabulary Forge and the Editor share.
//!
//! Everything here is plain data with serde. Forge owns every value; the
//! Editor holds copies it receives from `GET` endpoints and the event stream
//! and replaces them by id. See `ARCHITECTURE.md` for the protocol.

mod agent_names;
mod content;
mod source_index;
mod engine;
mod event;
mod extensions;
mod ids;
mod item;
mod item_text;
mod mentions;
mod request;
mod thread;
#[cfg(feature = "tls")]
pub mod tls;

pub use agent_names::*;
pub use content::*;
pub use source_index::*;
pub use engine::*;
pub use event::*;
pub use extensions::*;
pub use ids::*;
pub use item::*;
pub use mentions::{
    MAX_SKILL_NAME_BYTES, Mention, MentionNames, named_mentions, skill_mentions, valid_mention_name,
};
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

/// The wire-schema version both endpoints must support. Build hashes may differ.
pub const PROTOCOL_VERSION: u32 = 4;

mod collection;
pub use collection::*;
