# Nix tooling audit

Audited the current WSL working tree on 2026-09-13, including the uncommitted Cargo migration. The recommendations below have now been implemented in `nix/workspace.nix`, `nix/forge-service.nix`, the Python tooling, and CI workflows. See the [current runbook](runbooks/native-dev.md) for actual commands, validation limitations, and host setup. The table preserves the original findings as the migration rationale.

At audit time, `flake.nix` defined only a Linux development shell for x86_64 and aarch64. It pins Rust through `rust-toolchain.toml` and supplies native libraries and build tools. It has no application packages, apps, checks, formatter, or deployment modules. There is no checked-in `.github` workflow directory, `.envrc`, or dedicated `nix/` directory in this tree.

Nix should declare tool versions, build inputs, reproducible artifacts, and runnable tooling environments. Cargo should retain the Rust workspace and dependency graph. Application data and operations that require a real desktop, credentials, or network access need explicit runtime entry points.

| Priority | Site and current evidence | Recommended Nix integration |
| --- | --- | --- |
| First | [flake.nix](../flake.nix), [flake.lock](../flake.lock), [rust-toolchain.toml](../rust-toolchain.toml) | Keep one shared toolchain/native-library definition and use it for both development and package builds. Pin a Cargo build integration such as Crane. Derive development inputs from build/check definitions to prevent drift. |
| First | Repository entry; `.envrc` absent | Add automatic shell activation through direnv. Keep installation of Nix and shell-level direnv integration as host bootstrap steps; entering the repository must not automatically build or launch the application. |
| First | [Cargo.toml](../Cargo.toml), Cargo.lock, [scripts/build_support.py](../scripts/build_support.py) | Add a workspace source definition and cached dependency derivation. Include out-of-crate tests, schemas, embedded assets, fonts, catalog JSON, licenses, and packaging contracts. A Rust-only source filter would drop required inputs. Exclude `target`, `.dist`, local evidence, and the untracked handoff ledger. Fetch dependencies as declared inputs, then build offline. Keep the Python wrapper useful for incremental local Cargo builds. |
| First | [modules/frontend/Cargo.toml](../modules/frontend/Cargo.toml), [modules/backend/Cargo.toml](../modules/backend/Cargo.toml), [modules/cli/Cargo.toml](../modules/cli/Cargo.toml), [modules/installer/Cargo.toml](../modules/installer/Cargo.toml) | Define explicit `editor`, `forge`, `ae`, and `installer` package outputs, with a common artifact cache where profiles/features/targets are compatible. Separate development and release builds; avoid recompiling the entire workspace merely to obtain one helper. |
| First | [scripts/check.py](../scripts/check.py) and [tests](../tests) | Declare independent formatting, Clippy, unit/integration, and doc-test checks. Preserve serial execution where process fixtures require it. Give tests writable temporary state and explicit tool paths; validate socket/process tests in the sandbox before assigning them to ordinary derivations. Keep real-display or authenticated tests in separate runtime/CI jobs. |
| First | [scripts/codegen.py](../scripts/codegen.py), [scripts/capnp_codegen](../scripts/capnp_codegen), [modules/protocol/schema](../modules/protocol/schema) | Package the Cargo-pinned compiler plugin and Nix-pinned `capnp` together. Generate bindings into a build output and compare them with checked-in files in a drift check. Expose a separate codegen app for intentionally updating the working tree. Builds must not rewrite checkout files. |
| First | [scripts/audit_rust_target_registration.py](../scripts/audit_rust_target_registration.py), its [PowerShell wrapper](../scripts/audit_rust_target_registration.ps1), [tests/policy/dependencies.rs](../tests/policy/dependencies.rs) | Expose the registration audit as a flake check with declared Python and Cargo. Retain the dependency-policy test under Cargo. Update the policy test's hand-maintained manifest list to include `packaging` and `scripts/screen_demo`; the current list omits those workspace additions. |
| First | [scripts/file-size-ratchet.ps1](../scripts/file-size-ratchet.ps1), [scripts/file-size-allowlist.txt](../scripts/file-size-allowlist.txt) | Add the ratchet to the checks. It is currently absent from `check.py`, and PowerShell is absent from the Nix shell. Prefer a portable implementation or first verify the existing script on Linux. Review its Windows path assumptions and generated-file exclusions, which currently name `artisan_capnp.rs` and `composer_state_capnp.rs`. Updating the allowlist stays an explicit maintenance action. |
| First | [scripts/package.py](../scripts/package.py), [packaging/portable](../packaging/portable), [packaging/Cargo.toml](../packaging/Cargo.toml) | Package `payload-manifest-generator` and add a payload-archive derivation consuming already-built binaries. Use Nix output paths instead of discovering `target/debug`. Retain the existing manifest, ZIP layout, deterministic metadata, and tampering tests. A Nix-installed bundle and a portable archive are separate outputs: copying Nix-linked executables into a ZIP does not make them runnable on machines without their Nix runtime dependencies. |
| First | [modules/installer/RELEASE_TRUST.md](../modules/installer/RELEASE_TRUST.md), [trust_anchor.env](../modules/installer/release/trust_anchor.env), [manifest.rs](../modules/installer/rust/manifest.rs) | Make the public release key ID and public key explicit release derivation inputs. Preserve the compile-time failure for missing anchors. Do not silently use a development mode marker to make a release package build. |
| Next | [scripts/dev.py](../scripts/dev.py), [.cargo/config.toml](../.cargo/config.toml), [scripts/native_dev](../scripts/native_dev) | Add a `dev` app supplying the pinned tools and optionally prebuilt product inputs. Keep `.dist/dev`, home provisioning, databases, and credentials outside build derivations and the Nix store. The existing native launcher should continue to own staging and startup confirmation. A plain `nix run .#editor` needs an installation-layout-aware wrapper, not just the raw executable. |
| Next | [scripts/release_manifest.py](../scripts/release_manifest.py), [packaging/release/development.json](../packaging/release/development.json), [release_tool.rs](../packaging/release/rust/release_tool.rs) | Package `release-tool`; generate unsigned metadata from an exact archive and explicit public metadata. Derive or validate platform/architecture rather than applying the Windows x64 development example to Linux output. Expose signing as a runtime app with a key path supplied at invocation; private signing material must never become a store input. |
| Next | [scripts/verify-gpui-pin.ps1](../scripts/verify-gpui-pin.ps1), [docs/gpui-fork.md](gpui-fork.md) | Declare `gh` and the script runtime in a maintenance environment, or port the small verifier to Python. The current shell supplies neither `gh` nor PowerShell. This remote reachability check needs authenticated GitHub access and belongs in an explicit maintenance app or CI step, not a pure cached build check. Successful fetching of the pinned revision does not prove ancestry on the integration branch. |
| Next | [scripts/screen_demo](../scripts/screen_demo), [frontend Cargo feature/example](../modules/frontend/Cargo.toml), [parity_visual_proof.rs](../modules/frontend/src/parity_visual_proof.rs) | Package `screen-demo` and the `parity-visual-proof` example separately and expose visual-test apps. The latter requires `visual-proof`; ordinary `--all-targets` does not enable that feature. Pin fonts and capture dependencies, provide an external timeout, and use a validated desktop or software-rendering environment. A successful headless compile is not a rendering proof. |
| Next | [scripts/verify_visual.ps1](../scripts/verify_visual.ps1) | Retain its Win32 capture path for Windows. Add a Linux/WSLg capture entry point using the pinned visual tooling if Linux captures are needed. Installing PowerShell in Nix cannot provide `user32.dll`, Windows Forms, or the Windows compositor. |
| Next | [docs/native-performance.md](native-performance.md), Cargo's `performance` profile | Expose a performance app/shell that selects the optimized profile and declares any chosen profiling tools. Preserve explicit `ARTISAN_FRAME_CAPTURE*` inputs and write results outside the store. GPU, driver, monitor, desktop state, and workload still need to be recorded alongside results. |
| Next | CI workflows absent from this checkout | Add native Linux jobs using the same flake checks and package outputs. Publish artifacts only from successful jobs. Retain an explicit native Windows lane for MSVC builds, Windows process behavior, and capture tests. Add macOS support only with its own declared inputs and validation; the current flake is Linux-only. |
| Next | [flake.nix](../flake.nix), [scripts/build_support.py](../scripts/build_support.py), future CI configuration | Add a trusted binary cache for successful Nix derivations and optionally remote builders. Reuse dependency artifacts across compatible builds/checks. Nix does not cache ordinary Cargo `target` directories automatically, and local Cargo jobs and Nix builder concurrency need separate resource limits. |
| Next | [README.md](../README.md), [development runbook](runbooks/native-dev.md), [native-performance.md](native-performance.md), [gpui-fork.md](gpui-fork.md), [installer trust documentation](../modules/installer/RELEASE_TRUST.md) | Document canonical development, build, check, codegen, package, signing, visual, and update commands once outputs exist. Add a pinned Nix formatter/check. Keep host bootstrap and Windows instructions explicit, and keep proposed command names clearly separate from implemented ones. |
| Optional | [modules/backend](../modules/backend), [modules/cli](../modules/cli); no deployment definition found | If Forge is deployed to managed Linux hosts, add a NixOS service module and/or container package. Preserve the existing configured listener, QUIC transport, credentials, process ownership, and mutable database paths. Declare deployment policy only when an actual deployment target is chosen. |

The workspace has 17 members. The remaining library members—assets, catalog, database, domain, migrations, native_engine, protocol, transport, and ui—should be covered through the common Cargo build/check graph. They do not each need a separately maintained Nix package or a second dependency manifest.

The three backend child programs (`engine-owner-fixture`, `codex-wire-fixture`, and `directory-controller-fixture`) need explicit test artifacts and paths. The process and repository tests also invoke shell utilities and Git: the Nix test environment should declare those tools rather than inherit a workstation PATH. Product adapters under `modules/native_engine`, however, should continue to discover/manage the user's selected harnesses through their existing contracts. Ordinary Editor/Forge users should not acquire a new Nix requirement.

A useful proposed interface is `nix develop`, `nix build .#editor`, `nix build .#forge`, `nix build .#payload`, `nix flake check`, and runtime apps such as `nix run .#dev`, `.#codegen`, `.#verify-gpui-pin`, and `.#visual-proof`. These interfaces now exist; the explicitly Nix-dependent archive is named `nix-payload`, not `payload`. Cache credentials, native desktop runners, remote builders, and deployment hosts remain operator configuration.

The recommended order is shared build inputs/source selection, explicit packages and codegen, offline checks, release artifacts, then apps/CI/cache. Carry forward the known installer and UI test failures from the migration rather than suppressing them to obtain a green Nix check. This audit did not rerun those suites or investigate their causes.

Crane's reference setup demonstrates cached Cargo dependencies, separate build/check derivations, and shared development inputs; it is now the pinned integration used by this repository. [Crane quick start](https://crane.dev/examples/quick-start.html). Automatic shell activation is supported through direnv. [Nix environment activation](https://nix.dev/guides/recipes/direnv.html). Sharing successful store outputs can use an HTTP binary cache. [Nix binary cache setup](https://nix.dev/tutorials/nixos/binary-cache-setup.html).

Implementation notes: the Linux OS capture entry point is `capture-screen-demo`.
The pinned GPUI fork lacks Linux `render_to_image`, so the separately packaged
parity harness remains unable to provide internal PNG readback on Linux. The
Nix test gate uses pinned Nextest with serial processes and failing timeouts after
an ordinary Cargo run stalled in a UI asset test. Existing failing assertions
remain visible; these tooling changes do not certify a green product suite.

## Implementation validation

Validated in x86_64 WSL on 2026-09-13:

- Flake outputs evaluate for both Linux architectures. Rust/Nix formatting,
  workflow linting, test registration, Clippy, schema drift, and five Python
  tooling tests pass. Documentation tests pass.
- All 36 packaging tests and 17 visual-fixture tests pass. Prebuilt packaging
  produced identical ZIP bytes on repeated assembly; runtime signing passed
  with a disposable key outside the store.
- Development and release app staging each provisioned an isolated temporary
  home, staged all four binaries, and passed the shipping installation-manifest
  loader.
- The final release ZIP is 105.9 MiB. All four payload hashes, ZIP CRCs, unsigned
  manifest hash/size/platform, runtime closure (including Mesa and Git), and
  non-root Forge container configuration were verified.
- A real 1100×700 X11/WSLg window capture succeeded with pinned Mesa lavapipe.
  GPUI's internal parity readback remains unavailable on Linux.

The full test gate is **not green**: the original Cargo run exposed installer,
backend, migration, CLI, and UI failures, then stalled in a UI asset test.
The serial Nextest run completed with failure. Its per-test timeout reports hangs
as failures, and its Nix counter output preserves diagnostics in build logs.
Examples of existing failures include installer path-identity substitution,
backend fixtures comparing independently sampled timestamps, migration tests
expecting four migrations where fourteen now exist, and UI layout assertions.
The file-size gate reports 20 existing allowance violations; no allowance was
increased. The changed module declaration ordering in `native_run_dispatch.rs`
does not change its pre-existing 885-line size.

Native Windows execution, aarch64 builds, performance-profile execution, and a
live NixOS deployment were not validated on this host. The NixOS service was
configuration-evaluated, not deployed. Cache credentials, optional remote
builders, authenticated GPUI reachability checks, and desktop CI runners require
operator setup; no artifacts were published and no service was deployed.
