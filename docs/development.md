# Development

The engineering rules live in `AGENTS.md`; this page covers how to build, run and test.

## Prerequisites

- Windows 11 with WSL 2. Forge and `ae` build and run inside WSL; the Aqua Editor is a native Windows executable cross-compiled from WSL.
- Nix with flakes enabled (`experimental-features = nix-command flakes`). The flake provides the pinned Rust toolchain (`rust-toolchain.toml`), the MinGW cross toolchain and every other build tool.
- `cargo-nextest` for the test suites.
- Forge-only work also builds on plain Linux (`--linux`).

## The dev loop

Run everything from the checkout root. `nix run .#dev` takes a command and options:

| Command | Builds | Installs |
| --- | --- | --- |
| `run` (default) | Forge, `ae`, the Windows Editor | Forge and the Editor |
| `forge` | Forge and `ae` | Forge only |
| `editor` | `ae` and the Windows Editor | the Editor only |
| `build` | Forge, `ae`, the Windows Editor | nothing |
| `--client-only` | the Windows Editor | nothing; opens a new client |

Other options: `--linux` builds Forge-only on Linux, `--dhat` enables Forge heap profiling (Forge-only). `nix run .#release` is the same loop with Cargo's release profile; `nix run .#devfast` builds the Linux side with Blaze. Build output goes to `target/dev`.

## What a dev install touches

There is one Artisan install per system, and `run`, `forge` and `editor` replace it: they stage the build into a fresh `target/dev/stage-*` directory and run `ae install --from <stage> --yes`, the same path a release update takes. The data folders (`%LOCALAPPDATA%\Artisan Street\<App>\data` on Windows, `$XDG_DATA_HOME/artisan-street/<app>/data` on Linux) are kept. On systemd a Forge install runs as its own transient unit, so the current Forge keeps serving running turns until the replacement passes its readiness check; a failed candidate leaves the current Forge running.

Each invocation stages into its own directory and deletes it when the installer finishes, also when it fails, so concurrent dev installs never read each other's files.

To try a change without replacing anything, use `build` or `--client-only`. A client-only launch copies the Editor to `%TEMP%\artisan-editor-dev-*` so the build output stays writable while it runs, and connects to the installed Forge. Each launch deletes the copies left by clients that have since closed; a running client's copy is locked by Windows and kept.

Never point a dev install at a machine whose install you are not allowed to replace. For installer work, test against the stubs in `tests/nix/dev.py` and the `ae` tests instead.

## Building and testing

Builds stall the disks while the Editor is in use. Check `/proc/pressure/io` first (`some avg10` under 20), build one crate at a time with `-j2`, and never build or test `--workspace` locally.

```sh
CARGO_TARGET_DIR=target/integration cargo clippy -p <crate> --all-targets -j2 -- -D warnings
CARGO_TARGET_DIR=target/integration cargo nextest run -p <crate> -j2
rustfmt --edition 2024 --config-path rustfmt.toml <changed .rs files>
```

Crates with `cfg(windows)` code also need the Windows target:

```sh
nix develop .#cargo-windows --command cargo clippy -p <crate> --target x86_64-pc-windows-gnu --all-targets -j2 -- -D warnings
```

Outside the workspace:

```sh
python3 tests/nix/dev.py
python3 tests/release.py
python3 vendor/highlighting/verify.py
python3 vendor/mermaid-rs-renderer/verify.py
cargo nextest run --manifest-path vendor/highlighting/validation/Cargo.toml --locked --cargo-profile release
cargo run --manifest-path vendor/highlighting/validation/Cargo.toml --locked --release --bin check_patterns
```

CI (`.github/workflows/ci.yml`) runs these checks plus the workspace-wide clippy and test jobs. Pull requests land only on green CI.
