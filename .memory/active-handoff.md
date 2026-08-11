# Active Branch Handoff

Last updated: 2026-08-11. Branch continuity only; durable verified status is in [`docs/status/backend-completion-matrix.md`](../docs/status/backend-completion-matrix.md).

## Working State

- Repository `C:\Users\sander\Desktop\artisan-editor`; direct `master` → `origin/master`.
- Focused validation `48875b1c`, pairing `4f7e2a2c`, acceptance docs
  `2f634890`, and bounded backend persistence `f48ce041` are pushed.
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

- Renderer banners/toaster and `svelte-sonner` are gone; host notifications remain.
  Typography is `{ text, code }`, safely maps legacy records, and drops Lora.
  Rich-link favicons win scoped prose margins; focused checks and review pass.
- Effort menu groups GPT 5.6 Max/Ultra, Claude Max, and Grok none; Cursor Max Mode remains separate and Haiku has no flag. Ultra has a catalog-owned caveat tooltip, the group separator keeps equal insets, and speed accents stay on the outer model trigger rather than its policy dropdown.
- Trace audit: Codex exposes summaries plus raw text (raw dropped); Fable's Artisan
  `-p` path receives summaries but not raw; Grok has `thought`; Cursor suppresses it.
  Adjacent summaries now share one auto-open verb/rail disclosure, close upward on
  settlement, and unmount after transition. Fidelity remains unmodeled.
- New-thread draft recovery stays hidden during normal navigation and appears only after a route failure.
- Explicit New thread actions now discard only their target composer slot and
  revision-reset the app-scoped draft controller. Reset/alignment share a lock,
  and accepted first messages clear interruption-safely before navigation.
- A quick Codex stop can reach `turn/interrupt` before `turn/started`. Forge then
  records the cancel outbox row as `undeliverable`, and the live run continues.
- Hidden scrollbar chrome and its five fake right-side gutters are removed app-wide;
  scrolling, clear inactive fades, new-thread spaces, and pinned model controls remain intact.
- The app-icon star is centered at 75% with its geometry, shadow stack, and gradient;
  Windows injects a pre-rasterized seven-resolution ICO so packaging takes 6.99s.
- Working rows prefer bounded assistant previews even when unread; lifecycle stays authoritative, marquee/prose code is bare muted monospace, and overflowing live previews use a readable 30-second cycle.
- Jump-to-latest chrome follows the rail-aware prose column, not the whole card.
- Storage repair is verified: raw frames became run watermarks, text is cadence-
  batched, patches/surfaces retain 256/512 per thread, native inboxes are consumed,
  erased threads delete images, and cold metadata recovery is background/indexed.
  Migration compacts logically; `db:compact` safely reclaims the physical 4.17 GiB.
  Backend/protocol/data prerequisites and status docs are pushed in `f48ce041`.
- All settled work history mounts collapsed; terminal Checklists hide; paste bump is gone while
  duplicate rejection remains. Review passes; protected shared work prevents staging.
- Forge attention/replay, child cleanup, curated names, and terminal retention
  pass focused tests, checks, and review.
- Agent names allocate transactionally and uniformly without replacement; persisted
  names never reroll, banks have no popularity weights, and collisions/restart replay pass.
- Steering waits for successful Engine Send; failure restores the draft, crash recovery
  queues once, and bounded contiguous cuts preserve single/multi-steer order. Review is clean.
- Forge work outranks local rail settlement; only current hydration applies before
  shell open. Active disclosures retain live detail, rail regions remain interactive,
  model-transition wording waits for its source, and event observers coalesce.
- Codex/Claude keep root ownership while native descendants become provider-neutral
  subagent lifecycle/transcripts. Root filters parentless turns; inspection uses
  durable identity, Back/Escape restores root, and generic Claude jobs stay excluded.
- Child lifecycle advances only global recency; aggregate/root activity owns the
  reader cursor. Unread Complete/failed outcomes stay Working with blue/red dots
  until focused root reading or explicit Settle; active work and Idle stay unmarked.

## Verification

- Installed 0.2.40 established Editor→Forge loopback transport and closed cleanly.
  Pairing passes UI 44, native 76, root types/builds, and independent review;
  area gates reach only 6/1 protected frontend/desktop stale assertions.
- Effort/speed follow-up passes 4 focused files/38 tests, scoped format/lint,
  root TypeScript, the production frontend build, generated CSS inspection, and SER scans.
- Disclosure correction passes 2 files/9 tests, format/lint, frontend and packaged
  Electron builds, packaged verification, and bundled-predicate inspection. The
  broader UI fix retains its earlier static/build/SER, native/73, and review gates.
- New-thread lifecycle/default restoration passes 13 focused files/93 tests,
  scoped format/lint/SER, root TypeScript, and the production frontend build.
  Full validation passed its global pre-test gates, then its complete Vitest
  phase stopped making progress and was ended after the exact process went idle.
- Attention retention passes 6 files/42 tests, format/lint/SER, build, and review.
- Steering acknowledgement passes 5 files/49 tests plus static/build/review;
  delivery, fallback, and restart prove no early projection, old-run resume, or
  duplicate replacement. Render order and same-item boundaries pass 52 integrated
  tests plus frontend/Forge types, scoped checks/build, and final review.
- Reasoning disclosure passes 12 focused tests, lint, client/SSR build, SER scans,
  and final review. The aggregate frontend gate has five unrelated stale assertions.
- Storage/native integration passed 43 focused tests, then the final optional-usage
  regression passed after its exact-optional fix; backend format/lint and root types pass.

## Dirty-Tree Integration Notes

- Native-subagent/name-allocation, storage retention, and backend status documentation
  are committed in `f48ce041`; unrelated frontend presentation work remains dirty.
- Selector/speed and new-thread default persistence share view, settings,
  protocol/schema/service, route, migration, and source-gate files with protected
  work, so the verified fix remains uncommitted.
- Assistant preview and attention retention share protected rail/navigation,
  protocol, and migration work; do not stage or publish those files alone.
- Steering acknowledgement shares composer/route, projection/repository,
  protocol/store/workspace, handoff, and status files with protected work; do
  not stage those whole files.
- Protected page patch hash: `84cb787c1f2422da8c5fb5c41a00837151590e10`.

## Product Continuity

- One Forge per Artisan home owns config, secrets, state, log, and data.
- Workspace/thread identity is encoded into both primary surface URLs; Forge
  remains authoritative when a thread is reassigned, detached, or removed.
- Installed renderer is sandboxed at `artisan://app`; Forge does not host the
  SPA. `ae open --handoff` performs one-time loopback pairing.
- Forge state and Codex SQLite are home scoped. `CODEX_HOME` is user global;
  Claude shares `~/.claude` because relocating config also moves credentials.
