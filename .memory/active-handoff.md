# Active Branch Handoff

Last updated: 2026-07-31. Branch continuity only. Durable verified status is in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master`,
  tracking `origin/master`. Thermonuclear remediation is committed through
  clean-checkout repair `45fa22bb`; canonical route restoration is the current
  verified integration milestone.
- Protected user work remains: `modules/frontend/src/routes/threads/+page.sv`
  stays staged but uncommitted and `.mcp.json` stays untracked. One unstaged
  canonical-route adaptation remains on top of that protected page.
- Production Engines are Codex CLI and Claude Code CLI.

## Invariants

- Pure transforms are ordinary TypeScript. I/O, configuration, concurrency,
  shared state, lifecycle, and external capabilities use Effect Services and
  Layers. Schema-decode every external or persistence boundary.
- Forge owns application state. Engine adapters own native subprocess
  protocols; orchestration owns thread/run lifecycle and durable affinity.
- Native provider IDs never cross engines. Portable handoff is a private,
  bounded, immutable checkpoint with a fixed cut and SHA-256 integrity hash.
- A portable target always starts fresh. Compatible same-engine resume is
  version/model gated and carries the new ordered request content explicitly.

## Active Work

- Canonical product routes are `/t/:workspace/:thread` for conversations and
  `/e/:workspace/:thread` for editing; editor file identity remains in `?file=`.
  Workspace and historical thread IDs are encoded/canonicalized centrally.
- The editor route subscribes to the authoritative thread list. Reassignment
  unmounts the old editor before moving to the new workspace route; detach moves
  to `/t/_/:thread`, so stale file reads cannot retain revoked workspace scope.
- `/threads/:id` and `/editor?workspace=...` remain compatibility entry points;
  the protected `/threads` draft remains the pre-creation route.
- Thermonuclear remediation and clean-checkout repair are committed as
  `9414199b` and `45fa22bb`; final independent reviews were clean.

## Verification

- `pnpm run validate` is green in the integration checkout and a frozen-install
  clean clone of `45fa22bb`: format, zero-warning lint, TypeScript, production
  frontend and isolated Forge builds, 292 Vitest files/1,958 tests (7 skips),
  native format/clippy, and 45 Rust tests.
- The repair reviewer, global source guard, and both final thermos reviews are clean.
- Route-focused navigation, editor, pairing, desktop, and Forge suites pass 65
  tests. Final route-milestone `pnpm run validate` is green: formatting,
  zero-warning lint, TypeScript, production frontend and Forge builds, 292
  Vitest files/1,960 tests (7 skips), native format/clippy, and 45 Rust tests.
- No development server was started.

## Dirty-Tree Integration Notes

- The five prerequisite Drizzle migrations through `20260730110447` are now
  committed. Their snapshots form the lineage merged by `20260730161038`; this
  removes the dirty-checkout-only migration dependency.
- Validation builds Forge once into `.dist/validation/forge`; release/watch keep
  `.dist/forge`, so the gate cannot depend on or collide with a watched artifact.
- Protected page patch hash: `84cb787c1f2422da8c5fb5c41a00837151590e10`.

## Product Continuity

- One Forge per Artisan home owns config, secrets, state, log, and data.
- Workspace/thread identity is encoded into both primary surface URLs; Forge
  remains authoritative when a thread is reassigned, detached, or removed.
- Installed renderer is sandboxed at `artisan://app`; Forge does not host the
  SPA. `ae open --handoff` performs one-time loopback pairing.
- Codex app-server/exec fallback and Claude stream-json are production adapters.
  Claude has no steer/approval/question/subagent support.
- Forge state and Codex SQLite are home scoped. `CODEX_HOME` is user global;
  Claude shares `~/.claude` because relocating config also moves credentials.
