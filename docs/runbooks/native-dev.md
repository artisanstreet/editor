# Native development with Cargo and Nix

Cargo owns the Rust workspace and dependency graph. The flake pins Rust, native
libraries, Crane, build tools, generated bindings, packages and quality gates.
It defines Linux x86_64 and aarch64 outputs. Local validation is on x86_64 WSL;
aarch64 needs a native builder. A real desktop session is required for rendering.

## Start working

Install Nix with flakes enabled on Linux/WSL, then:

```sh
nix develop
cargo dev
```

This is the incremental Cargo workflow (`python3 scripts/dev.py` and, on
Windows, `scripts/dev.ps1` wrap the same command). `cargo dev` builds the four
product binaries, installs them as a signed `dev`-channel release into the
per-user **Artisan Street Dev** installation through the shipping installer
code, and launches the installed Editor, which starts its owned Forge through
the product APIs. The dev installation sits beside the real one
(`$XDG_DATA_HOME/Artisan Street Dev`, `%LOCALAPPDATA%\Artisan Street Dev`),
never inside it, and keeps its database, credentials, and instance identity
across runs.

Running `cargo dev` again while the dev Editor is open retires it the way an
update does and launches the new build, so every iteration exercises the real
update path. Every build is a distinct version such as
`0.0.0-dev.1284+g1a2b3c4d5e.dirty.b0123456789` and shows its channel and commit in
the window title and under Settings → About.

```sh
cargo dev                        # build, install, launch or relaunch
cargo dev stage                  # build and install without launching
cargo dev --profile performance  # optimized rendering (or --release)
cargo dev where                  # dev root, active version, build identity
cargo dev prune --keep 1         # drop superseded dev versions
cargo dev --root /abs/path       # a separate dev installation (or ARTISAN_DEV_ROOT)
```

Previous versions stay installed for rollback (three by default, `--keep N`).
`CARGO_TARGET_DIR` is respected; do not share target directories across
worktrees.

For automatic activation, install direnv, add its hook to your shell, and run
`direnv allow` in this checkout. `.envrc` enters the flake shell; it does not build
or launch the Editor. Configure nix-direnv on the host for cached activation.

The shell defaults to two Cargo jobs. Override `CARGO_BUILD_JOBS` when appropriate.
Nix builder concurrency is separate: on this WSL machine use `--max-jobs 1 --cores 2`.

## Build and launch Nix outputs

```sh
nix build .#development --max-jobs 1 --cores 2
nix run .#dev
nix build .#release --max-jobs 1 --cores 2
nix run .#editor
```

The `dev`, `performance`, and `editor` apps install their prebuilt binaries into
the same Artisan Street Dev installation as `cargo dev` (the root honors
`ARTISAN_DEV_ROOT`) and launch it. Store outputs remain immutable. Individual
raw `editor`, `forge`, `ae`, and `installer` packages are also available;
use the layout-aware app to launch the Editor. Helpers (`dev-launcher`,
`payload-manifest-generator`, `release-tool`, `capnp-codegen`) build independently.

Release installers embed the public values in
`modules/installer/release/trust_anchor.env`. The checked-in anchor is for
pre-release development only. Rotate it before production; see
[installer trust](../../modules/installer/RELEASE_TRUST.md). Missing anchors remain
compile errors. Private signing keys are never derivation inputs.

Crane vendors Cargo.lock dependencies, builds offline, and caches artifacts per
compatible Cargo profile. Embedded fonts, licenses, schemas, catalog JSON,
external tests and packaging contracts are included. Build products, evidence
and the local handoff ledger are excluded.

## Quality gates

```sh
nix flake check --keep-going --max-jobs 1 --cores 2 -L
nix build .#checks.x86_64-linux.codegen -L
nix fmt
# Incremental Cargo equivalent:
python3 scripts/check.py --runner nextest
```

Independent checks cover Rust/Nix formatting, Clippy, test registration, Python
tooling tests, the file-size ratchet, schema drift, serial unit/integration tests,
and documentation tests. The Nix test gate uses pinned Nextest, one test process at
a time, with a two-minute per-test timeout; timeouts fail the gate. Native
Windows can use the default Cargo runner (`python scripts/check.py`). Visual-proof compilation explicitly enables its Cargo
feature. Test fixtures use absolute paths to the three built backend examples;
packaging tests use bounded inputs and the real payload generator. The Python
check command stops at the first failed gate; Nix `--keep-going` reports independent
failures. No additional product tests are skipped. The timeout and process isolation behavior is
provided by [Nextest](https://nexte.st/docs/features/slow-tests/).

For individual Cargo backend tests, first build
`cargo build --locked -p artisan-backend --examples`. Fixtures are discovered
under the active profile's `examples` directory, or supplied through absolute
`ARTISAN_ENGINE_OWNER_FIXTURE`, `ARTISAN_CODEX_WIRE_FIXTURE`, and
`ARTISAN_DIRECTORY_CONTROLLER_FIXTURE` variables. Packaging tests require the
artifact variables that `scripts/check.py` supplies.

The file-size allowance can only shrink: run
`python3 scripts/file_size_ratchet.py --update` after extracting modules. Growth
or new oversized files fail even with `--update`. Existing baseline violations
and installer/UI failures remain visible until fixed in the product code.

## Schemas and maintenance

```sh
nix build .#generated-bindings
nix run .#codegen                 # from the repository root; updates all 3 schemas
nix run .#codegen -- --check      # compares without modifying files
nix run .#verify-gpui-pin         # runtime GitHub access; gh auth may be required
nix develop .#maintenance
nix flake update                # deliberately update pinned Nix inputs
```

Review generated bindings with schema changes. `Cargo.lock` pins Rust crates and
the compiler plugin; `rust-toolchain.toml` pins Rust; `flake.lock` pins Nixpkgs,
the Rust overlay and Crane. Update those separately and rerun the gates.

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

For native non-Nix packaging, use `python3 scripts/package.py --profile release`
in a suitable native build environment with the public trust variables exported.
That workflow keeps the existing archive contract; validate runtime dependencies
on the destination platform. The scripts also accept `--bin-dir`, `--generator`,
`--layout`, or `--tool` to consume prebuilt inputs without invoking Cargo.
`scripts/release_manifest.py` requires public metadata matching those binaries;
`packaging/release/development.json` is the Windows x64 example.

## Desktop and performance tools

```sh
nix run .#screen-demo
nix run .#capture-screen-demo -- --software --output evidence/visual/screen-demo.png
nix develop .#visual
nix develop .#performance
ARTISAN_FRAME_CAPTURE="$PWD/evidence/frame-capture.json" nix run .#performance
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
alongside results. Performance packages retain debug line tables. The performance shell supplies perf and hyperfine; WSL kernel
restrictions may limit perf. See [native performance](../native-performance.md).

## Windows, CI, caches and deployment

Native Windows uses Rust 1.98, Visual Studio C++ tools, Python 3.11+ and
`scripts/dev.ps1`, which imports the Visual Studio environment and runs
`cargo dev` (`-StageOnly`, `-Performance`, `-Release`; other arguments pass
through). A checkout reached over `\\wsl.localhost` builds under
`%LOCALAPPDATA%\Artisan Street Dev\build\<checkout-id>` because Cargo cannot
use a target directory on a network share. Nix manages WSL/Linux dependencies; the Windows SDK and Win32
capture remain native. CI runs a Windows Cargo lane and independent Linux flake
checks. Artifact jobs depend on both passing. Manual desktop workflows require
self-hosted runners labelled `artisan-desktop`, logged into a real desktop;
use only trusted refs on those hosts.

Optional cache setup: set repository variable `CACHIX_CACHE` to your cache name
and secret `CACHIX_AUTH_TOKEN` to its push token. CI configures that cache for
reads, then pushes successful outputs only after all gates on the default branch.
No cache account or token has been configured by this change. Developers can
configure that same cache with `cachix use <name>` after checking its public key.
Keep credentials in host/CI secret storage. The flake does not add unknown cache
keys to machine trust settings.

Remote builders are optional host configuration. For example, after setting up a
trusted SSH Nix builder, use
`nix build .#release --builders 'ssh-ng://builder x86_64-linux - 2 1'`.
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
