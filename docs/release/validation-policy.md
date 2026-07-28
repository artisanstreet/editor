# Release validation policy

## Ordinary CI

`.github/workflows/ci.yml` is the clean-checkout gate for every push and pull
request. It uses the repository's pinned `pnpm@11.7.0`, installs the committed
lockfile with `--frozen-lockfile`, and runs `pnpm run validate` on
`windows-2025`.

Node 24 is deliberately selected because the repository's verified local
toolchain is Node `v24.18.0`; no package currently declares a root `engines`
range. Keep the workflow on Node 24 until the project records a different
supported runtime and verifies the complete gate there.

Ordinary CI must remain hermetic and must not:

- require an installed or authenticated Codex CLI;
- run a paid, subscription-backed, or otherwise live Engine action;
- upload test transcripts, temporary databases, workspace contents, logs, or
  other fixtures as artifacts.

The deterministic fake child process and committed byte-faithful Codex
transcripts are the ordinary-conformance fixtures. Every committed transcript
must be sanitized before review: remove credentials, cookies, authorization
headers, account identifiers, absolute user paths, workspace contents, and any
other user or provider-private data. Never use a CI log or artifact as a
fixture-recording channel.

## Explicit live Codex verification

`release-validation.yml` always starts with the ordinary clean-checkout gate.
Its `live-codex` job is created only when a dispatcher explicitly supplies
`run_live_codex: true`; it also requires the dedicated self-hosted runner label
`artisan-live-codex` and the protected `live-codex-validation` environment.
Creating that environment with required reviewers is an administrative
prerequisite before enabling the workflow.

The runner must have an already authenticated Codex CLI available as `codex`,
or set the runner-local `ARTISAN_CODEX_EXECUTABLE` to its executable path. The
job runs only the existing `ARTISAN_ENGINE_LIVE=1` version/handshake/account-read
probe. That probe must remain non-billable and must never start a turn, send a
user prompt, record a transcript, or echo authentication material.

Codex CLI is the only live production-engine policy covered here. This workflow
does not install or invoke Claude, another agent CLI, or an agent SDK.

## Release acceptance

A release candidate requires both the ordinary release gate and the packaged
desktop release gate to pass. Live Codex remains an explicit opt-in; a skipped
opt-in job is not an ordinary-CI failure. Record the runner identity, selected
opt-ins, and outcome in the release record without including sensitive command
output or fixture data.

## Desktop integration dependency gates

The current renderer release contract is a strict static Svelte build to
`.dist/frontend`, validated by `pnpm run check:frontend` and included in
`pnpm run validate`. Renderer source remains limited to the protocol and
renderer-safe transport client boundary. The transport package exports typed
Electron MessagePort shape adapters without importing Electron, and its
in-process MessageChannel harness proves reconnect, replay, bounded control
and stream traffic, and scoped teardown against the same client contract. The
current evidence is `.tests/transport/artisan-client.test.ts` and
`.tests/transport/artisan-client-protocol-server.test.ts`; neither is
packaged-Electron restart equivalence.

Electron packaging is required release-only evidence. Every manually dispatched
release validation run executes the `packaged-desktop` job on the protected
native-capable runner, which runs `package:desktop` followed by
`verify:desktop-package`; ordinary CI and the ordinary release gate do neither.
`package:desktop` emits only `.dist/electron-release/win-unpacked`, the exact
Editor directory incorporated into the managed distribution archive. It does
not emit NSIS, embed Forge, install integrations, or own update/uninstall state.
The verifier inspects the actual ASAR, requires the launcher entry plus the
bundled static frontend with its loopback-Forge CSP variant, rejects any
preload/utility/backend payload, and rejects an embedded
`resources/artisan-forge` tree.

Electron Builder's standard Windows signing path is enabled. The protected
release environment supplies `ARTISAN_WINDOWS_CSC_LINK` and
`ARTISAN_WINDOWS_CSC_KEY_PASSWORD`, mapped only for the packaging step to
Electron Builder's `CSC_LINK` and `CSC_KEY_PASSWORD`. Production packaging fails
closed when either secret is absent. After packaging, the workflow calls
`Get-AuthenticodeSignature` on the exact `Artisan Editor.exe` consumed by the
distribution builder and requires status `Valid` before any distribution
release is retained.
