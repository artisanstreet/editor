# Phase 1 implementation packet: hermetic Cap'n Proto proof

You are a bounded implementation worker in an isolated Git worktree. Do not spawn subagents.

Read `docs/PLAN.md` completely and then read the reconnaissance report at `%TEMP%\artisan-editor-opencode-phase1-current\quic-capnp.report.md`. Treat the report as input, not authority: verify relevant APIs against installed crate sources or primary upstream sources.

## Objective

Implement the Phase 1 Cap'n Proto feasibility proof so Bazel owns schema generation as an explicit action and a root-level external Rust test exercises the generated bindings. The controller will integrate root dependencies, `MODULE.bazel`, workspace membership, lockfiles, and final target labels.

## Exclusive file ownership

You may create or edit only:

- `modules/protocol/**`
- `scripts/capnp_codegen/**`
- `tests/protocol/**`
- this packet file

Do not edit root files, `MODULE.bazel`, lockfiles, other modules, `.github`, or `docs/PLAN.md`.

## Requirements

- No `anyhow` in first-party source or manifests. Use typed errors and `thiserror` where an error type is needed.
- Use Cap'n Proto Rust `0.27.0` APIs.
- Put project-specific executable tooling under `scripts/`, not `.scripts` or `tools/`.
- Do not use a Cargo `build.rs` for schema generation.
- Add a minimal proof schema, a deterministic code-generation action/rule or wrapper with all schemas/tools/outputs explicit, and an external `rust_test` under `tests/protocol/` that serializes and deserializes a real generated type.
- Prefer Bazel action outputs as compilation inputs. If checked-in generated Rust is necessary for rust-analyzer, add an explicit Bazel freshness verifier; do not silently let Cargo and Bazel compile different sources.
- Avoid shell-dependent path handling. The proof must be viable on Windows/MSVC.
- Keep product API decisions out of the proof; clearly name fixtures as Phase 1 feasibility code.
- All Rust targets must use edition 2024 and the root lint config where appropriate. Generated code may need narrowly scoped lint allowances.
- External tests use the normal Rust test harness and `#[test]`.
- Use `apply_patch` for hand-written edits. Run formatting and any focused Bazel checks the current graph permits.
- Review `git diff --check` and your complete diff, then commit once with a focused message. Do not push and do not create or merge a PR.

Report the commit SHA, files changed, commands run, remaining controller integration, and any verified Windows/codegen limitation.

## Implementation record (2026-08-24)

Completed and integrated by the controller. Evidence:

- `capnp` and `capnpc` are pinned to `0.27.0`. The compiler is the official Cap'n Proto 1.5.0 Windows archive, fetched by Bazel with SHA-256 `21501909dc051347563d4e83394eb848204f558e959501c8061dbeaaf0651988`.
- `//scripts/capnp_codegen:capnpc_rust` wraps the upstream Rust generator. `//modules/protocol:phase1_proof_codegen` declares the schema, compiler, plugin, and generated Rust output as one shell-free `CapnpRustCodegen` action. No Cargo `build.rs`, PATH lookup, or checked-in generated binding is used.
- `//modules/protocol:protocol` consumes the generated action output directly. `//tests/protocol:phase1_proof_test` exercises a generated type from outside the crate, proves a real serialize/deserialize round trip, and checks deterministic bytes.
- Bazel action inspection shows both executables and the schema as inputs and `bazel-out/.../modules/protocol/src/phase1_proof_capnp.rs` as the declared output. The generated file identifies Cap'n Proto 1.5.0 and `capnpc` 0.27.0.
- `bazel run @rules_rust//tools/rust_analyzer:gen_rust_project -- --bazel <bazelisk> //...` succeeds. The generated project contains the protocol library, external proof test, and generator binary; the protocol crate's source roots include Bazel's generated-output directory. Machine-specific analyzer output is ignored rather than committed.
- Focused codegen/library/test targets pass. The full `bazel build //...` and `bazel test //...` pass with 21 build targets and six root test targets respectively.
- The feasibility fixture deliberately makes no product schema decision. The current compiler repository is Windows x86-64 specific; add verified platform-selectable compiler repositories (or a source-built tool) before claiming macOS/Linux support.
