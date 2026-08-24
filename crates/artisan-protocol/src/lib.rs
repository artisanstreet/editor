//! Shared envelopes, commands, events, queries, and validation ported from `modules/protocol`.
//!
//! Canonical wire shapes live in `schema/*.capnp`; the `*_capnp` modules are
//! machine-generated bindings committed from those schemas.
//!
//! Regenerate with (repository root, after `cargo install capnpc`):
//!
//! ```text
//! capnp compile -o rust -I crates/artisan-protocol/schema \
//!   --src-prefix crates/artisan-protocol/schema \
//!   crates/artisan-protocol/schema/common.capnp \
//!   crates/artisan-protocol/schema/stream.capnp \
//!   crates/artisan-protocol/schema/handshake.capnp
//! ```
//!
//! then move the emitted `*_capnp.rs` files here and refresh the schema
//! digests in `.tests/protocol/generated/schema-manifest.json` in the same
//! commit.

#[allow(clippy::all, clippy::pedantic)] // machine-generated capnpc-rust output is lint-frozen upstream
pub mod common_capnp;
#[allow(clippy::all, clippy::pedantic)] // machine-generated capnpc-rust output is lint-frozen upstream
pub mod handshake_capnp;
#[allow(clippy::all, clippy::pedantic)] // machine-generated capnpc-rust output is lint-frozen upstream
pub mod stream_capnp;
