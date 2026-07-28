# Active Branch Handoff

Last updated: 2026-07-28
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

## Dev/Install Separation (current milestone)

- Dev world: `pnpm run dev:forge` (browser-dev profile, built bundle on 4848,
  the only composition that passes `--serve-frontend`), `pnpm run dev` (Vite
  HMR on 4849 proxying `/api` + `/health`), `dev:open`/`dev:pair` (`ae open`,
  optionally `--origin`), `dev:ae`/`dev:ae-bootstrap` source-run CLI wrappers.
  Dev badge + `[Dev]` title key off a non-`default` `/health` profile. Debug
  Rust builds refuse the installed home.
- Installed world: `ae open` (and `artisan://forge/start`) launches the
  Electron editor from the version payload with `--forge-profile=<name>`;
  `ae open --browser` / `--origin` keep the paired-browser flow. The editor is
  a sandboxed, context-isolated, bridge-free window that loads the bundled
  frontend from `artisan://app`, obtains `{endpoint, pair_code}` from hidden
  `ae open --handoff` stdout (one-time, never argv/disk), pairs cross-origin
  at `/api/pair` with credentials, and connects WS to the adopted loopback
  endpoint. Session cookie lives in a non-persistent partition.
- Forge static hosting is per-profile config (`serve_frontend`; env
  `ARTISAN_STATIC_FRONTEND_ROOT` only when enabled). Installed default
  profiles serve `/health` + `/api` control/WS only; SPA routes 404. The
  frontend stays bundled in the forge payload (one build pipeline); serving is
  gated, not the artifact. Gate: `.tests/forge/static-hosting-gate.test.ts`
  plus Rust `process.rs`/`profile.rs` unit gates.
- Frontend endpoint adoption is restricted to non-HTTP(S) pages
  (`lib/runtime/forge-endpoint.ts`): a crafted `#pair=…&forge=…` link on a
  Forge-served page cannot redirect host-scoped loopback cookies to a sibling
  port. `websocket-binding` allowed_origins default `artisan://app` plus
  http-host CORS/`SameSite=None` cover the pre-session surfaces.
- `package:desktop` stages `.dist/frontend` into `.dist/desktop/frontend`
  with the loopback `connect-src` CSP patch (browser copy stays `'self'`);
  `desktop-builder.yml` ships `frontend/**`; `verify-packaged-desktop.ps1`
  proves renderer-present, loopback-CSP, bridge-free, no embedded Forge.

## Other Standing Facts

- `ae` profiles/setup/doctor contain no project roots; doctor repair restores
  installation/profile/protocol state only; only explicit `ae setup` creates
  profiles. Uninstall removes an exact owned tree; data removal stays explicit.
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

- 2026-07-28 dev/install separation slice: root TypeScript clean; focused
  Vitest green — `.tests/forge` + `.tests/desktop` + `.tests/cli` (17 files,
  69 tests) and `.tests/frontend` (31 files, 169 tests) including the new
  static-hosting gate, forge-endpoint, renderer-host, pairing-fragment, and
  WS-target suites; `cargo fmt`/`clippy -D warnings` clean; cargo tests 16
  bootstrap + 21 CLI passing. Desktop shape tests and the packaged verifier
  now assert the renderer payload. A headed Electron smoke passed via CDP
  against the dev Forge: `artisan://app` load, fragment consumed/stripped,
  `[Dev]` title and badge, cross-origin pairing, WS transport, hydrated
  shell; `package:desktop` plus the rewritten `verify:desktop-package`
  passed. Still unverified: the installed-payload path end to end (real
  install, Rust `ae open` → packaged editor; unit-tested only), and
  release-only/signing CI gates.
- Earlier standing results: the Claude adapter revival passed full validate
  (220 files, 1,511 passed, 7 skipped) on 2026-07-28; Windows distribution
  artifact/lifecycle and packaged-bootstrap gates passed 2026-07-27; live
  installed-runtime acceptance (pairing, hydration, real Codex/Claude turns,
  restart restoration) passed on the synchronized 0.1.0 install.
- Known frictions: aggregate `pnpm run validate` has intermittently exceeded
  bounded capture windows; Electron's ignored install script can block
  dependency preflight; Windows Application Control has blocked one CLI Rust
  test binary in the past (not reproduced today).
