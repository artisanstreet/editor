# Visible-stream interface (backend → wire → composer)

Live tv contract for sends: a new send must become visible incrementally,
not only at response completion. This doc records the exact backend wire
ordering the composer may rely on, and the backend invariants that keep it
true. Read with `modules/backend/src/connection.rs` (`drive_until_end`),
`modules/backend/src/conversation_delivery_driver.rs`,
`modules/backend/src/conversation_delivery_writer.rs`, and
`modules/backend/src/native_run_dispatch.rs` (`launch_claim`,
per-observation batch commits).

## Frame order on one subscribed connection

1. `Response` to the send command (`MessageQueued` receipt) on the
   request's bidirectional stream. Admission only; nothing is projected
   yet.
2. `PatchBatch` with the user turn/item (`ItemUpsert` user message) on the
   server delivery stream. This is published from the dispatch launch
   (`launch_claim`, `Started` receipt), NOT from provider startup: the
   launch durably projects the user turn/item and publishes the commit
   wake synchronously before any provider process is admitted.
3. `PatchBatch` frames with the assistant start (`ItemUpsert` assistant message)
   on the first provider delta.
4. Further `PatchBatch` frames carrying contiguous patch ranges
   (`ItemAppend` fragments) as the turn streams. Every committed delta
   publishes its own wake and each wake re-reads to the durable tail,
   but commits before one wake coalesce into a single replay batch, so
   receivers must read batch contents, not count frames. A burst larger
   than the observation channel is drained inline while the steer ack
   is outstanding (the dispatch steer arm polls the provider future
   exactly while draining `next_observation`).
5. `Event(EngineObservation)` envelopes for durably committed engine
   observations (activity/tool rows via `Replace` checkpoints into the
   observation ledger), interleaved on the same delivery stream in commit
   order, each carrying its thread-scoped `delivery_sequence`.
6. Terminal `TurnLifecycle` patch (`Cancelled`/`Completed`/`Failed`) only
   after the run actually settles. No terminal is emitted while the
   provider turn is held.

Cursors are authoritative per subscriber: patch batches advance the
patch cursor contiguously, observation events advance the
`delivery_sequence` cursor. A reconnect replay redelivers from the stored
cursors; nothing is ever re-minted.

## Backend invariants (do not regress)

- Every durable commit path publishes its thread wake on success:
  launch (`Started`), every observation batch, steered projection,
  terminal settlement. A commit without a publish withholds already
  durable state from every live subscriber until the next unrelated
  commit.
- Wakes are hints, never payloads: each wake performs a bounded
  authoritative re-read to the durable tail. Draining loops until
  `Current`, so coalesced wakes lose no events.
- The single shared `ConversationCommitNotifier` instance must back both
  the dispatch config and the request handler. Two independent notifier
  instances silently disconnect commits from delivery: each component
  looks correct in isolation while nothing ever streams.
- The command reply path (per-request bidirectional stream) and the
  observation pipeline (server-initiated uni stream) share only QUIC
  connection flow control. While the driver is idle it selects over
  accept-or-wake with no deadline; once a request is accepted, its
  dispatch and the following delivery scan run serially to completion,
  and each delivery write is bounded. A slow peer can therefore hold
  the connection inside a delivery scan until the bounded write
  resolves.
- One `PatchBatch` per committed delta is not guaranteed: several
  commits before one wake coalesce into a single replay batch read to
  the durable tail. Receivers must read batch contents, not count
  frames.
- `launch_claim` publishes only on `Started`. `AlreadyStarted` is durable
  replay owned by the recovery path; publishing there would wake
  subscribers for state another attempt owns.
