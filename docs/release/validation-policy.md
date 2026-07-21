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
- build or load the native bounded-file-store addon; or
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

## Explicit native-addon verification

The same manually dispatched workflow creates `native-addon` only when
`run_native_addon: true`. It requires the separate `artisan-native-addon`
self-hosted runner label and protected `native-addon-validation` environment.
The isolated runner needs the project-supported Windows GNU/Rust toolchain and
must be approved for native filesystem smoke and crash-recovery execution.

The job sets `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1` immediately before invoking the
package's canonical `verify:local` gate. That variable is intentionally absent
from ordinary CI and all other release jobs. Do not add native execution to
`pnpm run validate`, and do not expose native outputs or temporary filesystem
state through workflow artifacts.

## Release acceptance

A release candidate requires both the ordinary release gate and the packaged
desktop release gate to pass. Live Codex and the standalone native-addon
recovery jobs remain explicit opt-ins; a skipped opt-in job is not an
ordinary-CI failure. Record the runner identity, selected opt-ins, and outcome
in the release record without including sensitive command output or fixture
data.

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
The verifier inspects the actual ASAR and unpacked runtime paths, creates a
unique temporary user-data directory, clears external `NODE_PATH`, starts the
packaged executable with a bounded deadline, and requires one machine-readable
success record. The smoke opens the exact staged `node-pty`, bounded-file-store,
and Koffi bindings in two distinct utility epochs. It creates a thread through
`ArtisanClient`, records accepted utility termination plus old/new utility
epochs and PIDs, reconnects over newly transferred `MessagePortMain` pairs,
proves semantic duplicate-safe durable replay even when the client regenerates
transport timestamps, accepts a later command, and then disposes the utility.

The same packaged executable creates the real BrowserWindow, loads the renderer
through the custom protocol, and verifies the narrow preload bridge. Electron
delivers trusted keyboard activation to `New chat` before and after the utility
restart and trusted native mouse input to Marketplace and the
chat/editor/orchestrator controls. The smoke also verifies Marketplace focus
restoration, trusted composer input, right-pane keyboard reachability, truthful
no-file/no-terminal states, the activity/taskbar bridge, accessible names,
computed wide/narrow pane state, and Electron's real 200% zoom factor. It never
starts an Engine run or calls a model. Temporary smoke data, including the
native-store root, and descendant processes are removed on every outcome.

- build the existing static renderer and package it as the renderer payload;
- assert the main/utility entry points and production-only files are present in
  the expected package layout;
- assert `node-pty`, the bounded native addon, and Koffi are explicitly staged
  and unpacked while keeping ordinary CI free of addon loading;
- run the existing typed client reconnect/replay fixtures through transferred
  Electron control and stream ports, including a forced utility-process restart;
- prove single-instance ownership and cleanup without publishing temporary
  workspaces, databases, or logs.

The packaged Electron gate is the mounted renderer proof: it provides
keyboard/focus, accessible-name, computed responsive-layout, and real
browser-zoom checks without adding an external browser dependency or starting a
development server. The visual-fixture route remains supplementary source-level
coverage for semantic, reduced-motion, high-contrast, and long-label states.
