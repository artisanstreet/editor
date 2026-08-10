# Active Branch Handoff

Last updated: 2026-08-10. Branch continuity only; durable verified status is in [`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master` → `origin/master`.
- Forge lifecycle `810e07b4` is pushed; local/remote `master` match and 0.2.27 runs.
- Root cleanup is isolated from unrelated work still present in the checkout.

## Invariants

- Pure transforms are ordinary TypeScript. I/O, configuration, concurrency,
  shared state, lifecycle, and external capabilities use Effect Services and
  Layers. Schema-decode every external or persistence boundary.
- Forge owns application state. Engine adapters own native subprocess
  protocols; orchestration owns thread/run lifecycle and durable affinity.
- Forge/Editor ship only Artisan-owned runtime code. Provider executables stay
  external and are reached only through CLI or ACP process adapters.
- Native provider IDs never cross engines. Portable handoff is a private,
  bounded, immutable checkpoint with a fixed cut and SHA-256 integrity hash.
- A portable target always starts fresh. Compatible same-engine resume is
  version/model gated and carries the new ordered request content explicitly.

## Active Work

- Lifecycle milestone `810e07b4` passes 39 Rust/16 desktop tests, package, and review;
  full tests hit 3 unrelated assertions after 2,555 passes/7 skips. It is pushed.
- New-thread spaces, pinned model controls, hidden scrollbars, and clear inactive
  scroll fades are implemented without disabling containers.
- Working rows keep a bounded assistant preview separate from lifecycle; marquee code is bare muted monospace.
- Tool configuration lives under `.config/`; only Windows installer transport/test remain PowerShell.
- A latched recovery epoch and browser clock-gap monitor recover after suspend without reloading.
- Streaming repair is in progress: terminal observations can leave prose `streaming`;
  late mounts expose a partial token; live activities drive `Waiting` until terminal.
- Open work-session body/header spacing matches; real trace status retains `gap-5`.
- Changed-file rows/header now share an edge, the all-known aggregate sits at
  right, and the gap matches row `py-1.5`. Opt-in thread notifications are
  event-driven and persistent; Windows registers stable toast identity early,
  repairs legacy Start Menu links in place, and the native installer carries
  the same AUMID/CLSID for new/repair flows.
- WebGL context-loss recovery now registers before shader allocation, hides a
  corrupt canvas behind its glass material, and rebuilds every invalidated GL
  resource after restoration.
- The thread proximity rail now uses projected engine/model/live status, gives
  working and ordinary rows separate bounded scroll regions with a conditional
  `gap-4`, and exposes DEV-only ×20 stress sliders. Settled rows are newest-first
  in elapsed-time groups: unlabeled today, Yesterday, Last 3 days, Last 7 days,
  and a final Past month catch-all, with `gap-4` between groups. It remains
  uncommitted with the protected frontend layout slice.
- Codex 0.145.0 usage is schema-correct. Its generation-owned meter/row-refresh
  repair passes 13 tests, TypeScript, lint/format, SER build, and review; full
  validation reached 7 unrelated dirty frontend source assertions after both builds.
- Provider runtime boundary: the SDK/package and embedded 266.1-MiB Claude CLI
  are removed. Claude runs/usage use the external CLI; Codex usage/auth uses ACP
  only. Exact candidate SEA is 105,915,904 bytes; the process matrix is green.
- The 2026-08-08 regression repair makes Forge-owned active work outrank local
  rail settlement, applies only the current hydration's thread snapshot before
  opening the shell, lets active disclosures close without dropping their live
  detail tree, and keeps every rail region interactive. Started model-transition
  wording waits for its source. Optional event observers coalesce independently.
- Codex and Claude preserve root ownership while native child/grandchild work
  becomes provider-neutral `subagent` lifecycle and `subagent_transcript`
  content. Root rendering now filters to parentless turns; worker inspection
  filters by durable agent identity. Back/Escape restores root, while root traces
  name one worker or count multiple. Generic Claude jobs stay excluded.
- Individual child lifecycle no longer settles parent thread metadata; the
  orchestration-group lifecycle is authoritative. Global activity still tracks
  worker recency, while `reader_activity_at` advances only for root-visible
  activity. Read acknowledgement also requires focused, visible reader attention.
- The editor route subscribes to the authoritative thread list. Reassignment
  unmounts the old editor before moving to the new workspace route; detach moves
  to `/t/_/:thread`, so stale file reads cannot retain revoked workspace scope.
- `/t/:workspace/:thread` and `/e/:workspace/:thread` are the sole thread/editor
  routes. `/` owns draft creation, activity, recents, and inspector behavior;
  `/threads` is removed and all new-thread navigation targets `/`.

## Verification

- Waiting/status and native-subagent clusters pass their focused suites, build,
  type, lint/SER, migration, erasure, and independent-review gates.
- Assistant preview/status passes 6 files/46 tests plus production frontend.
- Provider CLI/ACP milestone passes 14 files/112 tests, TypeScript, Forge checks,
  and a real 105,915,904-byte candidate SEA with zero provider assets/SDK markers;
  independent review found no code blocker. Full validation clears format, lint,
  typechecks, and both builds, then hits 3 unrelated dirty frontend assertions;
  native checks and all 66 Rust tests pass separately.

## Dirty-Tree Integration Notes

- Native-subagent integration spans shared engine/backend/frontend files in the
  mixed checkout. Transcript isolation and status/read repairs are verified but
  uncommitted because staging those whole files would also publish unrelated
  protected work.
- Selector polish shares `model-selector/view.sv` and the shell source gate with
  protected in-progress frontend work, so it remains uncommitted with that slice.
- Assistant preview shares the protected rail/prose files; its migration follows
  three untracked migrations and must not be published alone.
- Protected page patch hash: `84cb787c1f2422da8c5fb5c41a00837151590e10`.

## Product Continuity

- One Forge per Artisan home owns config, secrets, state, log, and data.
- Workspace/thread identity is encoded into both primary surface URLs; Forge
  remains authoritative when a thread is reassigned, detached, or removed.
- Installed renderer is sandboxed at `artisan://app`; Forge does not host the
  SPA. `ae open --handoff` performs one-time loopback pairing.
- Codex app-server/exec fallback and the external Claude CLI are production adapters.
  Native child lifecycle/roster, ordered transcript replay, approvals, and
  read-only participant conversation inspection are supported.
- Forge state and Codex SQLite are home scoped. `CODEX_HOME` is user global;
  Claude shares `~/.claude` because relocating config also moves credentials.
