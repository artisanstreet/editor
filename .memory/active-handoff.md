# Active Branch Handoff

Last updated: 2026-08-01. Branch continuity only. Durable verified status is in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master`,
  tracking `origin/master`. Thermonuclear remediation is committed through
  clean-checkout repair `45fa22bb`; canonical route restoration `923fb65f` and
  unreleased compatibility-route removal remain verified. Permission repair
  `fc9182b0`, reconnect `3738bc64`, favorite stability `75c58c0`, provider
  selection `d4a2807b`, and provider usage `12543d3` are pushed and verified.
- Hosted GitHub Actions are disabled and `.github/workflows` is intentionally
  absent while Artisan remains unpublished. Local `pnpm run validate` remains
  the milestone gate; release tooling is retained for future publication.
- Protected user work: `.mcp.json` stays untracked. The concurrently authored
  host-suspend/engine-inactivity slice is validated but remains outside this
  frontend commit. Sander explicitly removed `routes/threads/+page.sv` after
  folding its draft flow into `routes/+page.sv`.

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

- Canonical product routes are `/t/:workspace/:thread` for conversations and
  `/e/:workspace/:thread` for editing; editor file identity remains in `?file=`.
  Workspace and historical thread IDs are encoded/canonicalized centrally.
- The editor route subscribes to the authoritative thread list. Reassignment
  unmounts the old editor before moving to the new workspace route; detach moves
  to `/t/_/:thread`, so stale file reads cannot retain revoked workspace scope.
- No compatibility thread/editor URLs exist: `/t/:workspace/:thread` and
  `/e/:workspace/:thread` are the sole product contracts. The root page `/` is
  the pre-creation draft route: it hosts the activity grid, recent threads,
  and the draft composer whose first send creates the thread. The dedicated
  `/threads` route is removed; the command menu's "New thread" navigates to
  `/`, and the layout treats `/` as a thread surface for the inspector panel.
- The transcript proximity hover rail mounts only on
  `/t/:workspace/:thread`; root, settings, draft, and editor routes never
  instantiate it.
- Canonical thread pages derive `<thread title> › Artisan Editor` from the
  authoritative live thread item, so title refinements update browser chrome.
- Thermonuclear remediation and clean-checkout repair are committed as
  `9414199b` and `45fa22bb`; final independent reviews were clean.
- Both frontend review passes reject the uncommitted root-draft/settings/context
  milestone; see [`frontend-ser-thermonuclear-review-2026-07-31.md`](../docs/status/frontend-ser-thermonuclear-review-2026-07-31.md):
  C-01–C-09 record correctness/accessibility failures; Q-01–Q-11 record the
  non-generator Effect dialect, fragmented controllers, duplicate/non-atomic
  settings flows, and repository-wide SER baseline violations. The active goal
  is to fix every finding, including nits, enforce the repository-wide SER
  source gate, independently re-review, validate, commit, and push to `master`.
- Second remediation wave is integrated: draft first-send owns a stable command
  ID, atomic claim/retry, accepted-only completion, and created-state locks;
  route usage has owner-aware leases; model policy coalesces current intent and
  reconciles authoritative results; image visibility cancels keyed fetch work.
  Shared lifecycle machinery replaced five duplicated queue/fiber controllers;
  FileReader and clipboard failures are tagged. Behavioral controller-race and
  delayed-cancellation tests are green.
- Quality decomposition is integrated: fixture command/query domains, sidebar
  usage, composer, thread panel, and workspace-tab state are split below the
  pressure thresholds. `SessionDefaultsController` is now the sole live owner
  of catalog/default/favorite/compaction state; policy writes coalesce and only
  remember authoritative reconciliation. `RouteNavigation` is the sole typed
  SvelteKit `goto` boundary, and the editor file tree uses direct SER callbacks.
  No application source retains the superseded catalog stream or raw `goto`.
- The SER gate fails closed on every Effect member outside a yielded generator
  or sanctioned pipe operator and audits direct capability programs plus exact
  synchronous queue ingress. Browser DOM, object URLs, and callback-owned
  lifecycle work use typed shared boundaries. `svelte-effect-runtime` 4.2.1
  supplies the transformed `yield* ... is not iterable` callback fix. All C/Q
  and hostile follow-up findings are independently closed.
- The final hostile pass found seven race/truthfulness defects plus literal
  editor-boundary/gate gaps. All are repaired: model fields patch atomically;
  stale defaults, route, and folder reads cannot publish; overlapping draft
  claims wait; retention becomes visibly unverified; fallback speed uses the
  selected model; editor host calls are yielded/tagged. Frozen-tree review
  also closed policy-retry and hidden-image publication races, then hardened
  browser-global detection against uncatalogued APIs and lexical decoys.
- Context-window usage flows engine → surface storage → composer gauge;
  migration `20260731100810_panoramic_power_man` adds its aggregate columns.
  `drizzle.config.ts` correctly targets `persistence/tables.ts`; the previously
  generated drop-everything migration was caught, deleted, and never applied.

## Verification

- Focused remediation/controller/lifecycle tests, adversarial interleavings,
  component integrity, and the expanded SER gate are green. The complete
  frontend suite passes 63 files/357 tests. On SER 4.2.1, production build,
  TypeScript, format, zero-warning lint, and diff checks pass. Three independent
  frozen-tree reviewers are closed. `pnpm run validate` passes 313 Vitest files,
  2,067 tests with 7 skips, both production builds, native format/clippy, and 45
  Rust tests. The frontend SER milestone is the current committed `master` HEAD.
## Dirty-Tree Integration Notes

- The five prerequisite Drizzle migrations through `20260730110447` are now
  committed. Their snapshots form the lineage merged by `20260730161038`; this
  removes the dirty-checkout-only migration dependency.
- Validation builds Forge once into `.dist/validation/forge`; release/watch keep
  `.dist/forge`, so the gate cannot depend on or collide with a watched artifact.
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
