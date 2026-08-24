//! `SQLite` persistence for the native Forge: migration replay, journal, projections.
//!
//! Store decision (2026-08-24, greenfield revision): implemented with `sqlx`
//! (`SQLite` feature, Tokio runtime) rather than raw `rusqlite` so repository SQL is
//! compile-time verified while Rust SQL and schema evolve together.
//!
//! Constraints that remain binding regardless of driver:
//! - Drizzle-authored migration SQL under `modules/backend/drizzle` is the canonical
//!   schema definition; the runner replays those files verbatim.
//! - A single writer actor owns the connection; journal ordering and atomic
//!   projection stay obvious, and pooling is never used for writes.
//! - Transaction scope, statement order, and connection ownership stay explicit
//!   inside the actor; domain code sees only typed command enums.
//! - Static repository SQL uses compile-time checked macros against prepared
//!   offline data; dynamic or replayed migration SQL uses the runtime API by necessity.
//! - The bundled engine version is pinned deliberately so pragma behavior matches
//!   the TypeScript implementation at the behavioral level.
