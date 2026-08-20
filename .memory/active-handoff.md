# Active Branch Handoff

Last updated: 2026-08-19. Branch continuity only; durable verified product status belongs in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Thread Loading Surface (2026-08-19)

- `thread-route-gate.svelte`'s loading state is now a single centred `FadeArc` spinner —
  no skeleton bars, no known-thread title/preview text, no mocked composer card (all removed
  at the user's direction; the catalog wiring that fed the known-thread header went with it).
  The retained-snapshot path still paints the real route immediately; `aria-label="Loading
  thread"`, the snapshot read, and the forked `Load` that `thread-open-performance.test.ts`
  pins are unchanged and the test passes. Verified live via CDP: 39 sampled loading frames
  show spinner only (0 skeletons, empty text, centred), then the full route paints.

## Hover-Card Entrances (2026-08-19)

- The floating surfaces now share one entrance grammar (transitions.dev tooltip): `--tt-*`
  tokens in theme.css, `t-tt` / `t-tt-presence` utilities in utilities.css, `t-tt-in`
  keyframes and reduced-motion durations in animations.css. The thread rail's hover card
  (`thread-hover-rail.svelte`) enters by animation rather than transition because its first
  engagement mounts the card already shown — a transition has no earlier frame there and
  popped (pre-existing); `backwards` fill is what holds the 450ms dwell. The context-window
  gauge card (`context-usage-gauge.svelte`) had no entrance at all; it now wears
  `t-tt-presence`, keyed on bits-ui's `data-starting-style`/`data-ending-style`, plus the
  middleware's transform origin.
- vendor.css: the `.t-dropdown` popover enter never played — the open rule won the first
  painted frame — fixed with a `data-starting-style` rule after it, and the dropdown now
  grows from `--bits-floating-transform-origin` (the wrapper publishes it; nothing applied it).
- Verified empirically via headless CDP against the dev app: dropdown enter shows
  starting-style frames then a smooth 0.97→1 scale+fade; the rail card holds opacity 0
  through the dwell, fades over ~150ms, rests at `transform: none` (glass stays sampled),
  conceals in ~50ms. The gauge card itself was not mountable on the dev landing route (no
  usage aggregate) — its mechanism is the one the dropdown probe proved.
- Gates: shell-source-layout, sidebar-identity-and-thread-rail, context-gauge-tone,
  context-window, ser-effect-discipline (5 files / 80 tests) and oxfmt pass. The
  conversation turn navigator still reveals with only a backdrop fade — untouched.

## Prose Rendering Overhaul (2026-08-19)

- Streaming markdown was O(N²): every 12–40ms word tick re-parsed the whole message
  (`@comark/svelte`'s `parse()` builds a fresh parser per call and never engages comark's
  incremental streaming mode) and `wrap_streaming_words` rebuilt every word node of the
  whole tree, so the renderer re-rendered everything per tick. `content.svelte` now owns
  one `createParse` per message and parses with `{ streaming: true }`; append-only ticks
  re-parse only the tail, and the words plugin wraps only nodes past
  `state.reusableNodes.length` so the settled prefix keeps node identity. Reveal pacing
  is frame-floored (≥16ms) and drains backlog by revealing more words per tick
  (`get_streaming_word_pacing`).
- Shiki now highlights each fence once, the moment it closes mid-stream, instead of
  queueing every block for the settle frame: `MakeConversationFenceHighlightPlugin`
  (settled-highlighting.ts) is a synchronous post hook — SER discipline forbids
  async/await outside `Effect.tryPromise` — that substitutes from a shared token cache,
  records misses as pending, and skips the one unterminated fence. The rendering workers
  drain pending between two cheap tail parses. The settle parse and reopened threads
  reuse cached tokens; `TryResidentConversationSettledMarkdownPlugins` lets a settled
  mount with resident grammars parse exactly once. Comark strips one trailing newline
  from fence bodies — `WarmConversationFenceTokens` mirrors that when pre-tokenizing.
- Mermaid diagrams gained a zoom overlay (`mermaid-renderer.svelte`): click to open,
  fit-to-screen, wheel zoom around the pointer, drag pan, double-click refit, Esc /
  backdrop close. Styles in prose.css (`docs-mermaid-zoom-*`).
- Coverage: `conversation-fence-highlight.test.ts` (tokens, cache identity, open-fence
  skip, pending flow, nested fences), updated `conversation-streaming-words`,
  `conversation-markdown`, `conversation-rich-markdown`, `conversation-fence-info`
  contracts. Full frontend suite 973/974 (only the protected composer line-budget
  mirror), typecheck and production build clean.

## Desync Layer Two: Zombie Connections (found and fixed 2026-08-19, after 0.2.93)

- A freeze recurred on installed 0.2.93 — which already contains the skip-ahead fix — with
  the transcript stopped mid-stream while the sidebar showed newer activity. Forge log for the
  window: `artisan:forge-session-failed … WebSocket closed`, plus a 16-minute host suspend
  earlier and historical fatal 4GB OOMs from pre-fix Forge instances (current Forge is stable
  at ~140MB). The failure class is a connection that dies without a close event reaching the
  renderer: the server heartbeats and kills silent peers, but the client had no equivalent, so
  a zombie socket waited forever while every surface froze silently.
- Fix one (`modules/transport/src/internal/client-connection.ts`): an inbound-liveness
  watchdog per session. The welcome advertises the heartbeat cadence; a healthy session never
  goes `interval*2 + timeout` without an inbound frame (idle peers get pinged), so silence
  past that limit fails the session into the reconnect supervisor — whose budget a once-ready
  session does not spend. Production detection is within ~75s; suspend resumes reconnect
  immediately.
- Fix two (`+layout.svelte`): an exhausted supervisor no longer waits solely for the overlay's
  manual retry — the shell re-arms `client.RetryConnection` every 15 seconds while the phase
  stays exhausted and immediately on window focus.
- Coverage: `artisan-client-reconnect.test.ts` "reconnects a session whose inbound frames
  silently stop" (new `mute_current_connection` harness fault: frames vanish, ports stay
  open); fake-protocol welcome heartbeats are now configurable (generous defaults so ordinary
  tests never trip the watchdog; establishment-deadline tests declare huge values because
  their TestClock jumps are not silence). Transport suite 27 files / 161 tests green.
- Unrelated: `steering-stages.ts` currently fails `tsc` (`Fiber.RuntimeFiber` missing) from a
  concurrent in-flight session's edits — not part of this work.

## Desync: Mutation Skip-Ahead (root cause found and fixed 2026-08-19)

- The recurring "thread freezes mid-stream, OS notification still arrives, Ctrl+R heals" desync
  reproduced against installed 0.2.91, which does contain the snapshot-storm fixes. Root cause is
  older code: mutation handlers (`HandleCommand`, `HandleThreadCreate`, settings) delivered only
  their own routed events, and admission advanced `delivered_journal_sequence` to their maximum —
  skipping every event run fibers journaled since the last notifier wake. The handler holds
  `event_delivery_lock` for its whole execution, so a slow mutation widens the window; when the
  skipped window holds a run's final stretch, nothing ever wakes the thread again. The OS
  notification still arrives because the desktop shell notifies from its own connection.
- Fix: `ReadyConnectionRuntime` now exposes only `DeliverCommittedTail` (reads
  `journal.ReadTrustedTail` from the connection cursor; server.ts implements it; the git mutation
  path already did this and now shares it). Regression contract:
  `.tests/backend/ready-mutation-tail-delivery.test.ts`.
- Hardening, same failure shape: each subscription's projection delivery and every delivery
  phase (`IsolateDeliveryPhase`) is isolated, so one poisoned projection read can no longer
  starve conversation patch delivery on every wake after admission advanced the cursor. The
  growth pushed `live-events.ts` past the 550-line decomposition cap (it was already over), so
  the per-event projection loop now lives in `subscriptions/projection-patches.ts` and the
  event-affects predicates in `patch-selection.ts`; `journal-trusted-tail-performance` pins the
  shared `DeliverCommittedTail` operation instead of per-site `ReadTrustedTail` calls.
- Renderer self-heal: `thread-route.svelte` runs `WatchConversationLiveness` — while durable work
  reports an active run and the conversation stream has been silent for 30 s, it runs one
  `ReconcileConversationAndInteraction` probe. Source contract:
  `.tests/frontend/thread-route-liveness.test.ts`.
- Completed two WIP tests from the dirty wave that predated the claim lifecycle and guidance
  anchor: `live-event-delivery-performance.test.ts` (registers `subscription_claims`) and
  `protocol-ack-window-capacity.test.ts` (welcome head is 4 with the anchor journaled after the
  three seeds).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; work directly on `master`. Stability fixes
  `68c2ebb3` / `25a54741`, verified status `e3db47ce`, and ordered first-message startup fix
  `3e15801e` plus lifecycle closure `aecb2c64` are pushed.
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
