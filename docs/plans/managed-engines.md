# Forge-managed engine binaries

- Status: implemented on branch `managed-engines` (not merged)
- Scope: how the Forge installs, versions, resolves, and launches every engine CLI (Codex,
  Claude Code, Grok Build, Cursor Agent, OpenCode2)

## Outcome

Artisan manages its own engine binaries. The Forge installs each engine from the vendor's
official distribution into its own state directory, verifies it against the vendor-published
digest, keeps a few generations for instant rollback, and launches only those managed
executables with an environment it builds explicitly. Nothing is resolved from `PATH`, WinGet,
`LOCALAPPDATA`, npm shims, or systemd drop-ins. The Editor shows each engine's install state,
version, selection, and available updates, and can pick another version or roll back.

This replaces the production failure mode where the WSL Forge ran the Windows `claude.exe`
through an npm shim on `/mnt/c` with Windows credentials, and the hand-placed Codex wired through
a systemd drop-in (`ARTISAN_CODEX_EXECUTABLE`).

## Versioning policy

- Default selection per engine is `latest`: the vendor's current release on its default
  channel, resolved from the vendor feed at install/update time. There is no version table in
  code.
- Each engine has a code-level compatibility floor. The Forge never installs, selects, or runs a
  version below it. Capability checks (for example Claude thinking display) stay version and
  capability based.
- Integrity is mandatory, in one of two modes per catalog entry (see "Integrity modes").
  `VendorDigest` (Claude, Codex, OpenCode2): the digest comes from the vendor manifest or the
  registry's published integrity at resolution time and the download is verified against it
  before anything is extracted. `TrustOnFirstDownload` (Grok, Cursor): the vendor publishes no
  digest, so the first HTTPS download of each version is hashed and recorded, and every later
  download of that version must match.
- The Forge checks for a newer `latest` at startup and every 6 hours (bounded requests, one
  engine at a time). A newer version is installed in the background as a new generation and
  activated when no process is using the engine; running processes keep their generation.
- The user can hold an engine at an explicit version (`use <engine> <version>`) or return to
  `latest`. Automatic updates only apply while the selection is `latest`. The selection is Forge
  state and survives restarts.
- Rollback switches to the most recently active previous generation (no download) and holds the
  engine at that version, so the next automatic update does not undo it; `use <engine> latest`
  resumes updates. The active generation, up to three previous generations, and at most one
  pending generation stay on disk; older generations are pruned (a generation still executing on
  Windows is removed on a later prune).
- "Latest" means the vendor's default channel: Claude's `latest` pointer (the channel its
  installer and auto-updater default to), Codex's npm `latest` dist-tag, and `OpenCode2`'s npm
  `beta` dist-tag (its V1 `latest` line is a different product).

## Per-engine source of truth

| Engine | Feed (latest) | Version list | Artifact | Integrity | Floor | Platforms |
| --- | --- | --- | --- | --- | --- | --- |
| Claude Code | `https://downloads.claude.ai/claude-code-releases/latest` | npm `@anthropic-ai/claude-code` versions (official Anthropic package, same version numbers) | `…/claude-code-releases/<v>/<platform>/claude[.exe]` single native binary | SHA-256 + size from `…/<v>/manifest.json` `platforms.<platform>` | 2.1.220 | linux-x64, linux-arm64, win32-x64, darwin-arm64, darwin-x64 |
| Codex | npm dist-tag `latest` of `@openai/codex` | npm `@openai/codex` stable `X.Y.Z` versions | npm `@openai/codex@<v>-<platform>` tarball, full `package/` tree | npm `dist.integrity` (SHA-512) of the platform package | 0.142.5 | linux-x64, linux-arm64, win32-x64, darwin-arm64, darwin-x64 |
| OpenCode2 | npm dist-tag `beta` of `@opencode-ai/cli-windows-x64` | npm versions `0.0.0-beta-N` | npm tarball member `package/bin/opencode2.exe` | npm `dist.integrity` (SHA-512) | 0.0.0-beta-17778 | win32-x64 |
| Grok Build | `https://x.ai/cli/stable` | none published: latest plus versions this Forge downloaded before | `https://x.ai/cli/grok-<v>-<platform>[.exe]` single binary (`linux-x86_64`, `linux-aarch64`, `windows-x86_64`, `macos-aarch64`, `macos-x86_64`) | trust on first download (SHA-256 + size recorded) | none | linux-x64, linux-arm64, win32-x64, darwin-arm64, darwin-x64 |
| Cursor Agent | release named in the package URL of `https://cursor.com/install` | none published: latest plus versions this Forge downloaded before | `https://downloads.cursor.com/lab/<v>/<os>/<arch>/agent-cli-package.tar.gz`, `dist-package/` tree, entry `cursor-agent` | trust on first download | none | linux-x64, linux-arm64, darwin-arm64, darwin-x64; Windows ships only a zip, which Artisan does not install yet |

Provenance (researched 2026-09-26):

- Claude Code: `https://claude.ai/install.sh` and `install.ps1` download
  `downloads.claude.ai/claude-code-releases/{latest,<v>/manifest.json,<v>/<platform>/claude}` and
  verify the manifest SHA-256. At research time `latest` was 2.1.283 and `stable` 2.1.274;
  2.1.282 `linux-x64` was `3afe8535…61eed3`, 238767288 bytes. The native binary self-updates
  unless disabled; the Forge sets `DISABLE_UPDATES=1`.
- Codex: `@openai/codex` publishes per-platform builds as npm versions `<v>-<platform>` (the
  `optionalDependencies` of the main package). `0.156.0-linux-x64` integrity
  `sha512-/PX399IS…YaTrA==`, 148509615 bytes, entry `vendor/x86_64-unknown-linux-musl/bin/codex`
  alongside `codex-path/rg` and `codex-resources/` (bwrap, zsh). GitHub releases of
  `openai/codex` also publish SHA-256 digests, but their downloads redirect across hosts; the
  npm registry serves the same builds without redirects and with SHA-512 integrity.
- OpenCode2: `@opencode-ai/cli-windows-x64`, dist-tag `beta` (`latest` is the V1 `opencode`
  line and is never used). `0.0.0-beta-19271` still ships `package/bin/opencode2.exe`.
- Grok Build: `https://x.ai/cli/install.sh` (and `install.ps1`) read `https://x.ai/cli/stable`
  (1.0.41 at research time) and download `https://x.ai/cli/grok-<v>-<platform>` without any
  digest; `.sha256`/`.sig` sidecars do not exist (404). The artifact is served without redirects.
  The installer's Windows extras (a git payload and hook executables) are not installed.
- Cursor Agent: `https://cursor.com/install` pipes
  `https://downloads.cursor.com/lab/<v>/<os>/<arch>/agent-cli-package.tar.gz` straight into
  `tar --strip-components=1` (2026.09.26-dd393fe at research time: 182876559 bytes, 579 regular
  files and directories below `dist-package/`, entry `cursor-agent`, a bash launcher for the
  bundled `node`). `install.ps1` downloads `windows/<arch>/agent-cli-package.zip`. No digest,
  manifest, or sidecar is published (403 on every probe), and no official npm package exists.

## Integrity modes

Owner decision, 2026-09-26: "Trust their download over HTTPS and record the hash we get the
first time." Rationale: Grok and Cursor publish no checksums, and managing them (fixed
environment, managed launch, versioning) is worth more than leaving them on `PATH`; recording
the first download at least guarantees that a version never silently changes afterwards.

- `VendorDigest` (Claude Code, Codex, OpenCode2): unchanged.
- `TrustOnFirstDownload` (Grok Build, Cursor Agent): HTTPS only with normal TLS verification
  (plain HTTP is refused; redirects are not followed), from the URL the vendor's official
  installer uses. The SHA-256 and size of the exact downloaded bytes are recorded in
  `toolchain/<engine>/trust.json` as (version, platform, url, sha256, size, first seen). Any
  later download of the same version and platform (reinstall, repair, reselecting a pruned
  version) must match the record or fails with `trust_mismatch`; the record is never
  overwritten. A new version gets its own record. The installed executable is then verified on
  every launch like any other generation.
- Neither vendor publishes a version list: `latest` comes from the installer's own source, and
  the version picker offers the current release plus every version this Forge downloaded
  before (status carries `vendorVersionList = false` and Settings says so).
- The weaker guarantee is visible: `ae engine list|status` prints "verified by vendor checksum"
  or "trusted on first download (hash recorded <date>)", and Settings shows the same on the
  engine's Installation section.

## Archive policy

Archives are extracted only after their bytes are verified, by a script-free tar parser bounded
by a per-engine policy in the catalog, sized from the real vendor artifacts (inventoried
2026-09-26) with headroom:

| Engine | Real archive | Expanded bound | Entries | Name bytes | Metadata |
| --- | --- | --- | --- | --- | --- |
| Codex | `0.157.1-linux-x64`: 46 files, 391,130,194 bytes, longest name 103 (ustar prefix); empty owner fields | 1 GiB | 512 | 255 | PAX (npm) |
| Cursor Agent | `2026.09.26-dd393fe` linux-x64: 454 files, 125 directories, 52 GNU long names, 583,125,383 bytes, longest name 132; padded to the 10 KiB tar record | 1.5 GiB | 4096 | 512 | GNU long names |
| OpenCode2 | `0.0.0-beta-19271`: 2 files, 210,120,955 bytes | 512 MiB | 1024 | 255 | PAX (npm) |

Regular files and directories are the only members ever created; links, devices, FIFOs, and
metadata forms outside the policy are rejected (none of these archives contains a link), so
extraction can never be redirected outside the staging directory. Names must be relative and
normalized and may not collide case-insensitively. Every rejection is a typed reason carrying
its limit and the archive member, for example `too_many_entries: 632 entries, limit 512`,
`expanded_too_large: … bytes, limit …`, `unsupported_entry: symlink package/bin/x`,
`name_too_long`, `unsafe_name`, `name_collision`, `archive_malformed: checksum of entry 3`,
`archive_truncated`, `archive_trailing_data`.

Opt-in tests install the real archives and run `--version` of the result:
`ARTISAN_REAL_ENGINE_ARCHIVES=<dir> cargo test -p artisan-native-engine real_artifact`, with
`<dir>` holding `codex-<version>-linux-x64.tgz` and `cursor-<version>-linux-x64.tar.gz`.

## Install failures

A failed install (anything except another holder of the install lock) is recorded in
`toolchain/<engine>/install-failure.json`: the reason code, its detail, the version, the number
of consecutive failed attempts, and when. A successful install removes it. `ae engine
list|status` and the Editor status show `failed: <detail>` for an engine without a usable
install (and "last update failed" beside a ready version) instead of "not installed"; `ae
engine install` prints the detail and exits with status 4. The Forge retries a failed engine
one minute after the first failure, doubling per attempt up to the six-hour update interval
(an engine held at an installed version is only retried on request).

## Layout

Everything lives beside the Forge database (the Forge state directory):

```text
<forge state>/toolchain/<engine>/
  install.lock            exclusive: install, switch, prune, profile registration
  use.lock                shared by every running engine process; a switch needs it exclusively
  state.json              active generation, up to 3 previous, optional pending (atomic replace)
  selection.json          "latest" or one exact version (atomic replace; missing = latest)
  install-failure.json    last failed install: reason, detail, attempts (removed on success)
  versions/
    generation-<32 hex>/  one verified install (executable path, size, SHA-256 recorded in state)
    staging-<32 hex>/     in-progress install, removed by the next install
  home/                   private (0700) HOME of the engine process
    .claude/              CLAUDE_CONFIG_DIR (Claude)
    .codex/               CODEX_HOME (Codex)
```

`<forge state>` is the directory holding the Forge database (`<installation>/data`, for the WSL
dev Forge `~/.local/share/Artisan Street Dev/data`; `/var/lib/artisan-forge` under the NixOS
module). `OpenCode2` keeps its existing
`toolchain/opencode2` root; its format-1 `state.json` is still read.

## Resolution

Every Forge spawn of an engine (runs, model discovery, account usage, version and auth probes)
resolves through `artisan_native_engine::resolve_launch_target`: the active generation from
`state.json`, with the executable's size and SHA-256 recorded at install time verified (the
rehash is skipped only when this process already verified the same file identity, size, and
change times). There is no `PATH`, `LOCALAPPDATA`, `WinGet`, or npm-shim discovery anywhere; the
old discovery code (`codex/discovery.rs`, `account_usage_resolve.rs`, PATH search in the Claude,
Grok, and Cursor modules) is deleted. The Forge registers its database at startup
(`register_managed_database`); `ae engine` passes its instance database or `--database`.

Run launches (`VerifiedCodexLaunch`, `VerifiedClaudeLaunch`) carry a shared `use.lock` lease for
the life of the run. A generation switch needs that lock exclusively, so an update or selection
made while a run is live is recorded as `pending` and activated by the Forge once the engine is
idle; a running process never has its generation switched underneath it.

Developer override: `ARTISAN_<ENGINE>_EXECUTABLE` is honoured only as an absolute path to a
regular file (relative names are rejected rather than searched), carries no lease, and is shown
as an override in `ae engine list` (with a warning) and in the Editor status. The systemd drop-in
that set `ARTISAN_CODEX_EXECUTABLE` on the WSL Forge is obsolete once Codex is installed through
the Forge; remove it after the managed install is ready (it would otherwise keep overriding the
managed Codex). Note that a Forge started by `ae` strips every `ARTISAN_*` variable, so overrides
only reach a Forge started by systemd or `forge-host`.

## Forge engine manager

`modules/backend/src/engine_manager.rs` runs one dedicated thread per Forge:

- at startup it installs every supported engine's selection (`latest` by default) in the
  background, publishing `installing` with download progress, then `ready vX` or `failed` with a
  reason; unsupported engines report `unsupported` with the vendor reason; a failed engine is
  retried with backoff (see "Install failures");
- every 6 hours it re-reads the vendor feed; engines following `latest` install the newer release
  as a new generation, engines held at a version are left alone;
- every minute it activates `pending` generations whose engine is idle;
- Editor requests (select a version or `latest`, roll back, list versions) queue to the same
  thread, so operations never race; a failed lock (for example a live `OpenCode2` profile) is
  retried on the next pass rather than reported as a failure.

## Status push and Editor

Protocol (fresh ordinals): `readEngineInstalls` (Request @47) and `changeEngineVersion` (@49)
answer `engineInstalls` (Response @45), an `EngineInstallSnapshot` with one
`EngineInstallStatus` per engine: phase (`notInstalled`, `installing` with progress, `ready`,
`failed` with reason, `unsupported` with reason), active, held, latest, pending, and rollback
versions, and whether a developer override is set. `listEngineVersions` (@48) answers
`engineVersions` (@46), newest first, each marked installed, active, or below the floor. The
`engineInstalls` Event (@12) pushes the snapshot to connections that read it, following the
recent-threads rule so an older Editor is never sent an event it does not know.

The Forge attaches the manager to the account-usage service (both are an engine's host state);
every snapshot change wakes connection delivery through the host-state notifier, and the
delivery driver pushes only a changed snapshot.

The Editor reads the snapshot when a Settings engine page opens and then follows pushes. The
Installation section shows the Forge's status copy (version, installing with progress, failed or
unsupported with the reason, a pending version waiting for an idle engine, an active override),
the selection ("Follows the latest release" or "Held at X"), and actions: Use latest, Roll back to
the previous version, and Choose version, which lists the vendor's versions with Use buttons
(below-floor versions are shown but not selectable).

## Environment policy

The Forge builds each engine environment from scratch (`env_clear`, then exactly these
variables; `artisan_native_engine::build_environment`):

- `HOME` = `<forge state>/toolchain/<engine>/home` (private); on Windows also `USERPROFILE`,
  `APPDATA`, and `LOCALAPPDATA` inside that home;
- the engine config home: `CLAUDE_CONFIG_DIR=<home>/.claude`, `CODEX_HOME=<home>/.codex`;
- `PATH` = the generation's own tool directories (Codex `vendor/<triple>/codex-path` with its
  bundled `rg`), then the Forge's own absolute `PATH` entries (the operator's tools such as `git`
  that engines run on the user's behalf). On Linux every `/mnt/<drive>/…` entry is removed, so a
  WSL Forge never reaches Windows binaries or npm shims. An empty result falls back to
  `/usr/local/bin:/usr/bin:/bin`. Engine binaries themselves are never looked up on `PATH`;
- `LANG`/`LC_ALL` = the Forge's `LANG` when it is a UTF-8 locale, else `C.UTF-8`;
- a pass-through allowlist: `TERM`, `TZ`, `TMPDIR`, `TEMP`, `TMP`, proxy and CA variables
  (`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` and lowercase forms, `SSL_CERT_FILE`, `SSL_CERT_DIR`,
  `NODE_EXTRA_CA_CERTS`), Windows loader essentials (`SYSTEMROOT`, `WINDIR`, `COMSPEC`,
  `PATHEXT`, `PROGRAMDATA`, `PROGRAMFILES`, processor variables), and only the engine's own
  credential variables when the operator set them on the Forge (`ANTHROPIC_API_KEY`,
  `CLAUDE_CODE_OAUTH_TOKEN` for Claude; `OPENAI_API_KEY`, `CODEX_API_KEY` for Codex;
  `XAI_API_KEY` for Grok; `CURSOR_API_KEY` for Cursor);
- vendor self-update is disabled: `DISABLE_UPDATES=1` for Claude (Anthropic's documented switch
  for distributing Claude Code through your own channel).

## Sign-in

A managed engine starts without credentials. The account-usage reads run the managed CLI, so
readiness reports `needs sign-in` (and `not ready` with "engine is not installed on this Forge"
before the first install finishes). The owner signs in once per Forge host with the managed
binary and exactly the environment the Forge uses; the installation's `ae` finds its own Forge
(`--database PATH` selects another):

```sh
ae engine login claude
#   runs the managed `claude auth login` with the Forge CLAUDE_CONFIG_DIR
ae engine login codex -- --device-auth
#   runs the managed `codex login --device-auth` with the Forge CODEX_HOME
```

Arguments after `--` replace the default sign-in arguments (for example
`ae engine login claude -- setup-token`, or `-- auth login --console` for API billing). An
operator can instead provide `CLAUDE_CODE_OAUTH_TOKEN` / `ANTHROPIC_API_KEY` /
`OPENAI_API_KEY` to the Forge service; those are the only credential variables passed through.

Option, not implemented (needs explicit approval): copy the Windows credentials
(`%USERPROFILE%\.claude\.credentials.json`, `%USERPROFILE%\.codex\auth.json`) into the Forge
engine homes. It would sign a WSL Forge in without a browser, but moves long-lived secrets across
an OS boundary automatically.

## Migration

- Existing OpenCode2 installs (`toolchain/opencode2/state.json` format 1) keep working: format 1
  is read, and the next write upgrades to format 2.
- Codex and Claude are installed on first Forge start. Until then their status is `installing`
  and runs, usage reads, and model discovery report the engine as unavailable instead of falling
  back to `PATH`.
- Owner steps on the WSL Forge after deploying (also in `docs/runbooks/remote-forge.md`): wait for
  `ready` (Settings, or `ae engine list --database …`), sign in (above), then remove the
  `ARTISAN_CODEX_EXECUTABLE` drop-in, restart the service, and delete
  `~/.local/share/artisan/codex/0.156.0`.

## Known gaps

- Grok Build and Cursor Agent are trusted on first download (weaker than a vendor checksum; the
  first download of each version is not independently verified). Cursor on Windows is
  unsupported until zip packages can be installed. Neither vendor documents a switch to turn off
  its CLI's own self-update; Grok is launched with `--no-auto-update` where it accepts it.
- `OpenCode2` stays Windows x64 only (its harness integration is certified there); the beta npm
  channel also publishes Linux builds.
- The native readiness probes in `claude/probe.rs`, `codex/probe.rs`, `grok/probe.rs`, and
  `cursor/probe.rs` take a bare program path; they apply the managed environment when the
  program is a managed engine, and keep the caller's environment only for test fixtures.
- Grok runs carry no use lease yet (its launch capability predates the managed seat), so a Grok
  switch can activate while a Grok run is live.
- `Choose version` lists the npm version history (Claude uses the official
  `@anthropic-ai/claude-code` package for the list, and its native manifest for the artifact);
  a listed version without a native build fails with `feed_platform_missing`.
- The per-process verification cache skips rehashing a file whose identity, size, and change
  times are unchanged; a same-size in-place rewrite that also restores those times would not be
  detected until the Forge restarts.

## Delivery

1. Plan (this document).
2. Engine-agnostic catalog, feeds, state format 2, selection, authority, install pipeline
   (moved from the CLI), retention, rollback, and `ae engine
   list|status|versions|install|update|use|rollback|login`.
3. Managed-only resolution and the environment builder for every spawn site; ambient discovery
   deleted.
4. Forge engine manager (startup install, periodic update, activation when idle), protocol,
   status push, and the Editor Settings Installation section.
