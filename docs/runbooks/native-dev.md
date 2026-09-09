# Native dev runbook — `bazel run //:dev` / `scripts/dev.ps1`

One command builds the native Editor frontend and the Forge backend,
stages them into an isolated development installation under
`<workspace>/.dist/dev`, provisions that home through the existing CLI
custody APIs, and launches the **staged** Editor. The Editor then runs its
shipping startup plus one opt-in step: it starts its newly owned Forge,
connects over authenticated QUIC, completes its initial queries, and
writes the startup receipt the launcher waits for. The launcher reports
an honest result — ready, failed with stage, or timeout — and stops the
Editor it owns when startup is not confirmed.

## Usage

Authoritative (Bazel owns the build):

```text
bazel run //:dev
bazel run //:dev -- --stage-only
bazel run //:dev -- --dev-dir C:\scratch\artisan-dev --stage-only
bazel run //:dev -- --bin-dir <dir-with-ae-editor-forge-installer> --stage-only
```

Windows developer convenience (same runner, cargo-built binaries; never
wired into Bazel):

```text
scripts/dev.ps1
scripts/dev.ps1 -Release -StageOnly
scripts/dev.ps1 -DevDir C:\scratch\artisan-dev
```

`dev` flags (after the `--` Bazel passthrough separator):

| Flag | Effect |
| --- | --- |
| `--dev-dir PATH` | Isolated root. Default: `<workspace>/.dist/dev` (`BUILD_WORKSPACE_DIRECTORY`, else current directory). Must be absolute. |
| `--bin-dir PATH` | Use prebuilt `ae`/`editor`/`forge`/`installer` binaries from `PATH` instead of the Bazel runfiles search. `dev.ps1` and root validation use this. |
| `--stage-only` | Stage and provision without launching the Editor. |
| `--help` / `-h` | Print usage. Exit code 2 on invalid flags, 1 on stage failure. |

## What one run does

Seven plain-text stages, no TTY codes, suitable for piping:

1. **resolve** — derive `<dev>`, `<dev>/home`, `<dev>/home/versions/dev`.
2. **binaries** — locate `ae`, `editor`, `forge`, `installer` (`--bin-dir`,
   then `$RUNFILES_DIR` with Bzlmod `_main`, legacy workspace, and flat
   layouts, then `$RUNFILES_MANIFEST_FILE` with the same prefixes, then
   the `bazel-bin` sibling layout). Under `bazel run`, these are `data`
   dependencies of `//scripts/native_dev:dev`, so both product binaries
   build before staging.
3. **lock** — acquire the inter-run OS lock (auto-released if the stager
   dies) and refuse while the dev readiness receipt identifies a Forge
   running the **staged** binary.
4. **provision** — `credentials::provision_or_load` plus a
   `NativeInstanceConfig` write. Reuses the existing instance identity,
   credentials, database, and dev data on repeat runs; mints only on first
   run. Runs before activation, so a failed provision never strands a
   `complete` manifest.
5. **stage** — copy changed binaries into `versions/dev.staging`, write
   and verify the payload manifest there, then swap it into
   `versions/dev` (previous tree retained until the swap completes, then
   removed). Identical trees skip activation entirely, so a running dev
   session is never disturbed. A failed update leaves the active version,
   manifest, and data exactly as they were.
6. **manifest** — write `<home>/installation.json` (`active`/`complete`,
   `active_version: "dev"`) last and validate it with the shipping
   `InstallationManifest::load` (previous manifest restored on failure).
7. **startup/launch** — Mint a per-launch receipt path
   (`startup-receipt-<pid>.json`, so a stale receipt can never confirm a
   new launch), fail closed when a stale file cannot be removed, reconcile
   a stale Forge readiness receipt (below), then spawn the **staged**
   Editor with `ARTISAN_HOME=<home>`, `ARTISAN_DEV_STARTUP_RECEIPT=<dev>/
   startup-receipt.json`, and the manual-forge escape hatches stripped.
   Wait up to 90 seconds for the receipt: `ready` (authenticated QUIC +
   initial project/thread queries complete) prints `ok` and the launcher
   keeps waiting for normal Editor exit; `failed` (secret-free stage),
   timeout, or early Editor exit stops the owned Editor — releasing the
   owned Forge through its lease and Job Object containment — and fails
   the stage honestly.

## Relation to the Electron dev runner and checklist

The legacy `.scripts/dev/runner.ts` supervised a Forge Rolldown watcher
plus the Vite frontend with worktree-derived ports, a pairing secret, and
`@artisanstreet/checklist` lane presentation. The native counterpart keeps
the shape but not the machinery:

| Electron runner | Native `dev` |
| --- | --- |
| Forge watcher build + Vite dev server | Bazel `data` deps on `//modules/backend:forge` + `//modules/frontend:editor` (or one `cargo build` in `dev.ps1`) |
| Worktree-derived ports | No ports: QUIC loopback endpoint comes from the owned readiness receipt |
| Pairing secret / same-origin codes | No secret: QUIC bootstrap capability + certificate pin from provisioned custody; startup confirmation reuses the owned session's own receipt, never a separate probe |
| `@artisanstreet/checklist` TUI lanes | Numbered `dev: stage i/N ... ok` plain lines (no TTY codes by design) |
| Manual `ARTISAN_DEV_FORGE_HOME` backend | Owned Forge started by the staged Editor itself; the manual escape hatch is stripped |

No Node/Cargo wrapper performs the authoritative build: Bazel owns it
(`dev.ps1` is a developer convenience with an honest build driver), and
the only new executable is the small native `scripts/native_dev` crate,
as the Rust port plan allows.

## Listener budgets and prompt delivery

`requests_per_connection` and `admission_capacity` are **lifetime**
budgets, not concurrency limits: the backend closes a connection after
that many completed requests (`BudgetReached`). There is no codified
production default (the installer never provisions instance values; `ae
setup` takes explicit args), so dev uses large bounded values —
admission `1024`, per-connection `65536` — that survive usage polling,
catalog refreshes, and a full day of composer use. A value like 32 would
disconnect normal use after a few dozen requests.

`prompt_delivery` accepts any nonempty string up to 256 bytes without
control characters or line breaks; the Forge enforces the identical rule
(`native_run_dispatch`), and `queue` is the value the CLI fixtures use.
The `dev_instance` suite asserts the dev value is accepted and the
rejections hold.

## Repeat invocation and updates

- Rebuilding (new binary bytes) then re-running stages the new binaries
  through verify-then-swap while preserving the dev database,
  credentials, and instance identity. Close the previous dev Editor
  first: staging refuses while its Forge is live.
- To start over, delete `.dist/dev` (or pass a fresh `--dev-dir`).
- The real installed application is never addressed: `Layout::discover`
  inside the staged Editor sees only the explicit `ARTISAN_HOME`, and the
  debug CLI guard additionally refuses the installed home.

## Restarting the same dev installation

Closing the Editor kills its owned Forge, which then cannot remove its
readiness receipt — and the Forge's no-clobber publish refuses to
overwrite it, so the next launch died at stage 7 with a readiness
failure. The dev runner reconciles this under the staging lock before
spawning: a receipt identifying a **live** staged Forge is refused
(`previous dev Forge still running`); a receipt that parses as valid
Forge readiness but names no live staged Forge is stale — its
(pid, executable) identity provably describes no running owned process —
so the regular file is removed with a `removed stale readiness of dead
forge pid N` line; symlinks, reparse points, directories, oversized, or
malformed bytes are preserved and refuse the launch. Orphan publish
temporaries (`.artisan-forge-ready-<pid>-<seq>.tmp`) are swept; anything
else in the home is untouched. The runtime and CLI no-clobber invariants
are unchanged: the runner never writes a receipt, only removes a
proven-stale one, and credentials, database, and instance identity are
never re-minted by a restart.

## Failure behavior

| Symptom | Meaning | Action |
| --- | --- | --- |
| `dev binary missing: ...` | Not launched via `bazel run` and no `--bin-dir` | Use `bazel run //:dev`, `dev.ps1`, or pass `--bin-dir` |
| `dev staging is locked ...` | Another `dev` run is staging | Wait, or remove the lock only when no run is active |
| `previous dev Forge still running with pid ...` | Stale dev session owns the home | Close the previous dev Editor/Forge, then retry |
| `staged payload is not verified: ...` | Scratch tree drifted | Active version untouched; fix the cause and retry |
| `shipping manifest loader rejected the dev home: ...` | Manifest would not launch | Previous manifest restored; report |
| `existing dev instance is invalid: ...` | `instance-v2.json` corrupted | Delete the file (a fresh identity is minted; dev data stays) |
| `cannot clear stale startup receipt ...` | Unremovable file at the per-launch receipt path | Remove it by hand; the launch refuses rather than reading stale `ready` |
| `stale readiness at ... is malformed / is not a regular file / exceeds its size bound` | Unparseable or unsafe Forge receipt | Preserved; remove it by hand after confirming no Forge runs on the home |
| `editor startup not confirmed ...` / `stage 7/7 startup ... failed` | Receipt `failed`, timeout, or early exit | Stage detail names the phase; owned Editor already stopped |
| `path must be absolute` | Relative `--dev-dir` | Pass an absolute path |
| `invalid arguments: ...` | Unknown flag | See `--help` |

## Acceptance procedure (root gate)

1. `bazel build //:dev` — both binaries built as dependencies.
2. `bazel test //tests/native_dev/...` — args, layout, launch/receipt,
   manifest, instance-preservation, runfiles, and stage-coherence suites
   green (includes different source/staged directories, failed-update
   preservation, lock contention, receipt failure/timeout, `_main`
   runfiles and paths with spaces).
3. `bazel run //:dev -- --stage-only` — stages 1–6 print `ok`; rerun
   prints reused/preserved with no activation.
4. `bazel run //:dev` (or `scripts/dev.ps1`) — Editor window opens,
   stage 7 confirms `ready` (authenticated + initial queries), projects/
   threads load over QUIC. Exercise more than 32 requests (usage
   refreshes, catalog reads, composer sends) to prove the lifetime
   budget. Close the Editor; Forge stops; exit 0.
5. Kill the Editor process mid-startup once: stage 7 must fail honestly
   (timeout/early exit) with no orphan Forge.
6. Confirm `%LOCALAPPDATA%\Artisan Street` (or platform equivalent)
   untouched, and no `ARTISAN_DEV_FORGE_HOME` process remains.

No live end-to-end run is claimed by this packet: the real build,
launch, and QUIC connection await the root native gate.

## Ownership

- Owned: `scripts/native_dev/**`, `scripts/dev.ps1`,
  `tests/native_dev/**`, this runbook, root `BUILD.bazel` alias +
  aggregate entries, root `Cargo.toml` member, `/.dist/` ignore.
- Boundary edits to existing product code (explicit, minimal):
  - `modules/frontend/src/dev_startup_receipt.rs` (new): opt-in receipt,
    no-op without `ARTISAN_DEV_STARTUP_RECEIPT`, secret-free by
    construction, inline unit tests.
  - `modules/frontend/src/native_transport_service.rs`: four hook calls
    in `service_main` (ready after initial catalog; failed on delivery /
    catalog / startup errors) plus `StartupError::failure` widened to
    `pub(crate)`.
  - `modules/frontend/src/lib.rs` + `modules/frontend/BUILD.bazel`: one
    module registration line each.
  - The release installer's stage/activate machinery stays untouched: it
    serves signed releases (different schema, private API); the dev
    crate mirrors its tmp/previous/rename discipline locally instead of
    extracting it.
- `modules/cli`, `modules/installer` logic, and `native_profile_usage`
  are untouched.
- Known follow-ups for the root/Bazel worker: `cargo generate-lockfile`
  + `Cargo.Bazel.lock` repin for the new member (workspace-pinned
  `serde_json`/`thiserror`/`fs2`, `sha2 = "=0.10.9"`); then run the
  acceptance procedure above. `dev.ps1` (both its `cargo metadata
  --locked` target-dir probe and its `--locked` build) requires the
  re-resolved lock first. `dev.ps1` resolves the real Cargo target
  directory from metadata, so `CARGO_TARGET_DIR`, config `target-dir`
  (including relative dirs and paths with spaces), and the shared
  vendor cache all work.
