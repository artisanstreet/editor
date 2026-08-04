# Active Branch Handoff

Last updated: 2026-08-04. Branch continuity only. Durable verified status is in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master`, tracking
  `origin/master`. Verified milestones are committed and pushed directly.
- Hosted GitHub Actions are disabled and `.github/workflows` is intentionally
  absent while Artisan remains unpublished. Local `pnpm run validate` remains
  the milestone gate; release tooling is retained for future publication.
- Protected user work: `.mcp.json` stays untracked. Concurrent host-suspend,
  engine-inactivity, and frontend work remains outside scoped commits. Sander
  folded the removed `routes/threads/+page.sv` draft flow into `routes/+page.sv`.

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

- Dev modes keep Vite and Forge on isolated loopback ports behind runner-owned
  Portless aliases; linked worktrees use native branch prefixes. Guarded aliases,
  exact Host policy, IPC ownership, rebuild restarts, and the TUI remain intact.
- The Forge fatal overlay now uses a finite user-visible error-code catalog. The
  existing `ArtisanClientError` remains the transport source; the gate preserves
  safe client/protocol diagnostics for classification while the visible footer
  renders only the muted monospaced error code. The ASCII mark centers during
  progress and aligns with the recovery copy for settled failures.
- Account-menu usage now invokes its SER refresh Effect on open; yielding the
  function itself had left cold caches on skeletons forever. Full validation
  passes; provider single-flight/backoff remains follow-up.
- The composer model trigger has a `gap-2` logo/name gap and the rail's former
  command button is now a `New thread` link to `/`; both remain uncommitted with
  protected frontend work.
- Transcript follow state is position-derived; local sends top-align their exact
  projected item and an in-composer jump resumes the true tail. Thinking verbs
  advance only after hiding for live detail, and duplicate file paths group with
  conservative aggregate diff counts, including historical rows.
- Conversation Markdown links resolve page names through Forge's strict public
  contract; retained favicons render when available and a compact Tabler world
  covers cold-start, missing, and broken assets without prose image margins.
  Blob URLs are lifecycle-owned; non-HTTP(S) links never request metadata.
- Unlocked thread titles synchronously follow the latest accepted queued or
  steering user message; manual renames remain locked. Forge repairs historical
  stale titles through narrow compatible evidence replay before serving a
  browser, and projection rebuild derives the same durable title transition.
- Claude stream-json correlates each result with its run-owned tool-use start,
  preserving command/search names and settling file rows once with truthful
  counts when provider metadata exists; ambiguous parallel results stay unknown.
- Live-run forensics fixed burst event-buffer failure, stale activity shimmer,
  trace liveness, command overflow, immutable context provenance, and provider
  turn-start wording. Flow control tolerates bursts and fails only after 30s.
- Presentation instructions stay in additive system fields. Shiki code cards,
  bounded KaTeX, sanitized Mermaid, inert HTML, and SER-owned lazy rendering are
  implemented. Streaming prose uses a bounded correction-preemptible queue with
  reduced-motion completion and single-use animation generations.
- The editor route subscribes to the authoritative thread list. Reassignment
  unmounts the old editor before moving to the new workspace route; detach moves
  to `/t/_/:thread`, so stale file reads cannot retain revoked workspace scope.
- `/t/:workspace/:thread` and `/e/:workspace/:thread` are the sole thread/editor
  routes. `/` owns draft creation, activity, recents, and inspector behavior;
  `/threads` is removed and all new-thread navigation targets `/`.
- Context-window usage flows engine → surface storage → composer gauge;
  migration `20260731100810_panoramic_power_man` adds its aggregate columns.
  `drizzle.config.ts` correctly targets `persistence/tables.ts`; the previously
  generated drop-everything migration was caught, deleted, and never applied.

## Verification

- `pnpm run validate` passes 335 Vitest files plus 3 skipped and 2,322 tests plus
  7 skipped; both builds, TUI smoke, and all 45 native tests pass on the current
  Portless runner milestone.
- Streaming-Markdown focused verification passes 22 tests; the renderer/SER
  regression set passes 45. Production build and `tsc` pass. Live HMR confirms
  adaptive entrances, single-use reparses, final-token release, wrapper collapse,
  and compositor-hint cleanup. Independent race re-review found no remaining
  issue; prior KaTeX, Mermaid, and full-width code-card probes remain verified.
- Portless runner verification: 36 focused tests, workspace `tsc`, frontend
  production build, Forge validation build, scoped lint, and format check pass;
  independent lifecycle/security re-review found no remaining issue.

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
