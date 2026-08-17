# Active Branch Handoff

Last updated: 2026-08-17. Branch continuity only; durable verified product status belongs in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; work directly on `master`. The stability
  implementation is pushed through commit `25a54741aee38a1cccd3f283a65fe9a0b6a7e106`.
- A large renderer/backend/provider milestone remains intentionally dirty (200+ paths). Preserve
  unrelated frontend, protocol, transport, migration, lockfile, documentation, and test hunks. Do
  not stage shared files wholesale.
- Stability incident closure covers first-message preservation on new-thread creation and installed
  desktop recovery from renderer process loss; only the status commit/final audit remains.
- Repository-required `sanders-skill` governs Effect architecture, validation, Git safety, and
  subagent workflow. Workers use Terra/medium, own disjoint files, preserve dirty work, and do not
  spawn further workers.

## Active Stability Incident

- Installed diagnostics prove the black window is a dead Chromium renderer, not an overlay or
  navigation failure. Version 0.2.74 logged `render-process-gone` reason `oom` (exit -536870904) at
  2026-08-17 09:56:11Z; 0.2.76 logged reason `crashed` (exit -2147483645 / 0x80000003) at
  11:38:59Z. While the fix was being isolated, installed 0.2.77 logged another OOM at 11:57:59Z
  and preserved a 373 MB trace. `main.ts` only logs/captures this event; it never recovers, so
  Ctrl+R has no renderer process to reload. Add deferred, coalesced
  `DesktopForgeLifecycle.Reconnect()` recovery guarded against shutdown/destruction; do not
  synchronously navigate inside `render-process-gone`.
- Desktop containment is committed and pushed in `25a54741`: a deferred, coalesced, closeable recovery
  controller invokes the existing authenticated `DesktopForgeLifecycle.Reconnect()` exactly once
  per renderer-loss event and catches typed failures and defects. Focused desktop coverage passes
  6/6; the post-restart `validate:desktop` passes 13 files / 52 tests plus the production build.
- `pnpm run build` installed 0.2.78 and intentionally replaced the running 0.2.77 window. All live
  Editor/Forge processes now resolve under `versions/0.2.78`; its 9,648,886-byte `app.asar` reports
  version 0.2.78 and contains `artisan:renderer-recovery` in `main.js` plus the scoped draft claim
  implementation in the production frontend chunks. Its initial session contained no unplanned
  renderer loss before the controlled smoke test below.
- The installed recovery smoke test is complete. The sole renderer PID 13744 was terminated after
  exact executable/type/parent validation; desktop main PID 7372 survived and created replacement
  renderer PID 18400 under the same main in 1.6 seconds. The 0.2.78 diagnostic recorded the loss and
  finished its trace from PID 18400. A three-second persistence check found both processes healthy.
- The OOM is real but its allocator-level cause is not yet proven. The crash trace shows the
  renderer producing frames until termination and GPU task memory around 262 MiB, with no
  unresponsive or GPU-watchdog event. Treat automatic recovery as required containment and keep
  pressure/root-cause work evidence-driven.
- First-message loss is a route handoff race in the committed baseline: the thread route acquires
  the retained draft claim before registering its release finalizer. Route replacement in that
  window strands `active_claim`; the new route waits forever, while thread creation already cleared
  the composer and the ordinary second message succeeds. The current dirty tree contains the right
  Effect structure: register cleanup first, atomically claim/publish with `uninterruptibleMask`,
  deliver via the explicit thread scope, and supersede an impossible stale claim for another thread.
  The controller now registers `claim.Release` in the caller's Effect scope before publishing the
  claim, and the route consumes that invariant. The exact route-scope replacement regression plus
  five integration files pass 36/36. `validate:frontend` passes format, lint, type-aware checking,
  and the production build; its suite stops only on the documented unrelated approval wording and
  composer 595-vs-560 line-budget mirrors. Independent review found the scoped lifecycle clean.
  The isolated milestone is committed and pushed in `68c2ebb3`; the post-restart focused controller,
  route-source, and exact regression cluster passes 3 files / 8 tests.
- Independent desktop review found and drove closure of a shutdown race: the controller now retains
  the recovery Fiber, `Close()` awaits its interruption, and the memoized desktop cleanup composes
  that cancellation before Forge cleanup. Close-during-reconnect coverage passes; the final desktop
  gate passes 13 files / 52 tests plus the production build. Follow-up review also caught and closed
  a synchronous `runFork` self-reference hazard; Fiber-observer settlement is now approved clean.
- No dev server was started.

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

1. Observe 0.2.78 during normal use; any later renderer loss should reload in place and remain in the
   same diagnostic session rather than leaving an unrecoverable black window.
2. Continue renderer memory-pressure analysis separately if captured evidence identifies a bounded
   source; do not claim the OOM root cause from queue topology alone.

## Known Baselines Outside This Incident

- Root TypeScript previously retained unrelated recovery-test/Guidance errors. Full frontend
  previously retained unrelated approval/composer and dirty UI/loading mirror failures. Use affected
  gates and report exact current blockers rather than broad claims.
- Query handlers still collapse projection decode causes to `ProjectionUnavailable`; durable-row
  failures should eventually log thread/item identity without exposing content.
