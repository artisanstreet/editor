# Artisan PostHog and Sentry End-to-End Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Add privacy-preserving product analytics and actionable crash/error monitoring across Artisan Editor and Forge without exposing prompts, code, repository data, terminal data, paths, credentials, or arbitrary user content.

**Architecture:** Use PostHog as Artisan's only analytics platform and Sentry as the separate reliability platform. Forge is the authority and network egress point for product events from both the renderer and backend; Electron main, the renderer, and Forge report only sanitized runtime failures to their appropriate Sentry project. A single home-scoped consent file is read before SDK startup and exposed to the renderer through typed Forge APIs.

**Tech Stack:** Electron 43, Svelte/SvelteKit static renderer, Effect TypeScript, WebSocket control protocol, Node 22/24 Forge SEA, Rust `ae` CLI, `posthog-node`, `@sentry/electron`, `@sentry/browser` or the compatible Electron renderer entry, `@sentry/node`, Sentry CLI/source-map upload.

---

## 1. Decisions

### Vendor split

- **PostHog:** all marketing and product analytics. Do not add Fathom.
- **Sentry:** crashes, exceptions, release health, regressions, and later a small amount of sampled performance tracing.
- **Do not enable PostHog error tracking** in parallel with Sentry; this would duplicate the error inbox.
- **Do not enable session replay in either product.** Artisan renders source code, prompts, model output, diffs, and terminal content, so replay has an unacceptable accidental-capture surface.

### Vendor/project organization

Recommended cloud layout:

- PostHog organization: `Artisan`
  - Production project: `Artisan Product`
  - Non-production project: `Artisan Sandbox`
  - One production dashboard workspace, segmented by `surface`; this keeps website, Editor, and Forge analytics in one place.
- Sentry organization: `Artisan`
  - Project: `artisan-editor` for Electron main, native Electron crashes, desktop renderer, and Forge-served browser renderer errors.
  - Project: `artisan-forge` for the Forge Node/SEA backend.
  - One cross-project reliability dashboard in the Sentry organization.

Use the EU data region for both vendors unless the business has already selected a different legal/data-residency posture. Sign DPAs and record retention/deletion settings before production rollout.

### Recommended consent defaults

Use two independent, explicit choices:

- `usage_analytics`: `unset | enabled | disabled`
- `crash_reports`: `unset | enabled | disabled`

Recommended behavior:

- No PostHog or Sentry events are transmitted while the setting is `unset`.
- First run presents a concise choice with equal enable/disable affordances; it must not block local use.
- Development, test, validation builds, and local manual builds default to disabled and use no production keys.
- PostHog enable/disable applies immediately.
- Sentry JavaScript capture applies immediately where supported; native Electron crash-handler enable/disable takes effect after restart and the UI says so.
- Disabling telemetry must not send a final "disabled" event.
- Existing bounded local diagnostics remain separate from vendor telemetry and are never uploaded automatically.

If legal/product later chooses default-on with opt-out, that is a policy change, not an implementation shortcut. The tri-state model should remain so the software can distinguish "not asked" from an explicit answer.

## 2. Current Artisan Architecture That Shapes the Design

The implementation must preserve these existing boundaries:

- `modules/desktop/src/main.ts` is the privileged Electron entry. The renderer is sandboxed with `contextIsolation: true`, `nodeIntegration: false`, and no preload/general IPC surface.
- `modules/desktop/src/main.ts` already observes `unresponsive`, `responsive`, and `render-process-gone`, and has bounded, opt-in local renderer diagnostics.
- `.config/desktop.vite.config.ts` patches the packaged renderer CSP to allow only the custom `artisan://app` origin plus the loopback Forge.
- `modules/frontend/src/lib/runtime/frontend-runtime.ts` and `browser-frontend-runtime.ts` compose the renderer runtime and connect to Forge through the existing typed WebSocket transport.
- `modules/frontend/src/lib/browser/runtime-surface.ts` already distinguishes `desktop` and `browser` surfaces.
- `modules/forge/src/entry.ts` and `forge-host.ts` own Forge lifecycle; Forge can run independently of Electron and may serve the same frontend in development/headless modes.
- `modules/forge/src/memory-telemetry.ts` already records coarse local memory high-water marks because Forge has experienced very large native/external-memory growth.
- `modules/cli/rust/instance.rs` owns the home-scoped `config.json`, `secrets.json`, state, and log paths. `modules/cli/rust/process.rs` launches Forge; `modules/cli/rust/commands.rs` launches Electron and can pass the same telemetry config path to both.
- `.scripts/build/runner.ts` supplies `ARTISAN_RELEASE_VERSION` to the desktop release, but Forge branding currently reads the workspace version. Release/version propagation must be made identical before Sentry release health is trusted.
- The current production frontend, desktop, and Forge builds do not intentionally produce/upload private source maps.
- There is no current account or organization identity model. Forge pairing/session tokens are authentication capabilities, not user identity, and must never be sent to analytics.

## 3. Target Data Flow

```text
                        PRODUCT ANALYTICS

Desktop renderer ──typed TelemetryIntent──┐
Forge-served browser ─TelemetryIntent─────┼──> Forge ProductTelemetry service
Forge domain lifecycle ─authoritative─────┘      │
                                                  ├─ consent gate
                                                  ├─ strict schema/allowlist
                                                  ├─ pseudonymous installation ID
                                                  ├─ bounded in-memory queue
                                                  └─ posthog-node ──> PostHog

                        ERROR MONITORING

Electron main/native ── @sentry/electron/main ───────> Sentry artisan-editor
Renderer JS errors ─── @sentry/electron/renderer
                       OR @sentry/browser* ───────────> Sentry artisan-editor
Forge Node errors ──── @sentry/node ─────────────────> Sentry artisan-forge
Forge hard exit/OOM ── next-start unclean-exit record > Sentry artisan-forge

*Use the Electron renderer entry only if a spike proves it works without adding
 a preload/general IPC bridge. Otherwise use @sentry/browser directly. Do not
 weaken Artisan's renderer boundary solely for telemetry.
```

Product analytics goes through Forge because that gives Artisan one consent gate, one identity, one event schema, and one queue. Sentry is direct per runtime because errors must still be captured when Forge, transport, or the renderer itself is failing.

Telemetry must be strictly non-blocking: every API returns success/no-op to product code, uses short network timeouts, and drops data when offline or backpressured. Analytics failure must never fail a thread, run, settings save, startup, or shutdown.

## 4. Consent and Anonymous Identity

### Home-scoped file

Add an atomic, private `telemetry.json` beside the existing home files:

```json
{
  "version": 1,
  "installation_id": "random UUID generated locally",
  "usage_analytics": "unset",
  "crash_reports": "unset",
  "updated_at": "ISO timestamp"
}
```

Rules:

- `ae setup` creates the file with a cryptographically random ID and both choices `unset`.
- Existing homes lazily create it on the next `ae` command without sending anything.
- Use the same private atomic-write and symlink protections as `config.json`/`secrets.json`.
- `ae` passes only `ARTISAN_TELEMETRY_CONFIG_PATH` to Forge and Electron. Do not pass consent or IDs in command-line arguments.
- The renderer queries and updates preferences through typed Forge RPC. Forge owns file locking/atomic updates while running.
- Add CLI commands for headless users: `ae telemetry status`, `ae telemetry enable-analytics`, `ae telemetry disable-analytics`, `ae telemetry enable-crash-reports`, `ae telemetry disable-crash-reports`, and `ae telemetry reset-identity`.
- Resetting identity requires explicit confirmation, flushes no prior data, and generates a new ID locally.

### PostHog identity

- PostHog `distinct_id` is `install_<installation_id>`.
- This identifies an installation, not a human. Dashboards must say "active installations," not "users."
- Do not use Forge `instance_id`, backend IDs, paths, repository IDs, machine hostname, OS username, auth tokens, pair codes, cookies, email addresses, or IP-derived identity.
- Set PostHog's geo-IP-disable property on every product event.
- Never send raw thread, run, project, workspace, connection, or message IDs.
- For retry deduplication, derive `$insert_id` locally from a one-way digest of the installation ID plus the canonical local event ID; do not expose the canonical ID as a queryable property.
- Renderer-generated client sessions use a random UUID and rotate on renderer start or after 30 minutes of inactivity. This may be sent as PostHog's session property but must not be persisted as identity.

### Future account identity

No identify/alias/group flow belongs in v1 because Artisan currently has no account or organization domain. When authenticated accounts exist, write a separate migration plan that aliases an anonymous installation exactly once to a stable account ID and adds PostHog groups for organizations. Never infer this from Forge authentication capabilities.

### Sentry identity

Do not make Sentry and PostHog joinable by default. Either omit `user` entirely or assign Sentry a separately salted installation digest solely for affected-install counts. Do not send username, hostname, email, IP, project name, or PostHog distinct ID.

## 5. Shared Privacy Contract

### Never capture

The following are forbidden in PostHog events, Sentry events, breadcrumbs, spans, logs, or attachments:

- Prompt or composer text
- Model responses, observations, generated content, or reasoning
- Source code, file contents, diffs, patches, clipboard content, or images
- Terminal commands, terminal output, process stdout/stderr, or environment variables
- Repository, project, workspace, branch, remote, or file names
- Absolute or relative filesystem paths, working directories, home directories, or URLs containing paths/query strings
- API keys, bearer tokens, pair codes, cookies, authorization headers, connection strings, DSNs with secrets, or credentials
- Arbitrary exception objects, engine diagnostics, provider payloads, request/response bodies, database rows, transport envelopes, or serialized Effect causes
- Thread, run, message, project, workspace, connection, or session capability IDs
- Custom model IDs, profile names, tool names, MCP server names, routine names, or integration names supplied by a user

Any credential-like value encountered in diagnostics is replaced with `[REDACTED]`.

### Allowed common properties

Every product event is constructed from a typed allowlist. Common properties are:

- `event_schema_version`: integer
- `surface`: `desktop_renderer | browser_renderer | forge`
- `environment`: `production | staging | development | test`
- `release`: canonical release string
- `app_version`: semantic version
- `release_channel`: fixed enum
- `platform`: fixed OS enum
- `arch`: fixed architecture enum
- `forge_mode`: `local | headless`
- `is_packaged`: boolean
- Optional anonymous client session UUID

Feature properties must be fixed booleans, bounded numeric measurements, or fixed enums. `engine_id` may be sent only if it matches Artisan's built-in catalog. `model_id` may be sent only if it matches the shipped/resolved catalog; otherwise send `custom_or_unknown`. Never send a provider profile name.

### Cardinality rules

- No raw opaque IDs as event properties.
- Numeric durations and token counts are numeric properties, not strings.
- Error details use stable `artisan_code`/failure-category enums, never raw messages.
- Tool analytics use a fixed category (`file_read`, `file_write`, `shell`, `search`, `browser`, `integration`, `other`), never the raw tool name.
- Setting analytics use a fixed setting-key enum and an allowlisted boolean/enum value.
- Every new event/property requires an event-catalog review and a redaction test.

## 6. PostHog Event Catalog

Use lower-case snake-case names. Prefer durable outcomes over clicks. Start with the P0 catalog below; do not instrument every UI interaction.

### P0 system and activation events

| Event | Authoritative source | Allowed event-specific properties | Purpose |
|---|---|---|---|
| `forge_started` | `modules/forge/src/entry.ts` after successful host acquisition | `forge_mode`, `cold_start_duration_ms`, `previous_exit: clean | unclean | unknown` | Installed base and startup reliability |
| `forge_stopped` | Forge release/finalizer | `uptime_ms`, `shutdown_reason: requested | parent_disconnect | signal | update` | Clean lifecycle; never rely on it for active-use sessions |
| `editor_session_started` | Renderer after ready state and telemetry consent | `surface`, `forge_connection: local | remote`, `time_to_ready_ms` | Product sessions and startup activation |
| `forge_connection_finished` | Renderer connection controller | `outcome: connected | failed`, `attempt: initial | reconnect | resume`, bounded `duration_ms`, stable `failure_code` | Connection reliability without raw endpoints |
| `project_added` | Forge project-directory service after durable success | `kind: git | directory`, `source: picker | explicit`, `is_first_project` | Activation |
| `project_opened` | Forge project/thread affinity boundary | `kind: git | directory`, `is_first_open` | Active product use; no project identity |
| `thread_created` | Forge accepted thread-create command | `engine_id`, catalog-safe `model_id`, `permission`, `has_image_attachment`, `is_first_thread` | Activation and configuration mix |
| `run_started` | Forge canonical run transition, not the Send button | `engine_id`, catalog-safe `model_id`, `permission`, `continuation_kind`, `has_image_attachment` | Run funnel denominator |
| `run_finished` | Forge canonical terminal transition | all `run_started` dimensions plus `outcome: completed | failed | cancelled`, `duration_ms`, stable `failure_code`, optional numeric input/output/cache token counts | Core success and reliability metrics |

Use one `run_finished` event with an `outcome` property rather than separate completion/failure event names. It prevents taxonomy drift and makes success-rate formulas consistent.

### P0 adoption/outcome events

| Event | Source | Allowed properties |
|---|---|---|
| `engine_setup_finished` | Engine installation/auth service | built-in `engine_id`, `operation: install | update | authenticate`, `outcome`, stable `failure_code` |
| `model_transition_finished` | Canonical continuation transition | built-in source/target engine, catalog-safe source/target model, continuation strategy, outcome, stable failure code |
| `tool_approval_resolved` | Forge tool control plane | fixed tool category, `decision: approved_once | approved_session | denied`, `wait_duration_ms` |
| `usage_interruption_detected` | Usage interruption service | built-in engine, catalog-safe model, fixed interruption kind |
| `usage_recovery_finished` | Usage interruption service | `mode: manual | automatic`, outcome, wait duration bucket |
| `thread_retention_changed` | Forge settings acceptance | fixed retention enum only |
| `feature_used` | Authoritative feature service | fixed `feature` enum only: `subagent_graph`, `checkpoint_rollback`, `workspace_review`, `routine`, `capability`, `preview`, `thread_search` |

`feature_used` is acceptable only with a closed enum in code. If a feature needs dimensions or an outcome, promote it to a dedicated typed event rather than adding arbitrary properties.

### P1 events after P0 is trustworthy

- First-run onboarding step completion/drop-off
- Update offered/downloaded/applied/failed
- Marketplace routine/capability install and invoke outcomes, using only fixed categories and built-in identifiers
- Notification permission/result
- Thread search use, without query or result text
- Model/harness favorites, without custom identifiers
- Agent-graph fanout and completion metrics using bounded counts, never child prompts or IDs
- Feature-flag exposure events if PostHog flags are later introduced

### Events explicitly not captured

- Page/route views inside thread, file, diff, terminal, or settings surfaces by default
- Keystrokes, composer changes, prompt length, code size, filenames, branches, repository language, command names, or clipboard events
- Individual streamed observations/tokens
- Every database/transport operation
- Hover, focus, scroll, resize, and generic button clicks

## 7. PostHog Funnels, Retention, and Dashboards

Create these saved dashboards in the production PostHog project:

### Executive/product health

- Weekly active installations
- First successful run rate
- Median time from first `editor_session_started` to first completed `run_finished`
- D1/W1/W4 installation retention after first successful run
- Completed runs per active installation
- Version adoption by release channel

### Activation funnel

```text
editor_session_started
→ forge_connection_finished(outcome=connected)
→ project_added or project_opened
→ thread_created
→ run_started
→ run_finished(outcome=completed)
```

Break down only by release, platform, surface, and built-in engine. Do not break down by unique IDs.

### Run reliability

- Run completion/failure/cancellation rate
- Median and p95 duration
- Stable failure-code distribution
- Completion by app version, OS, built-in engine, and catalog-safe model
- Usage interruption and recovery outcomes

### Feature adoption

- Active installations using model transitions, approvals, subagent graphs, workspace review, routines/capabilities, and previews
- New-feature first use and repeat use
- Engine setup/auth success rate

### Forge operations

- Forge starts and unclean previous exits
- Connection/reconnect success and duration
- Cold-start time by release
- Clean-shutdown ratio

Do not create PostHog alerts until at least two weeks of baseline data exists. Early alerting belongs in Sentry, not on volatile product metrics.

## 8. Sentry Capture Policy

### Editor project

Capture:

- Electron main uncaught exceptions and unhandled rejections
- `StartDesktop` bootstrap failure currently handled at `modules/desktop/src/main.ts:649-658`, after sanitizing the cause
- Native Electron main/renderer crash minidumps through `@sentry/electron/main`
- `render-process-gone` with only Electron's reason, exit code, version, and coarse memory bucket
- Renderer unhandled JavaScript errors/rejections
- Svelte route/root error-boundary failures
- Repeated handoff/recovery failure after the existing bounded retry is exhausted
- Renderer unresponsive episodes only when they exceed a defined threshold; report one issue on recovery or process loss, not every heartbeat stall

Do not capture:

- Expected disconnect/reconnect attempts
- User cancellations, denied approvals, validation errors, authentication-needed states, or offline states
- Console output wholesale
- DOM events, fetch/XHR breadcrumbs, URLs with path/query, or local diagnostic trace files

### Forge project

Capture:

- Forge bootstrap/host acquisition failures
- Unhandled exceptions and rejections
- Database lease/migration/invariant failures
- Observation persistence abandonment
- Unexpected orchestrator invariant/finalization failures
- Unexpected WebSocket/HTTP handler failures after protocol errors have been classified
- Engine startup failures only when Artisan classifies them as an internal defect; provider configuration/auth/usage errors remain product outcomes, not Sentry issues
- Bounded-memory high-water incident at a severe threshold as one warning issue per process, using only numeric memory buckets
- Previous unclean Forge exit detected on next startup

Do not capture raw `Cause.pretty`, `failure.diagnostic`, provider output, command envelopes, database rows, terminal/session output, request bodies, headers, endpoints, or paths.

### Forge hard crashes and OOM

`@sentry/node` cannot reliably transmit after a native hard crash, OS kill, or OOM. Add a local crash marker/heartbeat:

- Write a minimal previous-process record containing release, start time, last heartbeat, and coarse memory buckets.
- Mark it clean during normal finalization.
- On next start, report one sanitized `forge_unclean_exit` event if crash reporting is enabled, then rotate the marker.
- Never attach `forge.log`, renderer JSONL diagnostics, traces, core dumps, or database files automatically.

This provides occurrence/release evidence, not a fabricated stack trace.

### Sentry configuration baseline

For all SDKs:

- `sendDefaultPii: false`
- Session replay disabled
- Sentry structured log capture disabled initially
- Performance tracing disabled in P0 (`tracesSampleRate: 0`)
- Custom `beforeSend` and `beforeBreadcrumb` based on the shared sanitizer
- Remove request bodies, headers, cookies, query strings, user data, server names, environment variables, process arguments, and local paths
- Disable default console, DOM, fetch, and XHR breadcrumbs; add only allowlisted custom lifecycle breadcrumbs
- Capture fatal/unhandled errors at 100%, then use Sentry server-side rate limits for storms
- Keep 30 days of error data initially

After the beta privacy audit, optionally enable 2–5% performance sampling for these fixed span names only:

- `editor.startup`
- `editor.forge_handoff`
- `renderer.ready`
- `forge.startup`
- `forge.command` with a fixed command category
- `forge.database` with a fixed operation category
- `run.prepare`

No raw route, URL, SQL, project, engine command, or file name may become a span name or description.

## 9. Error Sanitization

Create one browser/Node-safe pure sanitization package used by all three runtimes.

Sanitization stages:

1. **Construct from allowlists:** manual events set only known tags/context.
2. **Drop dangerous containers:** remove `request`, `user` PII, cookies, headers, bodies, environment, modules if unnecessary, mechanism data, and arbitrary extras.
3. **Normalize exception values:** known domain errors become class + stable Artisan code; raw provider/user-data errors become a generic message.
4. **Scrub strings:** redact bearer tokens, API-key patterns, URLs/query strings, connection strings, emails if accidentally present, Windows/POSIX home paths, and command-line arguments.
5. **Normalize frames:** retain app-relative module/frame names needed for source maps; remove absolute local paths and `node_modules` noise.
6. **Bound size:** cap breadcrumb count, string length, cause depth, and context object depth.
7. **Canary check:** if a test sentinel such as `ARTISAN_TELEMETRY_FORBIDDEN_CANARY` survives, drop the whole event.

For unknown exceptions, prefer losing the exception message over risking user content; source-mapped stack frames plus error class usually remain actionable.

## 10. Release and Source-Map Strategy

Canonical releases:

- `artisan-editor@<ARTISAN_RELEASE_VERSION>+<commit>`
- `artisan-forge@<ARTISAN_RELEASE_VERSION>+<commit>`

Environments:

- `production`, `staging`, `development`, `test`
- Local development and tests do not initialize production clients.

Required build changes:

- Pass the same release version and commit into frontend, Electron main, and Forge SEA builds.
- Generate hidden source maps for `modules/frontend/vite.config.ts`, `.config/desktop.vite.config.ts`, and `.config/forge.rolldown.config.ts`.
- Inject Sentry debug IDs and upload maps during the official release build with Sentry CLI/build tooling.
- Keep Sentry auth tokens in the release environment only. They must never enter a bundle, manifest, config file, log, or artifact.
- Public DSNs and the PostHog project key may be build configuration, but production code must still be gated by consent.
- Exclude `.map` files from Electron `files`, Forge SEA assets, the distribution artifact, and installed directories after upload.
- Associate releases with the commit when the release environment provides repository access.
- Add a release verification step that resolves synthetic minified Editor and Forge errors to expected TypeScript/Svelte source lines.

The Forge SEA build needs an early spike: confirm that bundled `@sentry/node` initializes in the SEA runtime and that a `forge-main.cjs` stack maps through uploaded debug-ID source maps. If it does not, use a tiny external Sentry bootstrap only if the SEA integrity/security model permits it; otherwise retain manual sanitized event capture and document the mapping limitation rather than weakening packaging.

## 11. Sentry Alerts and Ownership

### Editor alerts

- New production regression: notify `#artisan-reliability` immediately after two affected installations or three events in 10 minutes
- Fatal/native crash spike: five events or three installations in 15 minutes
- Renderer process-gone issue newly introduced in the latest release
- Release health below 99.5% crash-free sessions after a minimum sample size

### Forge alerts

- Any new production database invariant/migration issue
- Unclean-exit/OOM warning on two installations in 30 minutes
- Observation persistence abandonment on any production installation
- New production regression after two affected installations or three events in 10 minutes

Every Sentry project gets:

- A named primary owner and backup
- CODEOWNERS-equivalent issue ownership by frame/module
- A weekly triage SLA
- `ignored`, `expected`, `needs-fix`, and `privacy-review` workflow tags
- A rule that any issue containing forbidden content is immediately restricted, deleted after investigation, and treated as a telemetry privacy incident

## 12. Rollout Phases

### Phase 0 — Governance and vendor configuration

- Approve EU/US region, DPA, retention, consent copy, and privacy-policy wording.
- Create PostHog production/sandbox projects and Sentry Editor/Forge projects.
- Restrict vendor access to the smallest team.
- Record event owner, event purpose, retention, and legal basis in an event catalog.

Exit criterion: project settings and privacy decisions are written down; no SDK is in code yet.

### Phase 1 — Foundation with no vendor egress

- Implement `telemetry.json`, CLI commands, typed preferences API, installation/session identity, no-op telemetry contracts, sanitizer, and Privacy settings UI.
- All SDK adapters remain disabled/no-op.

Exit criterion: tests prove unset/disabled never performs network I/O and preferences work for desktop and headless Forge.

### Phase 2 — PostHog sandbox

- Enable only internal development/beta homes against `Artisan Sandbox`.
- Instrument P0 lifecycle/outcome events.
- Inspect every property of at least 100 synthetic/real internal events.
- Verify deduplication, offline behavior, and immediate opt-out.

Exit criterion: zero forbidden values, no duplicate terminal events, and complete activation/run funnels.

### Phase 3 — Sentry sandbox

- Enable Editor and Forge error SDKs for internal builds.
- Trigger controlled Electron-main, renderer-JS, renderer-native/process-gone, Forge handled, Forge unhandled, and previous-unclean-exit fixtures.
- Verify source mapping, release/environment tags, and redaction canaries.

Exit criterion: at least 95% of synthetic stack frames map to the expected source and no event contains forbidden data.

### Phase 4 — Limited production beta

- Expose first-run consent to beta installations.
- Keep tracing, replay, logs, and PostHog autocapture disabled.
- Review PostHog events and Sentry issues daily for two weeks.
- Tune only server-side sampling/alerts; do not broaden payloads to make dashboards easier.

Exit criterion: no privacy incidents, telemetry overhead within budget, alerts actionable, and event volume/cost forecast acceptable.

### Phase 5 — General availability

- Enable the choices for all production installations.
- Publish privacy docs, deletion/reset instructions, and data processor list.
- Establish monthly event-catalog/cardinality/cost audit and quarterly retention/access audit.
- Add P1 events only through schema review and redaction tests.

## 13. Success Criteria

Privacy and control:

- 0 prompt, response, code, diff, terminal, path, repository, credential, request-body, or arbitrary exception payload fields in a 100-event beta audit.
- 100% of product events decode through a strict schema before enqueue.
- `unset`/`disabled` causes zero vendor network requests.
- PostHog stops immediately on disable; native crash-report restart requirement is accurately shown.
- Installation identity can be reset locally and used to fulfill deletion requests.

Analytics quality:

- Exactly one `run_started` and one `run_finished` per canonical run despite retries/reconnects.
- Activation and run funnels reconcile with local synthetic counts within 1%.
- Development/test events never appear in the production PostHog project.
- Every dashboard labels anonymous installations accurately.

Reliability quality:

- Controlled Editor main, renderer, and Forge failures appear in the correct Sentry project, release, and environment.
- At least 95% of controlled production-build frames source-map correctly.
- Forge unclean-exit reporting works after a forced kill without claiming a nonexistent stack trace.
- Sentry alerts route to an owner and do not page for expected provider/auth/user-cancel errors.

Performance/cost:

- Telemetry adds no synchronous network work to product paths.
- Under offline conditions, no UI/run operation slows or fails due to telemetry.
- Queue memory is bounded; shutdown flush has a short hard deadline.
- Initial monthly event volume and Sentry error volume remain within the approved budget.

## 14. Implementation Tasks

### Task 1: Prove SDK/runtime compatibility before changing architecture

**Objective:** Validate the two risky runtime boundaries: Sentry renderer without preload and Sentry Node inside Forge SEA.

**Files:**
- Create: `.tests/spikes/sentry-electron-no-preload.ts` or an equivalent throwaway fixture outside production modules
- Create: `.tests/spikes/sentry-forge-sea.ts`
- Do not commit spike code unless it becomes a stable deep test

**Steps:**

1. Build a minimal sandboxed Electron window matching `modules/desktop/src/main.ts` and test `@sentry/electron/renderer` without a preload.
2. If it requires IPC/preload, prove `@sentry/browser` captures a renderer exception directly under the intended CSP.
3. Bundle `@sentry/node` with the Forge SEA config and verify startup plus one local mock-envelope capture.
4. Verify a generated Forge stack can be associated with a source map/debug ID.
5. Record the chosen renderer SDK in this plan's implementation PR.

**Gate:** Do not add a preload or general IPC bridge solely for Sentry.

### Task 2: Add home-scoped telemetry preferences in `ae`

**Objective:** Make consent and anonymous identity available before either process initializes telemetry.

**Files:**
- Create: `modules/cli/rust/telemetry.rs`
- Modify: `modules/cli/rust/lib.rs`
- Modify: `modules/cli/rust/instance.rs`
- Modify: `modules/cli/rust/commands.rs`
- Modify: `modules/cli/rust/process.rs`
- Test: Rust unit tests colocated with these modules

**TDD sequence:**

1. Add failing tests for first creation, legacy home migration, atomic write, unsafe destination rejection, status, individual enable/disable, and identity reset.
2. Run `cargo test -p artisan-editor-cli telemetry -- --nocapture`; expect failures.
3. Implement the versioned schema and CLI commands with private atomic writes.
4. Pass `ARTISAN_TELEMETRY_CONFIG_PATH` to Forge and Editor and assert environment propagation in existing process/launch tests.
5. Re-run the focused tests and `cargo clippy -p artisan-editor-cli --all-targets -- -D warnings`.
6. Commit: `feat(cli): add home-scoped telemetry preferences`.

### Task 3: Define typed wire contracts

**Objective:** Prevent the renderer from sending arbitrary analytics objects or changing consent through an untyped endpoint.

**Files:**
- Create: `modules/protocol/src/telemetry.ts`
- Modify: `modules/protocol/src/control-contract/queries.ts`
- Modify: the appropriate control mutation contract file
- Modify: `modules/protocol/src/control-contract/wire.ts`
- Modify: `modules/protocol/src/control-rpc.ts`
- Modify: `modules/protocol/src/index.ts`
- Test: `.tests/protocol/telemetry.test.ts`

**Contracts:**

- `telemetry.preferences.query`
- `telemetry.preferences.update` with only the two tri-state settings
- `telemetry.intent.capture` as a closed discriminated union for renderer-only P0 events
- No `Record<string, unknown>`, arbitrary event name, arbitrary properties, raw URL, or text field

**Verification:**

1. Write decode-rejection tests for extra properties, arbitrary names, prompts, paths, tokens, and oversized values.
2. Run `pnpm run test:focus .tests/protocol/telemetry.test.ts`; expect red, implement, then green.
3. Run `pnpm run validate:transport`.
4. Commit: `feat(protocol): define telemetry consent and intent contracts`.

### Task 4: Create the shared privacy/sanitization package

**Objective:** Give PostHog and all Sentry runtimes one pure, testable privacy boundary.

**Files:**
- Create: `modules/observability/package.json`
- Create: `modules/observability/src/index.ts`
- Create: `modules/observability/src/privacy.ts`
- Create: `modules/observability/src/event-catalog.ts`
- Modify: workspace/module build configuration if required
- Test: `.tests/observability/privacy.test.ts`
- Test: `.tests/observability/event-catalog.test.ts`

**Tests must include:** Windows and POSIX paths, URLs/query strings, bearer/API-key/connection-string patterns, emails, nested causes, circular/oversized objects, provider diagnostics, environment maps, canary values, catalog-safe/custom model IDs, and cardinality rejection.

**Verification:** `pnpm run test:focus .tests/observability` and the module build/check.

**Commit:** `feat(observability): add strict telemetry privacy boundary`.

### Task 5: Add backend telemetry contracts and a no-op implementation

**Objective:** Let domain code emit typed outcomes without importing PostHog or Sentry.

**Files:**
- Create: `modules/backend/src/telemetry/product-telemetry.ts`
- Create: `modules/backend/src/telemetry/noop.ts`
- Modify: `modules/backend/src/runtime/backend-runtime.ts`
- Modify: `modules/backend/src/index.ts`
- Test: `.tests/backend/product-telemetry.test.ts`

**Design:** An Effect service accepts only the closed event catalog. The default layer is no-op. `make_desktop_backend_layer` accepts/provides a runtime adapter so tests and embedded compositions remain deterministic.

**Verification:** prove disabled/no-op cannot throw, block, or perform I/O; run focused backend tests and `pnpm run check`.

**Commit:** `feat(backend): add vendor-neutral product telemetry service`.

### Task 6: Implement Forge consent and PostHog adapter

**Objective:** Make Forge the sole PostHog egress point.

**Files:**
- Create: `modules/forge/src/telemetry/preferences.ts`
- Create: `modules/forge/src/telemetry/posthog.ts`
- Create: `modules/forge/src/telemetry/runtime.ts`
- Modify: `modules/forge/src/config.ts`
- Modify: `modules/forge/src/entry.ts`
- Modify: `modules/forge/src/forge-host.ts`
- Modify: `modules/forge/package.json`
- Test: `.tests/forge/telemetry-preferences.test.ts`
- Test: `.tests/forge/posthog-telemetry.test.ts`

**Behavior:** bounded in-memory queue, short timeout, no disk event spool, `$geoip_disable`, deterministic `$insert_id`, immediate opt-out, consent watcher, short shutdown flush, sandbox/production host selection, and total failure isolation.

Use a local fake HTTP collector in tests. Assert exact outbound JSON and zero requests for unset, disabled, development, invalid, or forbidden events.

**Verification:** `pnpm run test:focus .tests/forge/telemetry-preferences.test.ts .tests/forge/posthog-telemetry.test.ts`, `pnpm run check:forge`, and `pnpm run validate:forge`.

**Commit:** `feat(forge): add consent-gated PostHog analytics adapter`.

### Task 7: Instrument authoritative Forge outcomes

**Objective:** Emit the P0 catalog once at canonical domain transitions.

**Files likely to change:**
- `modules/forge/src/entry.ts`
- `modules/backend/src/orchestration/agent-orchestrator.ts`
- Orchestration repository/continuation files that own canonical run transitions
- Project-directory service
- Engine installation/authentication service
- Tool control plane
- Usage interruption service
- Model transition/continuation service
- Tests: existing lifecycle suites plus `.tests/backend/product-analytics-lifecycle.test.ts`

**Rules:** Instrument after durable acceptance/terminal transition, never on renderer click or transport receipt. Use canonical event IDs for dedupe. Compute duration/outcome in Forge. Normalize provider/configuration failures to stable codes before telemetry.

**TDD cases:** accepted vs duplicate command, reconnect/replay, start failure, completion, cancellation, persistence failure, model transition, approval, usage recovery, unknown custom model, and disabled telemetry.

**Verification:** reconcile a synthetic fixture's local run count to fake PostHog requests exactly; run orchestrator lifecycle/performance tests and backend validation.

**Commit:** `feat(backend): emit canonical product lifecycle analytics`.

### Task 8: Add renderer intents and Privacy settings

**Objective:** Capture only UI/session facts unavailable to Forge and give users control.

**Files:**
- Create: `modules/frontend/src/lib/telemetry/controller.ts`
- Create: `modules/frontend/src/lib/telemetry/session.ts`
- Modify: `modules/frontend/src/lib/runtime/frontend-runtime.ts`
- Modify: `modules/frontend/src/lib/runtime/browser-frontend-runtime.ts`
- Create: `modules/frontend/src/routes/components/settings/privacy.svelte`
- Create: `modules/frontend/src/routes/settings/privacy/+page.svelte`
- Modify: `modules/frontend/src/routes/components/settings/nav.svelte`
- Test: `.tests/frontend/telemetry-controller.test.ts`
- Test: `.tests/frontend/privacy-settings.test.ts`

**Renderer events:** `editor_session_started`, connection outcome, and narrowly approved P0 UI-only feature use. Do not add generic route/page capture.

**UI:** explain each category, show unset/enabled/disabled, show native crash restart status, link privacy docs, expose identity reset, and state exactly what is never collected.

**Verification:** source/behavior tests prove no direct PostHog dependency or endpoint exists in the renderer and no event fires before consent/connection readiness.

**Commit:** `feat(frontend): add telemetry controls and typed UI intents`.

### Task 9: Add Sentry sanitizer and Electron-main crash reporting

**Objective:** Capture main/native Editor failures without leaking content.

**Files:**
- Create: `modules/desktop/src/error-monitoring.ts`
- Modify: `modules/desktop/src/main.ts`
- Modify: `modules/desktop/package.json`
- Test: `.tests/desktop/error-monitoring.test.ts`
- Test: extend `.tests/desktop/renderer-death-recovery.test.ts`

Initialize `@sentry/electron/main` before ordinary application work only when crash consent is enabled. Add sanitized manual capture at bootstrap failure, fatal recovery failure, severe unresponsive episode, and `render-process-gone`. Preserve the existing local diagnostics behavior and never upload its files.

**Verification:** use a local transport/fake DSN; inspect envelopes, consent-off behavior, and sanitizer output.

**Commit:** `feat(desktop): add consent-gated Sentry crash reporting`.

### Task 10: Add renderer JavaScript error reporting

**Objective:** Capture renderer exceptions while preserving no-preload/no-general-IPC architecture.

**Files:**
- Create: `modules/frontend/src/lib/error-monitoring/runtime.ts`
- Create: `modules/frontend/src/lib/error-monitoring/sentry.ts`
- Modify: renderer runtime composition and root error boundary
- Modify: `modules/frontend/package.json`
- Modify: CSP only if the direct browser transport requires the exact Sentry ingest origin
- Test: `.tests/frontend/error-monitoring.test.ts`
- Test: `.tests/desktop/desktop-config.test.ts`

Initialize only after consent is known. If the compatible Electron renderer SDK from Task 1 works without preload, use it; otherwise use `@sentry/browser`. Allow only the exact HTTPS ingest origin in `connect-src`, not wildcards.

**Verification:** controlled rejection and render error reach the fake collector; DOM/fetch/console breadcrumbs, URL paths, prompts, and canaries do not.

**Commit:** `feat(frontend): add privacy-gated renderer error monitoring`.

### Task 11: Add Forge Sentry and unclean-exit detection

**Objective:** Capture actionable Forge failures, including evidence of hard exits on the next launch.

**Files:**
- Create: `modules/forge/src/telemetry/sentry.ts`
- Create: `modules/forge/src/telemetry/crash-marker.ts`
- Modify: `modules/forge/src/host-entry.ts` and/or earliest executable bootstrap
- Modify: `modules/forge/src/entry.ts`
- Modify: `modules/forge/src/memory-telemetry.ts`
- Modify: `modules/forge/package.json`
- Test: `.tests/forge/sentry.test.ts`
- Test: `.tests/forge/crash-marker.test.ts`
- Test: extend `.tests/forge/memory-telemetry.test.ts`

Initialize `@sentry/node` before Forge application imports when consent is enabled. Capture only classified internal failures. Implement heartbeat/clean-marker rotation with coarse data.

**Verification:** force-kill a fixture, restart it, and assert exactly one sanitized unclean-exit event; verify normal shutdown reports none.

**Commit:** `feat(forge): add Sentry errors and unclean-exit evidence`.

### Task 12: Wire releases and private source maps

**Objective:** Make production issues source-mapped and attributable to the exact Editor/Forge release.

**Files:**
- Modify: `modules/frontend/vite.config.ts`
- Modify: `.config/desktop.vite.config.ts`
- Modify: `.config/forge.rolldown.config.ts`
- Modify: `.scripts/build/build-forge-sea.ts`
- Modify: `.scripts/build/runner.ts`
- Modify: `.config/desktop-builder.yml` if explicit map exclusion is needed
- Test: `.tests/build/desktop-packaging-metadata.test.ts`
- Test: `.tests/build/forge-sea-assets.test.ts`
- Create: `.tests/build/observability-release.test.ts`

**Verification:** official dry-run build emits maps outside install artifacts, injects/uploads debug IDs using a mock or sandbox Sentry org, and resolves synthetic frames. Assert no Sentry auth token or `.map` enters release artifacts.

Run `pnpm run build`, `pnpm run verify:desktop-package`, and `pnpm run verify:distribution:windows` in the release candidate environment.

**Commit:** `build: publish private Sentry source maps by release`.

### Task 13: Create dashboards, alerts, documentation, and deletion runbook

**Objective:** Turn telemetry into an operated system rather than unused SDK traffic.

**Artifacts:**
- Create an in-repo event catalog/privacy document in the project's established docs location
- Create a telemetry incident/deletion runbook
- Configure PostHog dashboards and Sentry alerts listed above
- Document owners, retention, region, costs, and monthly review

Export dashboard definitions through vendor APIs/config where supported, or record links and screenshots in the runbook. Do not store vendor personal API keys in the repository.

**Commit:** `docs: add Artisan telemetry catalog and operations runbook`.

### Task 14: Execute staged validation and production rollout

**Objective:** Prove data quality/privacy before general availability.

**Commands:**

- `cargo test --workspace`
- `pnpm run test:protocol` if introduced; otherwise focused protocol suite
- `pnpm run test:frontend`
- `pnpm run test:backend`
- `pnpm run test:forge`
- `pnpm run test:desktop`
- `pnpm run validate:full`
- Packaged desktop/deep distribution verification

Follow Phases 2–5 and record each exit criterion. Do not enable tracing, replay, autocapture, or P1 events during this task.

**Commit:** `chore: enable privacy-reviewed production observability`.

## 15. Risks and Trade-offs

- **No preload:** This is a valuable security boundary. Direct renderer Sentry may require one exact CSP egress. The alternative is losing some pre-connection JS errors; adding a general preload is not justified.
- **Forge as analytics egress:** Product events are lost when Forge is unavailable. This is acceptable because analytics must not become a durable product dependency. Sentry still captures direct runtime failures.
- **Installation-level identity:** Retention measures installations, not people. This is honest and adequate until accounts exist.
- **Explicit consent lowers sample size:** Prefer lower volume over covert collection in a code/prompt product.
- **Source-map uploads disclose Artisan source to Sentry:** Restrict access and accept only if the vendor/DPA posture permits it. Source maps contain Artisan's code, never user repositories, and are not shipped to users.
- **Forge SEA:** SDK bundling and source-map behavior must be proven, not assumed.
- **Unknown exception messages:** Sanitizing aggressively can reduce diagnostics. Prefer stable error codes and source-mapped frames; never compensate by uploading arbitrary causes/logs.
- **Two Sentry projects:** This adds one reliability boundary but prevents unrelated renderer/backend issues from grouping together. Cross-project dashboards retain one operational view.

## 16. Open Questions Requiring Product/Legal Approval

1. Confirm EU data region for PostHog and Sentry.
2. Approve explicit opt-in copy and whether the two choices appear on first run or in a non-blocking onboarding card.
3. Confirm 90-day PostHog raw-event retention and 30-day Sentry retention, or choose shorter supported values.
4. Identify telemetry owner, reliability owner, privacy incident contact, and vendor-budget owner.
5. Decide whether exact numeric token counts are useful enough for P0; if not, omit them rather than bucket them without a use case.
6. Confirm that Artisan source-map upload to Sentry is acceptable under the repository's source-access policy.
7. The marketing site is not present in this repository. When instrumented, it should use the same PostHog production project with `surface=marketing`, cookieless page analytics, and its own strict event list; it does not change this Editor/Forge architecture.

## 17. References

1. PostHog Node SDK: https://posthog.com/docs/libraries/node
2. PostHog Electron guidance: https://posthog.com/docs/libraries/electron
3. PostHog web analytics: https://posthog.com/docs/web-analytics
4. Sentry Electron SDK: https://docs.sentry.io/platforms/javascript/guides/electron/
5. Official Sentry Electron package features and initialization: https://github.com/getsentry/sentry-electron
6. Sentry Node SDK: https://docs.sentry.io/platforms/javascript/guides/node/
7. Sentry event filtering and `beforeSend`: https://docs.sentry.io/platforms/javascript/configuration/filtering/
8. Sentry JavaScript source maps: https://docs.sentry.io/platforms/javascript/sourcemaps/
