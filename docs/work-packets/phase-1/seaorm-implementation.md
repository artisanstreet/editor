# Phase 1 implementation packet: SeaORM SQLite proof

You are a bounded implementation worker in an isolated Git worktree. Do not spawn subagents.

Read `docs/PLAN.md` completely and then read the completed reconnaissance report at `%TEMP%\artisan-editor-opencode-phase1-current\seaorm-tests.report.md`. Treat the report as evidence to verify, not as authority. Check relevant APIs against installed crate sources or primary upstream sources before writing code.

## Objective

Implement one reviewable SeaORM/SQLite feasibility proof. A root-level external Bazel test must open a real in-memory SQLite database, exercise a SeaORM-derived test entity, create its table, insert and query data, prove transaction rollback, and close cleanly. This packet must not choose the native product schema or introduce the migrations crate. The controller will integrate root dependency versions, lockfiles, aggregate targets, and exact Crate Universe labels.

## Exclusive file ownership

You may create or edit only:

- `modules/database/**`
- `tests/database/**`
- this packet file

Do not edit root manifests or lockfiles, `MODULE.bazel`, other modules, `.github`, `docs/PLAN.md`, or `modules/migrations`.

## Requirements

- No `anyhow` in first-party source or manifests and no dependency-policy exception. Use `thiserror` and preserve useful SeaORM sources.
- Use SeaORM `2.0.2` with default features disabled and only the verified SQLite, Tokio, macro, and SQLite-returning features needed by the proof.
- Keep the database crate boundary small and production-credible: explicit SQLite connection configuration and typed connection errors. Do not add repository traits, an actor framework, product entities, import behavior, or migration policy.
- The feasibility entity and its table name belong to `tests/database/`, not the product database API.
- Configure in-memory SQLite deterministically so connection-pool behavior cannot create separate accidental databases. Disable noisy SQLx logging in the test setup.
- Exercise actual SeaORM derive macros and APIs rather than raw SQL alone. Verify insert/query behavior and that a rolled-back transaction leaves no committed row.
- The external test lives under `tests/database/`, uses the normal Rust test harness with `#[tokio::test]`, and is a focused Bazel `rust_test` target. Keep the test bounded and independent of files or external services.
- All first-party Rust targets use edition 2024 and the root lint config. Proc-macro dependencies must be explicit where `rules_rust` requires them.
- Do not add `sea-orm-migration` in this proof. Migration history and product schema are later, separate stack packets.
- Use `apply_patch` for hand-written edits. Run formatting and any focused checks the current graph permits.
- Review `git diff --check` and the complete diff, then commit once with a focused message. Do not push and do not create or merge a PR.

Report the commit SHA, files changed, commands run, remaining controller integration, and any verified Windows/SQLite/Bazel limitation.

## Implementation record (2026-08-24)

Completed within ownership. Evidence summary for the controller:

- **Files:** `modules/database/src/lib.rs`, `modules/database/Cargo.toml`, `modules/database/BUILD.bazel`, `tests/database/seaorm_sqlite.rs` (entity + proof live here), `tests/database/BUILD.bazel`.
- **Dependency posture:** `sea-orm =2.0.2`, `default-features = false`, features `sqlx-sqlite`, `runtime-tokio`, `macros`, `sqlite-use-returning-for-3_35`; `thiserror.workspace = true` retained; `tokio =1.53.1` (`rt`, `macros`) declared as a dev-dependency of `artisan-database` so the resolved graph can produce `@crates//:tokio`. No `sea-orm-migration`, no `anyhow`.
- **API verification:** every used API was checked against the installed `sea-orm-2.0.2` registry source (`ConnectOptions::min_connections/max_connections/sqlx_logging` defaults with `sqlx_logging: true`, `Database::connect<C: Into<ConnectOptions>>`, `ConnectionTrait::execute<S: StatementBuilder>(&S)` incl. the `TableCreateStatement` impl, `TransactionTrait::begin`/`DatabaseTransaction::rollback(self)`, `DatabaseConnection::close(self)`, re-exported `sea_query`) and against fetched `sea-query 1.0.2` sources (`ColumnDef::{integer,string,boolean,not_null,auto_increment,primary_key}`). SeaORM 2 note discovered by compiling: `ActiveModelTrait::default()` and `Default::default()` both exist on generated `ActiveModel`s; upstream idiom `..ActiveModelTrait::default()` is required to avoid E0034.
- **Runtime verification (scratch workspace outside the repo, real bundled SQLite via libsqlite3-sys 0.37.0 / sqlx 0.9.0):** `cargo test` — 5/5 pass; `cargo clippy --all-targets` clean under deny(all)+deny(pedantic); `cargo fmt --check` clean; repo files are byte-identical to the verified scratch sources; resolved scratch lock contains no `anyhow`.
- **Controller integration:** Exact workspace pins, Cargo and Crate Universe lockfiles, root aggregate targets, and the missing Bazel `thiserror` edge are integrated. `bazel build //...` succeeds and `bazel test //...` passes all five root test targets, including the SeaORM proof; a clean repeat completes from cache in under one second for each command.
- **Bazel-specific finding:** Crate Universe 0.73.0 resolves SQLx 0.9's target-specific `sqlx-core/offline` edge on Windows without automatically enabling the paired `sqlx-sqlite/offline` implementation. The database manifest declares that exact driver feature directly so Cargo resolves its required serde edge and Bazel compiles a coherent `Executor` implementation. Re-audit this narrow alignment dependency when rules_rust or SQLx changes.
- **Limitations observed:** The proof uses bundled SQLite and the native MSVC C toolchain successfully. No direct proc-macro dependency is needed in either first-party Bazel target; the generated Crate Universe graph carries SeaORM, thiserror, and Tokio proc-macro edges.
