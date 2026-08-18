# Active Branch Handoff

Last updated: 2026-08-18. Branch continuity only; durable verified product status belongs in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; work directly on `master`. Stability fixes
  `68c2ebb3` / `25a54741`, verified status `e3db47ce`, and ordered first-message startup fix
  `3e15801e` are pushed.
- A large renderer/backend/provider milestone remains intentionally dirty (200+ paths). Preserve
  unrelated frontend, protocol, transport, migration, lockfile, documentation, and test hunks. Do
  not stage shared files wholesale.
- Repository-required `sanders-skill` governs Effect architecture, validation, Git safety, and
  subagent workflow. Workers use Terra/medium, own disjoint files, preserve dirty work, and do not
  spawn further workers.

## Active First-Message Incident

- Installed 0.2.85 SQLite evidence identifies the exact failed thread `21880101192863744`:
  `thread.create` was accepted at 09:03:42Z and attention was acknowledged at 09:03:48Z, proving
  the route mounted, but there is no `thread.send_message`, orchestration message, run, or
  conversation item. The next new thread sent normally 10.8 seconds after creation.
- The installed bundle contains the earlier route-scope claim fix. Its emitted SER output exposed
  a second race: asynchronous claim acquisition and the intentionally untracked delivery check
  compiled as independent one-shot reactive sites. On a cold mount, launch could observe no claim
  before acquisition finished, never rerun, and leave the composer blocked without throwing.
- `thread-route.svelte` now performs claim and thread-scope delivery launch inside one sequential
  Effect startup boundary. The controller still owns route-scope claim cleanup; delivery remains
  in `thread_scope`; retry retains its submit gate. The new transform regression asserts that SER
  emits one startup site and no standalone claim site.
- Focused controller/draft/open/route coverage passes 6 files / 26 tests. Exact touched formatting,
  lint, and diff checks pass. The current full `validate:frontend` run passed formatting, lint,
  type-aware production SSR/client builds, and 164/165 test files (962/963 tests); it stopped only
  on the protected unrelated composer line-budget assertion (603 versus 560).
- Independent lifecycle review is clean: interruption between claim and fork releases safely,
  route teardown interrupts thread-owned delivery, and retry cannot duplicate a live delivery.
  Final clean-baseline audit also preserved submit-gate ownership while its older UI exposes retry
  during delivery, and made successful completion clear the retained draft even if route release
  raced first without clearing a newer claim. The release-then-complete regression passes and the
  independent re-review has no high or medium findings. Coverage remains transform/source-contract
  based rather than a mounted delayed-claim simulation. No development server was started.
  Installed 0.2.85 still predates this source fix; no installed replay has been claimed.

## Renderer-Loss Containment

- Installed diagnostics proved prior unreloadable black windows were dead Chromium renderers:
  versions 0.2.74 and 0.2.77 exited for OOM; 0.2.76 crashed. The allocator source remains unproven.
- `25a54741` defers and coalesces authenticated renderer replacement, rejects shutdown-time
  recovery, and awaits cancellation before Forge cleanup. `validate:desktop` passed 13 files / 52
  tests plus production build. Installed 0.2.78 replacement smoke kept desktop main alive and
  replaced its killed renderer in 1.6 seconds.

## Protected Dirty Integration

- Loading milestone: immediate retained/skeleton route paint, shared bounded generation-safe reads,
  backend set-based recovery/index work, and recovery-gated subscriptions. Prior focused baseline:
  frontend 28 files / 177; transport 143/143; backend loading/query 18 files / 66.
- Lossless-delivery milestone removes finite Artisan-owned transport/engine/renderer work-queue
  capacities while retaining byte, deadline, retry, concurrency, cache, and durable-history bounds.
  Queue-focused baseline: transport 8/62; backend/Engine/lifecycle 14/149; frontend/protocol/dev
  13/166. Do not reintroduce silent overflow.
- Conversation work already present: Claude question canonicalization, duplicate start-failure
  removal, steering acknowledgement shimmer, one live thinking summary, context-origin gauge,
  honest fallback wording, thinking-token persistence/projection, and latest-thought status line.
- Renderer work already present: sent-turn anchor lifetime fix, first-submission anchor seed, stepped
  shimmer animation, paused hidden spinner, and focus-gated god-rays rAF.
- Preserve provider Installation UI, retained shell/reconnect behavior, responsive Settings/UI,
  trace rendering, Marketplace/provider management, terminal work, and WSL support documentation.
- Shared dirty files mix milestones. Stage only coherent, verified hunks with proven dependency
  closure; otherwise leave them explicitly dirty.

## Next Order

1. On the next intentional package build, replay a cold new-thread first submission against the
   installed artifact before claiming installed closure; 0.2.85 still contains the reproduced race.
2. Continue renderer memory-pressure analysis separately only if captured evidence identifies a
   bounded source; do not infer the OOM root cause from queue topology.

## Known Baselines Outside This Incident

- Root TypeScript previously retained unrelated recovery-test/Guidance errors. Full frontend has
  the protected composer line-budget mirror above. Use affected gates and report exact current
  blockers rather than broad claims.
- Query handlers still collapse projection decode causes to `ProjectionUnavailable`; durable-row
  failures should eventually log thread/item identity without exposing content.
