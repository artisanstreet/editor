//! `SQLite` persistence boundary owned by Forge.
//!
//! Forge is the only production process that opens this database. The crate
//! owns the connection policy and, as the native schema lands, its entities,
//! repositories, and transaction boundaries.

mod connection;
pub mod entities;

pub use artisan_domain::WorkspaceId;
pub use connection::{ConnectError, SqliteConfig, connect};
