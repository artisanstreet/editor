# Reference steer-burst fixture (`codex-wire-steer_burst`, `codex-wire-steer_reject`)

Fixture-only packet. Sole source: `tests/backend/codex_wire_fixture.rs`
(basename-selected scenarios, no environment plumbing). The backend worker
exclusively owns the `native_run_dispatch` tests and `owner`/`interaction`
source that drive these scenarios; this doc records the wire contract the
fixture guarantees so both sides agree without further coordination.

## Scenario selection (unchanged mechanism)

The parent copies the built fixture to `codex-wire-<scenario>` per test
(+ platform suffix). No environment variables, no new binaries, no sleeps
anywhere: every reply is emitted synchronously while handling the
requesting stdin line.

## `turn/start` (both scenarios)

Validates `threadId` exactly like `strict` (missing id → `-32600`, no
inference), records params to `turn-start-params.json`, answers the actual
turn id (`turn-1`), emits one initial valid `item/agentMessage/delta`, and
stays live — **no terminal event**. The turn closes only on
`turn/interrupt` (`steer_burst`) or EOF.

## `turn/steer` (both scenarios)

Validated against the production verb (`engine_owner/codex.rs`
`steer_live_turn`): `threadId == "thread-fixture-1"`,
`expectedTurnId == "turn-1"`, `input` a non-empty array whose first item
carries non-empty text. **Every** received request — valid or not — is
appended as one JSON line (`id` + full `params`) to
`steer-requests.jsonl` in the project cwd and flushed before any reply,
so a test counting lines observes exactly one provider write per steer
(no retry may ever add a second).

- `steer_burst` + valid: emits 64 valid `item/agentMessage/delta` frames
  (same thread/turn/item `item-steer-1`, cumulative distinct content
  `burst-00 …burst-63`) **before** the correlated success result
  (`{"id","result":{"ok":true}}`), then stays live.
- `steer_reject` (any steer) and invalid steers in either scenario:
  correlated JSON-RPC error (`-32600` for bad target/shape, `-32001`
  for a valid steer the provider rejects), no terminal, no new run,
  never a success.

## `turn/interrupt` (`steer_burst` only)

Answers the interrupt ack (`{"id","result":{"ok":true}}`) plus the
cancelled terminal (`turn/completed`, `status: "interrupted"` → provider
`Cancelled`). All other scenarios keep their historical no-reply behavior.

## Backend-test contract (backend worker owns the test)

- Configure `observation_capacity=4`: 64 burst frames ≫ capacity, so
  reading the correlated steer ack causally requires draining the burst
  through the existing `handle_observation` path — the §7 split-borrow
  shape, proven against a live turn rather than a channel mock.
- Assert `steer-requests.jsonl` holds exactly 1 line (single write,
  original request id + verbatim params).
- Assert every observation commits in order and the steer resolves per
  scenario (`Steered` on the burst ack; typed refusal on the reject
  error with payload preserved).
- Assert no deadline failure while the 64 are drained with the ack
  outstanding.

## Untouched

All pre-existing scenarios (`strict`, `reject_always`, `interleave`,
`resume_interleave`, `resume_mismatch`) emit byte-identical traffic:
`turn/steer` and `turn/interrupt` outside the steer scenarios still
receive no reply, and accepted non-steer turns still complete inline.
