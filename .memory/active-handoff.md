# Active Branch Handoff

Last updated: 2026-08-20. Branch continuity only; durable verified product status belongs in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Conversation Range Motion (verified 2026-08-20)

- The turn navigator's hover/focus reveal now uses the model picker's exact dropdown grammar:
  250ms from 0.97 on entry, 150ms toward 0.99 on exit, shared easing, and a right-centre origin.
- Its width follows the same asymmetric beats. The transform lives on a wrapper around the glass,
  preserving backdrop material once open; reduced motion disables both animation and transition.
- Focused coverage passes 12 tests. Frontend format, lint, and production build pass; aggregate
  tests reach 995/996 with only the existing composer line-budget assertion failing.

## Thread Navigation Scroll (verified 2026-08-20)

- Per-thread scroll memory is removed completely: its Effect service, browser runtime layer,
  content stamps, restore decision, scroll-event recording, and dedicated memory tests are gone.
- `PositionLoadedThread` still waits for the rendered view state and a Svelte tick, then assigns
  `scrollTop` directly to the current bottom exactly once. Its latch flips only after the DOM
  assignment so the reactive state write cannot interrupt the initial placement.
- The send-anchor, follow pin, jump-to-latest, pagination preservation, and reduced-motion paths
  remain intact. Five focused frontend files pass 44 tests. Frontend format, lint, and production
  build pass; aggregate tests reach 991/992 with only the protected composer line budget failing.

## Thread-Title Mode (verified and pushed 2026-08-20; `2d50e125`)

- Settings switches between harness-summary and latest-user-message titles; summary is the
  default, manual renames win, and engines without a summary fall back to the message title.
- Claude harvests the newest transcript `ai-title` after process output closes. One resolved
  managed home drives both spawn and lookup; replay derives the same summary/version as live.
- Independent review is clean. Claude coverage passes 25 tests, frontend title coverage 60;
  backend recording/defaults and exact rebuild pass. Broader rebuild retains its unrelated
  two-runtime/vacuum timeout; root TypeScript retains protected `steering-stages.ts` errors.

## Live Work Disclosure (verified 2026-08-20)

- Unfinished work sessions now preserve their initially-open state even when mounted after reply
  prose began. Newer work reopens, newer prose folds, successful settlement folds same-batch
  replies, user disclosure wins, and failed/cancelled/interrupted settlement stays open.
- Focused disclosure/status/source contracts pass 106 tests; frontend format/lint/build pass.

## New-Thread Opening Surface (2026-08-20)

- `new-thread-route.svelte` now centres a borderless 3:2 panel on the composer column. Its
  2:1 tracks hold the newest-first thread list and year token calendar without intrinsic sizing
  changing the ratio. Its blue `New thread` foot opens the custom project picker.
- The project menu keeps shared glass/hover grammar, an unconditional separator, and a final
  `New project` row that opens the native folder picker and adopts the result. Review is clean;
  focused contracts pass 19 tests and frontend lint/format/build pass. Shared files remain mixed
  with concurrent work, so stage only the milestone's owned hunks.

## Conversation Delivery Starvation (fixed 2026-08-20)

- Conversation patches are written outside the journal, so event predicates could reject the
  only wake carrying a patch; duplicate/empty-tail wakes could also be discarded. Conversation
  delivery is now cursor-driven on every wake, and refused publication no longer advances its
  patch cursor.
- `thread-route.svelte` deterministically compares settled work with the transcript session and
  resyncs after a 2-second grace. The 30-second liveness probe now runs only for active work.
- Coverage lives in `conversation-delivery-performance.test.ts`,
  `live-event-delivery-performance.test.ts`, and `thread-route-liveness.test.ts`.
- Remaining mapped holes: missing-stream updates are ignored (`registry.ts`); `Queue.offerUnsafe`
  admission is discarded while sequence advances (`offers.ts`); `subscription.stopped` is not
  handled; connection reset has a read/update race; one poisoned conversation row can prevent
  patch reads; outer claim publication can swallow registration refusal.

## Recent Renderer/Transport Milestones

- Dev optimizer churn: `vite.config.ts` pins bounded lazy dependencies and excludes per-file
  Tabler/Shiki packages. Navigation no longer adds optimized entries or reloads open tabs.
  `src/routes/debug/overlay/+page.sv` is an unrelated stale untracked draft.
- Loading: `thread-route-gate.svelte` shows only the centred `FadeArc`; retained snapshots still
  render immediately.
- Hover cards/dropdowns share the transitions.dev entrance grammar through theme, utility, and
  animation tokens. Dropdown starting style and floating transform origin are fixed.
- Streaming Markdown now uses one incremental Comark parser per message, identity-preserving word
  wrapping, shared Shiki token caching, closed-fence highlighting, and Mermaid zoom. The prior
  frontend snapshot was 973/974 with only the protected composer line-budget assertion.
- Zombie connections: a client inbound-liveness watchdog forces reconnect after heartbeat silence;
  the shell retries exhausted sessions every 15 seconds and on focus.
- Mutation skip-ahead: mutation handlers now deliver the committed journal tail, projection
  delivery phases are isolated, and the renderer has a bounded conversation self-heal probe.

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; work directly on `master`. Stability fixes
  `68c2ebb3` / `25a54741`, verified status `e3db47ce`, ordered first-message startup `3e15801e`,
  and lifecycle closure `aecb2c64` are pushed.
- The worktree intentionally contains 200+ mixed renderer/backend/provider paths. Preserve
  unrelated frontend, protocol, transport, migrations, lockfile, documentation, tests, and
  untracked files. Stage only coherent hunks with proven dependency closure.
- Repository-required `sanders-skill` governs Effect architecture, tooling, Git safety, and
  subagent workflow. Workers use Terra/medium, own disjoint files, preserve dirty work, and do
  not spawn workers unless explicitly authorized.

## Active First-Message Incident

- Installed 0.2.85 evidence showed a created thread with no `thread.send_message`, run, or
  conversation item. Emitted SER output exposed claim acquisition and delivery launch as
  independent one-shot reactive sites; launch could observe no claim and never rerun.
- `thread-route.svelte` now sequences claim and thread-scoped launch in one Effect startup
  boundary. Focused controller/draft/open/route coverage passes 6 files / 26 tests; independent
  lifecycle review is clean. Installed 0.2.85 predates this source fix.

## Protected Integration And Next Order

- Preserve retained loading, lossless unbounded work queues, provider installation UI, shell
  reconnect behavior, responsive settings, conversation/trace work, terminal/WSL support, sent
  turn anchors, first-submission seed, shimmer, and focus-gated god rays.
- On the next intentional package build, replay a cold new-thread first submission before
  claiming installed closure. Continue memory-pressure work only from captured evidence.
- Known aggregate baselines include the composer line-budget assertion, dirty session-default
  concurrency work, and unrelated protected type/format failures. Report exact current blockers.
