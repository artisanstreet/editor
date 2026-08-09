# Active Branch Handoff

Last updated: 2026-08-09. Branch continuity only. Durable verified status is in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master` → `origin/master`.
- Protected `.mcp.json` stays untracked; unrelated engine-inactivity and
  frontend work remains outside the regression milestone.

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

- Bazel 9.2/Bzlmod pins Node 24.18/pnpm 11.7, rejects lock drift, and uses an
  explicit-environment TypeScript runner. Forge is one CommonJS Node SEA with
  137 verified assets, fail-closed/concurrency-safe materialization, and atomic
  last-good publishing. Rust launches it with legacy compatibility; releases
  require exactly the SEA.
- The shared development supervisor is now the private Effect-native package
  `@artisanstreet/runner` at `C:\Users\sander\Desktop\runner`, pinned at
  `5b48d0d4`. `Runner.make(...)` is a closed `Effect<never, Runner.Error>`;
  scoped supervision, readiness, routing, and the Artisan-colored Bun/OpenTUI
  worker are internally provided. Artisan and VSX now consume the same package.
- Dev modes keep Vite and Forge on isolated loopback ports behind runner-owned
  Portless aliases; linked worktrees use native branch prefixes. Guarded aliases,
  exact Host policy, IPC ownership, rebuild restarts, and the TUI remain intact.
- The Forge fatal overlay now uses a finite user-visible error-code catalog. The
  existing `ArtisanClientError` remains the transport source; the gate preserves
  safe client/protocol diagnostics for classification while the visible footer
  renders only the muted monospaced error code. The ASCII mark centers during
  progress and aligns with the recovery copy for settled failures.
- Installed 0.2.16 exposed suspend recovery exhaustion. A latched recovery epoch
  and scoped browser clock-gap monitor now recover after wake without reloading.
- Clean CodeMirror documents, inactive drafts, transcript DOM, patch IDs, image
  URLs/retries, attachment backlogs, and WebGL frames are bounded. Streamed
  reducer and render-slot updates do not scan or regroup history. Adjacent trace
  activity projection is linear (no repeated array copies), hidden diagnostics
  are not collected, and settled trace DOM stays lazy. Shiki grammars, KaTeX,
  and Mermaid load only on demand.
- The thread proximity rail now uses projected engine/model/live status, gives
  working and ordinary rows separate bounded scroll regions with a conditional
  `gap-4`, and exposes DEV-only ×20 stress sliders. It remains uncommitted with
  the protected frontend layout slice.
- Codex 0.145.0 usage is schema-correct. Its generation-owned meter/row-refresh
  repair passes 13 tests, TypeScript, lint/format, SER build, and review; full
  validation reached 7 unrelated dirty frontend source assertions after both builds.
- The 2026-08-08 regression repair makes Forge-owned active work outrank local
  rail settlement, applies only the current hydration's thread snapshot before
  opening the shell, lets active disclosures close without dropping their live
  detail tree, and keeps every rail region interactive. Started model-transition
  wording waits for its source. Optional event observers coalesce independently.
- Codex app-server notification ingress is unbounded by design. `1,024` is a
  warning/recovery threshold only; exact unused bookkeeping methods are opted
  out, while every run/lifecycle notification and correlated response is kept.
- The editor route subscribes to the authoritative thread list. Reassignment
  unmounts the old editor before moving to the new workspace route; detach moves
  to `/t/_/:thread`, so stale file reads cannot retain revoked workspace scope.
- `/t/:workspace/:thread` and `/e/:workspace/:thread` are the sole thread/editor
  routes. `/` owns draft creation, activity, recents, and inspector behavior;
  `/threads` is removed and all new-thread navigation targets `/`.

## Verification

- The exact staged tree passes 137-asset/native/CLI/migration/process-host SEA
  smoke, `//:forge_sea`, TypeScript, frontend, Forge, and native checks. Its nine
  milestone files pass 30 tests (one skipped); Forge emits only the 386,274,304-byte
  executable. Full `//:test` reports seven existing baseline assertions outside
  this slice.
- The integrated protected worktree passes `//:verify` and `pnpm run validate`:
  356 Vitest files/2,488 tests plus Bun and 54 Rust tests.
- Streaming-Markdown focused verification passes 22 tests; the renderer/SER
  regression set passes 45. Production build and `tsc` pass. Live HMR confirms
  adaptive entrances, single-use reparses, final-token release, wrapper collapse,
  and compositor-hint cleanup. Independent race re-review found no remaining
  issue; prior KaTeX, Mermaid, and full-width code-card probes remain verified.
- Local SER 4.2.4 passes 805 tests, five browser cases (32 real PubSub reruns),
  type/format/lint, build/pack, and packed SvelteKit/Playwright smoke. SHA-256:
  `43F069599DA65BEDF589271813F8B8361619A7CF8203C4BBDE9848A4BB4BF42C`.
- Current editor/composer/layout retention suites pass. CodeMirror keeps an
  8-document/4 MiB hot cap; composer ingress and anchor wake state are bounded.
  The thread rail passes 24 focused tests and its production SER/Vite build.
- The core regression matrix passes 10 files/145 tests. Post-review disclosure,
  rail, and generation-guard verification passes 4 files/56 tests. Independent
  engine/transport review found no issue (2 files/47 tests); frontend review's
  two interaction findings are fixed and covered. Production frontend builds.
- The local 4.2.4 candidate passes 20 focused files/113 tests across SER,
  transport recovery, retention, trace, and store behavior. Trace/store alone
  pass 28 tests including a 4,096-activity chain and legacy diagnostics.
  Frontend/Electron package build, packaged verifier, aggregate format, lint,
  and focused tests pass; current root and frontend TypeScript checks pass.

## Dirty-Tree Integration Notes

- Selector polish shares `model-selector/view.sv` and the shell source gate with
  protected in-progress frontend work, so it remains uncommitted with that slice.
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
