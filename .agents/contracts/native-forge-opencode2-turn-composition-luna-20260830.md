# Implementation contract: native Forge OpenCode2 turn composition

You are one external GPT-5.6 Luna Max implementation worker. Work only in the
isolated worktree/branch supplied by the launcher. Do not spawn descendants,
publish, merge, modify controller state, or run broad/native builds.

## Exact base and shipping outcome

Base must be exact clean commit
`0adfc618c1c6d8b51977f9148dd4ca0d41ade756`.

Implement packet `NATIVE-FORGE-OPENCODE2-TURN-COMPOSITION-v1`: the normal
production `forge_runtime::run` path must own one bounded dispatcher that takes
durably queued message dispatches through the existing launch/bind/batch/
terminal repository fences, a certified installed OpenCode2 process, real
CreateSession/Prompt/SSE handling, post-commit conversation notification, and
child cleanup/reap. This is composition of accepted capabilities, not another
policy-only leaf.

The separately owned conversation-state/Statig/controller/host lane is live.
Every file it touches is strict no-touch even if Git/worktree/process audits
show concurrent changes. The completed C2 CLI payload-admission packet is also
separate; do not edit `modules/cli/rust/{commands,payload,manifest}.rs`.

## Worker-owned paths

Only these production paths:

```text
modules/backend/src/native_run_dispatch.rs             (new, private)
modules/backend/src/forge_runtime.rs
modules/backend/src/engine_owner/mod.rs
modules/backend/src/engine_owner/operation.rs
modules/backend/src/engine_owner/process.rs
modules/backend/src/engine_owner/http.rs
modules/backend/src/engine_owner/stream.rs
modules/backend/src/engine_owner/event.rs
modules/backend/src/engine_owner/observation.rs
modules/backend/src/engine_owner/framing.rs
modules/database/src/repository/dispatch_payload.rs
modules/cli/rust/engine_catalog.rs
```

Only these focused test paths, plus new
`tests/backend/native_run_dispatch.rs`:

```text
tests/backend/engine_owner_configuration.rs
tests/backend/engine_owner_fixture.rs
tests/backend/engine_owner_fixture_smoke.rs
tests/backend/engine_owner_streaming.rs
tests/backend/forge_runtime.rs
tests/database/message_dispatch_payload.rs
tests/cli/native_opencode2_authority.rs
```

All other paths are strict no-touch. In particular do not edit any `lib.rs`,
Cargo/BUILD/lock file, protocol/schema/migration/transport file, installer or
packaging file, frontend/UI file, `proof.rs`, `conversation_host.rs`, any
`conversation_*`/Statig/controller source or test, docs, `.agents`, or
`modules/engines/**`. Report needed VP-owned registrations/dependency edges in
the receipt; do not make them.

## Frozen authority and ownership

- Add the smallest opaque public CLI bridge in `engine_catalog.rs` that resolves
  the managed installed OpenCode2 authority from the database path and returns
  a non-clonable, non-serializable, redacted `VerifiedOpenCode2Launch` token.
- Reuse the existing catalog/managed-state authority. Require engine ID
  `opencode2`, version `0.0.0-beta-17778`, the existing pinned revision/hash,
  active generation, and fresh executable identity revalidation immediately
  before spawn. No PATH/runfiles/source/arbitrary-path/ambient fallback.
- Production process recipe is fixed/private: `serve --stdio --port 0`, cwd is
  the attached project root, environment cleared, exact managed OpenCode home/
  config/XDG variables, fresh local `OPENCODE_PASSWORD`, and empty
  `OPENCODE_SERVER_PASSWORD`.
- Keep child/stdin/stdout/stderr/password/endpoint/session channel/cancellation/
  reap custody solely inside `EngineOwner`. The existing raw-path config must
  become crate-private/test-only; production accepts only the verified token.
- Use private state-bearing enums/types, private fields, and ownership transfer.
  Add no public boolean state bag or TypeScript-counterpart presentation model.

## Production ordering and fences

`forge_runtime::run` must resolve/revalidate authority, start the owner and
existing startup reconciliation, start one dispatcher, and publish readiness
only after those owners are live. Shutdown stops claims, cancels/awaits the
active attempt and child reap, drains the listener, removes readiness, stops
the directory controller/app, and finally releases custody.

For one dispatch attempt preserve this order:

```text
claim queued/expired dispatch
-> read payload plus attached project RootPath
-> mint existing IDs/origins/credentials/patch IDs
-> launch_claimed_run
-> spawn/readiness/health/CreateSession certified engine
-> bind_run_provider
-> authorize exactly one Prompt only after BindRunProvider::Bound
-> consume bounded OpenCode2 SSE events
-> commit each bounded batch and publish notifier only after durable success
-> paired complete/fail terminal settlement
-> close/reap child before admitting another attempt
```

Replay rules are strict: `AlreadyStarted` never spawns; `AlreadyBound` never
prompts; `AlreadyCommitted` never replays provider input; already-terminal
outcomes perform no external action. Every DB retry reuses the identical
original receipts, sequence/digest/patch IDs, and credentials.

Pre-bind failures close/reap without `fail_run` and leave durable state for
expiry reconciliation. Post-bind loss without a known terminal is an unknown
outcome and must never retry Prompt. Only known provider success completes;
known failed/cancelled/interrupted terminals use the existing bounded failure
API. Restart reconciliation contacts no provider and never re-prompts.

## OpenCode2 adapter and bounds

Decode the certified envelopes for `session.text.delta`,
`session.execution.succeeded`, `.failed`, and `.interrupted`; validate expected
session ID, bounded durable sequence/envelope ID/delta, ignore unrelated events
without retaining payload, and treat EOF/process exit/timeout/missing terminal
as failure/unknown—not success.

Retain existing domain bounds and use these assembly ceilings:

```text
readiness 15s; health 10s; prompt 30s; SSE 30m; close/reap 5s
JSON body 8 MiB; SSE line 64 KiB; SSE event 8 MiB; readiness line 64 KiB
headers 64; buffer 8192 bytes; stderr 64 KiB; observation sink 8; control queue 1
```

No credential, prompt text, raw SSE, executable/project path, auth header, or
secret may appear in Debug/Display/logs/persisted errors. Replace unsafe Debug
derives on touched observation/event types when needed.

## Meaningful focused tests

Cover authority resolution/revalidation/redaction/no fallback; attached project
root lookup; exact args/cwd/cleared environment; real OpenCode2 CreateSession/
Prompt/SSE envelope and URL/session validation; fresh run-to-terminal flow;
post-commit notifier ordering; `AlreadyStarted`/`AlreadyBound`/
`AlreadyCommitted`; duplicate-prompt prevention; bounds/backpressure; pre-bind,
post-bind-unknown, cancellation, missing-terminal and reap behavior; restart
reconciliation; and absence of secrets/prompts/SSE/paths from diagnostics.
Avoid source-string-search tests.

## Worker checks and completion

Use only source checks: rustfmt on every owned Rust file, `git diff --check`,
exact scope audit, and clean status. Do not run Cargo, Bazel, rustc, Clippy,
native apps, network, or installed engines; the VP owns registration and the
sole jobs=1 native gate.

Review the entire diff and commit exact final bytes with:

```text
feat(backend): execute native OpenCode2 turns
```

Return exact model/session/PID lineage, base, immutable commit, changed paths,
checks, required VP registration/dependency edits, replay/redaction audit,
clean status, and remaining uncertainty. Do not claim gates not run.
