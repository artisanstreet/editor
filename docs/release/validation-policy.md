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

A release candidate requires the ordinary release gate to pass. The opt-in jobs
are additional evidence only when their corresponding validated release policy
requires them; a skipped opt-in job is not an ordinary-CI failure. Record the
runner identity, selected opt-ins, and outcome in the release record without
including sensitive command output or fixture data.
