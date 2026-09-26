# Build, dev-loop, and release pipeline plan

- Status: Phases 0 and 1 implemented, then revised to Nix-orchestrated stages (branch `build-pipeline`); Phases 2–5 open
- Drafted: 2026-09-25
- Scope: how every Artisan binary is built, identified, staged, run, released,
  and updated — from a local edit to a signed stable release
- Primary platform: Windows x64 desktop, with the repository living in WSL
  (`\\wsl.localhost\Ubuntu\...`) and Forge hosts on WSL/Linux

## Revision: one command deploys both halves (2026-09-26)

Supersedes the dev-loop parts below wherever they conflict.

- **`nix run .#dev` is the Rust runner itself**, no shell wrapper. It builds
  the checkout's outputs in one `nix build` (Linux payload; inside WSL also
  the Windows payload and the Windows runner) and deploys:
  - **Forge**: the Linux payload installs as a `dev` release into
    `$XDG_DATA_HOME/Artisan Street Dev`; the installed `ae setup ...
    --autostart --listen auto:4433 --host-name <distro>` makes the Forge the
    product-owned systemd user service `artisan-forge-dev.service`
    (`ae start --foreground`, which reconciles stale readiness and publishes
    the host invitation `<root>/host.json`); `ae` is linked into
    `~/.local/bin`.
  - **Editor**: the Windows runner, through WSL interop, installs the Windows
    payload into `%LOCALAPPDATA%\Artisan Street Dev`, registers the Forge's
    invitation through the Editor's own import, and launches the Editor with
    `--host-home`. Outside WSL the Linux installation holds the Editor.
- The Editor never owns a Forge: `ARTISAN_DEV_OWNED_FORGE` and the Editor's
  owned-Forge start path are gone; the dev Forge is a registered host.
- A hand-deployed Forge (`~/.local/state/artisan-forge`, hand-written
  `artisan-forge.service`, GC roots, `ae` in the Nix profile) is adopted once
  into the default dev installation, with a backup; see
  `docs/runbooks/native-dev.md`.
- `scripts/install_forge_host.py` is removed; `forge-host` remains only as the
  remote-host check's fixture.

## Revision: Nix orchestrates, two stages (2026-09-25)

Supersedes the Cargo-driven dev loop and the `preview`/`performance` profile
design below wherever they conflict.

- **Nix is the orchestrator; Cargo compiles Rust.** `nix run .#dev` builds a
  payload with Nix (toolchain, cross toolchain, dependency cache, build
  identity, layout) and hands it to the dev runner, which only signs,
  installs, provisions, prunes, and launches (`dev run --payload <dir>`). The
  runner no longer builds; `cargo dev`, `dev.py`, `dev.ps1`, `package.py`,
  `release_manifest.py`, and `codegen.py` are gone. Payload archives come from
  `payload-manifest-generator --archive`; release metadata from
  `release-tool generate` called by Nix; bindings from `capnp` called by Nix.
- **Two stages, one codegen.** Debug (`production-debug`) and Production
  (`production`) both use opt-level 3, fat LTO, one codegen unit,
  `panic = "abort"`, mimalloc, and an x86-64-v3 baseline. Debug adds full debug
  info, debug assertions, overflow checks, and the GPUI inspector. GPUI
  `test-support` is only enabled by tests and the `visual-proof` feature, so it
  never ships. Outputs: `{linux,windows}-{debug,production}`.
- **Windows is cross-built from Linux** with the nixpkgs MinGW-w64 toolchain
  (`x86_64-pc-windows-gnu`), not MSVC and not LLVM-MinGW (`gnullvm`): the
  nixpkgs LLVM-MinGW compiler-rt build is broken at the pinned revision, while
  the GCC MinGW toolchain is cached. Executables link the CRT, the GCC runtime,
  and mcfgthread statically and import only Windows system DLLs. The GPUI
  renderer on Windows is wgpu, so no HLSL compiler is needed.
- **Identity:** `version = <workspace>-<channel>.<revCount>+g<commit>[.dirty].n<outhash>`,
  written by Nix into `resources/build-info.json`.
- **Measured** (see `docs/native-performance.md`): Debug renders the
  new-thread screen at 530 presentations per second sustained (was about
  10 fps in Cargo's unoptimized `dev` profile), 0.57 ms CPU draw; Production
  draws in 0.54 ms. Build cost on this 6-core WSL host: Windows Debug about
  13 min cold (dependencies included), about 7 min after a source change;
  Windows Production about 9 min cold. Linux Debug took about 35 min cold,
  dominated by a roughly 20 min fat-LTO link with full debug info.
- **Detached Editor:** the runner starts the Editor outside its job object
  (`CREATE_BREAKAWAY_FROM_JOB`), with output in `.dev-runner/editor.log`;
  WSL interop otherwise terminates Windows processes when the WSL session
  that started them ends.

## Implementation checkpoint (2026-09-25)

Delivered and verified on Linux (tests) and Windows (a `\\wsl.localhost`
checkout through `scripts/dev.ps1`: install, launch, relaunch over a running
dev Editor, pruning):

- `artisan-build-info`: payload-carried identity (`resources/build-info.json`)
  shown by `ae --version`, `installer --version`, the window title, a
  wordmark badge for non-stable builds, and Settings → About.
- `artisan_install` library (in `modules/installer`) with `ReleaseSource`
  (`Remote`, `Directory`, `Tree`), `--from`/`--channel`/`--skip-path`, the
  per-root local signing key, channel-pinned trust, `prune`, and an
  editor-first retirement mode for callers that replace the Editor.
- `cargo dev` (`run`, `stage`, `where`, `prune`) installs every local build as
  a signed `dev`-channel release into `Artisan Street Dev`; `dev.py`,
  `dev.ps1`, the Nix dev apps, `verify_visual.ps1`, and the desktop workflow
  all go through it. A no-change run takes about 5 s; an edit-to-relaunch
  cycle on Windows about 50 s.

Not done in Phase 1, carried forward:

- `cargo dev watch` (rebuild and relaunch on save).
- `ae update --from` / `ae rollback` on the permanent launcher (the installer
  binary supports `--from`; rollback is re-installing an existing version).
- A distinct AppUserModelID and single-instance identity for Artisan Dev;
  today it is distinguished by root, title, badge, and the absence of PATH,
  protocol, and shortcut registration.
- The Linux root rename to XDG-style `artisan-street` (Per-platform section).

Found along the way: an Editor-owned Forge cannot be stopped through the
permanent `ae` (reconnect capability custody is unavailable), so a release
update with the Editor open still cancels before activation. The dev runner
avoids it with editor-first retirement; the product fix belongs with the
Phase 4 updater.

## Outcome

There is exactly one artifact pipeline. A local edit, a nightly, and a stable
release all produce the same thing — a versioned, manifest-verified payload
installed under `versions/<version>/` and activated by an `installation.json`
pointer swap — and differ only in *who built it*, *who signed it*, and *which
channel installation it lands in*.

Concretely, when this plan is done:

1. Every running binary can say exactly what it is: version, commit, dirty
   state, profile, and build time — in the UI, `ae --version`, the installed
   manifest, and the Editor↔Forge handshake. "Is this an old build?" is never a
   question again.
2. Daily use runs a real installation (stable or nightly) that updates itself.
3. Rapid iteration runs a separate **Artisan Dev** installation that is rebuilt
   and relaunched in one command, through the *same* install/activate/update
   code path production uses, so the dev loop continuously exercises the real
   update machinery instead of bypassing it.
4. No workflow runs a raw `cargo build` output directly, and no build output
   lives anywhere except the one documented build root.

## Current state (surveyed 2026-09-25)

What is already solid and must be preserved:

- Versioned install layout `versions/<v>/bin/{ae,editor,forge,installer}` plus
  a crash-safe `installation.json` pointer swap
  (`modules/installer/rust/install/workflow.rs:469-558`, `install/state.rs:63-120`).
- Ed25519 signed release manifests with a compile-time-pinned trust anchor and
  key-id binding (`modules/installer/RELEASE_TRUST.md`).
- Hash-verified downloads, safe extraction, deterministic payload ZIPs
  (`scripts/package.py`, `packaging/portable`).
- The `dev` runner (`scripts/native_dev`) already stages into that same layout,
  hash-reuses unchanged binaries, and verifies with the shipping loader.
- Editor-owned Forge lifecycle resolved through `installation.json`
  (`modules/frontend/src/native_transport_service/service_lifecycle.rs:336-372`).

What is broken or missing:

| Problem | Evidence |
| --- | --- |
| No build identity anywhere. Product version is always `0.0.0`; dev payloads are the literal version `dev`. Nothing shows a commit. | `Cargo.toml:28`; `ae --version` via clap; no `build_info`/`GIT_SHA` in the tree |
| A second build of the same version cannot be installed: `versions/0.0.0` exists with different bytes → `TamperedRelease`. | `workflow.rs:202-237` |
| Five divergent launch paths with different binary sets, job counts, and dev dirs: `dev.ps1`, `dev.py`, `cargo dev`, Nix apps, and `verify_visual.ps1` (which runs raw `cargo run` output, unstaged). | `scripts/dev.ps1`, `scripts/build_support.py:9-26`, `.cargo/config.toml`, `nix/workspace.nix:245-249`, `scripts/verify_visual.ps1:64` |
| Windows builds from the WSL share fail: Cargo cannot hardlink in a UNC `target/`, `fs2` locks fail on the share, `cmd` stderr broke env import. Builds were therefore scattered to `%TEMP%\opencode\target-vendor` and hand-copied. | observed 2026-09-25; `docs/SESSION_HANDOFF.md:132,200,302-329` |
| Artifacts spread over `target/` (Linux only), `%TEMP%\opencode\target-vendor` (Windows, incomplete — no `forge.exe`), `.dist/{dev,build,diagnostics}`, `evidence/` (1,372 entries), `~/.local/state/artisan/*`. | survey |
| Build output is never cleaned: `target/` is 143 GB, `.dist/` 2.8 GB, `evidence/` 1.7 GB (all ignored, never pruned). | `du`, 2026-09-25 |
| Staging lock is held for the Editor's entire lifetime, so a rebuild while the dev Editor runs fails with `StagingLocked` instead of relaunching. | `scripts/native_dev/src/stage.rs:96-119`, `launch.rs:128` |
| Only one custom profile; `performance` inherits `dev` so it keeps `debug_assertions`; `release` is Cargo default; `gpui` `test-support` and `profiler` features are compiled into shipping builds. | `Cargo.toml:38-39,100-104` |
| CI builds only unsigned Linux Nix artifacts; no Windows artifact, no signing, no GitHub Release, although the installer's default manifest URL is GitHub `releases/latest`. Release key is still the `development` key. | `.github/workflows/ci.yml:34-87`, `modules/installer/rust/main.rs:24`, `release/trust_anchor.env` |
| No in-app update check, no rollback, no pruning of old versions, no channel-specific manifest URLs. | installer survey |
| Editor and Forge negotiate only a wire-protocol revision; build versions are never compared, and WSL/remote Forge hosts have no update path. | `modules/protocol/schema/artisan.capnp:312-350` |
| Planning docs still describe Bazel and list installer/updater/signing as open. | `docs/PLAN.md:467,694`, `docs/decisions/NATIVE_PRODUCT_SCOPE.md:133-134` |

## Target model

### Three channels, three installations

Each channel is an independent installation root with its own data, Forge,
identity, and update source. They coexist on one machine.

| Channel | Built by | Signed by | Install root (Windows) | Update source | Use |
| --- | --- | --- | --- | --- | --- |
| `stable` | CI on `v*` tag | release key (secret) | `%LOCALAPPDATA%\Artisan Street` | GitHub Release `stable` manifest | real usage |
| `nightly` | CI on every green `master` | release key | `%LOCALAPPDATA%\Artisan Street Nightly` | rolling GitHub prerelease `nightly` | dogfooding at "actual usage" parity |
| `dev` | your machine | per-machine dev key (generated once, never leaves the machine) | `%LOCALAPPDATA%\Artisan Street Dev` | the local build root | rapid iteration |

The dev installation is visibly different — window title suffix, `Dev` badge
with the short commit, distinct icon tint, distinct AppUserModelID, protocol
scheme, and single-instance mutex — so it can never be confused with, or
collide with, the daily driver. Dev data is isolated by default; an explicit
`--profile-from stable` snapshot-copies real data for reproduction work.

### Build identity

A small `artisan-build-info` crate (build script, no extra runtime deps)
stamps every binary with:

```text
version    0.4.0-dev.1284+g1a2b3c4.dirty   # SemVer; see below
commit     1a2b3c4d… (full), dirty flag, commit time
profile    dev | preview | release
channel    dev | nightly | stable
built_at   UTC timestamp (omitted when SOURCE_DATE_EPOCH is set — reproducible CI)
target     x86_64-pc-windows-msvc
```

Version scheme: the workspace `Cargo.toml` carries the next release number
(e.g. `0.4.0`); tags produce `0.4.0`; nightlies produce `0.4.0-nightly.<n>`;
local builds produce `0.4.0-dev.<commit-count>+g<sha>[.dirty.<hash-of-diff>]`.
Every distinct build therefore gets a distinct `versions/<v>/` directory, which
also retires the same-version `TamperedRelease` trap for honest rebuilds while
keeping the tamper check for genuine same-version mismatches.

Identity appears in: the profile menu footer and an About panel (click to copy),
the window title for non-stable channels, `ae --version`, `installation.json`,
the payload manifest, crash/log headers, and the Editor↔Forge `Hello`/`Welcome`
exchange (warn on mismatch, refuse on incompatible `editor_forge_compatibility_version`).

### Roles of `installer`, `ae`, and the `dev` runner

Today activation is implemented twice: `ae-installer`
(`modules/installer`, a binary-only crate) owns the production workflow —
manifest fetch, signature and hash verification, staging, retiring the running
Editor/Forge, the `installation.json` swap, and OS integrations — while the
`dev` runner (`scripts/native_dev`, which depends only on `artisan-editor-cli`)
re-implements staging and writes `installation.json` itself
(`stage.rs`, `manifest.rs`). That duplication is the root of dev/production
drift, so the target splits responsibilities as follows:

| Component | Role in the target |
| --- | --- |
| `artisan-install` (new library, extracted from `modules/installer/rust`) | The **only** code that verifies, stages, activates, rolls back, and prunes versions, and retires running processes. Payload *sources* are pluggable: HTTPS channel manifest (nightly/stable) or a local payload directory (dev). Trust is per channel: pinned release key vs the per-machine dev key. |
| `installer` binary | Thin bootstrap over `artisan-install`: first install on a clean machine, repair, uninstall. The only piece users download by hand. |
| `ae` (permanent launcher at `<root>/bin/ae.exe`) | The stable, user-facing entry for an installation: launching the active Editor (shortcuts, protocol handler, and PATH point at it, never at `versions/…`), Forge `start/stop/status/logs/doctor`, and — via `artisan-install` — `ae update`, `ae rollback`, `ae --version` (full build identity), `ae versions`. Keeps `Layout::discover` (`modules/cli/rust/paths.rs`), which the Editor already uses to find its installation. |
| Editor | In-app update UI calls `artisan-install` in-process for check/download/stage, then hands off to `ae`, which activates after the Editor exits and relaunches it — the same handoff the dev relaunch uses. |
| `dev` runner | A build frontend only: resolve the build root, `cargo build` the payload, assemble it with the payload-manifest generator, then call `artisan-install` with a local source and the `dev` channel. It no longer writes `installation.json` or copies binaries itself. |

Consequences: `cargo dev run` exercises byte-for-byte the activation path a
nightly update takes; `ae rollback` works identically on all three channels;
and `ae update` stops shelling out to the `installer` binary
(`modules/cli/rust/commands/cli.rs:377`).

### Local sources in `artisan-install`

Today the installer only accepts a manifest URL (`--manifest-url`, default
GitHub `releases/latest`). A local build can be installed only by zipping it,
signing it, serving the directory on `127.0.0.1` (plain HTTP is allowed for
loopback only, `install/workflow.rs:81-90`), and using a *debug-built*
installer, since only debug builds accept `--public-key`. Verification is
already separate from the network — `manifest::decode(bytes, signature, trust)`
(`manifest.rs:257`) — so the change is to make the *source* pluggable and the
*trust* channel-scoped, with everything after them unchanged.

**Sources.** One `--from <location>` argument (library: `ReleaseSource`),
auto-detected:

| Source | Looks like | Contents | Used by |
| --- | --- | --- | --- |
| `Remote` | `https://…/release-manifest.json` (or loopback `http`) | signed manifest + `.sig`; artifact fetched relative to it | stable, nightly, in-app updates |
| `ReleaseDir` | a directory containing `release-manifest.json` | the exact `release-tool` output: manifest, `.sig`, archive — read from disk instead of HTTP | testing a CI artifact or a locally packaged release before publishing; offline installs; air-gapped hosts |
| `Tree` | a directory containing `bin/editor(.exe)` + `payload-manifest.json` + `payload-manifest.sig` | an **unpacked** payload — no archive — with the per-file digest manifest signed by the dev key | the `cargo dev` rapid loop |

Everything downstream is shared: minimum-installer and version checks, staging
into `.stage-*`, per-file digest verification, retiring the running Editor and
Forge, the `installation.json` swap, and integrations. For `Tree`, "extract
archive" becomes "copy each declared file and verify its digest"; files whose
digest matches the currently active version are **hardlinked** from that
immutable version directory instead of copied (the same reuse the `dev` runner
does today), so an iteration that changes only `editor.exe` copies one file.
Sources are read-only, so a `Tree` on `\\wsl.localhost\…` (a cross-compiled
build in WSL) installs straight into `%LOCALAPPDATA%\Artisan Street Dev` without any
share locking or hardlinking problems.

**Channel-scoped trust.** A root records its channel and trusted key id when
it is first created, and every later install into that root must match both:

| Channel root | Accepts | Key |
| --- | --- | --- |
| `stable`, `nightly` | `Remote`, `ReleaseDir` | release anchor pinned at compile time (unchanged `RELEASE_TRUST.md` model) |
| `dev` | `Remote` (loopback), `ReleaseDir`, `Tree` | per-machine dev key: generated by `cargo dev` on first use; private seed stays in the user profile (`%LOCALAPPDATA%\Artisan Street Dev\keys`, ACL'd to the user) and never enters the repo, CI, or the Nix store; public key recorded in the dev root at creation |

So the *same* release-built `installer`/`ae` users run can install dev builds
— removing today's "debug installer only" requirement — while a dev-signed
payload can never activate in a stable or nightly root, and a release payload
can be installed into the dev root only by explicit `--channel dev` intent.
`--public-key` stays as a debug-build-only test hook.

**Rebuilds of the same version.** Dev versions are unique per build
(`-dev.<n>+g<sha>[.dirty.<diffhash>]`), so re-installs normally take the
fresh-version path. When the version already exists, today's code
re-downloads the artifact and compares byte for byte
(`verify_and_activate_existing`, `workflow.rs:212`); for local sources the
comparison runs against the source tree directly — identical means re-activate
(a no-op rebuild is instant); different means `TamperedRelease`, as now.

**Command surface.**

```text
installer install --from https://…/release-manifest.json        # today's behaviour
installer install --from D:\downloads\artisan-0.4.0-nightly.12\ # ReleaseDir, offline
ae update --from <path|url>                                     # same, via the permanent launcher
ae update --from <build-root>\payload --channel dev             # Tree, what `cargo dev run` calls in-process
```

The hidden `installer prepare-update` ("close the editor and retire Forge
before a local release build begins") is subsumed: retirement happens at
activation time, after the build has already succeeded, so a failed build
never takes the running app down.

### Per-platform installation

Researched 2026-09-25 against current platform guidance and what comparable
developer tools ship (Zed, VS Code/Cursor, Ghostty, Ollama, Warp, Docker
Desktop, Tailscale, 1Password). Sources are listed at the end of this section.

Two deployment shapes, one signing and channel model:

- **Desktop** — Editor + its owned local Forge + `ae`, installed per user,
  no admin. The Editor starts Forge as a child on `127.0.0.1` from its own
  payload, so Editor and local Forge are always the same build and need no
  service registration.
- **Host** — headless Forge + `ae` on a machine the Editor connects to over
  QUIC (WSL, Linux server, Mac). Runs as a per-user background service and is
  updated independently, so it depends on the build-identity handshake.

Cross-platform rules:

- **Everything the OS points at is stable.** PATH entries, the URL handler,
  shortcuts/desktop entries, and autostart point at a fixed launcher path,
  never at a `versions/<v>/` directory. (1Password's 2025 MSIX move broke SSH
  signing, autostart, and its browser extension on every update because the
  path contained the version.)
- **Channels are separate apps.** Each channel has its own root, app identity,
  and URL scheme: `artisan://` (stable), `artisan-nightly://`, `artisan-dev://`
  — the VS Code Insiders / Zed Preview model. Only stable claims `artisan://`.
- **Payload and data are separate.** Code lives in the install location; Forge
  data, config, and logs live in the platform's data/state location per
  channel, so reinstall, repair, and rollback never touch user data.
- **One trust scheme.** Every update on every platform is an Ed25519-signed
  manifest verified before activation (Sparkle's appcast signatures are also
  EdDSA/Ed25519, so the macOS path can reuse the same keys).
- **Every self-updating install disables its updater when installed by a
  system package manager** (apt/dnf, Homebrew formula, Nix), as Zed does.

#### Windows

- **Format:** keep the custom, signed, per-user installer and updater
  (`installer.exe` → `%LOCALAPPDATA%\Artisan Street[ Nightly| Dev]\`) with
  `versions\<v>\` + `installation.json` pointer and pointer-swap rollback. This
  matches Microsoft's "unpackaged / own installer" guidance for developer tools
  and what VS Code, Cursor, Zed, Warp, and Ollama ship (all per-user Inno or
  Squirrel-style installers). **Not MSIX**: no real PATH (only App Execution
  Aliases), no per-user services, filesystem virtualization of AppData, and
  weak control over update/rollback timing. A sparse "packaging with external
  location" identity package can be added later only if we need package
  identity (Windows 11 context menu, notifications, Store).
- **Stable launchers:** `%LOCALAPPDATA%\Artisan Street\bin\{ae,artisan,forge}.exe`
  are small launchers that read `installation.json` and exec the active
  version. Only this `bin\` goes on the user PATH, into the HKCU `artisan://`
  handler, Start Menu shortcut, and autostart.
- **Autostart:** replace the Task Scheduler logon task (`ae autostart`) with
  the per-user Run key `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
  pointing at the stable `forge` launcher. It is visible and toggleable in
  Settings → Startup apps and Task Manager, needs no admin, and is not
  treated as suspicious persistence by endpoint security the way hidden
  scheduled tasks are.
- **Uninstall entry:** write and keep current `HKCU\…\Uninstall\<id>`
  (`DisplayVersion` updated by every self-update) so Settings → Apps and
  WinGet see the real version.
- **Signing:** Authenticode-sign the installer and every shipped exe with one
  consistent identity via Azure Artifact Signing ($9.99/month). Outside the
  US/Canada it is available to organizations only (Norway is supported for
  organizations), so this needs a registered legal entity; fallback is an OV
  certificate in a cloud HSM. EV no longer bypasses SmartScreen; reputation
  accrues per file and certificate, and Smart App Control blocks unsigned
  binaries outright.
- **WinGet:** publish user-scope manifests, one ID per channel
  (`ArtisanStreet.Editor`, `ArtisanStreet.Editor.Nightly`), installer type
  `exe` with silent switches, `UpgradeBehavior: install`.
- **Build vs buy:** Velopack (Rust crate, active in 2026) would replace the
  installer/updater with per-user installs, delta updates, and channels, at
  the cost of its own package format and dropping our Ed25519 manifest
  scheme. Recommendation: keep ours — it is already built and signed-manifest
  first — but re-evaluate if updater maintenance becomes a burden.

#### Windows: choosing where Forge runs (WSL checklist)

On Windows, setup (`installer` first run, and `ae forge targets` any time
later) asks where Forge should run, as a multi-select checklist. The Editor is
always installed on Windows; only Forge placement is chosen.

```text
Where should Forge run?   ↑/↓ move · space toggle · enter confirm

  [ ] This PC (Windows)        Forge started by the Editor, projects on C:\…
  [x] WSL · Ubuntu (default)   systemd ✓ · Forge 0.4.0-nightly.12 installed → will update
  [ ] WSL · Debian             systemd ✓
  [-] WSL · Ubuntu-20.04       WSL 1 — convert with `wsl --set-version Ubuntu-20.04 2`
  [-] WSL · Alpine             no systemd — not supported
```

**Detection** (no VM boot needed to draw the list):

- Registered distributions come from
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` (per-distro
  `DistributionName`, `Version`, and `DefaultDistribution`), not from
  `wsl.exe --list` (UTF-16 output, and it can start the VM).
- Eligibility is WSL 2 plus systemd. Systemd and an existing Forge
  installation are probed only for WSL 2 distros, with a short timeout:
  `/run/systemd/system` and the host's `installation.json`. That boots the
  distro once, which is acceptable during setup. Ineligible rows are shown but
  disabled, with the one-line fix. For a distro whose only problem is systemd
  being off, the fix offered is "enable systemd" (write `[boot] systemd=true`
  and run `wsl --terminate <distro>`), after confirmation.
- A distro counts as a **configured WSL host** when it already has an Artisan
  Forge installation, or the Editor already has a saved host for it
  (`modules/frontend/src/native_hosts`).

**Defaults:**

| Situation | Pre-selected |
| --- | --- |
| One or more configured WSL hosts | those distros only (Windows unchecked) |
| No configured host, eligible WSL 2 + systemd distro exists | the default distro only (Windows unchecked) |
| No eligible WSL distro | This PC only (WSL rows disabled or absent) |

At least one row must be selected. When more than one is selected, a follow-up
single choice asks which is the **default** Forge for new threads. Existing
projects still route by location: `\\wsl.localhost\<distro>\…` goes to that
distro's Forge, and `C:\…` goes to the local Forge when one is selected. With
Windows unchecked, `C:\…` projects are opened through the default WSL Forge
via `/mnt/c`.

**What selection does:**

- *This PC*: today's behaviour. The Editor owns a local Forge from its own
  payload.
- *WSL · <distro>*, done by the Windows installer:
  1. Fetch the Linux `host` payload for the same version and channel from the
     same release.
  2. Run the Linux `installer` inside the distro via
     `wsl.exe -d <distro> --exec`. It installs to
     `~/.local/share/artisan-street/<channel>/`, runs `ae host enable`
     (systemd user units), and prints the invitation.
  3. Register the host in the Editor automatically. No manual "Add host from
     invitation".
  4. At runtime the Editor holds the WSL lifetime process (see Linux → WSL).
- An unchecked, previously installed target is not removed silently. It is
  offered for removal ("Remove Forge from Debian?"), with data kept unless
  `--remove-data` is given.
- Partial failure is not fatal: if a WSL install fails, the Windows install
  still completes, the failure is reported with its log, and
  `ae forge targets` retries.

**Non-interactive:** `--forge <list>` with `local`, `wsl` (the default
distro), or `wsl:<name>`, for example `--forge wsl` or
`--forge local,wsl:Ubuntu`, plus `--default-forge <target>`. `--yes` accepts
the defaults above. The selection is persisted per installation (in
`installation.json`, as `forge_targets` and `default_forge`) and is also
editable in the Editor under Settings → Hosts. The Editor's onboarding screen
shows the same checklist when setup ran without a terminal.

**Updates** keep every selected target on the Editor's build: `ae update`
and in-app updates update the WSL hosts in the same run. A host that cannot be
reached is updated on the Editor's next connect, through the build-identity
check.

Implementation note: no prompt library is used today (the installer is flag
and `--yes` driven). Add one small terminal prompt crate (e.g. `inquire`
`MultiSelect`/`Select`) in `artisan-install`'s CLI layer only. The
eligibility logic and target model live in the library, so the Editor's
onboarding UI and the terminal checklist share them.

#### Linux (systemd only)

**systemd is a hard requirement.** It is the init on every mainstream distro
(Ubuntu, Debian, Fedora, RHEL and clones, SUSE, Arch, NixOS) and on
Ubuntu-on-WSL; excluded are Alpine, Void, Gentoo-OpenRC, Devuan, Artix — a
small, self-selecting audience. Docker Desktop for Linux sets the same
requirement. The installer refuses when `/run/systemd/system` is absent (on
WSL, with a pointer to `[boot] systemd=true` in `/etc/wsl.conf`). No
OpenRC/runit/SysV support. `forge serve --foreground` remains for containers,
CI, and debugging — a run mode, not an init fallback.

- **Format:** signed tarball + `install.sh`, the Zed/JetBrains Toolbox/rustup
  model: `curl -fsSL https://<domain>/install.sh | sh`, per user, no root.
  **No official Flatpak, Snap, or AppImage**: the Flatpak sandbox breaks
  terminals, git, compilers, and language servers (`flatpak-spawn --host` has
  no proper TTY — the long-running VS Code/VSCodium Flathub complaint), Snap is
  Canonical-only, AppImage depends on FUSE (Ghostty dropped it).
- **Layout (XDG, lowercase id):**

  | Item | Location |
  | --- | --- |
  | Payload | `~/.local/share/artisan-street/<channel>/versions/<v>/` + `installation.json` |
  | CLI | `~/.local/bin/ae` (stable), `ae-nightly`, `ae-dev` → stable launchers |
  | Desktop entry | `~/.local/share/applications/street.artisan.Editor[.Nightly\|.Dev].desktop` with `MimeType=x-scheme-handler/artisan[-nightly\|-dev];`, registered via `xdg-mime` + `update-desktop-database` |
  | Icons | `~/.local/share/icons/hicolor/<size>/apps/` |
  | Config | `$XDG_CONFIG_HOME/artisan-street/<channel>/` |
  | Data, logs | `$XDG_STATE_HOME/artisan-street/<channel>/` |
  | Sockets | `$XDG_RUNTIME_DIR/artisan-street/` |

  This replaces today's `$XDG_DATA_HOME/Artisan Street`
  (`modules/installer/rust/platform.rs:139-150`); the space and capitals do not
  follow XDG convention.
- **Host service:** `ae host enable` writes `forge.socket` + `forge.service`
  into `~/.config/systemd/user/` with `ExecStart` pointing at the stable
  launcher, then `systemctl --user enable --now forge.socket`. Socket
  activation starts Forge on first connection. Updates swap the pointer and
  `systemctl --user restart`. Headless servers need `loginctl enable-linger`
  (the only step needing root/polkit); `install.sh` offers it and `ae doctor`
  checks it.
- **WSL:** Ubuntu WSL images ship with systemd enabled. A running user service
  does **not** keep a WSL instance alive (since WSL 2.6.1 the distro stops
  ~15–20 s after the last terminal closes), so the Windows Editor owns the
  lifetime: it installs the host payload through `wsl.exe -d <distro>`, then
  holds a `wsl.exe -d <distro> -- ae host attach` process for as long as it is
  connected, which boots the distro and socket-activates Forge. Always-on WSL
  Forge (linger + `.wslconfig` idle timeouts) is opt-in.
- **Binary baseline:** glibc **2.31** on x86_64 and **2.35** on arm64 for the
  Editor (dynamic; Vulkan, Wayland/X11, and fontconfig are loaded from the
  host), matching Zed. Forge can target glibc **2.28** (the VS Code Server
  floor) via an old-base build image or `cargo zigbuild`. Not musl: with
  systemd required, musl's main benefit (Alpine) is moot.
- **Later:** signed apt/dnf repositories for fleets, installing to
  `/opt/artisan-street` with a user unit in `/usr/lib/systemd/user/` and the
  self-updater disabled; the Nix flake/NixOS module stays, community-supported,
  built from the release tarball.

#### macOS

The versioned-directory layout used on Windows and Linux does **not** carry
over. Apple ties signing, notarization, library validation, TCC permissions,
and Background Items attribution to a sealed bundle; executables loaded from
`~/Library/Application Support` lose tamper protection, need
`disable-library-validation`, and are never Gatekeeper-assessed. Every
comparable app (Zed, VS Code, Cursor, Ghostty, Ollama) ships one
self-contained bundle and replaces it whole.

- **Format:** one `.app` per channel (`Artisan Street.app`,
  `Artisan Street Nightly.app`, `Artisan Street Dev.app`; separate bundle IDs
  and URL schemes), Developer ID signed, hardened runtime, notarized with
  `notarytool`, stapled; first install via DMG, updates via zip. Since macOS 15
  un-notarized apps cannot be opened by control-click, so notarization is
  mandatory. Needs an Apple Developer Program membership.
- **Contents:** `Contents/MacOS/{artisan,ae,forge}` — Forge and `ae` are
  signed and updated with the bundle.
- **Updates:** download signed zip, verify the Ed25519 manifest, stage the new
  bundle beside the old one, swap atomically, relaunch; keep one previous
  bundle in `~/Library/Caches/<bundle-id>/previous/` for `ae rollback`.
  Implement in `artisan-install` (our manifests), or adopt Sparkle 2 via the
  `sparkle-updater` crate (EdDSA appcasts, channels, admin prompt when needed,
  used by Ghostty) — decision below.
- **Start at login:** `SMAppService.agent` with the plist inside the bundle at
  `Contents/Library/LaunchAgents/` using `BundleProgram` (a bundle-relative
  path, so it survives moves and updates). It appears under System Settings →
  Login Items attributed to the app. Never hand-written plists in
  `~/Library/LaunchAgents`.
- **CLI:** "Install `ae` command" symlinks
  `/usr/local/bin/ae → /Applications/Artisan Street.app/Contents/MacOS/ae`
  (one admin prompt; the symlink survives updates because it points into the
  bundle), with a no-admin fallback to `~/.local/bin`. Check PATH first and do
  not nag on every launch.
- **Data:** `~/Library/Application Support/street.artisan.Editor[.Nightly|.Dev]/`
  for data, `~/Library/Logs/…` for logs — data only, no code.
- **Homebrew:** publish `artisan-street` and `artisan-street@nightly` casks with
  `auto_updates true` and `binary "…/Contents/MacOS/ae"` (since April 2026
  `brew upgrade` coexists with in-app updaters); a formula for headless
  `forge` + `ae`.
- **Headless Mac hosts:** a LaunchAgent only runs with a logged-in user. The
  default is the Zed/VS Code remote model — the Editor installs a matching
  Forge over SSH and runs it on demand; for always-on hosts, the CLI-only
  package offers `ae host enable --system`, which installs a LaunchDaemon with
  a `UserName` key (admin required), as Tailscale does for headless
  `tailscaled`.

#### What this means for `artisan-install`

Activation becomes a per-platform strategy behind one interface; everything
else (sources, trust, channels, version checks, retirement, rollback,
handshake) is shared:

| Platform | Activation | Rollback |
| --- | --- | --- |
| Windows | stage `versions/<v>/` → swap `installation.json`; stable launchers in `bin\` | swap pointer back |
| Linux | same, under the XDG payload root; restart user unit | swap pointer back |
| macOS | stage new `.app` → atomic bundle swap → relaunch | swap previous bundle back |

The dev channel keeps the fast `Tree` source on Windows and Linux; on macOS a
dev build is an ad-hoc-signed `Artisan Street Dev.app` assembled locally and
swapped the same way.

#### Sources

- Microsoft packaging overview: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/packaging/
- MSIX limits: https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-prepare,
  https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps
- 1Password MSIX issues: https://www.1password.community/discussions/1password/beta-release-new-msix-installer-for-1password-for-windows/155230
- Velopack: https://docs.velopack.io/getting-started/rust
- Artifact Signing: https://azure.microsoft.com/en-us/pricing/details/artifact-signing/,
  https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart
- SmartScreen reputation: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation
- WinGet upgrade behavior: https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade
- Zed on Linux/Windows/macOS: https://zed.dev/docs/linux, https://zed.dev/docs/windows, https://zed.dev/docs/macos
- Flatpak IDE issues: https://github.com/flathub/com.visualstudio.code/issues/197
- Ghostty install/prerelease: https://ghostty.org/docs/install/binary, https://ghostty.org/docs/install/pre
- Docker Desktop Linux requirements (systemd): https://docs.docker.com/desktop/setup/install/linux/
- WSL config and idle shutdown: https://learn.microsoft.com/en-us/windows/wsl/wsl-config,
  https://github.com/microsoft/WSL/issues/13416
- VS Code Server glibc floor: https://code.visualstudio.com/docs/remote/faq
- Gatekeeper in macOS 15: https://developer.apple.com/news/?id=saqachfa
- Bundle placement and notarization: https://developer.apple.com/documentation/bundleresources/placing-content-in-a-bundle,
  https://developer.apple.com/forums/thread/720018
- SMAppService: https://developer.apple.com/documentation/servicemanagement/smappservice
- Sparkle programmatic setup: https://sparkle-project.org/documentation/programmatic-setup/
- Homebrew `auto_updates` casks: https://github.com/Homebrew/brew/pull/21882
- Tailscale macOS variants: https://tailscale.com/docs/concepts/macos-variants

### Profiles

| Profile | Purpose | Settings |
| --- | --- | --- |
| `dev` | debugger, tests | Cargo default, but `[profile.dev.package."*"] opt-level = 2` so GPUI/wgpu deps are fast at runtime while workspace crates stay incremental; `debug = "line-tables-only"` for deps |
| `preview` (replaces `performance`) | the rapid-iteration daily build — release-speed runtime, fast incremental compile | `inherits = "release"`, `incremental = true`, `codegen-units = 256`, `debug = "line-tables-only"`, `debug-assertions = false` |
| `release` | nightly/stable | `lto = "thin"`, `codegen-units = 1`, `strip = "debuginfo"` with split PDBs archived as CI artifacts for symbolication |

GPUI `test-support` and `profiler` move behind workspace features that only
tests and the performance tooling enable, so they stop shipping. Whether the
`preview` profile should keep `debug_assertions` is a one-line decision; the
default above chooses parity with release.

### One build root, outside the share on Windows

All build output lives under one root, resolved once by the runner:

- Linux/WSL: `<repo>/target` and `<repo>/.dist/…` (unchanged, per repo convention).
- Windows: `%LOCALAPPDATA%\Artisan Street Dev\build\<worktree-id>\{target,dist}` where
  `<worktree-id>` is derived from the canonical repo path, so parallel worktrees
  never share or clobber a target dir.

`.dist/` gains a fixed shape with retention: `.dist/payloads`, `.dist/evidence`
(replacing root `evidence/`), `.dist/diagnostics` (pruned by age). The two
tracked files under `evidence/` move to `docs/`.
`%TEMP%\opencode\target-vendor` is retired.

### Windows builds from a WSL checkout

Two viable options; this is the main technical spike (Phase 2):

- **A. Cross-compile from WSL** with `cargo-xwin` to `x86_64-pc-windows-msvc`
  (clang-cl + `lld-link`, MSVC CRT/SDK headers fetched once). The build runs on
  ext4 at native speed, Nix can own the toolchain, and CI can produce Windows
  artifacts on Linux. Output is staged across to `/mnt/c/…` (the install root),
  which only needs plain file copies. Risk: GPUI/wgpu Windows build scripts or
  shader compilation that assume Windows-hosted tools.
- **B. Native Windows build** of the WSL checkout with the Windows build root
  above. Works today (proven 2026-09-25 with the three fixes) but compiles over
  9P/UNC, which is slow for source reads, and needs a Windows toolchain.

Recommendation: spike A first; keep B as the supported fallback and as the CI
Windows lane until A is proven equivalent (identical tests pass, visual proof
matches).

### One entrypoint

The Rust `dev` runner becomes the only implementation; everything else is a
thin wrapper (`cargo dev …`, `scripts/dev.ps1` → `cargo dev`, `nix run .#dev`
→ same binary). Commands:

```text
cargo dev run        # build (preview) → stage → activate → launch or relaunch Artisan Dev
cargo dev watch      # run, then rebuild + relaunch on save (debounced)
cargo dev stage      # build → stage → activate, no launch
cargo dev package    # produce a signed-by-dev-key payload + manifest in .dist/payloads
cargo dev install --channel nightly --from .dist/payloads/…   # exercise the real installer locally
cargo dev prune      # keep the active + N previous dev versions
cargo dev where      # print build root, install root, active version, running PID
```

`verify_visual.ps1` and the desktop CI job consume a staged installation path
(or `cargo dev where`), never `target/*/editor.exe`.

### The rapid loop, with parity

`cargo dev run` while Artisan Dev is already open:

1. Builds with the `preview` profile into the build root (incremental; the
   running exe is never overwritten because it lives in the install root).
2. Stages into a fresh `versions/0.4.0-dev.…/`, verifies the payload manifest,
   swaps `installation.json` — exactly the production activate path.
3. Signals the running dev Editor over its existing single-instance channel:
   "version X is active". The Editor persists session state, shuts down its
   owned Forge, and relaunches into the new version — the same path an in-app
   update takes.
4. The staging lock covers steps 2–3 only, not the Editor's lifetime.

Because the relaunch goes through the updater's own handoff, every dev
iteration tests update, Forge handover, and session restore. Old dev versions
are pruned automatically (keep 3).

### Release pipeline

- `ci.yml` keeps the Linux flake checks and the Windows lane, and the Windows
  lane starts caching (`Swatinem/rust-cache` or sccache) and uploading its
  release payload.
- New `release.yml`:
  - on green `master`: build release payloads (Windows x64; Linux Forge host),
    sign manifests with the release key from GitHub secrets, publish to the
    rolling `nightly` prerelease, keeping the last N nightly assets;
  - on `v*` tags: same, published as a GitHub Release, and the `stable`
    manifest updated.
- Per-channel manifest URLs baked into each channel's installer
  (`…/releases/download/nightly/release-manifest.json`,
  `…/releases/latest/download/release-manifest.json`).
- Rotate the `development` trust anchor to a real release key per
  `RELEASE_TRUST.md` before the first nightly is published; the dev channel
  uses a separate per-machine key so dev payloads can never validate as
  nightly/stable.

### Updates, rollback, and Forge hosts

- In-app: the Editor checks its channel manifest on startup and every few
  hours, downloads and stages in the background, and offers "Restart to
  update" (auto for dev). Implemented once in the installer/`ae` library and
  shared by all channels.
- `ae rollback` re-activates the previous version (pointer swap only);
  the installer prunes to active + 2.
- Forge hosts: the Linux Forge binary ships in the release payload; the Editor
  compares build identity on connect and offers "Update host", which runs the
  existing `install_forge_host` flow with the matching build. Nix hosts pin by
  flake rev.

## Delivery plan

### Phase 0 — Stop the bleeding (small, do first)

- Land the three Windows fixes from 2026-09-25 properly: `dev.ps1` stderr
  filtering (done, uncommitted), Windows build root outside the share,
  staging lock that works on local disk only (refuse UNC dev dirs with a clear
  error).
- Add `artisan-build-info` and surface it in the profile menu footer, window
  title (non-stable), and `ae --version`.
- Delete `%TEMP%\opencode\target-vendor` usage from docs; fix the stale
  `pnpm run dev:ae` hint in `modules/cli/rust/paths.rs:334`.

Exit: any running Artisan build shows its commit; `scripts/dev.ps1 -Release`
works from the WSL checkout with no manual environment.

### Phase 1 — One entrypoint and a real dev channel

- Extract `artisan-install` from `modules/installer/rust`; make the `installer`
  binary and `ae update` thin callers; replace the `dev` runner's own
  staging/`installation.json` writing with `artisan-install` using a local
  payload source.
- Add `ReleaseSource` (`Remote`, `ReleaseDir`, `Tree`), `--from`,
  channel-scoped trust recorded per root, and the per-machine dev key.
- Collapse `dev.py`, `dev.ps1`, `build_support.py`, and Nix app paths onto the
  `dev` runner commands above.
- Versioned dev staging (`versions/<dev version>`), pruning, `where`.
- Artisan Dev identity (title, badge, AppUserModelID, mutex, protocol scheme,
  separate install root).
- Relaunch-on-rebuild via the single-instance channel; scope the staging lock.
- Point `verify_visual.ps1` and `desktop.yml` at staged installs.

Exit: `cargo dev run` twice in a row, with the dev Editor open, relaunches into
the new build and the UI shows the new commit.

### Phase 2 — Build speed and the Windows toolchain spike

- Introduce `dev`/`preview`/`release` profiles and gate GPUI
  `test-support`/`profiler`.
- Spike cargo-xwin cross-compilation from WSL (option A); measure clean and
  incremental times against option B; decide.
- Enable CI caching on the Windows lane.

Exit: documented, measured incremental `cargo dev run` time for a one-line UI
change, and a decision record for A vs B.

### Phase 3 — Nightly releases

- Real release key; `release.yml` publishing signed Windows payloads to the
  rolling `nightly` prerelease; channel-specific installer manifest URLs.
- Version scheme wired into packaging (`nix/workspace.nix:4,204-218`,
  `packaging/release/*.json`).

Exit: a fresh Windows machine installs Artisan Nightly from GitHub with the
installer alone, and it reports the exact `master` commit.

### Phase 4 — Self-updating app, rollback, host parity

- In-app update check/stage/restart shared across channels; dev relaunch
  migrated onto it.
- `ae rollback`, version pruning.
- Editor↔Forge build identity in the handshake; "Update host" flow.
- First `v0.x.0` stable release.

Exit: nightly updates itself overnight; a bad nightly is recoverable with
`ae rollback`; a mismatched WSL Forge is detected and updatable from the UI.

### Phase 5 — Documentation

- Replace `docs/runbooks/native-dev.md` Windows/dev sections with the single
  entrypoint; add `docs/runbooks/release.md`; update `docs/PLAN.md` and
  `docs/decisions/NATIVE_PRODUCT_SCOPE.md` to record the decisions above.

## Decisions to confirm

1. **Channels:** three coexisting installs (`stable`, `nightly`, `dev`) with
   isolated data by default — or should dev share the daily driver's data?
2. **Windows toolchain:** spike cross-compilation from WSL first (recommended),
   or standardize on native Windows builds from the share?
3. **Hosting:** GitHub Releases (matches the installer's current default) or a
   dedicated bucket/CDN (e.g. R2) for manifests and payloads?
4. **Version scheme:** SemVer with `-nightly.<n>` / `-dev.<n>+g<sha>` as above.
5. **Platform order:** Windows desktop + Linux/WSL host first, then Linux
   desktop, then macOS — or is macOS needed sooner?
6. **Signing identity:** Azure Artifact Signing requires an organization
   outside the US/Canada. Is there (or will there be) a registered entity to
   sign as, or do we start with an OV certificate in a cloud HSM? macOS
   additionally needs an Apple Developer Program membership.
7. **macOS updater:** our own bundle-swap updater in `artisan-install`
   (one code path, our manifests) or Sparkle 2 via `sparkle-updater`
   (battle-tested, same Ed25519 keys, another dependency)?
8. **Preview profile assertions:** off for release parity (recommended) or on
   to catch bugs during daily use?

Resolved by research (2026-09-25): payload and data are separate on every
platform; Linux requires systemd and nothing else; no MSIX, Flatpak, Snap, or
AppImage; the Linux root is renamed to XDG-style `artisan-street`; Windows
autostart moves from Task Scheduler to the HKCU Run key.

## Suggested PR boundaries

1. `build-info` crate + UI/CLI surfacing.
2. Windows build root + lock + `dev.ps1` fixes.
3. Extract `artisan-install`; `installer`, `ae update`, and the `dev` runner
   call it (removes the duplicated activation code).
4. Runner command consolidation and wrapper removal.
5. Versioned dev staging, pruning, Artisan Dev identity.
6. Relaunch-on-rebuild.
7. Profiles and feature gating.
8. Cross-compile spike (decision record, possibly no merge).
9. Release key rotation + `release.yml` + channel manifests.
10. In-app updater + rollback.
11. Forge build-identity handshake + host update.
12. Stable launchers in `bin\`, per-channel URL schemes, HKCU Run autostart,
    uninstall entry, data/payload separation (Windows).
13. Host payload kind + `ae host enable` (systemd user units with socket
    activation) + Editor-held WSL lifetime and WSL host install.
14. Windows Forge-target checklist: WSL detection/eligibility, `--forge`,
    `ae forge targets`, persisted targets, onboarding UI, auto host
    registration, location-based routing.
15. Linux desktop: `install.sh`, XDG layout, `.desktop`/scheme registration.
16. macOS: per-channel bundles, notarization in `release.yml`, bundle-swap
    updater, SMAppService, `ae` command install, Homebrew casks.
17. WinGet manifests and Homebrew tap automation.
18. Docs.
