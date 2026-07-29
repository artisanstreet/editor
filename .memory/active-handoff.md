# Active Branch Handoff

Last updated: 2026-07-29
Branch continuity only. Durable status lives in
[`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository: `C:\Users\sander\Desktop\artisan-editor`
- Branch: `master` (GitHub default pre-release integration branch). Work
  directly on `master`; no branches, worktrees, or PRs unless requested.
- Distribution uses a temporary native bootstrap from GitHub Releases and
  permanent `ae` ownership. npm is outside the release and install contract.

## Integrated Direction

- Pure transformations stay ordinary TypeScript; I/O, time, randomness,
  configuration, concurrency, mutable ownership, failure, and external
  capabilities are Effect programs supplied through Services and Layers. One
  top-level Effect runtime per executable.
- Snowflake IDs are the shared non-secret identity mechanism; security tokens
  and nonces remain cryptographically random.
- Forge owns all application state; the frontend consumes typed RPC and native
  binary WebSocket frames only after pairing and authoritative hydration.
- Engines: Codex and Claude Code CLI adapters are registered in production;
  the codex-only boundary is retired. Steer/approval/question/subagents remain
  honestly unsupported for Claude; `Open` re-probes every run (~20s Windows).

## One Forge Per Home (current milestone)

- The profile concept is removed. Each Artisan home (`ARTISAN_HOME`, default
  `%LOCALAPPDATA%\Artisan`) hosts exactly one Forge: `config.json`,
  `secrets.json`, `state.json`, `forge.log`, and `data/` live at the home
  root beside `installation.json`. Both CLIs auto-migrate a single legacy
  `profiles/<name>/` directory into the home root at the path-resolution
  choke point (Rust `instance.rs`, TS `node-instance-store.ts`) and fail
  with an actionable error when several legacy profiles exist. No
  `--profile` flag or `ARTISAN_PROFILE` env remains anywhere.
- Dev world: `pnpm run dev:forge` (built bundle on 4848, the only composition
  that passes `--serve-frontend`), `pnpm run dev` (Vite HMR on 4849 proxying
  `/api` + `/health`), `dev:open`/`dev:pair` (`ae open`, optionally
  `--origin`), `dev:ae`/`dev:ae-bootstrap` source-run CLI wrappers. The dev
  home is `.dist/dev/forge-home`. Dev badge + `[Dev]` title key off
  `development: true` in `/health` (true iff `serve_frontend`). Debug Rust
  builds refuse the installed home.
- Installed world: `ae open` (and `artisan://forge/start`) launches the
  Electron editor from the version payload; `ae open --browser` / `--origin`
  keep the paired-browser flow. The editor is a sandboxed, context-isolated,
  bridge-free window that loads the bundled frontend from `artisan://app`,
  obtains `{endpoint, pair_code}` from hidden `ae open --handoff` stdout
  (one-time, never argv/disk), pairs cross-origin at `/api/pair` with
  credentials, and connects WS to the adopted loopback endpoint. Session
  cookie lives in a non-persistent partition.
- Forge static hosting is per-home config (`serve_frontend`; env
  `ARTISAN_STATIC_FRONTEND_ROOT` only when enabled). Installed homes serve
  `/health` + `/api` control/WS only; SPA routes 404. The frontend stays
  bundled in the forge payload (one build pipeline); serving is gated, not
  the artifact. Gate: `.tests/forge/static-hosting-gate.test.ts` plus Rust
  `process.rs`/`instance.rs` unit gates.
- Frontend endpoint adoption is restricted to non-HTTP(S) pages
  (`lib/runtime/forge-endpoint.ts`): a crafted `#pair=…&forge=…` link on a
  Forge-served page cannot redirect host-scoped loopback cookies to a sibling
  port. `websocket-binding` allowed_origins default `artisan://app` plus
  http-host CORS/`SameSite=None` cover the pre-session surfaces.
- `package:desktop` stages `.dist/frontend` into `.dist/desktop/frontend`
  with the loopback `connect-src` CSP patch (browser copy stays `'self'`);
  `desktop-builder.yml` ships `frontend/**`; `verify-packaged-desktop.ps1`
  proves renderer-present, loopback-CSP, bridge-free, no embedded Forge.
- Payload drift: bootstrap staging writes `payload-manifest.json` (relative
  path → sha256; format in `modules/bootstrap/rust/payload.rs`) into
  `versions/<v>`; `ae doctor` verifies (`modules/cli/rust/payload.rs`), names
  modified/missing/unexpected files, fails on drift, and reports pre-manifest
  versions (≤0.2.1) as `unverifiable`, which stays healthy. Diagnostic only.
- Engine-state audit: Forge-owned state (artisan.sqlite, forge-sessions.json,
  guidance, model-behaviour) and Codex's sqlite family (`CODEX_SQLITE_HOME` →
  `data_root/codex-sqlite`) are scoped to the home's `data_root`. By design
  `CODEX_HOME` (auth/config/rollout sessions) stays user-global; Claude has
  no state/credential split — `CLAUDE_CONFIG_DIR` would relocate
  `.credentials.json` and de-authenticate the user — so Forge-spawned Claude
  runs share `~/.claude` project history. Documented gap, unfixed.

## Sidebar Identity + Engine Usage (2026-07-29)

- New control queries `host.identity.query` (OS profile: display name via
  Windows CIM `Win32_UserAccount.FullName` / macOS `id -F` / Linux GECOS,
  hostname fallback; cached per process, never fails) and
  `engine.usage.query` (per-engine provider-account quota windows).
  Contracts in `modules/protocol/src/{host-identity,engine-usage}.ts`;
  handlers follow the extracted query-handler template.
- `Engine` seam gained optional non-billable `Usage` reporting
  `EngineAccountUsage { authentication, windows }`. Claude adapter reads
  `.credentials.json` and calls the undocumented
  `api.anthropic.com/api/oauth/usage` (per-model `limits[]`; 401 →
  unauthenticated value, no token refresh — refreshing would race the CLI).
  Codex adapter prechecks `auth.json` then spawns `codex app-server` for
  `account/rateLimits/read` (multi-bucket `rateLimitsByLimitId`, e.g.
  GPT-5.3-Codex-Spark). Grok has no engine adapter, so it never reports.
- Frontend: `left.identity` contract registry entry is live. Sidebar footer
  hosts `sidebar-identity.sv` — initials Avatar (new `ui/avatar` over
  bits-ui) + profile name, dropdown with Settings item, separator, and
  per-authenticated-engine usage bars; usage fetched lazily on first open
  (server may spawn a CLI), cached for the session. Fixture serves
  deterministic identity + usage data.
- Risk note: the Claude usage endpoint is undocumented and rate-limits
  aggressively without a CLI-style User-Agent (sent; poll ≥3 min if ever
  polled). Headless `/usage` text parsing is the documented fallback path.

## Other Standing Facts

- `ae` setup/doctor contain no project roots; doctor repair restores
  installation/instance/protocol state only; only explicit `ae setup` creates
  the Forge config. Uninstall removes an exact owned tree; data removal stays
  explicit.
- The per-user `artisan://` handler targets stable native `ae`; hidden
  `ae protocol` accepts only `artisan://forge/start`.
- Codex app-server support is minimum-version based with canonical kebab-case
  sandbox values. Browser WS clients request `ArrayBuffer` binary delivery.
- Sander's installed `0.1.0` Forge is bound to `127.0.0.1:52985` with a
  Portless `artisan` alias (`https://artisan.localhost/`). Rollback copies
  live under `%LOCALAPPDATA%\Artisan\.local-backup-*`; latest are
  `20260727-234111` and `20260728-000743-approval-ui`. Do not modify the real
  installation from repo work; use repo builds and tests.
- Release publication uses protected `candidate` dry-run/release/resume
  workflows; `v0.1.0` is published. Windows arm64, macOS, and Linux product
  gates remain.

## Verification Snapshot

- 2026-07-28 single-Forge-per-home refactor: full `pnpm run test` green (228
  files passed, 3 skipped; 1,548 tests passed, 7 skipped; ~286s); root
  TypeScript, oxlint, and oxfmt clean; `cargo fmt --check`/`clippy -D
warnings` clean; cargo tests 17 bootstrap + 28 CLI passing (including new
  legacy-profile migration gates in `instance.rs`). Functional dev-flow
  proof: the dev home migrated `profiles/browser-dev` to the home root via
  the choke-point migration, TS `ae setup`/`start` run profile-less, 4848
  `/health` reports `development:true`, the SPA serves, and Rust `dev:ae`
  `status`/`doctor`/`open`/`open --handoff` all operate on the single
  instance (doctor honestly reports the dev home's missing installation).
  Still unverified: the installed-payload path end to end (the real install
  migrates itself when a future release's `ae` runs there; simulated only in
  temp-dir tests), the packaged desktop editor against the profile-less
  handoff, and release-only/signing CI gates.
- Earlier standing results: the Claude adapter revival passed full validate
  (220 files, 1,511 passed, 7 skipped) on 2026-07-28; Windows distribution
  artifact/lifecycle and packaged-bootstrap gates passed 2026-07-27; live
  installed-runtime acceptance (pairing, hydration, real Codex/Claude turns,
  restart restoration) passed on the synchronized 0.1.0 install.
- Known frictions: aggregate `pnpm run validate` has intermittently exceeded
  bounded capture windows; Electron's ignored install script can block
  dependency preflight; Windows Application Control has blocked one CLI Rust
  test binary in the past (not reproduced today).
