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

Nix builds the **Debug** stage payload for the current platform (inside WSL:
Windows, cross-built with MinGW-w64), then the dev runner installs it as a
signed `dev`-channel release into the per-user **Artisan Street Dev**
installation through the shipping installer code and launches the installed
Editor, which starts its owned Forge through the product APIs. The dev
installation sits beside the real one (`$XDG_DATA_HOME/Artisan Street Dev`,
`%LOCALAPPDATA%\Artisan Street Dev`), never inside it, and keeps its database,
credentials, and instance identity across runs.

Running it again while the dev Editor is open retires it the way an update does
and launches the new build, so every iteration exercises the real update path.
Every build is a distinct version such as
`0.0.0-dev.1284+g1a2b3c4d5e.dirty.n0123456789` (commit, dirty tree, and Nix
output hash) and shows its channel and commit in the window title and under
Settings → About.

```sh
nix run .#dev                        # Debug stage: build, install, launch or relaunch
nix run .#dev -- --production        # Production stage
nix run .#dev -- stage               # build and install without launching
nix run .#dev -- where               # dev root, active version, build identity
nix run .#dev -- prune --keep 1      # drop superseded dev versions
nix run .#dev -- --linux             # the Linux build, also inside WSL
nix run .#dev -- --root /abs/path    # a separate dev installation
nix run .#dev -- --attach            # stay attached until the Editor exits
```

`nix run .#dev` returns once the Editor writes its startup receipt, which it does
after its first host connection completes the initial queries, whichever host
it opened (a registered host, or the owned dev Forge when none is registered);
without a receipt it stops the Editor and fails after 90 seconds. In a terminal
the Editor keeps writing to it. When the run's output is not a terminal (an
agent shell, `| tee`, CI) the Editor is detached from it, so the caller sees
end-of-file when the runner returns: on Unix its output goes to
`<dev root>/.dev-runner/editor.log`; on Windows it is started through the shell
and its output is not captured.

Flakes see tracked files only: `git add -N` new source files before building
(the app refuses to build while `.rs`, `.toml`, or `.nix` files are untracked).
Previous versions stay installed for rollback (three by default, `--keep N`).

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
Nix builder concurrency is separate: on this WSL machine use `--max-jobs 1 --cores 2`.

## Nix outputs

Tools (`payload-manifest-generator`, `release-tool`, `capnp-codegen`,
`forge-host`, `screen-demo`) build independently in Cargo's dev profile, each
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
