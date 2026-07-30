# Active Branch Handoff

Last updated: 2026-07-30
Branch continuity only. Durable verified status lives in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository: `C:\Users\sander\Desktop\artisan-editor`
- Branch/HEAD: `master` at `ef34399`, tracking `origin/master`. Work directly
  on `master`; do not create branches, worktrees, or PRs unless requested.
- The worktree contains extensive pre-existing Sander WIP across backend,
  protocol, transport, frontend, tests, migrations, and dev tooling. Preserve
  it. Stage only task-owned paths and never revert or overwrite unknown edits.
- Two production Engines are registered: Codex CLI and Claude Code CLI. The
  old Codex-only boundary is retired.

## Architecture That Must Stay True

- Pure transformations remain ordinary TypeScript. I/O, configuration,
  concurrency, shared mutable ownership, lifecycle, and external capabilities
  are Effect programs supplied through Services and Layers. One top-level
  Effect runtime exists per executable.
- Forge owns application state. Clients consume schema-validated typed RPC and
  native binary WebSocket frames after pairing and authoritative hydration.
- Engine adapters own native subprocess protocols; orchestration owns visible
  agent identity, thread/run lifecycle, policy, normalized observations, raw
  event retention, and durable native session affinity.
- Snowflake IDs are non-secret identities. Tokens and nonces remain
  cryptographically random.

## Active Milestone: Portable Compaction / Engine Switching

- Goal: reverse engineer native compaction in every production Engine and
  support a thread continuing on a different engine/model by extracting a
  usable compacted checkpoint and supplying it to the new Engine.
- Codex CLI 0.145.0: `thread/compact/start` returns no summary and public
  `contextCompaction` items contain only an ID. OpenAI remote compaction
  persists an encrypted `compaction` item; 1,741 local rollout records sampled
  had no plaintext compacted message. Same-engine model switching can use
  native resume with a version-gated model override. Cross-engine export uses
  a settled ephemeral fork plus an ordinary captured summarization turn, with
  canonical-transcript compaction as fallback. A provider alias can force the
  plaintext local compactor but is internal/experimental, not production.
- Claude Code 2.1.220: the official `PostCompact` hook supplies
  `compact_summary` in plaintext. Load an Artisan hook plugin per invocation
  via `--plugin-dir`; the hook records the pre-append transcript offset and
  sends untrusted input to a private receiver. Version-gated 2.1.220 parsing
  claims the following `compact_boundary`/`isCompactSummary` pair. Missing or
  uncorrelated output falls back to canonical compaction. Artisan currently
  ignores `compact_boundary`.
- Native session identifiers never cross engines. Define a bounded, immutable,
  typed checkpoint above adapters with source cut, summary plus post-boundary
  tail, version, and integrity hash. A target engine starts fresh and receives
  the checkpoint plus the next user request at user precedence; persist private
  lineage separately from native resume tokens.
- Effect/Effect AI research found useful typed model/prompt/chat capabilities
  but no abstraction that makes Codex/Claude CLI continuation portable. Keep
  CLI extraction/injection behind custom Effect Services and Layers; a
  provider-neutral summarizer can be added behind a separate service.
- Any implementation must coexist with Sander's modified
  `session-policy.ts`, orchestration repository/contracts/schema, protocol
  routes, frontend composer/model controls, and their tests. Inspect live diffs
  before touching those files.
- Persistent harness goal is active. Reverse engineering and focused adapter
  verification, independent review, and durable research documentation are
  complete. Validation is next. Do not mark this feature implemented in the
  completion matrix until product code and integration tests exist.

## Current Product Continuity

- Each Artisan home owns one Forge with config, secrets, state, log, and data
  at the home root. Both CLIs migrate one legacy profile and reject ambiguous
  multi-profile homes.
- Installed rendering uses the sandboxed, context-isolated, bridge-free
  Electron editor at `artisan://app`; hidden `ae open --handoff` performs
  one-time loopback pairing. Development may use Forge-hosted or Vite HMR
  browser rendering.
- Installed Forge does not host the SPA. Static hosting is a development
  opt-in. `package:desktop` bundles the renderer with loopback-only CSP.
- Forge-owned state and Codex SQLite are home-scoped. `CODEX_HOME` remains
  user-global. Claude runs share `~/.claude` because relocating
  `CLAUDE_CONFIG_DIR` also relocates credentials.
- Codex app-server and exec fallback plus Claude stream-json are production
  subprocess adapters. Claude honestly lacks steer/approval/question/subagent
  support.
- Session model selection resolves an explicit enabled native model rather
  than silently inheriting a user's CLI default. Reasoning completion is
  normalized for Claude and Codex exec; app-server exposes no equivalent
  terminal reasoning signal.
- Engine usage has a three-minute last-good backend cache and a
  schema-validated frontend cache. Claude's OAuth usage endpoint may return
  persistent 429, in which case the adapter uses headless `/usage`.
- `ae doctor` verifies payload manifests; pre-manifest payloads are reported
  as unverifiable but healthy. Do not modify the real installed 0.1.0 home;
  use repository builds and temporary fixtures.

## Verification / Known Red

- Portable-handoff focused suites: 39 passed, 1 skipped. Exact installed CLIs
  were Codex 0.145.0 and Claude Code 2.1.220. Schema, tagged source, persisted
  shapes, and strict-config behavior were inspected without printing user
  conversation content. Final two-pass independent review approved; task docs
  pass targeted oxfmt and `git diff --check`.
- Full `pnpm run validate` stopped on formatting in four pre-existing WIP files:
  `activity-status.test.ts`, `shell-source-layout.test.ts`,
  `project-locator.ts`, and `global.css`. Independent lint, TypeScript, frontend
  build, and Rust format passed. Full Vitest: 250 files passed, 3 skipped;
  5 files/10 assertions failed in existing catalog/manifest, Forge/project-ID,
  workspace-rebuild, and sidebar WIP. Rust tests passed 45/45; Windows
  Application Control blocked `cargo-clippy`. The research docs are isolated
  and verified; stage only their three task-owned paths for the milestone.
- Last broad green: 2026-07-28 single-Forge refactor, 228 test files passed,
  3 skipped; 1,548 tests passed, 7 skipped; TypeScript, lint, format, Rust
  format/clippy/tests all green.
- Aggregate `pnpm run validate` can exceed bounded capture windows. Electron
  install preflight and Windows Application Control have caused intermittent
  environment failures; distinguish them from product failures with focused
  commands.
