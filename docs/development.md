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

Forge's end-to-end tests (`crates/forge-http/tests/replay`) replay engine transcripts through a real Forge: the `replay_engine` example stands in for Claude, Codex or OpenCode, plays the steps in `crates/forge-engines/transcripts/replay/<engine>/<case>.jsonl` and fails the run, naming both frames, when Forge sends something the transcript did not expect. A transcript is JSON lines: a `transcript` header saying whether it was `recorded` or written by hand (`synthetic`), then one `launch` per engine process, each holding `out` frames the engine writes, `in` frames it waits for, `closed` where Forge closed its input, `exit` where the engine dies with a code and, for OpenCode, `request` lines with the HTTP reply. To turn a bug report into a fixture, record it from its engine traces (`traces/<engine>-<thread>-<ms>.jsonl` in the Forge data folder, one file per launch, given in launch order): `cargo run -p test-support --features transcript --bin record -- <fixture.jsonl> <trace.jsonl>... [--scrub <text>=<replacement>]...`. The recorder replaces your home folder with `/home/user` and refuses traces that dropped or cut frames; read the fixture before committing it and scrub any other names, paths, tokens or message text that are personal, since streamed deltas can split them where no scrub reaches. OpenCode traces hold only the server's console, not its HTTP traffic, so OpenCode fixtures are written by hand.

`crates/forge-http/tests/simulation.rs` drives the same replaying engine through seeded faults: a seed picks a session of messages, drafts, Forge restarts and kills, engines that die mid-turn, lost or cut requests, dropped event streams, failing disk commits and clock steps, then checks what a user would notice (no run left running, every acknowledged message delivered once, gap-free seq, the last draft kept, failures typed). Pull requests run a few fixed seeds; the nightly workflow sweeps hundreds. A failing seed prints its actions and `SIMULATION_SEED=<seed> cargo nextest run -p forge-http --test simulation --example replay_engine` to replay it. The disk faults and clock steps come from the `test-support` features of `forge-store` and `artisan-api`, which no shipped build enables.

`crates/forge-http/tests/contract.rs` installs the newest Claude Code, Codex and OpenCode the way Forge does and checks Forge reads their status probes; with an API key in the environment it also runs one prompt and fails on frame shapes no recorded transcript has. It needs the network, so pull requests skip it and the nightly workflow runs it, together with the Codex schema check against the newest Codex.

Released data stays readable: `crates/forge-store/tests/fixtures/released/` holds a database written by the last released Forge (schema 26), which today's Forge must open, migrate, back up and roll back, and `crates/editor-core/tests/core/released_forge/` holds that Forge's answers, which today's Editor must read. Make new ones with the released source, never with today's code.

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

## Terms in user-facing copy

Every user-visible string, the README and the docs use one word per concept. Internal identifiers keep their names.

| Term | Means | Not |
| --- | --- | --- |
| Forge | The server that runs engines and serves the Editor; settings that live there are "on this Forge". | host |
| machine | The computer a Forge runs on; "Pair this machine" adds its Forge to the account. | host, device |
| device | The computer the Editor runs on, for Editor-only settings. | machine |
| Artisan account | The account you sign in to; it lists your Forges. | AST account |
| engine | Claude Code, Codex or OpenCode. | harness |
| Try again | The label of every recovery button. | Retry |
