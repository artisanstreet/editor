# Native dev runbook — `bazel run //:dev`

One command builds the native Editor frontend and the Forge backend through
the authoritative Bazel graph, stages them into an isolated development
installation under `<workspace>/.dist/dev`, provisions that home through the
existing CLI custody APIs, and launches the staged Editor. The Editor then
runs its unchanged shipping startup: it starts its newly owned Forge and
connects over authenticated QUIC, exactly like the installed product.

## Usage

```text
bazel run //:dev
bazel run //:dev -- --stage-only
bazel run //:dev -- --dev-dir C:\scratch\artisan-dev --stage-only
bazel run //:dev -- --bin-dir <dir-with-ae-editor-forge-installer> --stage-only
```

Flags (after the `--` Bazel passthrough separator):

| Flag | Effect |
| --- | --- |
| `--dev-dir PATH` | Isolated root. Default: `<workspace>/.dist/dev` (`BUILD_WORKSPACE_DIRECTORY`, else current directory). Must be absolute. |
| `--bin-dir PATH` | Use prebuilt `ae`/`editor`/`forge`/`installer` binaries from `PATH` instead of the Bazel runfiles search. Root uses this for validation without a full `bazel run`. |
| `--stage-only` | Stage and provision without launching the Editor. |
| `--help` / `-h` | Print usage. Exit code 2 on invalid flags, 1 on stage failure. |

## What one run does

Seven plain-text stages, no TTY codes, suitable for piping:

1. **resolve** — derive `<dev>`, `<dev>/home`, `<dev>/home/versions/dev`.
2. **binaries** — locate `ae`, `editor`, `forge`, `installer` (`--bin-dir`,
   then `$RUNFILES_DIR`, then `$RUNFILES_MANIFEST_FILE`, then the
   `bazel-bin` sibling layout). These are `data` dependencies of
   `//scripts/native_dev:dev`, so `bazel run //:dev` builds both product
   binaries before staging.
3. **stage** — copy each binary into `<home>/versions/dev/bin` (plus the
   permanent `<home>/bin/ae` launcher), skipping byte-identical files so a
   repeat run leaves a live dev session undisturbed. Refuses early when the
   dev readiness receipt identifies a still-running dev Forge.
4. **manifest** — write `<home>/installation.json` (`active`/`complete`,
   `active_version: "dev"`) and validate it with the shipping
   `InstallationManifest::load`.
5. **payload** — write `payload-manifest.json` with fresh SHA-256 hashes and
   gate on the shipping `payload::verify` (`Verified`, or fail with the
   drift list).
6. **provision** — `credentials::provision_or_load` plus a
   `NativeInstanceConfig` write. Reuses the existing instance identity,
   credentials, database, and dev data on repeat runs; mints only on first
   run.
7. **editor** (skipped by `--stage-only`) — spawn the staged Editor with
   `ARTISAN_HOME=<home>`, `ARTISAN_DEV_FORGE_HOME` /
   `ARTISAN_DEV_FORGE_READY_FILE` stripped, streams inherited. The Editor
   verifies the payload, starts its owned Forge (`start_owned` lease with
   Job Object containment), and connects over authenticated QUIC. Closing
   the Editor stops its owned Forge; the dev exit code follows the Editor.

## Relation to the Electron dev runner and checklist

The legacy `.scripts/dev/runner.ts` supervised a Forge Rolldown watcher
plus the Vite frontend with worktree-derived ports, a pairing secret, and
`@artisanstreet/checklist` lane presentation. The native counterpart keeps
the shape but not the machinery:

| Electron runner | Native `dev` |
| --- | --- |
| Forge watcher build + Vite dev server | Bazel `data` deps on `//modules/backend:forge` + `//modules/frontend:editor` |
| Worktree-derived ports | No ports: QUIC loopback endpoint comes from the owned readiness receipt |
| Pairing secret / same-origin codes | No secret: QUIC bootstrap capability + certificate pin from provisioned custody |
| `@artisanstreet/checklist` TUI lanes | Numbered `dev: stage i/N ... ok` plain lines (no TTY codes by design) |
| Manual `ARTISAN_DEV_FORGE_HOME` backend | Owned Forge started by the Editor itself; the manual escape hatch is stripped |

No Node/Cargo wrapper performs the authoritative build: Bazel owns it, and
the only new executable is the small native `scripts/native_dev` crate, as
the Rust port plan allows.

## Repeat invocation and updates

- Rebuilding (new binary bytes) then re-running stages the new binaries
  while preserving the dev database, credentials, and instance identity.
  Close the previous dev Editor first: staging refuses while its Forge is
  live, and on Windows a running binary cannot be overwritten anyway.
- To start over, delete `.dist/dev` (or pass a fresh `--dev-dir`).
- The real installed application is never addressed: `Layout::discover`
  inside the staged Editor sees only the explicit `ARTISAN_HOME`, and the
  debug CLI guard additionally refuses the installed home.

## Failure behavior

| Symptom | Meaning | Action |
| --- | --- | --- |
| `dev binary missing: ...` | Not launched via `bazel run` and no `--bin-dir` | Use `bazel run //:dev` or pass `--bin-dir` |
| `previous dev Forge still running with pid ...` | Stale dev session owns the home | Close the previous dev Editor/Forge, then retry |
| `cannot stage ... (a previous dev ... may still run from it)` | Destination locked | Same as above |
| `staged payload is not verified: ...` | Staged tree drifted (extra files, tampered binary) | Delete `<home>/versions/dev` and retry |
| `shipping manifest loader rejected the dev home: ...` | Manifest would not launch | Report; do not hand-edit `installation.json` |
| `existing dev instance is invalid: ...` | `instance-v2.json` corrupted | Delete the file (a fresh identity is minted; dev data stays) |
| `path must be absolute` | Relative `--dev-dir` | Pass an absolute path |
| `invalid arguments: ...` | Unknown flag | See `--help` |

## Acceptance procedure (root gate)

1. `bazel build //:dev` — both binaries built as dependencies.
2. `bazel test //tests/native_dev/...` — argument, layout, manifest,
   instance-preservation, and runfiles tests green.
3. `bazel run //:dev -- --stage-only` — stages 1–6 print `ok`; rerun prints
   reused/preserved; `payload::verify` path exercised in-tree.
4. `bazel run //:dev` — Editor window opens, Forge starts owned (readiness
   appears under `.dist/dev/home/readiness`), projects/threads load over
   QUIC. Close the Editor; Forge stops; exit 0.
5. Confirm `%LOCALAPPDATA%\Artisan Street` (or platform equivalent)
   untouched, and no `ARTISAN_DEV_FORGE_HOME` process remains.

No live end-to-end run is claimed by this packet: Bazel is absent from the
implementation host, so stages 1–6 are covered by the new `tests/native_dev`
suite while the real build, launch, and QUIC connection await the root
native gate.

## Ownership

- Owned: `scripts/native_dev/**`, `tests/native_dev/**`, this runbook,
  root `BUILD.bazel` alias + aggregate entries, root `Cargo.toml` member.
- Boundary edits to existing product code: **none**. `modules/cli`,
  `modules/installer`, and `modules/frontend` are untouched; the existing
  profile-usage path (`native_profile_usage`) is retained verbatim. The dev
  crate only calls public CLI APIs (`manifest`, `payload`, `credentials`,
  `instance`, `process::readiness_status`).
- Known follow-ups for the root: repin `Cargo.Bazel.lock` for the new
  member on a Bazel host, then run the acceptance procedure above.
