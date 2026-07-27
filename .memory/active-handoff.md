# Active Branch Handoff

Last updated: 2026-07-27
Branch continuity only. Durable status lives in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository: `C:\Users\sander\Desktop\artisan-editor`
- Branch: `master`
- `master` is the GitHub default pre-release integration branch.
- Work directly on `master`; do not create branches, worktrees, or pull requests
  unless the user explicitly requests one.
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
  model catalog. Its curated inventory now contains only Codex, Claude Code, and
  Grok Build; thinking and permission options use sparse standardized scales,
  while speed options retain model-specific native multipliers and billing
  semantics. OpenCode and Antigravity are removed pending real adapters.
- Public generic commands no longer accept `thread.create`; clients must use
  the idempotent create RPC and its Forge-issued Snowflake ID. Historical
  durable event schemas remain readable for replay.
- `ae open` launches the paired browser. A future native-rendered client needs a
  Forge/CLI-owned public handoff and must not regain profile-secret access.
- `ae` profiles, setup, doctor repair, launch configuration, and Forge
  environment bootstrap no longer contain or infer project roots. Doctor repair
  now restores installation/profile/protocol state without project input.
- PowerShell/POSIX landing transports select a platform-native GitHub Release
  bootstrap, verify its digest, run it temporarily, and clean it up.
- `https://sonstabo.com/editor` and its `/windows` and `/unix` script endpoints
  are deployed on Vercel. Release publication uses protected `candidate`
  dry-run/release/resume workflows with immutable GitHub candidate bytes.

## Verification Snapshot

- The catalog redesign passed formatting, lint, TypeScript, the frontend build,
  focused suites, and 1,423 aggregate tests. Only three standalone Forge cases
  failed because `Artisan Forge.exe` is absent; packaging is blocked by the
  missing offline Electron 43.1.1 runtime. Scoped
  distribution formatting and 108 focused bootstrap/distribution tests pass.
- Windows distribution has signed GitHub retrieval, deterministic x64 archives,
  resumable activation/rollback, and owned integrations.
  Updates quiesce every running owned Forge profile, validate staged runtime
  semantics, restart the exact prior set, and restore the previous runtime on
  failure. Current versions require pointer/layout/integration/semantic health.
  New installs stay partial until bounded setup/start and typed running status
  finalize; legacy state maps pending. Uninstall stops all profiles, removes
  owned integrations, and uses a verified out-of-tree deletion helper; data
  removal stays explicit. Start Menu actions target stable launchers; updates retain
  active/rollback versions, and native commands are bounded and tree-killed.
  Rust bootstrap/CLI checks, Clippy, 19 unit tests, Windows transport tests,
  TypeScript, and focused artifact tests pass. Native release CI now emits the
  Windows bootstrap/checksum and embeds both Rust executables in the product
  archive. Real released-asset qualification, website routing, Windows arm64,
  macOS, and Linux product gates remain. The public landing page and script
  endpoints are live; the first product GitHub Release is still required.
  Candidate CI exposed and fixed Node 24 ESM resolution of extensionless
  TypeScript imports in the distribution builder.
- Permanent `ae` now exposes update/uninstall, treats plain invocation as open
  only for a healthy installation, combines Forge and installation diagnostics,
  repairs only manifest-owned integrations, and retains Forge data by default.
  Missing/corrupt installed release trust fails without replacement while
  doctor remains diagnostic; bootstrap alone seeds public trust.
- Disk-I/O hardening, managed Editor packaging, and focused regressions pass.
  Exact Codex-binary write measurement, CI Authenticode verification, and
  clean-runner packaging remain release qualifications.

## Handoff Maintenance

- Keep this file below 120 lines and 8 KiB.
- Replace resolved entries with the next actionable state; do not append
  completed milestone history, transcripts, machine incident detail, secrets, or
  artifact hashes.
- Promote durable verified truth to the completion matrix or relevant PRD.
