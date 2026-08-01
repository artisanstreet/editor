# Release validation policy

## Development validation

Artisan is not published, so hosted CI and release workflows are intentionally
absent. Every milestone is validated locally with `pnpm run validate` before it
is committed directly to `master`. The repository uses its pinned pnpm version
and committed lockfile.

Node 24 is deliberately selected because the repository's verified local
toolchain is Node `v24.18.0`; no package currently declares a root `engines`
range. Keep local validation on Node 24 until the project records a different
supported runtime and verifies the complete gate there.

Development validation must remain hermetic and must not:

- require an installed or authenticated Codex CLI;
- run a paid, subscription-backed, or otherwise live Engine action;
- upload test transcripts, temporary databases, workspace contents, logs, or
  other fixtures as artifacts.

The deterministic fake child process and committed byte-faithful Codex
transcripts are the ordinary-conformance fixtures. Every committed transcript
must be sanitized before review: remove credentials, cookies, authorization
headers, account identifiers, absolute user paths, workspace contents, and any
other user or provider-private data. Never use a validation log or artifact as
a fixture-recording channel.

## Explicit live Codex verification

Live Codex verification is not part of the local milestone gate. Run the
existing `ARTISAN_ENGINE_LIVE=1` version/handshake/account-read probe only as an
explicit manual check on an already authenticated development machine.

The machine must have an already authenticated Codex CLI available as `codex`,
or set `ARTISAN_CODEX_EXECUTABLE` to its executable path. The check runs only
the existing `ARTISAN_ENGINE_LIVE=1` version/handshake/account-read probe. That
probe must remain non-billable and must never start a turn, send a user prompt,
record a transcript, or echo authentication material.

Codex CLI is the only live production-engine policy covered here. This check
does not install or invoke Claude, another agent CLI, or an agent SDK.

## Release acceptance

There is no hosted release path while Artisan remains unpublished. Before the
first publication, restore a manually dispatched release workflow that freezes
an exact candidate and requires both the ordinary release gate and packaged
desktop release gate. Live Codex remains an explicit opt-in. Record the runner
identity, selected opt-ins, and outcome in the release record without including
sensitive command output or fixture data.

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

Electron packaging is required release-only evidence. Before publication, run
`package:desktop` followed by `verify:desktop-package` on a protected
native-capable machine; ordinary local validation does neither.
`package:desktop` emits only `.dist/electron-release/win-unpacked`, the exact
Editor directory incorporated into the managed distribution archive. It does
not emit NSIS, embed Forge, install integrations, or own update/uninstall state.
The verifier inspects the actual ASAR, requires the launcher entry plus the
bundled static frontend with its loopback-Forge CSP variant, rejects any
preload/utility/backend payload, and rejects an embedded
`resources/artisan-forge` tree.

Electron Builder's standard Windows signing path is enabled. A future protected
release environment must supply `ARTISAN_WINDOWS_CSC_LINK` and
`ARTISAN_WINDOWS_CSC_KEY_PASSWORD`, mapped only for the packaging step to
Electron Builder's `CSC_LINK` and `CSC_KEY_PASSWORD`. Production packaging must
fail closed when either secret is absent. After packaging, the future release
gate calls `Get-AuthenticodeSignature` on the exact `Artisan Editor.exe`
consumed by the distribution builder and requires status `Valid` before
retaining a distribution release.
