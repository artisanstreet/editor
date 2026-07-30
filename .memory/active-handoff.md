# Active Branch Handoff

Last updated: 2026-07-30. Branch continuity only. Durable verified status is in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master`,
  tracking `origin/master`. Portable continuation is committed/pushed through
  `1479292b`.
- Extensive pre-existing Sander WIP is present. Preserve it and stage only
  task-owned content. `modules/frontend/src/routes/threads/+page.sv` is
  pre-staged user work and must remain staged but uncommitted.
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

## Active Milestone: Portable Compaction / Engine Switching

- Canonical design and reverse engineering are in
  `docs/research/portable-engine-handoff.md`.
- Codex CLI 0.145.0 exposes no plaintext native compaction summary. Cross-engine
  export uses an exact settled ephemeral `thread/fork` and one constrained,
  no-tool structured summary turn. Invalid/unavailable export falls back to the
  canonical transcript at the fixed cut.
- Claude Code 2.1.220 exposes `compact_summary` through official `PostCompact`.
  An invocation-scoped plugin records the pre-append transcript identity and
  offset; a private receiver pairs it with the immediately following
  `compact_boundary`/`isCompactSummary` records.
- Codex and Claude native continuation require an explicit target model and an
  exact version-tested adapter decision. Codex also verifies the target through
  bounded `model/list` pagination.

## Completed Implementation

- Engine contracts expose native compatibility, portable export, ordered
  `next_content`, and private native-compaction results.
- Codex export is version gated, model probed, strictly correlated, bounded,
  pagination aware, and rejects tool/request activity. Resume sends exact
  ordered text/image input.
- Claude capture is private and summary-free publicly. It confines paths to the
  real Claude project root, verifies descriptor identity, bounds bytes/time,
  rejects symlinks/replacements/malformed input, waits the full race window for
  conflicts, treats duplicate delivery idempotently, and selects the latest
  valid compaction by transcript offset. The packaged helper cleans temp files.
- `thread-continuation-model.ts` owns checkpoint bounds, hashing, canonical
  fallback, logical post-boundary tail, and multimodal-safe injection.
- `ThreadContinuationService` chooses fresh/native/Claude-summary/Codex-export/
  canonical paths and persists immutable private lineage.
- Separate continuation tables/migration avoid absorbing dirty shared
  `schema.ts` WIP. Persistence pins exact journal cuts, verifies private
  compactions, prepares/opens/binds/fails atomically, serializes neighboring
  launches, reconciles cold-start stranding, and obeys erasure fences.
- Orchestration prepares before open, strips summary material from public
  observations, records native compaction privately after close, binds native
  identity atomically, wakes queued dispatch, and performs cold recovery once.
- Thread erasure deletes all continuation state. Forge packages the Claude hook
  helper as `claude-post-compact-hook.js`.
- End-to-end migrated-SQLite tests prove Claude → Codex, Codex → Claude,
  compatible Claude model resume, ordered multimodal input, private lineage,
  erasure, and three rapid serialized Codex launches.
- Canonical history is schema-decoded and bounded in SQL, ordered by same-agent
  logical run starts rather than the globally interleaved journal. Exact counts,
  earliest objectives, source cuts, and native-summary boundaries remain
  correct when the next request is queued before the prior run settles.
- Claude boundary UUID and trigger must agree across stream, transcript, hook,
  raw provenance, persisted state, and launch validation. Persisted summary
  hashes are recomputed on read and launch; tampering falls back canonically.
- The live model selector permits engine changes after a run settles and keeps
  other providers disabled only while a run is active. The existing durable
  session-policy update then drives the next run through continuation.

## Verification

- Final independent P0-P3 review is clean after resolving logical-run lineage,
  summary integrity, boundary-trigger, forward-compatible raw-frame, mailbox
  overflow, and content-only resume findings.
- `pnpm run check` passes. The final 14-file provider/continuation/packaging
  matrix passes 140 tests with 1 explicit skip. Native formatting/clippy and 45
  Rust tests pass.
- `pnpm run lint` and the production frontend build pass with pre-existing WIP
  warnings. Full Vitest reaches 1,837 passes and 7 skips; 10 failures are in
  unrelated dirty catalog/frontend/Forge/workspace-rebuild WIP.
- Aggregate `pnpm run validate` stops at four unrelated dirty formatting files:
  `.tests/frontend/activity-status.test.ts`,
  `.tests/frontend/shell-source-layout.test.ts`,
  `modules/backend/src/threads/project-locator.ts`, and
  `modules/frontend/src/lib/styles/global.css`.
- The engine-unlock regression, root TypeScript check, and production frontend
  build pass. No live preview was attached and repository rules prohibit
  starting a development server without an explicit request.
- The engine-unlock milestone is verified; no implementation work remains.

## Dirty-Tree Integration Notes

- Task-owned new files are the continuation model/service/repository/schema,
  migration `20260730121130_chilly_tarot`, Claude capture/helper, and focused
  continuation/package tests.
- Shared dirty files include `agent-orchestrator.ts`, `backend-runtime.ts`,
  backend `index.ts`, `forge.vite.config.ts`, provider adapters/tests, and other
  Sander WIP. Isolate task hunks when committing; do not absorb unrelated work.
- Unrelated untracked migrations `20260729084837`, `20260729132743`,
  `20260729141622`, `20260730093655`, and `20260730110447` are user WIP.

## Product Continuity

- One Forge per Artisan home owns config, secrets, state, log, and data.
- Installed renderer is sandboxed at `artisan://app`; Forge does not host the
  SPA. `ae open --handoff` performs one-time loopback pairing.
- Codex app-server/exec fallback and Claude stream-json are production adapters.
  Claude has no steer/approval/question/subagent support.
- Forge state and Codex SQLite are home scoped. `CODEX_HOME` is user global;
  Claude shares `~/.claude` because relocating config also moves credentials.
