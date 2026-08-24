# Phase 1 implementation packet: Quinn loopback proof

You are a bounded implementation worker in an isolated Git worktree. Do not spawn subagents.

Read `docs/PLAN.md` completely and then read the reconnaissance report at `%TEMP%\artisan-editor-opencode-phase1-current\quic-capnp.report.md`. Treat the report as input, not authority: verify relevant APIs against installed crate sources or primary upstream sources.

## Objective

Implement the Phase 1 direct-QUIC feasibility proof: a real Quinn client and server on loopback, real TLS, a bidirectional stream, bounded framing, deterministic shutdown, and negative tests. The controller will integrate root dependency versions, lockfiles, and shared Bazel state.

## Exclusive file ownership

You may create or edit only:

- `modules/transport/**`
- `tests/transport/**`
- this packet file

Do not edit root files, `MODULE.bazel`, lockfiles, protocol/database/frontend modules, `.github`, or `docs/PLAN.md`.

## Requirements

- No `anyhow` in first-party source or manifests. Use `thiserror` and explicit variants.
- Quinn is direct QUIC; do not introduce WebSocket, WebTransport, HTTP/3, Unix sockets, or browser compatibility.
- Use Quinn `0.11.11`, Tokio `1.53.1`, rustls with the ring provider, and rcgen only for test PKI. Keep default features off where the packet evidence supports it.
- Production transport code owns bounded length-prefixed framing and typed errors. The proof test owns ephemeral certificates and loopback orchestration.
- Bind `127.0.0.1:0`; use a fixed ALPN; wrap every potentially hanging await in a short timeout.
- Prove handshake, bidirectional request/reply, oversized-frame rejection before allocation, malformed/truncated input behavior where applicable, application close, and endpoint idle shutdown. Tests must be deterministic and must not require external network access.
- Do not invent authentication, pairing, reconnect, or product message semantics in Phase 1.
- External tests live under `tests/transport/`, use the normal Rust test harness plus `#[tokio::test]`, and build as focused Bazel `rust_test` targets.
- All Rust targets use edition 2024 and the root lint config.
- Use `apply_patch` for hand-written edits. Run formatting and any focused Bazel checks the current graph permits.
- Review `git diff --check` and your complete diff, then commit once with a focused message. Do not push and do not create or merge a PR.

Report the commit SHA, files changed, commands run, remaining controller integration, and any verified Windows/Quinn limitation.
