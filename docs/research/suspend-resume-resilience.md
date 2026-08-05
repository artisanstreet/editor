# Surviving Suspend, Hibernate, and Resume

Last updated: 2026-08-01

Status: researched, not implemented. Companion to
[system-wake-lock.md](system-wake-lock.md), which covers the sleeps we can
prevent. This document covers the ones we cannot.

## Problem

Some suspends are not preventable: user-initiated sleep (lid close, power
button, Start menu), Modern Standby on DC power revoking the power request
five minutes after the sleep timeout, hosts where the lock cannot be acquired,
and explicit hibernation. When the host comes back, Artisan must come back
honest.

The observed contrast is instructive. The Codex app resumes its thread on
wake. T3 Code presents a run as still working when its stream is dead, and
needs a manual retrigger. That difference is not luck — it is whether the
application treats a process that survived a suspend as trustworthy.

## What actually survives a suspend

| Thing                                       | Survives    | Notes                                                                                      |
| ------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------ |
| Processes, including across hibernate       | Yes         | Hibernate restores RAM verbatim. PIDs, handles, and memory come back exactly as they were. |
| Local stdio pipes to the engine child       | Yes         | No network is involved; these are unaffected.                                              |
| The engine CLI's TLS stream to the provider | **No**      | Dropped by the peer, load balancer, or NAT during the outage — **silently**.               |
| Forge ↔ frontend WebSocket                  | Usually not | Self-heals: the transport already owns heartbeat, reconnect, and stale-frame fencing.      |
| Node timers                                 | **Paused**  | libuv uses `CLOCK_MONOTONIC`, which excludes suspended time.                               |
| `Date.now()`                                | Jumps       | Wall clock advances by the full suspend duration.                                          |

The silent socket death is the crux. A half-open TCP connection delivers no
error: reads never return, writes appear to succeed until retransmission
finally gives up. Nothing tells the application anything.

Compose those rows and you get the exact reported symptom: **process alive +
pipe open + no bytes ever again + deadline not counting = permanently
"running"**. There is no bug to find in such a system; the state is simply
unrepresentable.

Node's own issue tracker has this as
[nodejs/node#38108](https://github.com/nodejs/node/issues/38108) — "hibernate
leads to hanging process due to endless wait for write, setTimeout never
called again" — reported on macOS, Windows, and Linux.

## Where Artisan is exposed today

- **`modules/engines/src/claude/engine.ts:735`.** `watch_timeout` races the
  event buffer closing against `Effect.sleep(options.timeout_ms)`, defaulting
  to 10 minutes (line 160). Two problems. It is a **total-run deadline, not an
  inactivity deadline**, so it cannot distinguish a healthy long run from a
  dead stream. And because Effect's clock is monotonic, it **pauses during
  suspend**: a run suspended three minutes in resumes with seven minutes still
  on the budget, against a stream that will never produce another byte. When
  it eventually fires it reports "Claude timed out after 600000ms", which is
  not what happened.
- **`modules/engines/src/codex/app-server-session.ts:198`.** A 10-second
  JSON-RPC `request_timeout_ms` has the same class of problem for requests
  in flight across a suspend.
- **Nothing records last-byte-received per run**, so "stalled" is not an
  expressible state anywhere in the model.

The good news is that the first item is worth fixing on its own merits, with
no power-management work attached.

## Design

### 1. Detect suspend and resume

**Cross-platform floor, no native code: a wall-clock gap heartbeat.** Tick a
timer in Forge every few seconds and compare the `Date.now()` delta against
the expected interval. A delta far beyond tolerance means the host was
suspended, and `delta - interval` is how long it was gone. This works on every
platform and every host, needs no FFI, and directly measures the quantity the
rest of the design depends on. Its one weakness is that it relies on the timer
firing at all after resume, which the Node issue above shows is not
guaranteed — so it wants a platform signal alongside it eventually.

Platform-native signals, which arrive earlier and can also give _pre_-suspend
notice:

| Platform | Mechanism                                                | Notes                                                                                                     |
| -------- | -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Windows  | `PowerRegisterSuspendResumeNotification` (`powerbase.h`) | `DEVICE_NOTIFY_CALLBACK` delivers `PBT_APMSUSPEND`, `PBT_APMRESUMESUSPEND`, `PBT_APMRESUMEAUTOMATIC`.     |
| Linux    | logind `PrepareForSleep(bool)` signal                    | `true` before suspend, `false` after resume. Pairs directly with the inhibitor lock in the wake-lock doc. |
| macOS    | `IORegisterForSystemPower`                               | Callback receives `kIOMessageSystemWillSleep` and `kIOMessageSystemHasPoweredOn`.                         |

Three caveats worth recording:

- `PBT_APMRESUMEAUTOMATIC` is the one that matters for unattended runs — it
  fires on resume even when no user is present. `PBT_APMRESUMESUSPEND` is not
  delivered if the resume was not user-initiated.
- systemd has a known bug ([systemd#30666](https://github.com/systemd/systemd/issues/30666))
  where `PrepareForSleep(false)` is **not emitted after resuming from
  hibernation**. Keep the gap heartbeat as a backstop even on Linux.
- The Windows path needs a native callback into Node from a foreign thread,
  which is the genuinely risky part of koffi usage. Worth a spike before
  committing.

Electron's `powerMonitor` can forward hints when the desktop app happens to be
running, but as with the wake lock it must never be the only source — the
window may be closed while Forge works.

**Recommendation: ship the gap heartbeat first.** It covers every host, costs
almost nothing, and unblocks everything below. Add native signals only when
pre-suspend notice earns its complexity.

### 2. Make deadlines activity-based and suspend-aware

Two changes in the engine layer, both valuable independently:

- **Add an inactivity deadline**, reset on every emitted observation. A
  healthy multi-hour run never trips it; a dead stream trips it in seconds.
  This is the single highest-value change in this document and needs no power
  integration at all.
- **Credit suspended time back.** On resume, extend every live deadline by the
  measured suspend duration so a three-hour hibernate cannot become a spurious
  timeout. Implementable as a suspend-aware Clock layer, or as a `Ref` of
  accumulated suspend time that the deadline fibers consult.

### 3. On resume, distrust and probe

Move every active run to a `suspect` sub-state on resume, then resolve it
quickly rather than waiting:

1. Check the child process is actually alive.
2. Open a short probe window — 15 to 30 seconds is enough.
3. Any observation arrives → back to `running`, silently. No user-visible
   drama for the common case where the stream survived.
4. Process dead → `interrupted`.
5. Process alive but silent past the window → terminate the orphan and mark
   `interrupted`. The Job Object machinery in
   `modules/engines/src/process/windows-job.ts` already gives clean process-tree
   kill on Windows.

The rule that matters: **never let the UI assert `running` for a run in
`suspect` past the probe window.** The entire complaint about T3 Code is that
it reports a state it has no evidence for.

### 4. Resume the thread, the way Codex does

Artisan already owns every piece needed for the good behavior:

- `AgentRuns.native_resume_json` persists the provider resume token
  (`modules/backend/src/orchestration/internal/run-lifecycle.ts:152`).
- `run.attempt`, `max_attempts`, and `queue_retry` already model retry as a
  first-class transition
  (`modules/backend/src/orchestration/internal/run-transitions.ts:35`).
- The portable handoff path
  ([portable-engine-handoff.md](portable-engine-handoff.md)) covers the case
  where a native resume is unavailable or incompatible.
- The event journal in SQLite is already the source of truth for the
  transcript, so nothing said before the suspend is at risk.

So resume-after-suspend is **not new machinery — it is a new trigger for
existing machinery**. A run interrupted by a detected suspend re-dispatches
through the ordinary retry path carrying its resume token.

Policy question: auto-resume or prompt? Recommendation is auto-resume for runs
interrupted by a _detected suspend_ specifically, bounded by `max_attempts`,
because queueing a multi-hour program is an explicit statement of "keep
going". Runs interrupted for any other reason keep today's behavior. Log the
distinction so the choice stays auditable.

### 5. Checkpoint before suspend where a signal exists

Where a pre-suspend signal is available — a logind `delay` inhibitor, or
`PBT_APMSUSPEND` — use the window to WAL-checkpoint the journal and stamp
active runs as suspended at a known time. That turns post-resume
reconciliation from an inference into a lookup, and it means a hibernate that
never resumes cleanly still leaves a consistent database. Without the signal,
ordinary journal durability already covers correctness; this is an
optimization and a diagnostics win, not a prerequisite.

## Why Codex behaves well and T3 Code does not

Worth stating plainly, because it is the design principle rather than an
anecdote. Codex keeps authoritative conversation state server-side and
re-fetches on reconnect, so a dead socket costs a round trip rather than a
session. A client that holds authoritative streaming state locally, with no
stall detection and a monotonic deadline that pauses during suspend, has no
mechanism by which it _could_ notice — it is holding a valid-looking file
descriptor and a timer that is not counting.

Artisan is architecturally on the Codex side already: the journal is durable,
resume tokens are persisted, retries are modelled. It simply does not yet act
on any of it when the host disappears.

## Verification

- Windows: `powercfg /sleepstudy` for the sleep record, `shutdown /h` to force
  hibernate, Event Viewer Kernel-Power IDs 42 (entering sleep) and 107
  (resume) to correlate against Artisan's own logs.
- The test that matters: start a long run, hibernate for longer than the
  engine timeout, resume, and confirm that within ~30 seconds the run is
  either producing output again or explicitly marked interrupted and
  re-dispatched. Never a stuck spinner.
- Regression guards: the inactivity deadline resets on every observation, and
  a synthetic suspend of N milliseconds extends live deadlines by N.

## Open questions

1. Auto-resume versus prompt for suspend-interrupted runs.
2. Is a koffi callback from a foreign thread safe enough for
   `PowerRegisterSuspendResumeNotification`, or is the gap heartbeat
   sufficient indefinitely?
3. Should `node-pty` terminal sessions be reconciled on resume as well, or
   only agent runs?

## Sources

- [nodejs/node#38108 — hibernate leads to hanging process, setTimeout never called again](https://github.com/nodejs/node/issues/38108)
- [Timers — Node.js documentation](https://nodejs.org/api/timers.html)
- [PowerRegisterSuspendResumeNotification (powerbase.h) — Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/powerbase/nf-powerbase-powerregistersuspendresumenotification)
- [PBT_APMRESUMEAUTOMATIC — Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/power/pbt-apmresumeautomatic)
- [systemd#30666 — no PrepareForSleep(false) when resuming from hibernation](https://github.com/systemd/systemd/issues/30666)
- [org.freedesktop.login1(5) — Linux manual page](https://man7.org/linux/man-pages/man5/org.freedesktop.login1.5.html)
- [Programmatically Capture Energy Saver Events on Mac (IORegisterForSystemPower)](https://ladydebug.com/blog/2020/05/21/programmatically-capture-energy-saver-event-on-mac/)
