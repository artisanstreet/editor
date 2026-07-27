# Artisan Distribution and First-Time Setup

Status: in progress — Windows first implementation verified; cross-platform
release acceptance remains planned

## Summary

Artisan has one public installation experience and one permanent command. The
landing page detects the visitor's operating system and presents the matching
copyable install command:

```powershell
# Windows
irm https://sonstabo.com/editor/windows | iex
```

```sh
# macOS and Linux
curl -fsSL https://sonstabo.com/editor/unix | sh
```

The downloaded script is a minimal transport adapter. It detects the platform,
downloads the matching temporary native `ae` bootstrap from GitHub Releases,
verifies its immutable version and digest, executes it from the platform's
temporary directory, and removes it after a verified handoff. The bootstrap is
not Artisan Editor, Artisan Forge, or the permanent Artisan CLI. It installs and
verifies the selected native product components, transfers command ownership to
the permanent `ae`, updates user-scoped integrations such as PATH, performs
first-time Forge setup, and then disappears.

After setup:

```sh
ae
ae update
ae doctor
ae uninstall
```

`ae` is the single command for setup, Editor launch, Forge lifecycle, repair,
updates, and removal. npm is not part of Artisan's installation, release,
runtime, update, repair, or uninstall contract.

## Product Principles

- There is one documented installation path across Windows, macOS, and Linux.
- Editor and Forge install together by default.
- First-time setup presents one recommended action; customization is secondary.
- Artisan Forge remains authoritative. Editor is a stateless client; permanent
  native `ae` owns the `artisan://` launch handoff.
- Installation and the permanent product do not depend on npm, Node from the
  user's PATH, an app store, apt repository, Homebrew, Flatpak, or AppImage.
- GitHub Releases is the artifact origin. Every artifact is authenticated before
  execution.
- Updates are owned by the permanent `ae`, not Electron or a package registry.
- All installation and integration writes are user-scoped unless the user
  explicitly chooses an elevated system-wide installation.
- Repair and uninstall operate from an installation manifest and never remove
  unrelated user state.

## First-Time Experience

The landing page uses client-side platform detection only to choose presentation.
It never starts installation automatically. It shows the detected command first,
allows the user to switch platforms, and always provides an inspect-before-run
download alternative.

The canonical public surface is
[`https://sonstabo.com/editor`](https://sonstabo.com/editor). Its stable
PowerShell and POSIX endpoints are deployed independently from product binaries
and resolve immutable GitHub Release assets.

The normal flow is intentionally short:

```text
Install Artisan

Installs:
✓ Artisan Editor
✓ Artisan Forge
✓ ae command
✓ artisan:// links

[ Install ]

Advanced options
```

Advanced options may expose:

```text
Components
☑ Artisan Editor
☑ Artisan Forge

Integrations
☑ Install the ae command
☑ Register artisan:// links
☐ Start Forge when I sign in
☐ Create a desktop shortcut
```

Editor and Forge are selected by default. A small shared core containing the
permanent `ae`, installation metadata, and integration repair logic is always
installed. The user-visible component choices do not create two competing
installation systems.

Running temporary `ae` without an existing installation starts setup. A healthy
permanent installation treats plain `ae` as `ae open`.

## Artifact Model

GitHub Releases contains a signed manifest and platform artifacts:

```text
release-manifest.json
release-manifest.sig
artisan-windows-x64.zip
artisan-windows-arm64.zip
artisan-macos-arm64.tar.zst
artisan-macos-x64.tar.zst
artisan-linux-x64-glibc.tar.zst
artisan-linux-arm64-glibc.tar.zst
```

Linux libc variants may be added when support is real and verified. Unsupported
platforms fail before downloading an incompatible artifact.

The release manifest includes at least:

- product version;
- supported operating system, architecture, and libc;
- Editor and Forge compatibility version;
- artifact byte size and SHA-256 digest;
- signing identity and manifest format version;
- minimum bootstrap and permanent CLI versions;
- archive entry allowlist or equivalent extraction constraints; and
- release channel.

The bootstrap verifies the signed manifest, verifies the selected artifact
digest, extracts into a new staging directory, validates the staged layout, and
only then changes the active installation.

## Managed Layout

Suggested user-scoped roots:

```text
Windows
%LOCALAPPDATA%\Artisan\

macOS
~/Library/Application Support/Artisan/

Linux
~/.local/share/artisan/
```

Each root follows the same logical structure:

```text
Artisan/
├── bin/
│   └── ae
├── versions/
│   ├── 0.1.0/
│   │   ├── editor/
│   │   └── forge/
│   └── 0.2.0/
├── current -> versions/0.2.0
├── installation.json
└── staging/
```

Windows may use an atomically replaced pointer file or junction where a symlink
is inappropriate. Consumers resolve the active version through one stable
installation boundary.

Forge profiles, conversations, projects, settings, and durable data are not
stored inside a version directory. Updating or uninstalling binaries must not
silently delete Forge data.

## Bootstrap State Machine

First-time setup is resumable:

```text
No installation
  → Resolve release
  → Download
  → Verify
  → Stage
  → Install integrations
  → Verify permanent ae
  → Verify Forge
  → Activate
  → Schedule bootstrap removal

Partial installation
  → Resume or roll back safely

Healthy installation
  → Delegate to permanent ae
```

The bootstrap records enough state to distinguish a missing installation from
an interrupted one. Re-running `ae` resumes or repairs rather than layering a
second installation over partial files.

The native distribution executables are Rust crates. Expected failures are
typed domain outcomes, resources have explicit lifetimes, downloads and process
output are bounded, and platform-specific operations stay behind narrow
modules. The TypeScript/Effect distribution implementation remains migration
evidence until native release qualification replaces its gates; it is not the
shipping runtime.

## Native Crate Architecture

The repository contains one Cargo workspace with two product crates:

```text
modules/bootstrap  → artisan-bootstrap
modules/cli        → artisan-editor-cli (binary: ae)
```

`artisan-bootstrap` is a disposable first-install executable and the retained
signed lifecycle engine. Its temporary invocation downloads and authenticates a
release, installs selected components, creates the stable command integration,
performs the explicit first-time setup handoff, and removes its temporary copy.
Each installed version retains its authenticated bootstrap at
`versions/<version>/bin/artisan-bootstrap[.exe]` so permanent `ae` can delegate
only update, owned repair, and uninstall mechanics.

The `ae` binary is the permanent user-facing CLI. It owns Forge profile setup,
lifecycle, diagnostics, logs, pairing, and the public command UX. It never owns
projects or conversations, never accepts a project root, and never creates a
profile implicitly. Forge remains the sole application-state authority.

The crates compile from the same source for Windows x64/arm64, macOS
x64/arm64, and Linux x64/arm64. Linux product manifests distinguish `glibc`
from `musl`. A platform is not considered released merely because its Rust
target compiles: its Editor/Forge payload, integrations, signing policy, and
clean-machine lifecycle gate must also pass.

## Command Ownership Handoff

The transport script places the bootstrap in a unique temporary directory:

```text
Windows: %TEMP%\artisan-bootstrap-<nonce>\ae-bootstrap.exe
macOS:   $TMPDIR/artisan-bootstrap-<nonce>/ae-bootstrap
Linux:  ${TMPDIR:-/tmp}/artisan-bootstrap-<nonce>/ae-bootstrap
```

Windows permits an executable to run directly from a temporary directory
without registering or installing that temporary executable. The bootstrap may
be unsigned during prerelease development and must clearly disclose the
resulting Unknown Publisher/SmartScreen warning. Authenticode remains a future
stable-release hardening layer, not a prerequisite for Artisan's own signed
release verification.

The permanent `ae` is installed into the Artisan-managed `bin` directory. It
never relies on or overwrites a package-manager shim.

The handoff is:

1. Install the permanent CLI into the Artisan-managed `bin` directory.
2. Add that directory to the user's PATH or install the platform-appropriate
   stable command link.
3. Invoke permanent `ae doctor` by absolute path.
4. Verify Forge can start, report status, and stop or remain running according
   to setup intent.
5. Verify the protocol and application integrations.
6. Finalize the installation manifest only after permanent health succeeds.
7. Exit the temporary bootstrap.
8. Have an out-of-tree helper remove the temporary bootstrap directory after
   its process exits.

The helper waits for the bootstrap process to exit before removal, especially
on Windows. Cleanup never targets the managed installation or user data. A
temporary-file cleanup failure is nonfatal and never rolls back an otherwise
healthy Artisan installation.

Existing shells may not observe a PATH update immediately. The bootstrap
finishes the current action by invoking permanent `ae` through its absolute
path, never by asking the current shell to resolve `ae` again.

## Permanent Command Surface

`ae` owns the entire installed product:

```text
ae                  Open Artisan; first-time setup only when native install is absent
ae setup            Create or update Forge installation/profile configuration
ae open             Pair and open the client
ae start            Start Forge
ae stop             Stop Forge
ae restart          Restart Forge
ae status           Inspect Forge
ae logs             Read or follow Forge logs
ae doctor           Diagnose and safely repair installation and integrations
ae update           Atomically update Editor, Forge, and the permanent CLI
ae uninstall        Remove installed binaries and integrations
```

Uninstall asks separately whether durable Forge data should be retained or
removed. Removing data is explicit and destructive; removing the application is
not permission to erase conversations or projects.

## Updating

Editor, Forge, and `ae` form one compatibility release. `ae update`:

1. resolves the requested channel;
2. downloads and verifies a complete compatible release;
3. installs beside the current version;
4. runs staged health and migration compatibility checks;
5. stops or drains the owned Forge instance when required;
6. atomically switches the active version;
7. restarts and verifies Forge;
8. rolls back the active pointer if health verification fails; and
9. removes old versions according to retention policy only after success.

Electron must not run a second updater. The temporary bootstrap version only
controls whether first-time setup understands the release manifest it is asked
to install.

## Protocol Registration

The registered scheme is `artisan`, with the current lifecycle request:

```text
artisan://forge/start
```

Protocol input is decoded as a strict typed contract. Arbitrary commands,
credentials, fragments, or shell strings are rejected.

Protocol registration points at stable permanent `ae`, never a versioned
Editor executable or the temporary download bootstrap. The internal protocol
command accepts only the fixed capability and delegates to the same Forge
health, one-time pairing, and browser-open path as `ae open`.

## Windows Integration

The permanent installer creates a Start Menu folder:

```text
%APPDATA%\Microsoft\Windows\Start Menu\Programs\Artisan\
├── Artisan Editor.lnk
├── Start Artisan Forge.lnk
├── Artisan Forge Logs.lnk
└── Uninstall Artisan.lnk
```

The primary shortcut targets the permanent Editor launcher. Maintenance
shortcuts target permanent `ae` with fixed argv. A desktop shortcut is optional.
Artisan does not attempt unsupported taskbar pinning.

`artisan://` is registered per user under:

```text
HKCU\Software\Classes\artisan
```

The open command passes the URL as one quoted argument to the permanent
launcher. User PATH is updated without overwriting unrelated entries, and
`WM_SETTINGCHANGE` is broadcast after a successful change.

Optional Forge sign-in startup uses a current-user scheduled task. The setup
screen makes autostart a separate choice from installing Forge.

The Windows Editor build emits only Electron Builder's unpacked application
directory. The managed distribution archive incorporates that directory beside
Forge and permanent `ae`; Artisan does not produce an NSIS installer or any
other parallel installation lifecycle.

Production Windows builds require Authenticode signing. Electron Builder reads
the certificate from `CSC_LINK` and its password from `CSC_KEY_PASSWORD`; the
protected release gate fails when either input is absent and independently
requires `Get-AuthenticodeSignature` to report `Valid` for `Artisan Editor.exe`.
Certificates and passwords remain protected release-environment secrets and are
never committed to the repository.

## macOS Integration

The bootstrap installs the signed and notarized application bundle at:

```text
~/Applications/Artisan Editor.app
```

An explicitly elevated system-wide mode may use `/Applications`. The app bundle
is the Finder, Spotlight, and Launchpad application entry; Artisan does not
silently modify the Dock.

The signed `Info.plist` declares `CFBundleURLTypes` for `artisan`. Electron
receives cold and warm protocol launches through the platform `open-url`
boundary.

The permanent CLI lives in Artisan's managed application-support directory and
is exposed through a stable user command link such as `~/.local/bin/ae`.
System-wide `/usr/local/bin/ae` requires an explicit elevated choice.

Optional Forge sign-in startup uses a per-user LaunchAgent or supported modern
login-item service. Installation does not silently enable it.

A signed/notarized `.pkg` with selectable Editor and Forge components may be
offered later as a native fallback. A `.dmg` is suitable for an Editor-only
manual distribution, but it is not the canonical complete installer because it
cannot reliably own Forge, CLI, and repairable integration setup.

## Linux Integration

The bootstrap installs a desktop entry at:

```text
~/.local/share/applications/artisan-editor.desktop
```

The entry targets the permanent absolute launcher and includes:

```ini
[Desktop Entry]
Version=1.0
Type=Application
Name=Artisan Editor
Comment=Open Artisan Editor
Exec=/absolute/artisan/path/bin/ae open %u
Icon=artisan-editor
Terminal=false
Categories=Development;
MimeType=x-scheme-handler/artisan;
StartupNotify=true
```

Icons are installed beneath:

```text
~/.local/share/icons/hicolor/<size>/apps/artisan-editor.png
```

Available desktop and MIME cache refresh commands are invoked best-effort.
`xdg-mime` registers `x-scheme-handler/artisan`. Missing desktop utilities are
reported but do not invalidate an otherwise usable headless Forge install.

The permanent CLI is exposed at `~/.local/bin/ae`. Setup explains when the
user's shell does not include that conventional user bin directory.

Optional Forge sign-in startup uses a user-level systemd service where
available. Other init systems remain unsupported until implemented and tested.

No apt repository, Flatpak, AppImage, or distribution store is required.
Native packages and portable Editor artifacts may be added later as alternative
delivery channels, but they must converge on the same managed layout and `ae`
installation manifest.

## Shortcut Ownership

Shortcuts are platform-native:

- Windows: Start Menu `.lnk` files, with optional desktop shortcut.
- macOS: the signed `.app` in Applications.
- Linux: a `.desktop` entry and icon theme assets.

Every shortcut targets the permanent installation. The bootstrap records every
created integration in `installation.json`, including its ownership fingerprint
where applicable.

`ae doctor` recreates missing owned integrations and reports integrations that
were replaced by the user or another application. `ae uninstall` removes only
entries that still point at the installation being removed.

## Installation Manifest

The durable installation manifest includes:

```json
{
	"format_version": 1,
	"active_version": "0.1.0",
	"channel": "stable",
	"components": {
		"editor": true,
		"forge": true
	},
	"integrations": {
		"ae_path": true,
		"protocol": true,
		"application_shortcut": true,
		"desktop_shortcut": false,
		"autostart": false
	}
}
```

The production schema additionally tracks verified artifact identity, install
root, platform, architecture, created integration paths, active/previous
version, interrupted transaction state, and timestamps. Secrets and Forge
credentials are never stored in this manifest.

## Security and Failure Requirements

- Manifest authentication is mandatory; a checksum fetched from the same
  unauthenticated location is insufficient by itself.
- Redirects, artifact origins, archive paths, sizes, and extraction totals are
  bounded and validated.
- Archives cannot write outside staging or create unsafe links.
- Executables are never launched before verification.
- Platform code-signing verification supplements manifest verification where
  available.
- Installation activation and version switching are atomic.
- Interruption leaves either the old healthy version active or a resumable
  staged transaction.
- PATH, protocol, shortcut, autostart, and uninstall operations are idempotent.
- Logs redact URLs or headers that may contain credentials.
- The landing transport downloads only the small platform bootstrap. Native
  product download begins only after the user explicitly executes it.

## Current Implementation Status

The native implementation now uses the release-manifest architecture:

- `@artisan/distribution` owns `ReleaseManifest`, `InstallationManifest`,
  `Installer`, `InstallationStore`, `Activation`, and `IntegrationLifecycle`,
  including platform selection, signature/artifact verification, bounded ZIP
  staging, resumable transactions, rollback, and fingerprinted ownership;
- the Windows product Layer owns stable `ae` and Editor launchers, user PATH,
  `artisan://`, Start Menu shortcut, optional desktop shortcut, and the managed
  installation root;
- `artisan-bootstrap` implements signed manifest and artifact verification,
  bounded ZIP and `tar.zst` extraction, staged activation, stable CLI
  integration, first-time handoff, repair, update, uninstall, and safe
  temporary cleanup across the supported target families;
- the Rust `ae` implements the permanent command surface, authenticated
  loopback Forge lifecycle, explicit profile setup, bounded logs, pairing, and
  delegation to the retained installer engine;
- permanent `ae` owns update, doctor/repair, and retained/destructive uninstall;
  Electron and package registries do not own updates; and
- Windows artifacts and the legacy Effect implementation retain deterministic,
  hermetic lifecycle coverage. The former packed-package gate is migration
  evidence only; native release qualification must exercise the Rust binaries
  through the landing transport.

The PowerShell and POSIX landing transports are implemented with platform,
architecture, and Linux-libc selection; pinned GitHub Release resolution;
SHA-256 verification; argument forwarding; temporary execution; and cleanup.
The website still needs to serve these scripts and render the detected command.
Windows arm64, macOS, and Linux product payload/release gates remain planned.
Authenticode is unavailable during the unfunded prerelease phase and must not
block unsigned prerelease artifacts.

### Runtime configuration

The production bootstrap embeds the official GitHub origin, stable channel,
signing-key identity, and Ed25519 public key at build time. First use atomically
persists that public configuration in `distribution.json`; permanent `ae` reads
it in later shells and does not require release environment variables.
Production bootstrap builds fail when signing trust is absent.

The following variables are explicit development/test overrides:

```text
ARTISAN_RELEASE_OWNER=<GitHub owner>
ARTISAN_RELEASE_REPOSITORY=<GitHub repository>
ARTISAN_RELEASE_CHANNEL=stable|beta|nightly
ARTISAN_RELEASE_KEY_ID=<manifest signing key id>
ARTISAN_RELEASE_PUBLIC_KEY_BASE64=<canonical base64 Ed25519 public key DER>
```

`ARTISAN_RELEASE_PUBLIC_KEY_FILE` may replace the base64 value. `ARTISAN_HOME`
may override the managed root with an absolute path; Windows otherwise uses
`%LOCALAPPDATA%\Artisan`. The trusted key is build input or persisted public
configuration, never fetched from the release it authenticates.

Release production requires `ARTISAN_RELEASE_SIGNING_KEY_ID` and exactly one of
`ARTISAN_RELEASE_SIGNING_KEY_PEM` or `ARTISAN_RELEASE_SIGNING_KEY_FILE`.
`ARTISAN_RELEASE_VERSION`, `ARTISAN_RELEASE_CHANNEL`,
`ARTISAN_MINIMUM_BOOTSTRAP_VERSION`, and `ARTISAN_MINIMUM_CLI_VERSION` select
manifest metadata. Signing private keys must not be committed.

### Ownership boundaries

The bootstrap installs or resumes the native product and then disappears. The
permanent `ae` owns launch, update, repair, and uninstall. Neither installation
nor `ae doctor --fix` silently creates a Forge profile: explicit `ae setup` is
the sole profile-creation boundary, and first-time UX must invoke it
deliberately. Forge owns profiles, projects, conversations, and durable data
outside version directories.

### Build and verification commands

```sh
pnpm run build:native
pnpm run test:native
pnpm run package:distribution:windows
pnpm run verify:distribution:windows
pnpm exec vitest run .tests/deep/distribution/windows-install-lifecycle.test.ts
pnpm run validate
```

### Release operation

`master` is the integration branch. The protected `candidate` branch is only a
fast-forward pointer to a commit already reachable from `master`. Pull requests
and pushes run staging verification but never publish.

Production candidates are created only by manually dispatching `Artisan release
pipeline` on `candidate`:

- `dry-run` builds and freezes the exact candidate, verifies its signed
  manifest and artifact manifest, and performs no provider writes;
- `release` uses the same frozen bytes, waits at the `release-approval`
  environment, creates a draft GitHub release, uploads exact missing assets,
  re-downloads and verifies provider bytes, then publishes it as the latest
  release; and
- `resume` restores the exact candidate bundle identified by version, full
  commit SHA, and originating Actions run ID. It never rebuilds artifacts.

The candidate manifest binds ordered filenames, sizes, and SHA-256 digests.
Existing tags, releases, or assets with a different identity are terminal
failures rather than overwrite targets. npm is not a release channel.

Packaging emits the signed Windows x64 ZIP, `release-manifest.json`, and
`release-manifest.sig` under `.dist/distribution-release`. Artifact verification
covers deterministic contents, signatures, hashes, layout, and unsupported
architecture failure. Lifecycle acceptance is hermetic: it does not mutate host
registry, PATH, shortcuts, scheduled tasks, or network.

## Implementation Milestones

### 1. Release Contract

- Windows: implemented and verified.
- macOS/Linux artifacts: planned.

### 2. Permanent Installer Services

- Shared contracts and Windows production Layers: implemented and verified.
- macOS/Linux production Layers: planned.

### 3. Temporary Native Bootstrap and Landing Transport

- Native Rust bootstrap and permanent handoff: implemented; Windows release
  compilation and clean-machine execution remain release-gated.
- Windows/macOS/Linux target selection and native bootstrap source:
  implemented.
- Versioned PowerShell/POSIX transport scripts: implemented with offline
  selection and safety tests.
- Landing-page routing and real released-asset execution: planned.
- npm publication is removed from the release plan.

### 4. Windows

- Windows x64 release, permanent CLI, PATH, protocol, shortcut, repair, update,
  rollback, and uninstall: implemented and verified.
- Windows arm64 and real clean-machine release acceptance: planned.

### 5. macOS

- Status: planned.
- Produce signed/notarized Editor and Forge payloads.
- Register `artisan://` in the signed bundle.
- Implement Applications, CLI link, LaunchAgent/login-item, repair, update,
  rollback, and uninstall behavior.

### 6. Linux

- Status: planned.
- Produce glibc artifacts with explicit architecture support.
- Install `.desktop`, icons, MIME handler, CLI link, and user systemd service.
- Verify behavior across the supported desktop environments and headless mode.

### 7. Release Gate

- Windows hermetic artifact and lifecycle gates: implemented.
- The Windows workflow builds the Rust binaries, embeds public release trust,
  emits the standalone bootstrap plus checksum, and includes native `ae` and
  the retained bootstrap in the product archive. Its clean-runner result must
  pass before this milestone is marked released.
- Released fresh-machine Windows qualification and all macOS/Linux gates:
  planned.
- Exercise a clean first install from the landing-page command.
- Exercise interrupted install and resume.
- Verify the temporary bootstrap removes itself only after permanent health
  succeeds.
- Verify a new shell resolves permanent `ae`.
- Verify Editor launch, `artisan://`, Forge lifecycle, update, rollback, repair,
  retained-data uninstall, and destructive-data uninstall.
- Verify installation and every subsequent lifecycle operation require no npm
  or package registry.

## Acceptance Criteria

The distribution milestone is complete when:

- a new supported machine can run the landing page's detected command and
  receive a healthy Editor, Forge, and permanent `ae` installation;
- both components are selected by default through one first-time experience;
- permanent `ae` owns all subsequent product lifecycle operations;
- the temporary native bootstrap safely removes itself after verification;
- shortcuts and `artisan://` work through permanent platform-native
  integrations;
- updates atomically preserve Editor/Forge compatibility and roll back on
  failed health checks;
- uninstall removes owned binaries/integrations without silently erasing Forge
  data;
- Windows, macOS, and Linux acceptance suites verify installation, repair,
  update, and removal; and
- the repository completion matrix records implementation only after the full
  release gates pass.
