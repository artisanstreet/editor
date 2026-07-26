# Active Branch Handoff

Last updated: 2026-07-26

This file contains branch continuity only. Durable verified product status lives
in [`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md);
product and architecture requirements are indexed by
[`docs/README.md`](../docs/README.md).

## Working State

- Repository: `C:\Users\sander\Desktop\artisan-editor`
- Branch: `master`
- `master` is the GitHub default branch and the repository's pre-release
  integration branch.
- Work directly on `master`; do not create branches, worktrees, or pull requests
  unless the user explicitly requests one.
- The current program is the repository-wide Effect rehabilitation requested on
  2026-07-26, including a shared Snowflake ID service with epoch
  `2026-06-19T00:00:00.000Z`.

## Integrated Direction

- Pure transformations remain ordinary TypeScript. I/O, time, randomness,
  configuration, concurrency, mutable ownership, failure, and external
  capabilities are Effect programs supplied through Services and Layers.
- Each executable owns one top-level Effect runtime. Nested `Effect.run*` calls
  below bootstrap boundaries must be removed, except for documented foreign API
  adapters that are narrow, interruptible, and scoped.
- Snowflake IDs are the shared non-secret application identity mechanism.
  Security tokens and cryptographic nonces must remain cryptographically random.
- Frontend-to-Forge transport converges on one typed RPC registry and native
  binary WebSocket frames. Codec choice remains benchmark-gated.

## Current Work and Remaining Integration

- The integrated Effect/Snowflake/native-binary transport migration passes the
  complete repository validation gate.
- Thread, workspace-inspection, and Marketplace read handlers are extracted
  from `protocol-server.ts`; Guidance/Model Behaviour mutations are also
  extracted. Continue splitting remaining mutation domains incrementally.
- Orchestration outbox operations and workspace persisted-row codecs now have
  dedicated modules. Both main repositories remain large; continue extracting
  cohesive ownership without duplicating the canonical transaction-local
  journal append invariant.
- The working tree remains shared and very dirty. Reconcile ownership and split
  coherent commits before publishing.

## Verification Snapshot

- The last durable full-product baseline is recorded in the completion matrix;
  do not replace it with narrow concurrent test results.
- On 2026-07-26, `pnpm run validate` passed formatting, lint, TypeScript, the
  production frontend build, and 1,301 tests across 183 files with 4 skips.
- The standalone Forge process regression found during integration was fixed by
  retaining its backend Layer in an explicit child scope and releasing the
  database lease before IPC disconnect.
- Run `pnpm run validate` before a milestone commit. If it fails, distinguish
  task-owned failures from unrelated dirty/generated format drift and record the
  exact current blocker here.
- Always run `git diff --check` and scoped formatting/lint/type checks for files
  owned by the milestone.

## Handoff Maintenance

- Keep this file below 120 lines and 8 KiB.
- Replace resolved entries with the next actionable state; do not append
  completed milestone history, transcripts, machine incident detail, secrets, or
  artifact hashes.
- Promote durable verified product truth to the completion matrix or the
  relevant PRD/design document, then remove it from this file.
