# Native development with Nix and Cargo

Nix orchestrates every build: it pins Rust, the native libraries, the Windows
cross toolchain, Crane, and the build tools, and it owns dependency caching,
build identity, packaging, and the quality gates. Cargo compiles Rust. The flake
defines Linux x86_64 and aarch64 outputs and cross-builds Windows x86_64 from
Linux; local validation is on x86_64 WSL. A real desktop session is required
for rendering.

## Start working

Install Nix with flakes enabled on Linux/WSL, then from the checkout:

```sh
nix run .#dev
```

`nix run .#dev` is the Rust dev runner (`scripts/native_dev`), not a wrapper
script. It builds the **Debug** stage of the checkout with Nix and deploys the
whole product through the shipping installer, in two halves:

1. **Forge** (Linux). The Linux payload installs as a signed `dev`-channel
   release into the per-user **Artisan Street Dev** installation
   (`$XDG_DATA_HOME/Artisan Street Dev`, default `~/.local/share`), retiring
   the running Forge the way an update does. The installed `ae` then
   configures the Forge (`ae setup ... --autostart --listen auto:4433
   --host-name <distribution>`) as the systemd user service
   `artisan-forge-dev.service`, which runs `<root>/bin/ae start --foreground`,
   and starts it (`ae start`). Once ready, the Forge publishes its private host
   invitation at `<root>/host.json`. The installation links `ae` into
   `~/.local/bin` (a new login shell puts it on `PATH`), so `ae status`,
   `ae doctor`, and `ae engine login claude` work without flags.
2. **Editor**. Inside WSL the Editor is the Windows build: the cross-built
   Windows runner runs through WSL interop, installs the Windows payload into
   `%LOCALAPPDATA%\Artisan Street Dev` (closing a running dev Editor first),
   registers the Forge's invitation exactly as **Add host from invitation…**
   does, and launches the installed Editor on that host. Outside WSL the Linux
   installation already holds the Editor, which is launched the same way.

Both installations sit beside the real ones, never inside them, and keep their
data across runs. Running `nix run .#dev` again upgrades both in place through
the real update path, so every iteration exercises it. Every build is a
distinct version such as `0.0.0-dev.1284+g1a2b3c4d5e.dirty.n0123456789`
(commit, dirty tree, and Nix output hash) and shows its channel and commit in
the window title and under Settings → About.

```sh
nix run .#dev                          # Debug stage: build, deploy, launch or relaunch
nix run .#dev -- --production          # Production stage
nix run .#dev -- stage                 # build and deploy; the Forge runs, the Editor is not launched
nix run .#dev -- where                 # both installations, active versions, the service
nix run .#dev -- prune --keep 1        # drop superseded versions on both sides
nix run .#dev -- --linux               # the Linux Editor (WSLg), also inside WSL
nix run .#dev -- --attach              # stay attached until the Editor exits
nix run .#dev -- --listen auto:4533 --host-name 'Ubuntu scratch'   # Forge address and name
nix run .#dev -- --root /abs/Linux/root --windows-root 'C:\abs\Windows\root'  # separate installations
```

The run returns once the Editor writes its startup receipt, which it does after
its first connection to the dev host completes the initial queries; without a
receipt it stops the Editor and fails after 90 seconds. In a terminal the
Editor keeps writing to it. When the run's output is not a terminal (an agent
shell, `| tee`, CI) the Editor is detached from it: on Unix its output goes to
`<dev root>/.dev-runner/editor.log`; on Windows it is started through the shell
and its output is not captured.

Flakes see tracked files only: `git add -N` new source files before building
(the runner refuses to build while `.rs`, `.toml`, or `.nix` files are
untracked). Previous versions stay installed for rollback (three by default,
`--keep N`). `nix`, `git`, and a systemd user manager (`systemctl --user`) are
required; Linux support is systemd-only.

### The dev Forge service

```sh
ae status
ae doctor                                  # includes the service line
systemctl --user status artisan-forge-dev.service
journalctl --user -u artisan-forge-dev.service
ae autostart --disable                     # stop the service and remove its unit
```

The unit is owned by the installation (`X-ArtisanInstallRoot=` names it):
`ae setup --autostart` rewrites it and restarts a running Forge when the
configuration changed, and never touches a unit of the same name it did not
write. The Forge stops on SIGINT; a readiness receipt left by a Forge that was
killed anyway is reconciled on the next start.

A separate `--root` gets its own unit name derived from the root's path, and
leaves the user environment alone: no `~/.local/bin/ae` link and no adoption
of an old Forge. Use it, with `--windows-root`, `--listen`, and
`--host-name`, for scratch deployments beside the real dev installation, and
remove them with `<root>/bin/ae autostart --disable` and by deleting the roots.

### Adopting a hand-deployed Forge

Before the product owned its service, a Forge was deployed by hand
(`scripts/install_forge_host.py`, since removed): `forge-host` from a pinned
store path, a hand-written `~/.config/systemd/user/artisan-forge.service`, and
its home in `~/.local/state/artisan-forge`. The first `nix run .#dev` for the
default installation adopts it, once:

- the old unit is stopped and disabled, and its custody lock proves the old
  Forge is gone;
- `forge.db` (with its WAL and shared memory) and `credentials/` are copied to
  `~/.local/state/artisan-forge/adopted-backup/`;
- the database is checkpointed, then the database, credentials (the host
  identity the Windows Editor's registration trusts), custody, model catalog,
  and the whole engine `toolchain/` move into the installation
  (`data/forge.sqlite3`, `credentials/`, `custody/forge.lock`, `data/`);
- the GC roots in `nix-roots/` are removed, and the unit, its drop-ins, and its
  saved copies move into the backup (`adopted-backup/systemd/`);
- a hand-added Artisan `ae` is removed from the Nix profile;
- `ADOPTED.json` marks the old home as a backup; later runs skip it.

The new service keeps the certificate, so the Windows Editor's existing
registration of the host stays valid; the runner registers the new invitation
path so the Editor refreshes from it. A machine without the old layout adopts
nothing.

### Stages

Both stages compile with the same optimization (Cargo profiles `production`
and `production-debug`): opt-level 3, fat LTO, one codegen unit, `panic =
"abort"`, mimalloc, and an x86-64-v3 CPU baseline. They differ only in what is
added for debugging:

| Stage | Cargo profile | Adds |
| --- | --- | --- |
| Debug | `production-debug` | full debug info, debug assertions, overflow checks, the GPUI inspector (`artisan-frontend/debug-tools`) |
| Production | `production` | line tables only, stripped, the release trust anchor |

`nix build .#linux-debug`, `.#linux-production`, `.#windows-debug`, and
`.#windows-production` build the payloads directly: `bin/{editor,forge,ae,installer}`
plus `resources/build-info.json`. Windows executables link the C and GCC
runtimes statically and import only Windows system DLLs.

For step-through work on a single crate, `nix develop` provides the pinned
toolchain for plain Cargo (`cargo build`, `cargo test`). Cargo output is never
launched as the product; `nix run .#dev` is the only launch path.

For automatic activation, install direnv, add its hook to your shell, and run
`direnv allow` in this checkout. `.envrc` enters the flake shell; it does not build
or launch the Editor. Configure nix-direnv on the host for cached activation.

The shell defaults to two Cargo jobs. Override `CARGO_BUILD_JOBS` when appropriate.
Nix builder concurrency is separate: on this WSL machine use `--max-jobs 2 --cores 3`
(for example through `NIX_CONFIG`, which `nix run .#dev` passes to its build).

## Nix outputs

Tools (`payload-manifest-generator`, `release-tool`, `capnp-codegen`,
`forge-host` (the remote-host check's fixture), `screen-demo`) build independently in Cargo's dev profile, each
against its own dependency graph.

Release installers embed the public values in
`modules/installer/release/trust_anchor.env`. The checked-in anchor is for
pre-release development only. Rotate it before production; see
[installer trust](../../modules/installer/RELEASE_TRUST.md). Missing anchors remain
compile errors. Private signing keys are never derivation inputs.

Crane vendors Cargo.lock dependencies, builds offline, and caches dependency
artifacts per stage and platform. Embedded fonts, licenses, schemas, catalog JSON,
external tests and packaging contracts are included. Build products, evidence
and the local handoff ledger are excluded.

## Quality gates

```sh
nix flake check --keep-going --max-jobs 1 --cores 2 -L
nix build .#checks.x86_64-linux.codegen -L
nix fmt
```

Independent checks cover Rust/Nix formatting, Clippy, test registration, Python
tooling tests, the file-size ratchet, schema drift, serial unit/integration tests,
and documentation tests. The Nix test gate uses pinned Nextest, one test process at
a time, with a two-minute per-test timeout; timeouts fail the gate. The native
Windows CI lane runs `python scripts/check.py --tests-only` with the default
Cargo runner. Visual-proof compilation explicitly enables its Cargo
feature. Test fixtures use absolute paths to the three built backend examples;
packaging tests use bounded inputs and the real payload generator. Nix
`--keep-going` reports independent failures. No additional product tests are skipped. The timeout and process isolation behavior is
provided by [Nextest](https://nexte.st/docs/features/slow-tests/).

For individual Cargo backend tests, first build
`cargo build --locked -p artisan-backend --examples`. Fixtures are discovered
under the active profile's `examples` directory, or supplied through absolute
`ARTISAN_ENGINE_OWNER_FIXTURE`, `ARTISAN_CODEX_WIRE_FIXTURE`, and
`ARTISAN_DIRECTORY_CONTROLLER_FIXTURE` variables. Packaging tests require the
archive variables that the `tests` and `packaging` checks supply.

The file-size allowance can only shrink: run
`python3 scripts/file_size_ratchet.py --update` after extracting modules. Growth
or new oversized files fail even with `--update`. Existing baseline violations
and installer/UI failures remain visible until fixed in the product code.

## Schemas and maintenance

```sh
nix build .#generated-bindings
nix run .#codegen                 # rewrites the checked-in bindings for all 3 schemas
nix build .#checks.x86_64-linux.codegen   # compares without modifying files
nix run .#verify-gpui-pin         # runtime GitHub access; gh auth may be required
nix develop .#maintenance
nix flake update                # deliberately update pinned Nix inputs
```

Review generated bindings with schema changes. `Cargo.lock` pins Rust crates and
the compiler plugin; `rust-toolchain.toml` pins Rust; `flake.lock` pins Nixpkgs,
the Rust overlay and Crane. Update those separately and rerun the gates.

## Claude thinking captures

Recapture before moving `CLAUDE_THINKING_DISPLAY_VERSION` or changing Claude
thinking decoding (see the [decision](../decisions/CLAUDE_THINKING_DISPLAY.md)).
Use the CLI Artisan resolves (`claude --version`), a signed-in account, and only
effect-free prompts; each capture is a billed turn and adaptive thinking may
skip a turn, so repeat until the scenario thinks.

```sh
S=$(python3 -c 'import uuid; print(uuid.uuid4())')
python3 scripts/claude_thinking_capture.py capture --out /tmp/start.jsonl --session "$S" \
  --prompt 'Read numbers.txt, think about which two values sum closest to 50, then answer in one sentence. Do not modify any file.'
python3 scripts/claude_thinking_capture.py capture --out /tmp/resume.jsonl --session "$S" --resume \
  --prompt 'Now think carefully about which pair sums closest to 60. One sentence.'
python3 scripts/claude_thinking_capture.py capture --out /tmp/flagless.jsonl --display '' --prompt '...'
python3 scripts/claude_thinking_capture.py sanitize /tmp/resume.jsonl \
  tests/fixtures/claude/summarized-resume.jsonl --session fixture-session-resume
```

`capture` uses Artisan's managed argv plus `--thinking-display` and writes one
`{"t_ms", "frame"}` record per stdout line; `sanitize` removes signatures,
identifiers, paths, and environment inventories while keeping frame order.
Record CLI version, model, argv, date, provider/auth category, and first-summary
timing in `tests/fixtures/claude/manifest.json`, keep `constructed-*` edge
fixtures labeled, then run `cargo test -p artisan-backend engine_owner_claude`.

## Packaging and signing

```sh
nix build .#nix-payload --out-link result-payload
nix build .#unsigned-release --out-link result-manifest
nix run .#export-closure -- "$PWD/artisan-linux.nar"
nix run .#sign-release -- --manifest "$PWD/result-manifest" \
  --key-file /absolute/private/seed --key-id development --output /absolute/signed.json
```

`nix-payload` is a deterministic ZIP of the four Nix-linked binaries and their
integrity manifest. It requires their Nix store closure; it is **not a standalone
Linux portable distribution**. `unsigned-release` describes this exact archive
with Linux, the matching CPU architecture and glibc metadata. No private key is
used. `export-closure` writes a NAR export containing the release app and runtime
closure; import it on a compatible Nix machine with `nix-store --import < file`.
Keep an installed profile or GC root for outputs you need to retain.

`payload-manifest-generator --archive` writes the archive (stored members in
lexical order, fixed metadata, byte-reproducible); `release-tool generate`
describes it from the public metadata in `packaging/release/development.json`
with the platform fields overridden.

## Desktop and performance tools

```sh
nix run .#screen-demo
nix run .#capture-screen-demo -- --software --output evidence/visual/screen-demo.png
nix develop .#visual
nix develop .#performance
ARTISAN_FRAME_CAPTURE="$PWD/evidence/frame-capture.json" nix run .#dev -- --production
```

`capture-screen-demo` launches the synthetic transcript harness through X11 or
XWayland and captures only its own process's window. This path was validated on WSLg with
a 1100×700 software-rendered capture. It closes that process after
capture, has a 45-second window-discovery deadline, and uses pinned xdotool and
ImageMagick. `--software` selects pinned Mesa lavapipe; omit it to exercise the
host GPU. This is an OS window capture, not GPUI's internal readback proof.

The `parity-visual-proof` package and `visual-proof`/`visual-proof-software` apps
compile the explicit case/viewport harness and retain its 45-second external
timeout. The pinned GPUI fork currently returns `render_to_image not implemented
for this platform` on Linux: those apps cannot yet produce its internal PNG
proof here. Its 17 fixture tests pass independently. Use the Linux window-capture
app or the native Windows capture workflow for current screenshots.

Graphical wrappers include pinned Mesa and add WSLg/NixOS driver library locations
when present. Embedded fonts are pinned by the source. Performance captures remain
outside the store; record GPU, driver, display refresh, workload and environment
alongside results. Production payloads retain debug line tables. The performance shell supplies perf and hyperfine; WSL kernel
restrictions may limit perf. See [native performance](../native-performance.md).

## Windows, CI, caches and deployment

Windows builds are cross-compiled from Linux/WSL by Nix (Rust target
`x86_64-pc-windows-gnu`, the nixpkgs MinGW-w64 toolchain); no Visual Studio
is needed. Inside WSL, `nix run .#dev` runs the Windows dev runner through WSL
interop, which installs into `%LOCALAPPDATA%\Artisan Street Dev`. Win32
capture (`scripts/verify_visual.ps1`) remains native and stages its build with
`wsl nix run .#dev -- stage`. CI runs a native Windows Cargo lane and
independent Linux flake checks. Artifact jobs depend on both passing. Manual
desktop workflows require self-hosted runners labelled `artisan-desktop`,
logged into a real desktop; use only trusted refs on those hosts.

Optional cache setup: set repository variable `CACHIX_CACHE` to your cache name
and secret `CACHIX_AUTH_TOKEN` to its push token. CI configures that cache for
reads, then pushes successful outputs only after all gates on the default branch.
No cache account or token has been configured by this change. Developers can
configure that same cache with `cachix use <name>` after checking its public key.
Keep credentials in host/CI secret storage. The flake does not add unknown cache
keys to machine trust settings.

Remote builders are optional host configuration. For example, after setting up a
trusted SSH Nix builder, use
`nix build .#linux-production --builders 'ssh-ng://builder x86_64-linux - 2 1'`.
An aarch64 output needs an aarch64 builder or explicitly configured emulation.
Do not commit SSH credentials or host-specific builder addresses.

`nix build .#forge-container` produces a container image with its runtime closure.
Load it with Docker/Podman, mount a writable `/data` owned by UID 65532, provision
credentials at absolute runtime paths, and supply Forge's complete argument list.
No ports, credentials, or deployment policy are invented by the image.

`nixosModules.forge` is opt-in. Import it, enable `services.artisan-forge`, supply
absolute **string** paths for `certificateFile`, `privateKeyFile` and
`bootstrapCapabilityFile`, and set every listener/native-run option in `policy`.
Systemd loads credentials at runtime; the module owns `/var/lib/artisan-forge`
and `/run/artisan-forge`. It preserves Forge's existing listener/custody contract.
No service has been deployed. Select and validate policy on the target host.
