# Active Branch Handoff

Last updated: 2026-07-27
Branch continuity only. Durable status lives in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository: `C:\Users\sander\Desktop\artisan-editor`
- Branch: `master`
- `master` is the GitHub default pre-release integration branch.
- Work directly on `master`; do not create branches, worktrees, or pull requests
  unless explicitly requested.
- Distribution uses a temporary native bootstrap from GitHub Releases and
  permanent `ae` ownership. npm is outside the release and install contract.

## Integrated Direction

- Pure transformations remain ordinary TypeScript. I/O, time, randomness,
  configuration, concurrency, mutable ownership, failure, and external
  capabilities are Effect programs supplied through Services and Layers.
- Each executable owns one top-level Effect runtime. Nested `Effect.run*` calls
  below bootstrap boundaries must be removed, except for documented foreign API
  adapters that are narrow, interruptible, and scoped.
- Snowflake IDs are the shared non-secret application identity mechanism.
  Security tokens and cryptographic nonces must remain cryptographically random.
- Frontend-to-Forge transport converges on one typed RPC registry and native
  binary WebSocket frames. Codec choice remains benchmark-gated.

## Current Work and Remaining Integration

- The rehabilitation PRD now requires an Orca-inspired root README with a
  visual major-feature table and a complete, verified shipped-feature inventory.
- The source-extension cleanup converted all 135 remaining frontend `.svelte`
  components to the TypeScript-first `.sv` format and all eight tracked `.mjs`
  test fixtures/loaders to `.ts`, including import and subprocess references.
- Forge is the only owner of application state; legacy client-owned thread and
  project inputs are removed.
- Thread, workspace-inspection, and Marketplace read handlers are extracted
  from `protocol-server.ts`; Guidance/Model Behaviour mutations are also
  extracted. Continue splitting remaining mutation domains incrementally.
- Typed `BannerService` notifications own connection status, five jittered
  exponential retries, immediate `Start Forge` recovery, and the future
  Sentry/PostHog reporting seam. `ae open` starts an already configured Forge
  before issuing the one-time pairing capability; only explicit `ae setup`
  creates a profile.
- Rust `artisan-bootstrap` owns authenticated release installation and retained
  update/repair/uninstall mechanics. Rust `ae` owns the permanent Forge command
  UX; only explicit setup creates profiles. Neither depends on npm or Node.
- Forge now has a durable project catalog with server-directory attachment,
  ID-only detach/list RPCs, ordered project-list snapshot/update subscriptions,
  ID-only thread assignment, and atomic create-in-project validation. The typed
  transport exposes catalog list/detach/subscription operations, and its
  dedicated create-thread RPC accepts only title/project intent while Forge
  generates the Snowflake thread ID and returns the authoritative projection.
  The frontend now consumes these APIs only after pairing and authoritative
  project/thread/runtime-catalog hydration.
- The application shell stays unmounted until the authenticated transport is
  ready and the initial Forge snapshots have loaded. Disconnects return the
  client to this inert state.
- Public message commands contain only user intent and request-local image
  tokens. Forge resolves the thread's attached project, derives working
  directory/project context, and replaces image tokens with durable Snowflake
  attachment IDs before persistence.
- The runtime model/capability catalog is a typed Forge query filtered by the
  registered engine adapters. Production frontend code no longer owns a static
  model catalog. It curates Codex, Claude Code, Grok Build, and Cursor Agent.
  Cursor statically curates Router/Auto, Composer 2.5, and Cursor Grok 4.5, then
  materializes every account-returned CLI configuration without aliasing native
  IDs. Provider inference covers major labs with an unknown-provider fallback;
  encoded effort/Fast variants remain native without invented controls. Models
  can carry an optional `disabled.reason`; runtime rejects them and the selector
  renders the reason. OpenCode and Antigravity remain removed.
- Public generic commands no longer accept `thread.create`; clients must use
  the idempotent create RPC and its Forge-issued Snowflake ID. Historical
  durable event schemas remain readable for replay.
- `ae open` launches the paired browser. A future native-rendered client needs a
  Forge/CLI-owned public handoff and must not regain profile-secret access.
- Browser WebSocket clients explicitly request `ArrayBuffer` binary delivery.
  Without it, Chromium supplies MessagePack frames as `Blob`, causing the client
  to close an authenticated `101 Switching Protocols` session before protocol
  negotiation. Exhausted connection banners again offer the fixed
  `artisan://forge/start` capability plus in-page retry. The per-user handler
  targets stable native `ae`, while setup/doctor repair own registration and
  Forge pairing remains inside `ae open`.
- Codex app-server support is minimum-version based rather than exact-version
  pinned. The adapter accepts additive JSON-RPC metadata while retaining
  unambiguous routing validation, and emits canonical kebab-case sandbox values
  (`read-only` / `workspace-write`) accepted by current Codex app-server
  releases.
- `ae` profiles, setup, doctor repair, launch configuration, and Forge
  environment bootstrap no longer contain or infer project roots. Doctor repair
  now restores installation/profile/protocol state without project input.
- PowerShell/POSIX landing transports select a platform-native GitHub Release
  bootstrap, verify its digest, run it temporarily, and clean it up.
- `https://sonstabo.com/editor` and its `/windows` and `/unix` script endpoints
  are deployed on Vercel. Release publication uses protected `candidate`
  dry-run/release/resume workflows with immutable GitHub candidate bytes.

## Verification Snapshot

- The source-extension migration passed frontend format/lint/build, root lint
  and TypeScript, focused fixture tests (91 passed, 1 skipped), independent
  review, and native format/Clippy. An aggregate run overlapped the concurrent
  Cursor catalog edit and reached 1,430 passing tests plus 6 skips before five
  transient catalog expectation mismatches. Aggregate formatting currently
  reports the concurrently edited Cargo manifests. Bootstrap's 8 Rust tests
  pass, while Windows Application Control blocks execution of the CLI Rust test
  binary.
- The catalog redesign passed formatting, lint, TypeScript, the frontend build,
  focused suites, and 1,423 aggregate tests. Cursor's focused catalog
  suite now passes 21 tests plus root TypeScript and the production frontend
  build. Full validation exceeded the 120s capture window during aggregate tests.
  Dependency preflight may still block on Electron's ignored install script. Only three standalone Forge cases
  failed because `Artisan Forge.exe` is absent; packaging is blocked by the
  missing offline Electron 43.1.1 runtime. Scoped
  distribution formatting and 108 focused bootstrap/distribution tests pass.
- Windows distribution has signed GitHub retrieval, deterministic x64 archives,
  resumable activation/rollback, and owned integrations.
  Updates quiesce every running owned Forge profile, validate staged runtime
  semantics, restart the exact prior set, and restore the previous runtime on
  failure. Current versions require pointer/layout/integration/semantic health.
  New installs stay partial until bounded setup/start. Uninstall stops all profiles, removes
  owned integrations, and uses a verified out-of-tree deletion helper; data
  removal stays explicit. Start Menu actions target stable launchers; updates retain
  active/rollback versions, and native commands are bounded and tree-killed.
  Rust bootstrap/CLI checks, Clippy, 19 unit tests, Windows transport tests,
  TypeScript, and focused artifact tests pass. Native release CI emits the
  Windows bootstrap/checksum and embeds both Rust executables in the product
  archive. `v0.1.0` was published from exact dry-run candidate
  `30249509490` at commit `4b30c0d6cef24ab2c7201a22b3524edded7275c2`;
  resume run `30251161900` restored, reverified, uploaded, redownloaded, and
  finalized the same five manifest-bound assets. The public Windows transport
  resolves the latest release, its checksum matches the downloaded native
  bootstrap, and the bootstrap starts successfully. Windows arm64, macOS, and
  Linux product gates remain.
- Permanent `ae` now exposes update/uninstall, treats plain invocation as open
  only for a healthy installation, combines Forge and installation diagnostics,
  repairs only manifest-owned integrations, and retains Forge data by default.
  Missing/corrupt installed release trust fails without replacement while
  doctor remains diagnostic; bootstrap alone seeds public trust.
- Sander's local `0.1.0` runtime was overlaid from the verified current
  `.dist/forge` build and both installed CLI locations now use the current
  release-mode Rust binary. The previous runtime is recoverable from
  `%LOCALAPPDATA%\Artisan\.local-backup-20260727-112031`. Local acceptance
  completed a fresh one-time pairing, WebSocket negotiation, authoritative
  hydration, rendered the real application shell, submitted and persisted a
  text-only message, entered the live agent-work state, and cancelled the test
  run without browser exceptions or transport loss. The send path now uses
  Effect 4's static `Queue.offerUnsafe` API, and Forge skips attachment inserts
  for empty attachment arrays instead of invoking Drizzle `values([])`. Forge
  static hosting now serves the SPA shell for extensionless client navigation
  routes, including hard-refreshed thread URLs, while reserved API/framework and
  missing asset paths remain 404; installed-runtime HTTP and browser hard-reload
  acceptance pass.
- The local runtime was rebuilt and overlaid again after removing Codex's exact
  `0.142.5` gate. Focused engine coverage passed 45 tests with one skip;
  TypeScript, frontend production build, and desktop build passed. The real
  packaged-Forge/Codex acceptance passed against installed
  `0.146.0-alpha.3.1`, including two streamed durable answers and restart
  restoration. A fresh browser pairing then showed the active `Working` label
  and rendered `APP_SERVER_FIXED` from the real app-server in six seconds.
- Final compatibility hardening rejects ambiguous envelopes with preserved
  invalid request IDs and updates the active-work source contract. Focused
  engine/frontend coverage passes 53 tests with one skip; formatting, lint,
  TypeScript, and the production frontend build pass. Full aggregate tests
  exceeded the five-minute capture window. Native Clippy remains blocked by the
  pre-existing `items_after_test_module` ordering in
  `modules/cli/rust/process.rs`, outside this compatibility change.
- The thread composer toolbar exposes model selection and one primary action.
  Its circular action uses the `card-glass` surface with a `white/25` SVG that
  rotates into a filled stop glyph while authoritative thread work is `running`
  or `waiting`; activating stop sends the existing typed `run.cancel` command
  and suppresses duplicate cancellation until Forge reports a terminal state.
  Authoritative thread events refresh both the conversation and work status,
  while Enter retains normal submit/steer behavior. File-picker, effort,
  permission, speed, voice, and legacy Build/Plan controls remain absent; image
  paste/drop remains supported. The focused shell contract (10 tests), frontend
  lint, root TypeScript, production frontend build, and `git diff --check` pass.
  The Svelte thread workspace groups resolved approval receipts into the
  matching concrete `Worked` trace as soon as that work group exists; pending
  approval requests remain visible at the top level.
  `workflow_mode` is removed from current policy, catalog, persistence mapping,
  provider options, and Codex app-server requests. The old SQLite column remains
  inert for backward database compatibility.
- Active work no longer renders the flat `Working` divider. It uses the shipped
  Artisan bitmap sprite with rotating `@artisan/data` verbs, yields to the latest
  concrete active engine event, and respects reduced motion with a still frame.
  Formatting, lint, TypeScript, 77 focused tests, and the production frontend
  build pass. A later full validation run was stopped at the user's request to
  avoid repeatedly rebuilding the product during UI iteration.
- The silent-black Forge startup was reproduced against Sander's installed
  runtime and fixed at two boundaries. The connection banner presents the
  current state before subscribing to later changes. The typed hello contract
  now explicitly distinguishes `fresh` from `resume`; a fresh stateless client
  starts at Forge's transactionally consistent authoritative journal
  watermark/current cursors instead of replaying the entire durable journal,
  while genuine reconnects still replay deltas. CDP acceptance confirmed HTTP
  101, typed control/stream hello and ready frames, successful fresh pairing,
  authoritative project/thread hydration, a rendered shell, and no browser
  errors. A speculative pre-activation WebSocket buffer was removed after
  independent review found lifecycle and memory-bound risks. The synchronized
  local runtime is live at
  `http://127.0.0.1:52985/`; rollback copies are under
  `%LOCALAPPDATA%\Artisan\.local-backup-20260727-130139` and
  `.local-backup-20260727-130614`, with the final pre-contract runtime at
  `.local-backup-20260727-131208`. Twenty-five focused
  protocol/WebSocket/lifecycle tests and TypeScript pass. The code fix is
  committed on `master` as `63ace8b`; it has not been pushed.
- The missing `artisan://` handoff was traced to the native Rust installer:
  Electron Builder protocol metadata never registers an unpacked `dir` target,
  and Rust activation previously recorded only PATH. Windows now registers the
  exact per-user command `"stable ae.exe" protocol "%1"`; the hidden CLI command
  accepts only `artisan://forge/start` and reuses `ae open`. First-time setup and
  `ae doctor --fix` repair missing owned state, doctor reports protocol health,
  foreign/partial unowned handlers are preserved, repair records ownership
  before registry mutation, and uninstall removes only an exact owned tree.
  Native tests pass 29 cases including an isolated HKCU lifecycle test; scoped
  Clippy, TypeScript, banner tests, release native build, frontend/Forge build,
  real registry repair/drift preservation, OS URL launch, HTTP 200, and local
  Forge status all pass. Sander's synchronized install is running at
  `http://127.0.0.1:52985/`; its pre-change runtime is recoverable from
  `%LOCALAPPDATA%\Artisan\.local-backup-20260727-133134`.
- Forge now allocates new thread identities as bare Snowflake decimals rather
  than `thread_<snowflake>`. Every frontend route builder uses the canonical
  bare segment; historical persisted `thread_` identities resolve through a
  compatibility lookup and legacy URLs replace-navigate to their canonical
  form. Thread navigation mounts at the transcript bottom with a direct,
  non-animated `scrollTop` assignment. A locally accepted user-message command
  waits for its durable projection, then smoothly aligns that user card 16px
  below the transcript top. A compensating end spacer shrinks as streamed
  assistant content grows, so stream patches never retrigger or fight the
  scroll. Intake-question responses do not arm user-message alignment.
  Thirty-one focused frontend/protocol tests, both Forge thread-creation
  integration cases, root TypeScript, frontend lint, and scoped lint pass.
  Per the user's request, this milestone did not rebuild the product.
- Disk-I/O hardening, managed Editor packaging, and focused regressions pass.
  Exact Codex-binary write measurement, CI Authenticode verification, and
  clean-runner packaging remain release qualifications.
- The installed Forge remains bound to `127.0.0.1:52985`, and the already
  running Portless proxy now has a persistent `artisan` alias to that port.
  `https://artisan.localhost/` returns HTTP 200 and
  `http://artisan.localhost/` redirects to HTTPS. The installed 0.1.0 CLI
  rejects an explicit HTTPS localhost origin, so pairing was launched with
  `ae open --origin http://artisan.localhost`; Portless performs the HTTPS
  upgrade while preserving the pairing fragment. No product rebuild was done.
- Interaction-triggered Forge disconnects were traced to the transport client
  discarding a fresh session's authoritative `replay.complete` journal/cursor
  baseline. The first live command event consequently looked like a journal
  gap and terminated the otherwise healthy socket. The client now adopts the
  non-regressing replay boundary, a single WebSocket demultiplexer owns both
  logical channels, initial send failures remain typed and unregister their
  request instead of hanging, and normal Forge session interruption no longer
  becomes an unhandled promise rejection. Thread routes also use debounced
  thread-event invalidation to resync the canonical conversation when a live
  projection misses the command-to-patch handoff. The installed runtime was
  rebuilt and overlaid; its preceding state remains recoverable from
  `%LOCALAPPDATA%\Artisan\.local-backup-20260727-144412`.
  Direct-Forge and Portless command/assistant completion acceptance pass. A
  fresh paired Chrome session then created a thread, submitted a real prompt,
  rendered the user and completed assistant messages live without reload, and
  showed neither reconnecting nor connection-error state. A long persisted
  thread opened with zero remaining scroll distance. Forty-six focused tests,
  root TypeScript, scoped formatting/lint, the production frontend build, and
  the desktop/Forge build pass. Full validation remains blocked first by the
  pre-existing formatter mismatch in
  `docs/status/backend-completion-matrix.md`; the aggregate Vitest run also
  exceeded a bounded two-minute capture without reporting a failure.
- Forge availability is now a layout-owned application gate rather than a
  connection banner. Initial connection renders a lightweight shell preview
  beneath a centered backdrop-blur status overlay; after the first successful
  hydration, the real sidebar/workspace remains mounted and visually frozen
  beneath reconnect, offline, and rehydration states. The covered shell is
  `inert`, focus moves into the status or first recovery action and returns to
  its prior shell control after recovery, progress is announced politely, and
  terminal failures use an alert. Connection versus hydration failures expose
  the appropriate recovery action. Hydration runs in scoped fibers, consumes
  the connection state from one current-emitting stream, and a generation
  counter rejects stale completion/failure updates after a connection
  transition.
  Seventeen focused frontend tests, scoped formatting/lint, `git diff --check`,
  and the production frontend build pass. Full validation still stops
  immediately on the pre-existing formatter mismatch in
  `docs/status/backend-completion-matrix.md`; no local install overlay was
  performed.
- Accepted thread messages now reconcile against their exact durable Forge
  receipt instead of relying on one best-effort post-submit refresh. The
  command itself remains at-most-once; after its receipt, an Effect schedule
  performs a short bounded canonical query until the matching user-message
  source reference is projected, then replaces the route snapshot. The
  canonical projection now preserves the accepted command/message identity as
  `source_refs.reference` alongside the journal event ID and sequence. This closes
  the send-before-subscription race without inventing client-owned optimistic
  state. The returned receipt reference also drives smooth alignment of the
  exact submitted turn, so another paired client's concurrent message cannot
  satisfy or scroll this submission. Completed `Thought for` / `Worked for`
  rows again render the requested bottom divider while the active Artisan verb
  row stays undivided.
  Twenty-eight focused frontend/projection tests, root TypeScript, scoped lint,
  `git diff --check`, and the production frontend plus Forge builds pass.
  Independent review's multi-client correlation finding is addressed by the
  exact receipt/source-reference match. The rebuilt runtime is synchronized to
  Sander's installed `0.1.0` Forge with rollback at
  `%LOCALAPPDATA%\Artisan\.local-backup-20260727-234111`; it is running and
  paired at `https://artisan.localhost/`. Live inspection confirms a loaded
  long thread starts at zero distance from the bottom and completed work rows
  have a computed 1px divider. Full validation still stops first on the
  pre-existing formatter mismatch in
  `docs/status/backend-completion-matrix.md`.
- Codex command/file approvals now retain a typed provider-neutral request
  descriptor across normalization, Engine runtime state, canonical projection,
  protocol decoding, and rendering. Ordinary command approvals preserve the
  user-visible reason, command, and working directory while the opaque
  JSON-RPC response identity remains response-only; raw provider method names
  and `call_*` item identifiers remain solely in provenance. Pending approvals
  render as compact action-specific decision cards, gate duplicate responses,
  and report rejected responses through `BannerService`; resolved approval
  receipts collapse under a completed concrete `Worked for …` disclosure while
  requests, active-session receipts, and thought-only receipts stay visible.
  Renderer-local heading IDs do not expose opaque interaction handles. A
  compatibility presentation guard also removes the
  already-persisted `item/.../requestApproval for call_*` strings from old rows
  without invalidating their protocol shape.
  The grouping follow-up passes 15 focused store/presentation tests, frontend
  lint, root TypeScript, the production frontend build, and `git diff --check`.
  Five focused test files (23 tests), root TypeScript, scoped lint, the
  production frontend build, the Forge bundle, installed-runtime HTTP status,
  and live Chrome inspection pass. The installed `0.1.0` Forge is running with
  the new bundle and paired at `https://artisan.localhost/`; the previous
  installed runtime and frontend remain recoverable from
  `%LOCALAPPDATA%\Artisan\.local-backup-20260728-000743-approval-ui`.
  Full validation still stops first on the pre-existing formatter mismatch in
  `docs/status/backend-completion-matrix.md`.
