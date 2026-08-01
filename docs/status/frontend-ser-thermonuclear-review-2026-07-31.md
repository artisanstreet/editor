# Frontend SER Thermonuclear Review — 2026-07-31

Status: **approved after remediation**. The original review rejected the
milestone. Every recorded finding and every hostile follow-up finding was
subsequently repaired, independently closed, and repository-validated on
2026-08-01; the original evidence remains below as the audit record.

This report records two independent review passes:

1. a strict frontend correctness and security pass over the dirty milestone;
2. a second code-quality pass using the literal Svelte Effect Runtime standard
   requested for Artisan.

No product-code fixes were made during the two original review passes. The
evidence below is against `master` at `12543d33` plus that uncommitted frontend
worktree; the remediation disposition later in this report records the fixes.
The worktree continued to receive unrelated provider-usage work while the
review ran; that work was preserved and rechecked rather than reverted.

## Approval Standard

Artisan is intended to be the reference implementation for SER. Passing the
compiler is not enough. The following are review requirements:

- Pure transformations may remain ordinary TypeScript only when they return a
  plain value and perform no mutation, I/O, scheduling, lifecycle work, or
  Effect construction.
- Every Effect-returning helper, handler, service operation, state transition,
  queue worker, and program is an `Effect.gen(function* () { ... })`, even when
  the body only assigns one state variable.
- State changes happen directly inside the generator body. They are not hidden
  in an `Effect.sync` blob and are not performed eagerly before an Effect is
  returned.
- Every effectful step is visible as `yield*`. `.pipe(...)` may attach recovery,
  retry, instrumentation, transformation, or finalization to an existing
  generator program; it does not replace generator control flow.
- Components execute work only through direct SER `yield*` sites. Event props
  carrying work return Effects and are invoked through yield-first event sites.
- There is no fire-and-forget application work, `Queue.offerUnsafe` bridge for
  durable user actions, raw Promise control flow, component `onMount` data or
  navigation work, manual Effect executor, or second runtime.
- Expected failures remain typed in the Effect error channel, and external
  values are schema-decoded at their boundary.

## Scope And Census

The current frontend worktree has 19 tracked frontend/test files with roughly
`+453/-340` lines and 15 untracked frontend additions. The largest additions
are:

- `settings/compaction-model.sv`: 482 lines;
- `project-folder-picker.sv`: 327 lines;
- `settings/engine.sv`: 292 lines.

The added or modified application code contains at least:

- 13 `Effect.flatMap` workflow sites;
- 3 `Effect.andThen` workflow sites;
- 13 `Effect.sync` state/program blobs;
- 2 `Effect.succeed` fallback programs;
- 1 `Effect.suspend` program;
- 1 `Effect.promise` navigation boundary;
- 10 `Queue.offerUnsafe` calls;
- 1 new `onMount` navigation path.

No manual executor was added by the dirty diff. The repository-wide frontend
census nevertheless finds 44 `Effect.flatMap`, 26 `Effect.andThen`, 89
`Effect.sync`, and 52 `Effect.succeed` occurrences across 39 files, plus an
existing manual `runFork` bridge and a multi-request async `tryPromise` blob.
Not every constructor occurrence is independently defective, but every
application program represented by one must be classified and migrated under
the approval standard above. There are no grandfathered exceptions for an SER
reference application.

## First Pass: Correctness And Accessibility Findings

### C-01 — Blocker: failed retention writes can leave destructive erasure enabled

[`threads.sv`](../../modules/frontend/src/routes/components/settings/threads.sv#L15)
updates local state first, queues `UpdateThreadRetentionPolicy`, and discards
every failure. Forge continues to read the durable policy in
[`thread-retention.ts`](../../modules/backend/src/threads/thread-retention.ts#L75).
The control can therefore say retention is disabled while Forge permanently
erases inactive threads under the previous enabled policy.

Compaction defaults use the same unreconciled pattern in
[`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv#L130),
with lower immediate impact but the same false-success contract.

### C-02 — Blocker: the first draft message is consumed before durable send succeeds

The root route stores the pending submission and navigates in
[`+page.sv`](../../modules/frontend/src/routes/+page.sv#L218). The routed thread
clears the store before dispatching `SendMessage` in
[`thread-route.sv`](../../modules/frontend/src/routes/components/thread-route.sv#L424).
On command failure only a banner remains. The draft composer has unmounted, so
its text and attachments cannot be retried.

### C-03 — High: fractional retention input is silently shortened

[`threads.sv`](../../modules/frontend/src/routes/components/settings/threads.sv#L46)
uses `Number.parseInt`, so `1.9` becomes `1` instead of being rejected. The
protocol intentionally requires an integer in
[`thread.ts`](../../modules/protocol/src/thread.ts#L183). This can schedule
irreversible erasure earlier than the value the user entered.

### C-04 — High: draft project, policy, and retry state have contradictory lifetimes

[`+page.sv`](../../modules/frontend/src/routes/+page.sv#L118) overwrites the
draft project with the first catalog result on every root mount, while the
module-global draft policy at
[`draft-thread.ts`](../../modules/frontend/src/lib/root/draft-thread.ts#L17) is
never cleared after successful creation. A failed post-create policy write also
leaves the component-local `created_thread_id` cached; changing projects before
retry reuses the old thread but constructs a URL from the new project at
[`+page.sv`](../../modules/frontend/src/routes/+page.sv#L207).

The resulting behavior is race-prone, remount-dependent, and capable of
creating or routing a thread under a project other than the visible selection.

### C-05 — High: context usage is not live during an active run

[`thread-route.sv`](../../modules/frontend/src/routes/components/thread-route.sv#L406)
refreshes a usage snapshot only after a generic thread event stream has been
quiet for 50 ms. Dense output can postpone the refresh for the entire active
run. The transport already exposes the authoritative
`SubscribeSurfaceUsageAggregate` projection in
[`registry.ts`](../../modules/transport/src/internal/subscriptions/registry.ts#L635).

### C-06 — High: the project folder picker has no keyboard traversal path

Folder click only highlights an entry, while entering it is exclusively bound
to `ondblclick` in
[`project-folder-picker.sv`](../../modules/frontend/src/routes/components/project-folder-picker.sv#L267).
Enter and Space cannot navigate into nested directories.

### C-07 — High: changing context on a previewed compaction model saves the wrong effort

[`policy-controls.sv`](../../modules/frontend/src/routes/components/model-selector/policy-controls.sv#L58)
shows an unselected model's default thinking level. `apply_pane_context` then
adopts the model but persists the previous selected model's shared
`thinking_level` in
[`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv#L249).
Changing context can therefore overwrite an unrelated reasoning default.

### C-08 — Medium: the compaction speed selector is a no-op

The shared controls render a speed selector in
[`policy-controls.sv`](../../modules/frontend/src/routes/components/model-selector/policy-controls.sv#L131),
but the settings handler only changes local display state in
[`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv#L254).
Nothing is persisted or applied, and the apparent choice disappears on
remount.

### C-09 — Medium: detailed context usage is inaccessible

[`context-usage-ring.sv`](../../modules/frontend/src/routes/components/context-usage-ring.sv#L53)
uses a non-focusable span as the tooltip trigger. Assistive technology receives
only the rounded percentage, while exact totals and the breakdown exist only
inside mouse-revealed tooltip content.

## Second Pass: SER And Code-Quality Findings

### Q-01 — Blocker: the new surfaces establish a second Effect dialect

The violation is systematic, not local. Representative examples include:

- `Effect.suspend` + `flatMap` + `sync` for `RefreshContextUsage` in
  [`thread-route.sv`](../../modules/frontend/src/routes/components/thread-route.sv#L118);
- `flatMap`/`andThen`/`sync` queue composition in
  [`project-folder-picker.sv`](../../modules/frontend/src/routes/components/project-folder-picker.sv#L79);
- combinator-built reads and queue workers in
  [`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv#L130),
  [`engine.sv`](../../modules/frontend/src/routes/components/settings/engine.sv#L35),
  [`models.sv`](../../modules/frontend/src/routes/components/settings/models.sv#L29),
  and
  [`threads.sv`](../../modules/frontend/src/routes/components/settings/threads.sv#L14);
- `Effect.succeed` fallback programs and `Effect.promise` navigation in
  [`+page.sv`](../../modules/frontend/src/routes/+page.sv#L118).

These all compile through SER, but compilation is not compliance. Each named
program must become an explicit `Effect.gen`; state assignments belong directly
in the generator and each remote, queue, timing, or foreign call is a visible
`yield*` step.

### Q-02 — Blocker: component queues are being used as fire-and-forget command buses

Durable settings actions mutate UI state and call `Queue.offerUnsafe` from
ordinary callbacks. A background forever-loop later performs the write and
ignores failure. This splits one user action across two ownership boundaries,
removes typed failure from the event, and produces the false-success bugs in
C-01.

The worst instances are the defaults queue in
[`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv#L130)
and the retention queue in
[`threads.sv`](../../modules/frontend/src/routes/components/settings/threads.sv#L14).

Durable actions should be named generator programs invoked at direct SER event
sites. If optimistic display is desired, the same generator owns the optimistic
transition, remote write, reconciliation, and typed failure path. Queues remain
appropriate only for genuine serialized lifecycle streams whose producer and
consumer are both Effect-owned.

### Q-03 — Blocker: compaction settings duplicate a second model-selector state machine

The new 482-line
[`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv)
duplicates model ordering, engine tabs, favorites, preview state, policy
controls, and persistence orchestration already present in the composer model
selector. It also makes one conceptual update non-atomic:
`adopt_for_pane` queues the compaction model, then `remember_model_defaults`
queues model defaults separately at
[`compaction-model.sv`](../../modules/frontend/src/routes/components/settings/compaction-model.sv#L223).
Forge can persist the selected model without the defaults displayed beside it.

The code-judo move is one reusable model-picker presentation and one canonical
settings controller. Model `curated | inherited | explicit(model_id)` as an
explicit tagged selection, then save selection, model defaults, and permission
through one `SaveCompactionDefaults` generator and one atomic
`UpdateSessionDefaults` patch.

### Q-04 — Blocker: root draft creation is encoded across three mutable owners

The root route grew from 161 to 295 lines and now combines activity queries,
thread-list subscription, catalog/default hydration, draft mutation, durable
thread creation, partial retry, pending-message handoff, and navigation.
Draft state is split between two module-global Svelte stores and the local
`created_thread_id` in
[`+page.sv`](../../modules/frontend/src/routes/+page.sv#L190).

Replace the incidental variables with one SER-owned `DraftThreadController`
service and an explicit state model, for example:

```text
Uninitialized
  -> Ready(project, policy)
  -> Created(project, policy, thread_id, pending_submission)
  -> Routed(thread_id)
```

Its `Initialize`, `SelectProject`, `UpdatePolicy`, `Submit`, `Retry`, and
`CompleteHandoff` operations are generators. The immutable creation project
travels with `Created`, so remount, retry, and route construction cannot infer
different answers from unrelated stores.

### Q-05 — High: the project picker extraction leaves two overlapping controllers

[`project-folder-picker.sv`](../../modules/frontend/src/routes/components/project-folder-picker.sv#L79)
owns browsing, selecting, loading, and picker state. `thread-panel.sv` still
owns another `ProjectRequest` queue with `assign`, `assign-picked`, `browse`,
and `load` modes at
[`thread-panel.sv`](../../modules/frontend/src/routes/components/thread-panel.sv#L210).
`AcceptPickedProject` is only an imperative queue bridge, and
`HandleProjectRequest` is an ordinary function conditionally returning Effects.

Make the picker success contract Effect-valued and yield it directly. Removing
`assign-picked` deletes an entire queue branch. Keep a panel queue only if it
represents real panel-owned serialization; otherwise use direct named
generators. `thread-panel.sv` is already 749 lines and should not absorb another
UI state machine.

### Q-06 — High: context usage lives in the wrong synchronization layer

The new usage feature adds a pull-based RPC refresh to the already busy thread
route and couples it to unrelated session/work refreshes. This duplicates an
authoritative transport subscription and requires run-id race guards in the
component.

Create one run-usage controller with a tagged state such as
`None | Loading(run_id) | Ready(run_id, aggregate) | Unavailable(run_id)`. Feed
it from `SubscribeSurfaceUsageAggregate`, use a snapshot only for initial or
recovery hydration, and expose the aggregate to `ThreadWorkspace`. This removes
`RefreshContextUsage` from `RefreshInteractionContext` entirely.

### Q-07 — High: navigation has four incompatible ownership paths

Navigation currently appears as:

- raw `onMount` + discarded `goto` in
  [`settings/+page.sv`](../../modules/frontend/src/routes/settings/+page.sv#L6);
- raw callback + discarded `goto` in
  [`sidebar-identity.sv`](../../modules/frontend/src/routes/components/sidebar-identity.sv#L475);
- an ordinary imperative `Navigate` helper in
  [`command-menu.sv`](../../modules/frontend/src/routes/components/command-menu.sv#L31);
- an untyped `Effect.promise` call in
  [`+page.sv`](../../modules/frontend/src/routes/+page.sv#L219).

Prefer real links when the interaction is navigation. Where programmatic
navigation is required, expose one tagged-error `Navigate` service operation
implemented as `Effect.gen` around the narrow foreign Promise call and invoke
it through a direct SER event site. `/settings` should render or redirect
canonically without a client-only mount detour.

### Q-08 — Blocker: the existing editor mount manually executes Effects

The repository-wide pass found `runFork` in
[`editor/mount.ts`](../../modules/frontend/src/lib/editor/mount.ts#L1). It starts
`EditorService.Attach` and interrupt/detach work outside SER, losing the managed
component runtime, service environment, and lifecycle ownership. The plain
[`surface.sv`](../../modules/frontend/src/lib/components/editor/surface.sv#L1)
then invokes that bridge from `onMount`.

This is an absolute SER rule violation even though it predates the dirty diff.
Remove `EditorSurfaceMount` and the manual bridge. Make the surface an SER
component, acquire `EditorService` through the runtime, and attach it through a
component generator whose scope/finalizer owns detach automatically.

### Q-09 — High: the existing development pairing path is one Promise blob

[`pairing.ts`](../../modules/frontend/src/lib/runtime/pairing.ts#L137) wraps two
fetches, JSON decoding, branching, and serialization inside one async
`Effect.tryPromise`, then collapses every failure to `false`. Effect cannot
observe or type individual steps, and schema decoding uses the throwing sync
API inside the Promise body.

Rewrite `AttemptDevelopmentSelfPair` as `Effect.gen`: yield a narrow typed
request effect for each fetch, yield schema decoding, yield the pairing
exchange, and model expected unavailable/rejected cases explicitly.

The root layout has the same older dialect—`flatMap`/`andThen` state workflows,
an `Effect.sync`-rooted retry program, a fetch `.then` inside `tryPromise`, and
`onMount` state mutation in
[`+layout.sv`](../../modules/frontend/src/routes/+layout.sv#L76). A poster-child
cleanup must migrate this shell path as one coherent lifecycle controller, not
leave it as the exception every new component copies.

### Q-10 — High: source-quality gates encode only curated exceptions, not the SER contract

The current lifecycle and source-quality tests scan a few manually listed
components. They do not scan the whole frontend, which is why the existing
editor `runFork`, pairing Promise blob, raw navigation, and combinator workflows
remain green.

Add one repository-wide frontend source gate over application files that rejects
manual executors, runtime construction outside hooks, Promise control flow,
`onMount` effect work, discarded Effects, and non-generator Effect program
roots. Keep Vite-powered build/tests as the syntax authority; the source gate
enforces architecture, not Svelte parsing.

### Q-11 — Medium: component size is below the hard limit but trending toward controller sprawl

No changed file crosses 1,000 lines. The current pressure points are
`thread-panel.sv` (749), `thread-composer.sv` (689), `sidebar-identity.sv`
(619), `model-selector/view.sv` (502), and the new `compaction-model.sv` (482).
The issue is not line count alone: these files own rendering plus queues,
subscriptions, retries, persistence, animation, and navigation.

Decomposition should follow capability ownership rather than visual fragments.
Extract the controllers described above first; the components then become
smaller naturally instead of moving the same state machine into arbitrary
subcomponents.

## Target Architecture

```text
ClientRuntime layers
├── DraftThreadController
│   └── initialize / select project / update policy / submit / handoff
├── SessionDefaultsController
│   └── model catalog / favorites / compaction selection / atomic save
├── RunUsageController
│   └── authoritative aggregate subscription / snapshot recovery
└── EditorSurfaceLifecycle
    └── scoped attach / detach owned by SER

Svelte components
└── direct yield* sites -> named Effect.gen programs -> controller services
```

This structure deletes the component command buses, module-global draft state,
duplicate model-setting state machines, pull-based usage synchronization, raw
Promise navigation, and manual editor runtime bridge.

## Required Remediation Order

1. Remove all manual frontend executors and whole-workflow Promise blobs.
2. Add the repository-wide SER architecture gate so regressions cannot hide in
   files omitted from curated tests.
3. Replace the draft stores/local retry variable with `DraftThreadController`
   and fix C-02/C-04 as part of that state-machine migration.
4. Consolidate compaction/model settings into one controller and one atomic
   defaults write; fix C-01/C-07/C-08 in the same slice.
5. Replace component command queues for durable actions with direct yield-first
   generator handlers.
6. Move context usage to the authoritative subscription controller and fix the
   tooltip accessibility contract.
7. Make project-picker selection Effect-valued and keyboard complete.
8. Migrate remaining frontend program roots to literal `Effect.gen`, then run
   the full repository SER census again with zero unexplained hits.

## Verification Performed

- The current production frontend build passed through the Vite/SER transform.
- The second pass's six focused architecture suites passed 31 tests; the first
  review's 11 focused files passed 94 tests.
- The provider reset helper changed during review and its current focused suite
  passed separately.
- `git diff --check` passed.
- Production output did not contain the development overlay.
- No pull request exists for `master`, so there was no PR discussion to fold
  into the review.
- No development server was started.
- Full `pnpm run validate` was not rerun for these review-only passes.

Passing validation does not override the findings: the current tests prove that
the implementation compiles and exercises selected behavior, while Q-10
explains why they do not enforce the required SER architecture.

## Remediation And Final Disposition — 2026-08-01

All original findings are closed in the remediated tree. The dispositions below
describe implemented behavior and regression gates, not intent.

| Finding | Disposition | Implemented evidence                                                                                                                                                                                                                                                                                                                                           |
| ------- | ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| C-01    | closed      | Retention writes own optimistic state, durable update, authoritative reconciliation, and a visible `Unverified` recovery state that disables destructive controls until retry succeeds. Compaction defaults use the canonical defaults controller.                                                                                                             |
| C-02    | closed      | `DraftThreadController` retains the exact submission and stable command ID until accepted delivery. Claims are owner-aware; completion happens only after `SendMessage` succeeds, and overlapping route scopes wait for release.                                                                                                                               |
| C-03    | closed      | Retention accepts only finite safe integers in range; fractional values are rejected instead of truncated.                                                                                                                                                                                                                                                     |
| C-04    | closed      | One controller owns project, policy, created thread, command, and retry state. Created state keeps its immutable project/policy across remount and retry.                                                                                                                                                                                                      |
| C-05    | closed      | `RunUsageController` consumes the authoritative usage subscription through owner-aware leases; stale streams and late releases cannot replace the selected run.                                                                                                                                                                                                |
| C-06    | closed      | Folder entries are semantic buttons with keyboard activation, and browsing uses one latest-request gate so a late response cannot replace the selected directory.                                                                                                                                                                                              |
| C-07    | closed      | Model-default updates are true per-field patches; adopting a previewed model persists that model's context and thinking defaults without stale sibling overwrites.                                                                                                                                                                                             |
| C-08    | closed      | Compaction speed is a real controller-owned policy write and authoritative reconciliation, not local display state.                                                                                                                                                                                                                                            |
| C-09    | closed      | The context gauge is keyboard focusable and exposes exact totals and breakdown through an accessible description.                                                                                                                                                                                                                                              |
| Q-01    | closed      | Frontend application workflows, handlers, state transitions, workers, and service operations use direct `Effect.gen`; effectful children are yielded. The repository gate rejects non-generator roots and unwrapped Effect/capability members.                                                                                                                 |
| Q-02    | closed      | Durable settings actions execute as one named generator from the SER event site. Unsafe queues remain only in counted, audited synchronous callback ingress with a yielded scoped consumer.                                                                                                                                                                    |
| Q-03    | closed      | `SessionDefaultsController` is the sole live catalog/default/favorite/compaction owner and persists compaction selection/defaults atomically. Shared model-picker presentation replaces the duplicate settings state machine.                                                                                                                                  |
| Q-04    | closed      | `DraftThreadController` owns initialization, selection, policy, submit, explicit retry, claim, release, and completion. The root route is presentation and orchestration only.                                                                                                                                                                                 |
| Q-05    | closed      | The picker has an Effect-valued selection contract and shared latest-request gate; the panel's duplicate `assign-picked` command branch is gone.                                                                                                                                                                                                               |
| Q-06    | closed      | Usage synchronization lives in `RunUsageController`; the thread route no longer polls usage through unrelated conversation refresh work.                                                                                                                                                                                                                       |
| Q-07    | closed      | `RouteNavigation` is the single typed `goto` capability. Raw navigation, mount-time redirect work, and discarded navigation Promises are absent from application sources.                                                                                                                                                                                      |
| Q-08    | closed      | The manual editor mount/executor was deleted. The SER editor surface yields scoped attach/detach through tagged adapter boundaries.                                                                                                                                                                                                                            |
| Q-09    | closed      | Development pairing is a yielded generator with narrow typed HTTP and schema-decode steps. The root shell uses SER-scoped lifecycle programs without Promise chains or `onMount` execution.                                                                                                                                                                    |
| Q-10    | closed      | The global source gate rejects manual executors, Promise control flow, `Effect.void` camouflage, non-generator workflows, unyielded capabilities, unapproved queue ingress, and browser-host work outside typed boundaries. Browser detection lexes executable tokens, ignores comment/string decoys, and fails closed on uncatalogued browser-global members. |
| Q-11    | closed      | Capability-based decomposition reduced the former pressure points. The largest frontend source is 555 lines; `thread-route.sv` is 560 and `thread-composer.sv` is 467. Controllers, fixture domains, DOM helpers, panel state, and composer concerns are split by ownership.                                                                                   |

Hostile follow-up reviews additionally closed stale model-field writes, stale
defaults/route/folder publication, ambiguous retention truthfulness, duplicate
draft claims, fallback-speed drift, direct editor/browser host operations,
FileReader/object-URL ownership, a post-finalizer hidden-image publication race,
and a policy-retry bypass. Three independent frozen-tree reviewers reported no
remaining correctness, quality, or SER finding after those repairs.

Final evidence is green: `pnpm run validate` passed formatting, zero-warning
lint, root TypeScript, the production Vite/SER build on
`svelte-effect-runtime` 4.2.1, the isolated Forge build, 313 Vitest files with
2,067 passing tests and 7 explicit skips, native formatting/clippy, and 45 Rust
tests. The frontend subset passes 63 files/357 tests, including the 23-test
global SER gate, and `git diff --check` passes. The generic Tailwind
pre-transform advisory is documented in `vite.config.ts`; Tailwind's
pre-transform filter is CSS-only, and no Svelte parser or transformed `yield*`
failure occurs.

## Approval Gate

Do not approve the frontend milestone until:

- [x] C-01 through C-09 are fixed and covered by behavior/accessibility tests.
- [x] Q-01 through Q-10 are resolved; Q-11 has an explicit decomposition plan
      enforced before those components grow further.
- [x] Every changed Effect program and state-changing handler is an
      `Effect.gen` with direct `yield*` at its SER execution site.
- [x] The repository-wide frontend has no manual Effect executor or
      whole-feature Promise control flow.
- [x] Durable UI actions have one owner and cannot leave optimistic and Forge
      state half-applied.
- [x] The production frontend build, focused behavior suites, source-quality
      gate, and full `pnpm run validate` all pass.
