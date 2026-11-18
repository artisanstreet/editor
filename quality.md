# Quality

Every dimension a product has to get right. Rate each 0–3 (0 = absent, 1 = ad hoc, 2 = deliberate, 3 = enforced by tooling or tests).

## 1. Product and experience

| Category | What "perfect" means |
| --- | --- |
| Product definition | Clear problem, target users, scope, non-goals, success metrics |
| UX | Information architecture, flows, discoverability, low friction, sensible defaults, undo, forgiving input |
| UI and visual design | Layout, spacing, typography, colour, iconography, hierarchy, one design system and token set |
| Interaction design | Feedback for every action; hover, focus, active, disabled and selected states; shortcuts; drag and drop; text editing that matches platform conventions |
| Motion | Purposeful, interruptible, consistent timing, honours reduced motion, never janky |
| State coverage | Every surface has empty, loading, partial, error, offline, overflow (huge text, huge lists) and first-run states |
| Content and copy | Microcopy, tone, actionable error messages, one term per concept |
| Onboarding | First run, time to first value, contextual guidance |
| Accessibility | WCAG 2.2 AA, screen readers (UIA, AX, AT-SPI), full keyboard operation, focus management, contrast, zoom and text scaling, high-contrast mode, captions |
| Internationalization | Externalized strings, plurals, RTL, Unicode and IME input, locale formats, time zones |
| Adaptivity | Window sizes, DPI, multi-monitor, touch, light and dark themes |
| Delight and polish | Craft in edge cases, satisfying feedback, a coherent feel |

## 2. Runtime quality

| Category | What "perfect" means |
| --- | --- |
| Performance | Startup time, input latency, frame pacing, throughput, perceived speed |
| Resource efficiency | Memory, idle CPU near zero, GPU, battery, disk IO, network |
| Scalability | Behaviour at 10× and 1000× data; virtualization; documented limits |
| Reliability | Crash-free rate, no hangs, graceful degradation |
| Resilience and recovery | Crash recovery, autosave, safe restart, resumable work, idempotent operations |
| Data integrity | No loss, atomic writes, transactions, corruption detection, backup and restore |
| Concurrency | No races, deadlocks or torn state; clear ownership; cancellation |
| Network tolerance | Flaky links, latency, timeouts, reconnect and resume, sync conflicts |
| Compatibility | OS versions, file formats, interop, forward and backward compatibility of stored data |
| Portability | Platform abstraction, no hardcoded paths, shells or encodings |
| Determinism | Same input, same output: builds, renders, tests |

## 3. Architecture and code

| Category | What "perfect" means |
| --- | --- |
| Architecture | Clear module boundaries, one dependency direction, separation of concerns |
| State management | Single source of truth, explicit state machines, ADTs that make illegal states unrepresentable |
| Data modelling and persistence | Schema design, migrations, versioning, normalization at the boundary |
| API design | Consistent, versioned contracts; backward compatibility; typed errors; pagination; limits |
| Error handling | Typed errors with context chains, no silent swallowing, actionable messages |
| Code quality | Readable names, small units, pure functions, no dead code, low complexity |
| Type safety | Strict types, validated boundaries, no escape hatches |
| Consistency | Lint and format enforced automatically, shared conventions |
| Dependency hygiene | Few, pinned, licence-compatible, maintained dependencies |
| Technical debt | Tracked, budgeted, deprecations planned |
| Extensibility | Extension points only where they are needed |

## 4. Verification

| Category | What "perfect" means |
| --- | --- |
| Test strategy | Each test fails for a user-visible reason; the right level for each risk (see Testing strategy below) |
| Performance testing | Benchmarks and budgets enforced in CI |
| Accessibility testing | Automated tree checks plus manual screen-reader passes |
| Cross-platform testing | Every supported OS, architecture and GPU path |
| Test reliability | Zero flakes, deterministic time and IO, fast feedback |
| Static analysis | Typecheck, clippy, dead-code and security analysis |
| Code review | Standards, scoped PRs, evidence for UI changes |

## 5. Security and privacy

| Category | What "perfect" means |
| --- | --- |
| Security | Threat model, authn and authz, secrets handling, input validation, sandboxing, least privilege, safe defaults |
| Supply chain | Lockfiles, SBOM, signed and reproducible builds, vulnerability scanning |
| Privacy | Data minimization, consent, retention, encryption at rest and in transit |
| Code signing | Signed binaries, verified updates |
| Vulnerability process | Disclosure policy, patch SLAs, advisories |

## 6. Delivery

| Category | What "perfect" means |
| --- | --- |
| Build system | Fast, incremental, hermetic, reproducible, one command |
| CI | Lint, typecheck, test and build every PR on every platform, cached, fast |
| Release engineering | Versioning, changelog, channels (nightly, beta, stable), rollback |
| Packaging and distribution | Installers, auto-update, clean uninstall, small download |
| Environments | Reproducible dev setup, dev and prod parity, seed data |
| Configuration | Settings schema, validation, migrations, feature flags |
| Infrastructure as code | Declarative, reviewed, versioned |

## 7. Operations

| Category | What "perfect" means |
| --- | --- |
| Observability | Structured logs, metrics, traces, symbolicated crash reports |
| Monitoring and alerting | SLOs, dashboards, actionable alerts |
| Diagnostics | Debug dumps, reproducible bug reports, support bundles |
| Incident response | Runbooks, postmortems |
| Telemetry | Opt-in, privacy-respecting usage insight |
| Cost | Infrastructure, egress and storage growth under control |

## 8. People and process

| Category | What "perfect" means |
| --- | --- |
| Developer experience | Fast setup, short edit-run loop, good tooling |
| Documentation | User docs, API reference, decision records, contributing guide |
| Project management | Roadmap, triage, priorities, definition of done |
| Collaboration | Branching, commit hygiene, PR process, ownership |
| Knowledge continuity | Low bus factor, decisions written down |
| Support and feedback | Bug intake, feature requests, user communication |

## 9. Business and legal

| Category | What "perfect" means |
| --- | --- |
| Licensing | Own licence, third-party compliance, attribution |
| Legal and compliance | Terms, privacy policy, accessibility law (EAA, ADA), export and industry rules |
| Brand | One identity and voice across site, app and docs |
| Discoverability | Landing page, SEO, store listings |
| Monetization | Pricing, billing, entitlements, trials, refunds |
| Sustainability | Maintainable pace, funding, end-of-life plan |
| Trust | Honest status, no dark patterns, transparent data use |

# Testing strategy

## Diagnosis (audit of 2026-11-17)

About 1,500 tests. Most individual tests check something real; the suite as a whole still fails at its job, for five reasons.

1. **About a third is noise.** Static and change-detector tests pin design values instead of behaviour, so they break on every design tweak and get updated in the same commit (71 of the last 80 test-touching commits also changed source).
   - editor-aqua: about 25% change-detectors (animation durations in `frame/glide.rs`, `thread_navigation.rs`, `ui/shimmer_text.rs`; pixel sizes in `ui/chat_bubble.rs`, `sidebar/mod.rs:567`), about 10% static or tautological (`markdown.rs:17` asserts a constant; about 19 tests test the test harness), about 4% smoke.
   - editor-ui: about 38 token and table tests in `tests/ui/kit/` (radius, surface, text role, row heights, hover alpha). `kit/enforcement.rs` is a source grep posing as 4 tests.
   - Forge crates: 14 to 25% change-detectors (exact CLI flag strings, exact JSON bodies, an EXPLAIN plan, backoff tables) and serde `deny_unknown_fields` tautologies in artisan-account and artisan-api.
2. **The full journey is never tested.**
   - Most decoder tests replay transcripts with no process, Forge, store or SSE involved; one forge-runs test (`src/tests.rs:1933`) runs a fake Claude process through the real adapter into runs and storage, and HTTP/TLS/SSE has its own integration tests.
   - Forge is tested with Rust-scripted `Harness` fakes that skip the decoders.
   - The Editor is tested against a fake host at `127.0.0.1:1`.
   - No test runs engine output → Forge → SSE → Editor → visible text. That chain is where the real bugs were: frozen threads, lost steers, resume, reconnect.
3. **No control over time or IO.** No virtual clock (tokio `test-util` is enabled but nothing uses `start_paused`). About 16% of forge-engines and forge-runs tests sleep in real time; some poll loops have no timeout and hang instead of failing. Nothing can inject a slow fsync, `SQLITE_BUSY`, a full disk mid-write or a kill mid-transaction.
4. **The tests that matter most never run.**
   - The `qualification` and `capture` features (all three real-Forge tests) are off in CI.
   - The pixel baselines are stale since 2026-11-06, have zero tolerance and sit behind `#[ignore]`.
   - macOS CI runs only four crates.
   - `settings/mod.rs:424` ("lay out identically") allows a 1000 px difference.
5. **Copied setup.** Setup is duplicated instead of shared, and test hooks live in production APIs:
   - 64 `Core::for_tests`, 50 hand-written `HostConfig`s, 82 `UiApp::new`, 15 full `Item` literals.
   - About 60 copies of the same host-lifetime race test.
   - About 8 `test_*` hooks and 10 `*Fixture` structs in `src/`.
   - Long runs of bare booleans in test APIs.

Structurally uncatchable today: upstream engine drift (OpenCode 2.1.0 recordings, no contract check), anything timing-dependent, IO faults, crashes mid-write, parser/store disagreement, migration edits (only schema v3 has a frozen fixture; later migration tests build old databases with today's code), shift-free streaming, the scene bound, frame budget, UIA cost.

## Principles

1. **A test exists because a user could see it fail.** Its name states the user-visible promise ("a steer sent mid-turn appears in the same turn"), not the mechanism.
2. **Assert outcomes, never design values.** No durations, pixel sizes, colours, copy strings, flag lists or JSON shapes unless they are a contract with something outside the repo, in which case test the contract (see layer 6).
3. **Prefer one deep test over ten shallow ones.** Real code at every layer; fake only the edges (engine process, clock, disk, network, OS).
4. **Deterministic by construction.** Virtual clock, seeded randomness, injected IO. No `sleep` in any test. A flaky test is a bug in the code or the harness: fix the cause, never retry.
5. **Snapshots are of meaning, not pixels.** Snapshot the semantic tree (visible text, roles, states, order) as readable text with `insta`. Review every snapshot diff like code; a diff nobody can read is a deleted test.
6. **Every escaped bug gets a regression test at the highest layer that would have caught it.**
7. **The suite is measured.** Mutation score, flake count and wall time are tracked; tests that no mutation ever fails are deleted.

## Layers

| # | Layer | Fakes | Asserts | Owns |
| --- | --- | --- | --- | --- |
| 1 | Pure logic | Nothing | Return values; properties with `proptest` | Parsers, decoders, folds, state machines, layout maths, text editing, mentions, deep links, pairing parsing |
| 2 | Transcript replay | Engine process (replays a recording) | Thread items in the store and on the SSE stream | Engine decoders + forge-runs + forge-store + forge-http together |
| 3 | Editor scenario | Forge (an in-process real Forge on layer 2's fake engine) | Semantic view tree: visible text, roles, focus, selection, scroll anchor | Conversation, composer, sidebar, settings, reconnect |
| 4 | Simulation | Clock, disk, network, process kills, all seeded | Invariants: no lost items, gap-free seq, no frozen thread, no lost draft | Concurrency, restart, disk pressure, reconnect storms |
| 5 | Visual and performance | GPU via the reference renderer | Perceptual diff with tolerance on a few screens; frame time and rebuild count budgets; UIA node diff counts | Design regressions, frame budget, partial redraw, a11y cost |
| 6 | Contract | Nothing; the installed engine binaries | Recorded shapes still match live output | Claude, Codex and OpenCode drift; Forge HTTP API compatibility |

Target weight: most tests in layers 1 and 3, a small set of very valuable layer 2 and 4 tests, a handful in 5 and 6.

### 1. Pure logic

- Keep the good ones: draft session, text editing, viewport, wheel smoothing, mentions menu, audio state, door, steer joining, seq writers.
- Add tests where real logic is untested:
  - `Screen::from_link`, `parse_pairing`, `parse_wsl_verbose`.
  - The `mcp_sign_in::callback` state check (security relevant).
  - `connection::follow` and `backoff`.
  - `merge_thread_page`, `prune_threads`. `RangeState` failure, stale completion, retry and readiness are already covered by `editor-aqua/tests/conversation/stored_rows.rs`.
  - Sidebar `row_state` and `splices`; marketplace labels.
- Use `proptest` for parsers and folds:
  - Any chunking of a transcript decodes to the same items.
  - Merge then prune is idempotent.
  - Text edits round-trip through UTF-16 offsets.
- `cargo fuzz` the engine decoders and the markdown parser.

### 2. Transcript replay (highest leverage)

- **Fake engine:** a binary that replays a recorded transcript on stdout and honours stdin (steers, interrupts, close), with timing driven by the test clock.
- **Path:** Forge spawns it through the real engine adapter, so the real decoder → run → store → HTTP/SSE all run, with a paused tokio clock.
- **Assertions:** on the items a real SSE client receives, including resume from `since=`.
- **Transcripts:** live under `crates/forge-engines/transcripts/<engine>/<version>/`.
  - A `record` command turns `trace.rs` traces into transcripts, so every real bug report becomes a fixture.
  - Hand-written transcripts are marked as such and kept to edge cases recordings can't produce.
- **Bugs this catches:** lost steers, background agents killed on stdin close, resume after restart, turn continuation, compaction, side queries.

### 3. Editor scenario

- **Shared harness:** one `tests/support` crate with:
  - `Scenario::new().with_forge(transcript).open_thread(..)`;
  - a builder for `Item`, `HostConfig` and `UiApp`;
  - one scene walker.
- **Moves out of `src/`:** `Core::for_tests`, the `test_*` hooks and the `*Fixture` structs become support code or disappear.
- **Snapshots:** the semantic view tree as text, e.g. `turn 3 › reply › "Done." [visible] ; composer [focused] "draft"`. Snapshots replace the pixel and duration assertions.
- **Scenarios** (each is one test):
  - Streaming never moves painted text.
  - Long threads past 512 items still update.
  - Reconnect after Forge restarts mid-turn.
  - A signed-out engine says "sign in" before "no models".
  - A draft survives restart.
  - A conflict shows the right choice.
  - Revisiting a thread lands at the latest message.
  - Selection survives a re-render.
  - The image paste and drop paths.
- **Host lifetime:** collapse the ~60 race tests into one table-driven test over every host-backed view.

### 4. Simulation

- **Seeded runner** in the FoundationDB and TigerBeetle style, with injected sources for everything nondeterministic:
  - clock and task scheduling;
  - disk (latency, `SQLITE_BUSY`, ENOSPC, torn writes);
  - network (drops, reorders, delays on SSE);
  - kill -9 of Forge or the engine.
- **Each run:** drives random user actions against layers 2 and 3, then checks invariants:
  - every committed item is eventually visible;
  - seq stays gap-free;
  - no thread stops updating while Forge has newer items;
  - drafts are never silently lost;
  - restart resumes live turns.
- **Reproduction:** a failing seed reproduces exactly and becomes a fixed regression case.
- **Schedule:** short on every PR, long nightly.

### 5. Visual and performance

- **Screens:** at most 15 canonical screens rendered with the reference GPU backend, perceptual diff with a small tolerance, baselines regenerated by one command, and diffs posted as PR artifacts.
- **Budgets:**
  - frame time;
  - rebuilds per frame for scroll, stream and hover (from `gating.rs` counters);
  - idle CPU;
  - UIA nodes diffed per frame (the aqua #43 regression);
  - startup time;
  - memory after opening a long thread.
- **How budgets fail:** in CI, on relative regression against master, not on absolute values.
- **The pixel baselines** in `tests/qualification/baselines/aqua/` are replaced by this, not revived.

### 6. Contract

- **Nightly:** run each installed engine (Claude Code, Codex, OpenCode) on a fixed scripted prompt and compare event shapes with the recorded transcripts.
  - Drift opens an issue with the new recording attached.
  - This replaces the tests that pin CLI flags and JSON bodies.
- **Forge HTTP API:** pairs of a previous-release client against today's Forge and vice versa.
- **Migrations:** a frozen database from every released schema version, migrated forward and checked against queries the app actually runs. Never build "old" databases with today's code.

## What to delete

- editor-ui `tests/ui/kit/` token and table tests (about 36, minus the interaction and geometry checks among them); move enforcement to clippy configuration.
- editor-aqua change-detectors:
  - durations and curves: glide, menu, sliding tab, seam motion, scroll edge, composer glide, scramble, shimmer, pulse;
  - pixel sizes: chat bubble, inbox height, spacer, chevron, dither.
  Behaviour worth keeping (animations finish, can be interrupted, honour reduced motion) moves to one scenario test each.
- Constant assertions (`markdown.rs:17`), harness self-tests (`qualification/screens.rs`, `parity/*`, `qualification/script.rs`), smoke tests (`settings/mod.rs:254`, `uia.rs:27` until it asserts real structure).
- Forge change-detectors: CLI flag strings, exact JSON request bodies, the EXPLAIN plan test, the backoff table, lease and fence registry tests, Debug output, PKCE tests that recompute the formula (keep the RFC vector test in `artisan-account/tests/browser.rs`), serde `deny_unknown_fields` and transport identity tests.
- Fix or delete `settings/mod.rs:424`.
- Shader tests keep finite-output and mask checks, drop exact RGB stops.

Before deleting, split each file's literal design pins from its invariant assertions (interruption continuity in `frame/glide.rs`, joined geometry in `ui/chat_bubble.rs`) and keep the invariants. Expected result: roughly a quarter to a third fewer tests and substantially more caught bugs.

## CI

- Use `cargo nextest` everywhere (already in the dev shell); `--no-fail-fast` on Linux too.
- Run layers 1–3 on every PR on Linux, Windows and macOS (macOS currently runs four crates).
- Run the `qualification` features, or delete what they gate; nothing is allowed to exist that never runs.
- Short simulation on every PR, long simulation and contract tests nightly.
- Performance budgets compared against master on a fixed runner.
- Fix the `target/` cache so it actually updates on hits.
- Zero-tolerance flake policy: a flake is quarantined within a day and fixed or deleted within a week.

## Measuring the suite

| Metric | Tool | Target |
| --- | --- | --- |
| Mutation score on logic crates | `cargo-mutants` | Rising; mutants surviving in core logic are gaps |
| Tests that no mutant fails | `cargo-mutants` | Deleted |
| Flake count | nextest retries report (detection only) | 0 |
| PR wall time | CI | Under 10 minutes |
| Escaped bugs with a regression test | Issue labels | 100% |

## Order of work

1. Delete the noise listed above and fix the CI gaps (`--no-fail-fast`, nextest, macOS coverage, qualification features, cache).
2. Shared `tests/support`, move test hooks out of `src/`, collapse host-lifetime tests.
3. Fake engine process and transcript replay through Forge with a paused clock, plus the `record` command.
4. Editor scenario harness with semantic snapshots; port the valuable conversation and composer tests onto it.
5. Clock, disk and network injection in Forge, then the seeded simulation runner.
6. Contract jobs, migration fixtures, performance budgets and the small visual suite.
7. `cargo-mutants` in a nightly job to keep the suite honest.
