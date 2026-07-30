# Active Branch Handoff

Last updated: 2026-07-30. Branch continuity only. Durable verified status is in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master`,
  tracking `origin/master`. Portable continuation is committed/pushed through
  `1479292b`; the settled-thread selector unlock is pushed through `4f6d32f`.
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

## Active Thermonuclear Remediation

- Sol-light workers own non-overlapping slices; the coordinator owns architecture,
  integration, independent review, full validation, status, and direct-master
  milestone commits. Existing dirty WIP remains user-owned and uncommitted.
- An early scoped-format mistake traversed 792 files; no shared dirty work was
  reverted. Review unowned formatting deltas before staging.
- Correctness/security: workspace authority revokes on detach/root change and
  subscribes before its initial snapshot; Claude compaction uses safe no-tools
  flags. Deterministic race and focused suites pass.
- Effect/SER: browser/helper lifecycles are scoped. Dropdown highlights use a
  component-owned SER queue/fiber worker with no manual scope/runSync boundary;
  frontend/lifecycle suites and production build pass.
- Protocol schemas are a 17-line facade over nine modules; subscriptions are
  eight scoped modules (largest 230). Project/session/runtime/routine/capability/
  control/tool/preview handlers are scoped modules. Live-event delivery stays
  typed, exact settings constructors replace casts, and uniform handler
  construction replaces the dependency bundle. Server/ready dispatch/mutations
  are 664/292/398 lines; the final protocol review fixes pass 70 focused tests.
- Orchestration is 916/345/856-line repository/acceptance/transaction dispatch;
  29 atomicity/continuation/structure tests pass.
- Transport public contract is 563/586 lines; the 1,559-line prefixed fixture is
  replaced by seven contextual modules (largest 526). The supported `client.ts`
  facade now targets `client-api/`, eliminating its file/directory collision.
  The live client is a
  379-line lifecycle composer over ten scoped domain API modules, each below
  500 lines. Subscription coordination is now a 45-line composer acquiring
  options, identity/trace, protocol send, and error reporting through Context
  Services/Layers over contextual ingress/registry/typed-delivery modules
  (largest 648), with unsafe projection double casts removed; 30 focused tests pass.
- Git and workspace now have contextual names and all files below 1,000. Git
  verification passes 77+35 tests; workspace passes 56 after independent codec
  and non-null cleanup.
- Routines is a 34-line composer over scoped modules (largest 506); 93 tests
  pass. Capabilities are below 1,000 after lifecycle/invocation/drift extraction
  and pass 39 tests. Catalog is a 30-line decoded composer (30 tests).
- Continuation persistence is a 19-line composer over modules at most 416 lines;
  independent structure/repository/service/compactor rerun passes 25 tests.
- Preview is contextual and bounded; its repository persistence boundary now
  uses Schema JSON decoding and typed missing-row failures, with 8 independent
  safety/structure tests passing.
- Codex is contextual and below 1,000 after executable/probe extraction; 98
  tests pass and missing fallback executables again use EngineProcessError.
  Guidance is 634/455 service/provider-sync with 53 tests. Persistence schema is
  a 12-line facade over 12 modules, preserving 66 table exports (89 tests).
- Conversation projection is an 11-line facade over modules at most 263 lines;
  persistence decoding is Schema-based and 16 independent tests pass.
- Filesystem replacement is 696/644-line service/replacement with a scoped
  construction context; 62 focused tests pass after typed path narrowing.
- Terminal is contextual and split into files at most 698 lines; production has
  no raw JSON or non-null assertions and 14 focused tests pass.
- Platform boundaries across bootstrap, CLI, Forge, desktop, and distribution
  now use Effect Schema and typed narrowing; 81 focused tests pass and a source
  guard bans raw JSON/non-null assertions in the 17 remediated files.
- Engine JSONL/auth/usage boundaries now use Effect Schema and typed capture
  narrowing; 56 tests pass, one is skipped, and a source guard is present.
- Journal/thread/orchestration persistence boundaries are Schema-decoded with
  typed invariant failures (101 tests). A second 18-file backend sweep removes
  remaining unsafe assertions/JSON across tools, workspace, Git, graph, preview,
  guidance, routines, and favorites with 187 tests passing.
- Model behaviour/favorites now use contextual filenames and Schema-decoded
  config/probe/graph boundaries; 83 focused tests pass.
- The protocol `control.ts`/`control/` ESM collision is removed via the
  `control-contract/` path; its regression guard and crash fixture pass 18 tests.
- A repository-wide source-quality guard enforces the 1,000-line ceiling,
  contextual filenames, no raw JSON/non-null assertions, and SER ownership.
- All reviewed positional bundles, double casts, `Effect.orDie` conversions,
  and basename collisions are removed; both final thermos verdicts are clean.
  Sander authorized the integrated milestone. Commit staging explicitly excludes
  the protected page, personal config, and five unrelated migrations; direct
  master commit/push is in progress.

## Verification

- `pnpm run validate` is green: format, zero-warning lint, TypeScript, production
  build, 292 Vitest files/1,959 tests (7 skips), native format/clippy, and 45 Rust
  tests. The global guard and both final thermos reviews are clean.
- No development server was started.

## Dirty-Tree Integration Notes

- Task-owned new files are the continuation model/service/repository/schema,
  compactor service and its test, migrations `20260730121130_chilly_tarot` and
  `20260730161038_silky_stingray`, and focused continuation tests. The Claude
  capture/helper files and their capture/packaging tests are deleted (staged).
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
