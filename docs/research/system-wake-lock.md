# Preventing System Sleep While Work Is In Flight

Last updated: 2026-08-01

Status: researched, not implemented. This document proposes the architecture
and records the platform findings behind it.

## Problem

A queued multi-hour work program dies when the host sleeps on its idle timer.
The agent CLIs Artisan orchestrates (Claude Code, Codex) take no power
assertion of their own, so a Windows host with a 15-minute sleep timeout
suspends mid-run and the queue never drains. Every desktop OS exposes a
sanctioned "I am doing work, do not idle-sleep" mechanism; Artisan takes none
of them today.

## Decision

Artisan holds a system wake lock for exactly as long as it owns unsettled
work, and releases it the moment the last work item settles.

- The lock lives in the **Forge backend process**, not the Electron shell.
- It is **derived, never commanded**: a single predicate over live run and
  queue state drives acquisition and release. No caller ever asks for a lock.
- It is **reference-counted against one OS handle**: many concurrent runs, one
  power assertion.
- It prevents **system sleep only, never display sleep**. The screen going
  dark and the session locking are correct and desirable during an overnight
  run.
- It is **best-effort**: a wake lock that cannot be acquired is logged and
  ignored. It never fails a run.

## Why the backend and not Electron

Electron ships `powerSaveBlocker` with a `prevent-app-suspension` mode, which
is the obvious first answer and the wrong one here. The installed editor is
"a windowed renderer host and nothing more" (`modules/desktop/src/main.ts`) —
`ae` owns the Forge lifecycle, and Forge runs and keeps running with no window
attached. A lock held by the Electron main process would evaporate the moment
the user closed the editor, which is precisely when an unattended overnight
run needs it most.

Forge owns the runs, so Forge owns the lock. This also means the feature works
for headless `ae` usage and for a browser-paired client with no desktop app
installed at all.

## What counts as unsettled work

`run.lifecycle` already carries the state machine
(`modules/protocol/src/control-contract/lifecycle.ts:245`), and the
orchestration repository already computes nearly the predicate we need:
`is_active_status` is `running | waiting` and `is_projectable_status` is
`queued | running | waiting`
(`modules/backend/src/persistence/orchestration/repository.ts:74`).

| State                | Hold the lock | Reasoning                                                   |
| -------------------- | ------------- | ----------------------------------------------------------- |
| `queued`             | Yes           | Dispatch is imminent and unattended; this is the core case. |
| `running`            | Yes           | Work is executing.                                          |
| `waiting` (retry)    | Yes           | A retry is pending and will fire on its own.                |
| `waiting` (approval) | See below     | Blocked on a human, not on the machine.                     |
| `interrupted`        | No            | Settled.                                                    |
| `completed`          | No            | Settled.                                                    |
| `cancelled`          | No            | Settled.                                                    |
| `failed`             | No            | Settled.                                                    |
| `closed`             | No            | Settled.                                                    |

Graph orchestration adds a second source: `AgentRuns.dispatch_status = 'queued'`
(`modules/backend/src/persistence/schema/orchestration.ts:265`) is durable
queued work that has not yet produced a `run.lifecycle` event. Both sources
must feed the count, or a queued graph node would let the host sleep before
its first dispatch.

Background model turns — thread continuation compaction, metadata refinement —
are real model calls that can be interrupted mid-flight and should hold the
lock while they run.

### The `waiting` ambiguity

`waiting` is overloaded. `modules/backend/src/conversation/projection/interaction.ts:90`
maps an approval in state `requested` to `waiting`, while line 176 maps a
retry-pending observation to the same. These deserve opposite treatment:

- **Retry-pending** progresses on its own. Hold.
- **Approval-pending** cannot progress until a human returns to the keyboard.
  Holding the lock means the host stays awake all night waiting for a click
  that is not coming. Releasing means the host sleeps and the user resumes on
  wake, losing elapsed time but nothing else.

Recommendation: treat approval-pending as a **grace period** rather than a
binary — hold for a few minutes after the request appears (the user may be one
room away), then release. This preserves the interactive case without burning
a night of power on a stalled queue. Splitting the projection so the two
`waiting` causes are distinguishable is a prerequisite.

## Platform backends

### Windows — koffi to `kernel32` power requests

`modules/engines/src/process/windows-job.ts` is the working template: it
already loads `kernel32.dll` through koffi, declares `__stdcall` signatures,
and owns handles behind a `Close`-shaped interface. A wake lock is a smaller
version of the same file.

```
PowerCreateRequest(REASON_CONTEXT)      -> HANDLE
PowerSetRequest(handle, PowerRequestSystemRequired)
PowerClearRequest(handle, PowerRequestSystemRequired)
CloseHandle(handle)
```

Use `PowerRequestSystemRequired`. Do **not** add `PowerRequestDisplayRequired`
— that keeps the screen lit for no benefit. `PowerRequestExecutionRequired`
targets process-lifetime suspension of packaged apps and is not what Forge
needs.

Prefer this over the older `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`:

- The power request is process-associated; `SetThreadExecutionState` is
  **thread-affine**, and the state persists only as long as the calling thread
  lives — fragile under Node's threading and koffi's call dispatch.
- `REASON_CONTEXT` carries a human-readable reason string that shows up in
  `powercfg /requests`. An invisible feature that changes OS power behavior
  should be able to explain itself, and this makes it self-documenting for the
  user and trivially debuggable for us.
- Reports on Modern Standby hardware describe `ES_SYSTEM_REQUIRED | ES_CONTINUOUS`
  behaving badly (notably battery drain with the lid closed), with the
  recommended remedy being a switch to `PowerSetRequest`.

**Hard OS limits worth designing around.** Per Microsoft's `PowerSetRequest`
documentation:

- On **Modern Standby systems on DC power**, system-required and
  execution-required power requests are **terminated 5 minutes after the
  system sleep timeout has expired**. On a modern laptop on battery, this
  feature cannot hold the machine awake indefinitely — no application can.
- Power requests are **terminated on user-initiated sleep** (power button, lid
  close, Start menu → Sleep) on all systems.

So the honest guarantee is: on AC power, or on a Traditional Sleep (S3)
machine, Artisan holds the host awake for the duration. On battery Modern
Standby, it does not. `powercfg /a` reports which sleep model the machine
uses; the backend can read it once at startup and let the UI tell the truth
rather than promise something the OS will revoke. Whether the "hibernate
after" idle timeout is equally covered should be confirmed empirically with a
short hibernate timer.

### macOS — a `caffeinate` child process

`/usr/bin/caffeinate` has shipped since OS X 10.8, needs no installation, no
entitlement, and no admin rights, and it is a thin wrapper over the same
`IOPMAssertionCreateWithName` IOKit assertion an application would take
directly.

```
caffeinate -i -w <forge_pid>
```

`-i` prevents idle **system** sleep (not display sleep — again, correct).
`-w <pid>` makes the assertion outlive nothing: when Forge exits for any
reason, including a crash, `caffeinate` notices and releases. That watchdog
behavior is a genuine advantage over an in-process assertion, which needs
explicit cleanup on every abnormal-exit path.

The FFI alternative — `IOPMAssertionCreateWithName` with
`kIOPMAssertPreventUserIdleSystemSleep` — buys assertion introspection and
nothing else here, at the cost of a second native code path. It is also
currently impossible without packaging work: Forge bundles only
`@koromix/koffi-win32-x64` (`forge.vite.config.ts:40`), so koffi is a
Windows-only capability in shipped builds today.

Caveat: closing the lid on battery still sleeps the machine regardless of
assertions. Verify with `pmset -g assertions`.

### Linux — a logind inhibitor lock

`org.freedesktop.login1.Manager.Inhibit(what, who, why, mode)` returns a file
descriptor; the lock releases when that descriptor and all its duplicates are
closed. Holding an fd for the lifetime of the work is exactly the ownership
model we want.

- `what` = `idle` inhibits the system entering idle mode, which is what
  triggers automatic suspend. `sleep` inhibits suspend and hibernation
  outright.
- `mode` = `block` makes it mandatory; `delay` only postpones.
- Both are polkit-gated —`org.freedesktop.login1.inhibit-block-idle` and
  `org.freedesktop.login1.inhibit-block-sleep`. `block-idle` is the one
  ordinary desktop applications are expected to take; blocking sleep is more
  restricted and can still be overridden by a sufficiently privileged user.
  **Take `idle`, not `sleep`.**

The zero-dependency implementation is to spawn
`systemd-inhibit --what=idle --mode=block --who=Artisan --why=... <blocking command>`
and kill the child to release, rather than speaking D-Bus directly. Verify
with `systemd-inhibit --list`.

On hosts with no logind session — containers, headless servers, some
distributions — this degrades to a no-op, which is correct: those machines do
not idle-suspend in the first place.

## Implementation shape

A `SystemWakeLock` Effect service under `modules/backend/src/runtime/`, with
one layer per platform selected at construction and a no-op layer for
unsupported platforms, development, and tests.

- Expose the lock as a **scoped resource** so Effect's own scope handling
  releases it, rather than a manual acquire/release pair that has to be
  correct on every failure path.
- Keep **one OS handle** behind an internal reference count. Acquiring a
  handle per run would multiply platform calls for no benefit and make leak
  diagnosis harder.
- Add a **linger** of roughly 60 seconds after the count reaches zero, so a
  queue draining item-by-item does not thrash the OS handle between every
  work item.
- The count-from-state predicate is pure and belongs in its own module — it is
  the part worth unit-testing exhaustively, with no OS calls involved.

### Observability and control

Log every acquire and release with the reason and the resulting count. Surface
the state in the UI — something as small as "Keeping this machine awake, 3
runs in flight" — because a feature that silently changes OS power behavior
should be legible, and because the user needs to be able to tell the
difference between "Artisan is holding the lock" and "the OS revoked it"
(the Modern Standby DC case above). Ship a setting to disable it, defaulting
to on.

## Verification

| Platform | Command                  | Expected                                                          |
| -------- | ------------------------ | ----------------------------------------------------------------- |
| Windows  | `powercfg /requests`     | Artisan's reason string listed under SYSTEM while a run is active |
| Windows  | `powercfg /a`            | Confirms Modern Standby vs S3 for interpreting the DC limit       |
| macOS    | `pmset -g assertions`    | `PreventUserIdleSystemSleep` attributed to caffeinate             |
| Linux    | `systemd-inhibit --list` | An `idle` block inhibitor owned by Artisan                        |

The end-to-end test that matters is the one that reproduces the reported
failure: set the host idle-sleep timeout to a few minutes, queue work longer
than that, walk away, and confirm the queue drains.

## Open questions

1. Approval-pending `waiting` — grace period, or hold indefinitely? Requires
   splitting the two `waiting` causes in the interaction projection first.
2. Should a detached terminal session with a live process count as unsettled
   work, or only agent runs?
3. Does the Windows power request survive an idle-triggered _hibernate_ as
   well as sleep?

## Sources

- [PowerSetRequest function (winbase.h) — Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-powersetrequest)
- [System Sleep Criteria — Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/power/system-sleep-criteria)
- [Awake: keep the system awake with the display off on Modern Standby — microsoft/PowerToys#48965](https://github.com/microsoft/powertoys/issues/48965)
- [Inhibitor Locks — systemd.io](https://systemd.io/INHIBITOR_LOCKS/)
- [org.freedesktop.login1(5) — Linux manual page](https://man7.org/linux/man-pages/man5/org.freedesktop.login1.5.html)
- [systemd-inhibit(1) — Arch manual pages](https://man.archlinux.org/man/systemd-inhibit.1.en)
- [caffeinate — ss64 macOS reference](https://ss64.com/mac/caffeinate.html)
