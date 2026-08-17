# WSL support

Status: prototype verified end to end on 2026-08-17 (Windows 11, WSL 2.7.11,
Ubuntu 24.04). Architecture decided; productization items listed below.

## Architecture

Artisan supports WSL the way VS Code Remote-WSL does: **Forge and `ae` run
inside the distro; the Windows Editor stays a stateless client** and pairs over
loopback. A Windows-hosted Forge reaching into `\\wsl$\...` is rejected:
`fs.watch(recursive)` delivers no events over 9P, project identity is keyed on
the absolute root path, and every workspace process (git, engines, terminals)
would execute on the wrong side of the boundary.

No protocol, transport, or frontend change is required. The wire already
carries opaque directory identities instead of host paths, the protocol models
the backend host identity as a remote fact, and the renderer is a sandboxed
page with no IPC surface. The verified chain is:

```text
Electron (Windows) ── ARTISAN_AE_COMMAND wrapper (.cmd)
  └─ wsl.exe -d <distro> -- sh ae.sh open --handoff
       └─ Linux node ae.js ── {endpoint, pair_code} on stdout
            └─ Linux Forge (host.js) on 127.0.0.1:<port>
                 └─ Windows renderer pairs over localhost-forwarded loopback
```

## Empirically verified

- **Loopback forwarding (NAT mode, default):** a Windows connection to
  `127.0.0.1:<port>` arrives at the Linux listener with
  `remoteAddress === "127.0.0.1"`. All loopback gates (forge config listen
  literal, websocket peer check, transport `require_loopback`, desktop endpoint
  validator, frontend endpoint decoder) pass unchanged. Mirrored networking is
  not required (a fallback that flips `~/.wslconfig` exists in the provisioning
  kit but did not trigger).
- **The non-SEA Forge bundle is platform-neutral.** `.dist/validation/forge`
  (`ae.js`, `host.js`, chunks, migrations) runs unmodified under Linux Node 24.
  The artifact override contract covers the Linux layout completely:
  `ARTISAN_FORGE_ROOT`, `ARTISAN_FORGE_EXECUTABLE`,
  `ARTISAN_FORGE_NODE_EXECUTABLE`, `ARTISAN_FORGE_NATIVE_RUNTIME`.
- **Native runtime:** `node-pty@1.1.0` compiles in-distro (build-essential +
  python3); `koffi@3.1.1` installs its linux-x64 prebuild. Both load through the
  existing `ARTISAN_NATIVE_RUNTIME` shims.
- **`ae` CLI adapters degrade correctly on Linux:** XDG state home
  (`~/.local/state/artisan`), autostart reports `unsupported`, native directory
  picker reports unavailable and the UI falls back to the opaque-ID browser.
- **Desktop handoff crosses the WSL boundary cleanly.** `ARTISAN_AE_COMMAND`
  pointed at a `.cmd` wrapper (`wsl.exe -d artisan -u root -- sh ae.sh %*`)
  yields a working `open --handoff` JSON line and `stop --instance-id`.
- **End-to-end:** Electron paired to the WSL Forge (`artisan://app`, project
  visible in the title), and a headless-browser session attached a git project
  on ext4 (`/root/src/demo`) through the directory browser with zero renderer
  console errors.

A reproducible provisioning kit (rootfs import without elevation, in-distro
setup, loopback probe, wrapper) lives at `%LOCALAPPDATA%\wsl-artisan\` with its
own runbook; it is developer tooling, not product code.

## Productization items

1. **Launch surface.** Packaged desktop resolves `bin\ae.exe` only
   (`modules/desktop/src/paths.ts`); WSL mode needs a first-class distro-aware
   launch choice instead of the `ARTISAN_AE_COMMAND` wrapper, plus distro
   enumeration (`wsl -l -q`) in setup UX.
2. **Linux Forge artifact.** Ship `host.js` + Linux Node + linux-x64
   `node-pty`/`koffi` (the SEA builder's win32 guard in
   `.scripts/build/build-forge-sea.ts` is policy, not architecture; the staging
   in `.config/forge.rolldown.config.ts` hardcodes win32 assets today).
3. **Codex toolchain.** Managed install fails `platform_unsupported` off
   Windows (`modules/engines/src/toolchain/distribution.ts`); Linux Codex ships
   archives, so the toolchain needs bounded archive extraction. Claude's
   distribution already resolves linux targets.
4. **POSIX polish.** `valid_directory_name`
   (`modules/backend/src/projects/project-directory-service.ts`) applies the
   Windows character blacklist on all platforms; `known_places` assumes a
   Windows/macOS home layout. Both files are hot in the current dirty milestone
   — fix after it lands.
5. **In-distro lifecycle.** Provisioning/update of the WSL-side payload should
   be owned by `ae` (the Rust CLI already compiles for Linux and knows the
   `artisan-forge` binary name), and WSL-side Forge state should be surfaced in
   `ae doctor`.
