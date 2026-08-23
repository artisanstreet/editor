# Artisan Observability

Artisan uses **PostHog for product analytics** and **Sentry for reliability**. The two systems have separate consent choices and separate purposes.

## Consent

`telemetry.json` lives in the Artisan home beside the other private home files. It contains a random installation ID and two independent tri-state choices:

- `usage_analytics`: `unset | enabled | disabled`
- `crash_reports`: `unset | enabled | disabled`

No vendor client may send while its choice is `unset` or `disabled`. Development, test, validation, and uncredentialed builds fail closed. Disabling analytics takes effect immediately. Native Electron crash-handler changes may require an Editor restart.

Headless installations use `ae telemetry ...`; the Editor exposes the same choices through typed Forge RPC. Installation IDs are generated locally, are not hardware-derived, and can be reset locally.

## Data flow

- Renderer product intents are a closed protocol union and travel through the existing Forge connection.
- Forge owns PostHog egress, consent gating, installation identity, validation, deduplication, and bounded buffering.
- Electron main/renderer report sanitized failures to the `artisan-editor` Sentry project.
- Forge reports sanitized internal failures to the `artisan-forge` Sentry project.
- No preload or general Electron IPC surface exists for telemetry.

## Never collected

The following are forbidden in analytics, Sentry events, breadcrumbs, spans, logs, and attachments:

- prompts, responses, reasoning, observations, or generated content;
- source code, file contents, diffs, patches, clipboard data, or images;
- terminal commands/output, process output, arguments, or environment variables;
- repository, project, workspace, branch, remote, file, host, or account names;
- local paths and URLs containing paths or query strings;
- tokens, pair codes, cookies, authorization values, connection strings, DSNs with secrets, or credentials;
- arbitrary exceptions, Effect causes, provider diagnostics, request/response bodies, transport envelopes, database rows, or attachments;
- raw thread, run, project, workspace, message, connection, or capability IDs;
- user-defined model, profile, tool, MCP, routine, or integration names.

PostHog autocapture and product session replay are disabled. Sentry replay, structured logs, client outcome reports, DOM breadcrumbs, console breadcrumbs, fetch/XHR breadcrumbs, request bodies, default PII, and initial performance tracing are disabled.

## Product event catalog

All event names are lower-case snake case. All properties are closed, typed, bounded, and reviewed. Durable outcomes are emitted at the canonical Forge transition rather than from UI clicks.

| Event | Owner | Safe purpose |
|---|---|---|
| `forge_started` / `forge_stopped` | Forge lifecycle | starts, clean shutdown, cold start, previous unclean exit |
| `editor_session_started` | renderer ready controller | installation sessions and readiness |
| `forge_connection_finished` | renderer connection controller | initial/reconnect/resume reliability |
| `project_added` / `project_opened` | Forge project service | activation without project identity |
| `thread_created` | accepted thread-create transition | activation and built-in engine/model mix |
| `run_started` / `run_finished` | canonical orchestration transitions | completion, failure, cancellation, duration |
| `engine_setup_finished` | engine setup service | built-in engine install/update/auth outcomes |
| `model_transition_finished` | continuation service | safe catalog transitions and outcome |
| `tool_approval_resolved` | Forge tool control plane | fixed tool category and decision only |
| `usage_interruption_detected` / `usage_recovery_finished` | usage service | classified interruption/recovery outcomes |
| `thread_retention_changed` | settings acceptance | fixed retention enum only |
| `feature_used` | owning product service | a closed feature enum only |

Common properties are limited to schema version, surface, environment, release, app version, release channel, platform, architecture, Forge mode, packaged state, and an ephemeral client session ID. Built-in engine/model IDs are allowed only after catalog validation; custom values become `custom_or_unknown`.

## Reliability capture

### Editor

Capture uncaught Electron-main and renderer failures, bootstrap failures, sanitized native-crash classifications from `render-process-gone`, exhausted handoff/recovery failures, and severe unresponsive episodes. Electron minidump and screenshot integrations stay disabled because they are attachments; native crashes are reported without a dump or fabricated stack. Expected reconnects, user cancellations, validation/authentication-needed states, offline states, and denied approvals are not Sentry issues.

### Forge

Capture bootstrap failures, unhandled failures, database migration/lease/invariant failures, persistence abandonment, unexpected orchestration/HTTP/WebSocket failures, severe bounded-memory incidents, and previous unclean exits. Provider authentication/configuration/usage errors remain classified product outcomes.

The local Forge crash marker contains only release, commit, timestamps, a clean flag, and a coarse memory bucket. It never contains a path, stack, log, dump, or user content. A next-start report is occurrence evidence, not a fabricated stack trace.

## Release configuration

Canonical releases are:

- `artisan-editor@<ARTISAN_RELEASE_VERSION>+<ARTISAN_RELEASE_COMMIT>`
- `artisan-forge@<ARTISAN_RELEASE_VERSION>+<ARTISAN_RELEASE_COMMIT>`

Official builds generate hidden source maps for renderer, Electron main, and Forge SEA. The Forge build injects its debug ID before creating the SEA blob so the shipped executable and private map match. `.scripts/build/upload-sentry-source-maps.ts` runs only when all release credentials are present, injects Editor debug IDs, uploads the Editor and already-injected Forge maps, and does not stage maps into installed artifacts. Sentry auth tokens are release-environment secrets and must never enter bundles, artifacts, configuration, or logs.

Runtime configuration names:

- `ARTISAN_TELEMETRY_CONFIG_PATH`
- `ARTISAN_TELEMETRY_ENVIRONMENT`
- `ARTISAN_RELEASE_VERSION`
- `ARTISAN_RELEASE_COMMIT`
- `ARTISAN_POSTHOG_PROJECT_KEY`
- optional `ARTISAN_POSTHOG_HOST` (restricted to the approved EU/US ingestion origins)
- `ARTISAN_SENTRY_EDITOR_DSN`
- `ARTISAN_SENTRY_FORGE_DSN`
- optional `ARTISAN_SENTRY_EDITOR_ORIGIN` for a tunnel/override (otherwise derived from the Editor DSN as the exact CSP `connect-src` origin)

Release-only upload configuration:

- `SENTRY_AUTH_TOKEN`
- `SENTRY_ORG`
- `SENTRY_EDITOR_PROJECT`
- `SENTRY_FORGE_PROJECT`

Public ingestion keys/DSNs do not bypass consent.

## Review rule

Every new event or property requires:

1. a named product owner and question it answers;
2. a closed schema and cardinality bound;
3. sanitizer fixtures containing fake secrets, paths, prompts, code, and terminal output;
4. disabled/unset zero-egress tests;
5. exactly-once tests when it represents a lifecycle transition;
6. an explicit retention rationale and dashboard consumer.

If those cannot be supplied, do not collect it.
