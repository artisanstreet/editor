# Disk I/O Safety

Artisan Forge treats sustained disk I/O as a correctness and hardware-safety
failure, not merely a performance problem. This policy was introduced after
reviewing the Codex SQLite feedback-log incident documented in
[`openai/codex#28224`](https://github.com/openai/codex/issues/28224) and the
related whole-file cache and subagent-session amplification reports.

## Failure Model

Small retained files do not prove low write volume. SQLite WAL reuse,
insert-and-prune loops, checkpoints, atomic whole-file replacement, duplicated
payloads, and repeated session fan-out can write orders of magnitude more data
than remains visible on disk. Read amplification can similarly hide behind the
filesystem cache while still burdening endpoint-security filters and storage.

The dangerous patterns are:

- persisting diagnostic or protocol events at token or frame frequency;
- storing more than one full representation of the same payload;
- replaying an unbounded history in one query or transport envelope;
- spawning an unbounded number of Engine processes that share one state store;
- polling by rereading an entire growing file;
- retaining append-only projections after their owning thread is erased;
- allowing logs, journals, WAL files, or temporary staging to grow without a
  documented bound.

## Enforced Safeguards

The production composition enforces these invariants:

- agent-group assignment count and concurrency have protocol and backend hard
  limits;
- Forge-spawned Codex processes use a Forge-owned SQLite directory and inherit a
  warning-level runtime log filter instead of contending with the user's normal
  Codex SQLite store;
- one Engine observation retains at most one full raw representation;
- conversation patch replay is page-bounded rather than loading all historical
  patches into one message;
- thread erasure removes the complete conversation projection, including
  sources and patches;
- SQLite uses memory-backed temporary storage, a bounded retained journal, and
  an explicit WAL checkpoint policy;
- detached Forge logs are truncated at startup and checked against the 4 MiB
  threshold once per second while running;
- `ae logs --follow` reads only bounded appended ranges and recognizes rotation
  or truncation.

These safeguards are regression-tested. A future implementation must not weaken
one of them merely because ordinary database or log file sizes appear small.

## Upstream Codex Boundary

Forge controls how many Codex processes it starts and where their SQLite state
is stored, but it does not control the internals of the installed Codex binary.
`RUST_LOG=warn` has historically not filtered Codex's separate persistent
SQLite feedback sink. Isolation therefore prevents cross-application
contention and makes Forge-owned state observable, but it is not proof that an
arbitrary Codex version performs no unnecessary writes. The active Forge log
check is likewise a last-resort growth guard, not a lossless rotating sink or a
byte-exact ceiling between checks.

Release qualification must exercise the exact bundled or selected Codex binary
and measure cumulative bytes, not only final file sizes:

1. Record process I/O counters and SQLite/WAL sizes.
2. Measure at idle, during a streaming response, during large tool output, and
   with maximum allowed agent concurrency.
3. Verify Forge returns to negligible writes after work settles.
4. Fail qualification if sustained unexpected writes continue, a configured
   bound is exceeded, or cleanup leaves an active writer behind.

No release status may describe disk I/O as bounded solely from code inspection;
the packaged-process measurement is the final gate.
