# Forge-managed engine binaries

- Status: in progress (branch `managed-engines`)
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
- Integrity is mandatory. The digest comes from the vendor manifest or the registry's published
  integrity at resolution time and the download is verified against it before anything is
  extracted. A platform without a vendor-published digest is unsupported; unverified bytes are
  never installed.
- The Forge checks for a newer `latest` at startup and every 6 hours (bounded requests, one
  engine at a time). A newer version is installed in the background as a new generation and
  activated when no process is using the engine; running processes keep their generation.
- The user can hold an engine at an explicit version (`use <engine> <version>`) or return to
  `latest`. Automatic updates only apply while the selection is `latest`. The selection is Forge
  state and survives restarts.
- Rollback switches to the most recent retained generation. The active generation plus up to
  three previous generations stay on disk.

## Per-engine source of truth

| Engine | Feed (latest) | Version list | Artifact | Integrity | Floor | Platforms |
| --- | --- | --- | --- | --- | --- | --- |
| Claude Code | `https://downloads.claude.ai/claude-code-releases/latest` | npm `@anthropic-ai/claude-code` versions (official Anthropic package, same version numbers) | `…/claude-code-releases/<v>/<platform>/claude[.exe]` single native binary | SHA-256 + size from `…/<v>/manifest.json` `platforms.<platform>` | 2.1.220 | linux-x64, linux-arm64, win32-x64, darwin-arm64, darwin-x64 |
| Codex | npm dist-tag `latest` of `@openai/codex` | npm `@openai/codex` stable `X.Y.Z` versions | npm `@openai/codex@<v>-<platform>` tarball, full `package/` tree | npm `dist.integrity` (SHA-512) of the platform package | 0.142.5 | linux-x64, linux-arm64, win32-x64, darwin-arm64, darwin-x64 |
| OpenCode2 | npm dist-tag `beta` of `@opencode-ai/cli-windows-x64` | npm versions `0.0.0-beta-N` | npm tarball member `package/bin/opencode2.exe` | npm `dist.integrity` (SHA-512) | 0.0.0-beta-17778 | win32-x64 |
| Grok Build | `https://x.ai/cli/stable` | — | `https://x.ai/cli/grok-<v>-<platform>` | none published | — | unsupported: xAI publishes no digest for the binary |
| Cursor Agent | version embedded in `https://cursor.com/install` | — | `https://downloads.cursor.com/lab/<v>/<os>/<arch>/agent-cli-package.tar.gz` | none published | — | unsupported: Cursor publishes no digest for the package |

Provenance (researched 2026-09-26):

- Claude Code: `https://claude.ai/install.sh` and `install.ps1` download
  `downloads.claude.ai/claude-code-releases/{latest,<v>/manifest.json,<v>/<platform>/claude}` and
  verify the manifest SHA-256. At research time `latest` was 2.1.283 and `stable` 2.1.274;
  2.1.282 `linux-x64` was `3afe8535…61eed3`, 238767288 bytes. The native binary self-updates
  unless `DISABLE_AUTOUPDATER=1`; the Forge sets it.
- Codex: `@openai/codex` publishes per-platform builds as npm versions `<v>-<platform>` (the
  `optionalDependencies` of the main package). `0.156.0-linux-x64` integrity
  `sha512-/PX399IS…YaTrA==`, 148509615 bytes, entry `vendor/x86_64-unknown-linux-musl/bin/codex`
  alongside `codex-path/rg` and `codex-resources/` (bwrap, zsh). GitHub releases of
  `openai/codex` also publish SHA-256 digests, but their downloads redirect across hosts; the
  npm registry serves the same builds without redirects and with SHA-512 integrity.
- OpenCode2: `@opencode-ai/cli-windows-x64`, dist-tag `beta` (`latest` is the V1 `opencode`
  line and is never used). `0.0.0-beta-19271` still ships `package/bin/opencode2.exe`.
- Grok Build: `https://x.ai/cli/install.sh` downloads `grok-<v>-<platform>` without any digest;
  `.sha256`/`.sig` sidecars do not exist (404). Only its Windows git payload has a sidecar.
- Cursor Agent: `https://cursor.com/install` pipes the package straight into `tar`; no digest,
  manifest, or sidecar is published (403 on every probe). No official npm package exists.

## Layout

Everything lives beside the Forge database (the Forge state directory):

```text
<forge state>/toolchain/<engine>/
  install.lock            exclusive: install, switch, prune, profile registration
  use.lock                shared by every running engine process; a switch needs it exclusively
  state.json              active generation, up to 3 previous, optional pending (atomic replace)
  selection.json          "latest" or one exact version (atomic replace; missing = latest)
  versions/
    generation-<32 hex>/  one verified install (executable path, size, SHA-256 recorded in state)
    staging-<32 hex>/     in-progress install, removed by the next install
  home/                   private (0700) HOME of the engine process
    .claude/              CLAUDE_CONFIG_DIR (Claude)
    .codex/               CODEX_HOME (Codex)
```

`<forge state>` is the directory holding the Forge database (`~/.local/state/artisan-forge` on
the WSL Forge, `/var/lib/artisan-forge` under the NixOS module). `OpenCode2` keeps its existing
`toolchain/opencode2` root; its format-1 `state.json` is still read.

## Resolution

Every Forge spawn of an engine (runs, model discovery, account usage, version and auth probes)
resolves the active generation from `state.json`, verifies the executable's size and SHA-256
recorded at install time (cached per file identity for the process lifetime), and launches that
path. There is no `PATH` lookup and no ambient discovery.

Developer override: `ARTISAN_<ENGINE>_EXECUTABLE` (`ARTISAN_CLAUDE_EXECUTABLE`,
`ARTISAN_CODEX_EXECUTABLE`, `ARTISAN_GROK_EXECUTABLE`, `ARTISAN_CURSOR_EXECUTABLE`) is honoured
only as an absolute path, is reported as `override` in status and logs a warning on every
resolution. The systemd drop-in that set `ARTISAN_CODEX_EXECUTABLE` on the WSL Forge is obsolete
once Codex is installed through the Forge; remove it after the managed install is ready.

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
  `CLAUDE_CODE_OAUTH_TOKEN` for Claude; `OPENAI_API_KEY`, `CODEX_API_KEY` for Codex);
- vendor self-update is disabled: `DISABLE_UPDATES=1` for Claude (Anthropic's documented switch
  for distributing Claude Code through your own channel).

## Sign-in

A managed engine starts without credentials. Readiness reports `needs sign-in`. The owner signs
in once per Forge host with the managed binary and environment:

```sh
ae engine login claude --database ~/.local/state/artisan-forge/forge.db
#   runs the managed `claude auth login` with the Forge environment (CLAUDE_CONFIG_DIR)
ae engine login codex --database ~/.local/state/artisan-forge/forge.db -- --device-auth
#   runs the managed `codex login --device-auth` with the Forge CODEX_HOME
```

Arguments after `--` replace the default sign-in arguments (for example
`ae engine login claude -- setup-token`, or `-- auth login --console` for API billing).

Copying Windows credentials into the Forge config home is technically possible (both CLIs keep
credentials in files under their config home on Linux) but is not done automatically; it needs an
explicit design decision.

## Migration

- Existing OpenCode2 installs (`toolchain/opencode2/state.json` format 1) keep working: format 1
  is read, and the next write upgrades to format 2.
- Codex and Claude are installed on first Forge start. Until then their status is `installing`
  and runs report the engine as unavailable instead of falling back to PATH.
- Owner steps on the WSL Forge after deploying: wait for `ready`, sign in (above), then remove the
  `ARTISAN_CODEX_EXECUTABLE` drop-in and `~/.local/share/artisan/codex/0.156.0`.

## Delivery order

1. Plan (this document).
2. Engine-agnostic catalog, feeds, state format 2, selection, and authority in
   `artisan-native-engine`; OpenCode2 becomes one catalog entry.
3. Install pipeline moved from the CLI into the shared authority (fetch seam, single-binary,
   tar member, and tar tree layouts, retention, rollback).
4. `ae engine list|versions|install|use|rollback|status|login`.
5. Managed-only resolution and the environment builder for every spawn site.
6. Forge engine manager (startup install, periodic update, activation when idle) and the status
   push; Editor Settings engine section.
