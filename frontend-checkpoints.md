# Artisan Editor Frontend Checkpoints

This is the build order for the Artisan renderer, not a list of screens to mock independently. Each checkpoint must leave a coherent, testable vertical slice. A checkpoint is complete only when its exit gate passes; visual intent or fixture-only behavior does not count as live integration.

## Status Vocabulary

- **Ready:** the current public protocol and typed `ArtisanClient` support live integration.
- **Fixture-ready:** the frontend can be designed and tested against protocol-shaped fixtures, but a required backend contract is absent or incomplete.
- **Blocked:** implementation would invent backend-owned behavior or cross the renderer boundary, so only design work should proceed.
- **Backend moving:** a contract exists in the current dirty worktree but is being implemented by the backend session. Consume it only after that session records a verified checkpoint in `MEMORY.md` and `backend-completion-matrix.md`.

## Global Completion Rules

- The renderer renders projections and sends commands through `@artisan/transport`; it never imports backend, engine, Electron, Node filesystem, Git, PTY, provider config, or database modules.
- Durable thread, run, file, terminal, Git, preview, permission, and settings state remains backend-owned. Local state is limited to drafts, selection, pane sizes, open popovers, scroll positions, and the user-owned tab model.
- Route files stay compositional. Real UI lives in route-local `components/` folders or shared custom components; `components/ui` remains shadcn-owned.
- Every bespoke transition has a reduced-motion state. Every mouse action has a keyboard path and visible focus state.
- Fixtures are typed from `@artisan/protocol` or a frontend view model derived from it. Do not create a second domain vocabulary just to make a mockup convenient.
- A live checkpoint needs visible loading, empty, reconnecting, stale, rejected, and failed states. A happy-path screenshot is not an exit gate.

## Checkpoint 0 — Freeze the Frontend Contract Map

**Status: Ready.** This prevents frontend work from silently depending on PRD examples that have not become public contracts.

- [x] Record every initial screen and interaction as `live`, `fixture`, or `blocked` in a small frontend contract registry.
- [x] Map live domains to `ArtisanClient`: thread list, thread work, thread retention, orchestration graph, terminal list/output, global guidance, Model Behaviour, events, reconnect cursors, and generic commands.
- [x] Mark workspace file/change envelopes as **backend moving** until the backend session verifies their service, router, client, and production composition.
- [x] Mark Git, preview lifecycle, Marketplace, identity/account, engine/session controls, file discovery/quick open, and packaged Electron bootstrap as contract gaps.
- [x] Define one owner for command IDs, correlation, retries, and projection subscriptions: the typed client layer, never individual components.

**Exit gate:** a reviewer can trace every planned interactive control to a public command/query/projection or to an explicitly named fixture, with no renderer-side side-effect shortcut.

## Checkpoint 1 — Scaffold the Frontend Module

**Status: Ready.** No frontend or Electron package exists yet.

- [x] Create `modules/frontend` as a SvelteKit frontend using Vite+, Svelte Effect Runtime, Effect 4, Tailwind CSS, shadcn-svelte, mode-watcher, and the repository's Svelte plugin stack.
- [x] Keep SvelteKit, SER, aliases, preprocessors, and adapters in `vite.config.ts`; add no `svelte.config.*` unless a verified tool limitation requires it.
- [ ] Add `better-svelte-check`, oxfmt, oxlint, Vitest, and component/browser testing commands to the workspace validation path.
- [x] Add package boundaries that allow imports from `@artisan/protocol` and `@artisan/transport` but fail if renderer code imports `@artisan/backend`, Electron, Node-only adapters, Drizzle, or engine packages.
- [x] Establish `src/lib/styles/global.css` and `fonts.css`, route-local `components/`, shared `components/custom/`, and shadcn-owned `components/ui/`.
- [x] Add a test-only frontend runtime that can supply a typed `ArtisanClient` fixture without teaching components whether the source is fake or live.

**Exit gate:** the package builds and validates in the monorepo, a minimal route renders through SER, and an import-boundary test rejects every forbidden dependency.

## Checkpoint 2 — Establish Artisan's Visual System

**Status: Ready.** Use [`barekey-design-language.md`](./barekey-design-language.md) as research, not source code.

- [x] Define Artisan-owned light and dark semantic tokens for canvas, panes, raised surfaces, text hierarchy, borders, focus, selection, diffs, permissions, conflicts, and agent/run states.
- [x] Choose licensed body, heading, and mono fonts; reserve Cal Sans with `-0.05em` tracking for the Artisan Editor wordmark as specified by the PRD.
- [x] Define density tokens around compact 28–36px controls, 16px icons, pane gutters, row heights, editor tabs, and right-pane sections.
- [x] Define surface levels and stop arbitrary one-off shadows, radii, opacity values, and transition durations from spreading through components.
- [x] Build a frontend-only visual fixture route covering typography, buttons, inputs, menus, tooltips, badges, status rows, tabs, empty states, skeletons, banners, permission prompts, diffs, and terminal chrome.
- [ ] Test light, dark, high-contrast, 200% zoom, reduced motion, long labels, and keyboard focus.

**Exit gate:** every primitive used by the shell has approved semantic tokens and interaction states, with no Barekey code, asset, or unlicensed font copied into Artisan.

## Checkpoint 3 — Build the Three-Pane Shell

**Status: Fixture-ready.** Its layout is local UI state, while most pane content can start as typed fixtures.

- [x] Implement the desktop grid at `272px minmax(720px, 1fr) 340px` with compact gutters and independent pane scrolling.
- [x] Give the left pane a compact brand header, action zone, scrollable thread region, and pinned user card.
- [x] Give the main pane separate mode controls and file-tab controls; do not merge those two concepts into one tab group.
- [x] Give the right pane a persistent, dense session stack with quiet empty states.
- [x] Collapse or hide the right pane before the main pane becomes narrower than its useful minimum; then collapse the left pane to an icon rail or overlay.
- [ ] Persist only local presentation preferences such as pane sizes and collapse state.

**Exit gate:** layout tests cover common desktop widths, narrow windows, zoom, independent scrolling, pane priority, and focus order without starting a development server.

## Checkpoint 4 — Make the Left Pane Real

**Status: Ready for threads and retention; fixture-ready for identity.** `ListThreads`, thread-list subscriptions, thread commands, and retention APIs exist. OS identity and account contracts do not.

- [ ] Render projects, linked projects, thread titles, two-line live status, pin/archive state, rehome suggestions, and the active thread from `ThreadListItem` projections.
- [ ] Add `New chat`, then `Marketplace` directly below it, followed by the independently scrollable thread list.
- [ ] Wire create, select, rename, pin, unpin, archive, restore, project assignment, project unlock, and rehome-suggestion actions through typed commands.
- [ ] Preserve the seven-day retention controls and explain pin exemptions without implementing deletion locally.
- [ ] Build a fixture-only bottom identity card with OS-image, deterministic-gradient, username, and machine-name fallbacks; keep it disconnected until an identity contract exists.
- [ ] Use a moving hover highlight and selected caret only if keyboard focus remains separately visible.

**Exit gate:** live MessagePort tests prove snapshot plus patch behavior, selection stability, duplicate-command safety, project grouping, status updates, and retention changes across reconnect.

## Checkpoint 5 — Build the User-Owned Workspace and Tab Model

**Status: Fixture-ready.** The tab model is frontend-owned, but file discovery and stable live file integration are not complete.

- [ ] Implement the main modes: `Text Editor`, `Chat`, and `Orchestrator`.
- [ ] Preserve active file, editor state, transcript scroll, composer draft, and selected orchestration node when switching modes.
- [ ] Implement normal, preview, pinned, dirty, diff-preview, and agent-change tab states plus overflow navigation.
- [ ] Replace an unpinned preview tab on temporary navigation and promote it on edit, double-click, or explicit pin.
- [ ] Surface changed files without opening every agent-touched file.
- [ ] Add quick-open, breadcrumbs, recent files, changed files, and tab overflow before considering a heavy file tree.

**Exit gate:** pure tab-model tests cover preview replacement, promotion, dirty preservation, close behavior, overflow, change badges, and mode switching with no backend dependency.

## Checkpoint 6 — Deliver the Chat Workflow

**Status: Partly ready.** Sending messages, steering, question and approval events, run lifecycle, completed assistant messages, and a compact current-work item exist; a transcript, reload-safe pending interactions, and dedicated model-delta stream contract do not.

- [ ] Use `GetThreadWork` only for its compact current agent/run identity and status; do not treat it as message history.
- [ ] Render live question and approval cards from canonical events, while keeping reload recovery fixture-backed until a current-interactions projection exists.
- [ ] Build the composer with durable command IDs, queued versus auto-steered follow-ups, attachments as future capability states, and explicit send/retry feedback.
- [ ] Render approvals and questions as first-class accessible actions, not terminal transcript fragments.
- [ ] Add rich markdown, code, tool activity, citations, and link cards against typed fixtures while keeping raw HTML off by default.
- [ ] Keep engine, model, reasoning, sandbox, and web controls out of message bubbles.
- [ ] Hold streaming token UI behind a defined bounded stream contract; do not infer deltas from native engine events.

**Exit gate:** public-protocol tests cover send, duplicate retry, queued/steered outcome, approval, question response, cancel, completed assistant output, reconnect, and a fixture contract for history and streaming gaps.

## Checkpoint 7 — Deliver the Orchestrator Workspace

**Status: Ready for known graph IDs and core controls; fixture-ready for cross-surface details.** Graph queries, subscriptions, commands, agents, assignments, runs, joins, artifacts, and heartbeat status are public. Thread-to-group discovery after a fresh UI start is not.

- [ ] Render coordinator, worker, dependency, join, retry, and synthesis topology from the canonical graph rather than reconstructing it from transcript timing.
- [ ] Use the same two-line identity/status language as thread rows: stable playful name, then specific present-tense work.
- [ ] Add graph and list views, selected-node details, runs, joins, heartbeats, and artifacts from the live graph.
- [ ] Keep agent thread access, agent-linked terminals, files touched, approvals, open-diff, promoted findings, and summaries fixture-backed until their cross-surface contracts exist.
- [ ] Wire rename, steer, stop, pause, resume, retry, cancel, close, answer, approve, and summarize actions where the public command supports them.
- [ ] Offer write-capable assignments only in the shared visible workspace; do not expose the protocol's isolated-workspace variant because the PRD's no-worktree rule is a product guarantee.
- [ ] Make blocked, waiting, joining, interrupted, failed, and complete states visually distinct without depending on color alone.
- [ ] Add live keyboard and command-palette actions for supported controls such as steer and stop; present open thread, open diff, promote finding, and summarize now only when a corresponding contract exists.

**Exit gate:** graph snapshot and ordered patch tests preserve selection, topology, terminal lifecycle states, command receipts, and reconnect replay without leaking provider-native agent names into primary UI. The checkpoint remains partly integrated until a thread can discover its existing graph IDs after restart.

## Checkpoint 8 — Build the Right Session Pane

**Status: Mixed.** Terminals and compact orchestration are ready; engine/model session controls, Git, ports, preview health, usage, and permission summaries need contracts.

- [ ] Create collapsible sections for session controls, branch and changes, agents, terminals and processes, ports and previews, permissions, and usage.
- [ ] Render terminal lifecycle and compact agent status from live projections.
- [ ] Use disabled, unavailable, loading, and unsupported capability states instead of controls that appear to work against fixtures.
- [ ] Keep low-frequency engine, model, effort, sandbox, permission, and web/search controls here once the backend exposes one canonical session policy.
- [ ] Keep branch, diff summary, and changed files tied to one workspace projection so the pane cannot show contradictory Git and Artisan state.

**Exit gate:** fixture tests cover every section and capability state; live tests cover terminals and agents; no unavailable control sends a fabricated generic command.

## Checkpoint 9 — Integrate Monaco, Files, Changes, and Review

**Status: Backend moving for controlled text files and changes; fixture-ready for Monaco; blocked for file discovery, language services, binary previews, and Git mutations.**

- [ ] Embed Monaco behind an editor adapter that owns models, view state, diagnostics, and disposal without leaking Monaco objects into domain components.
- [ ] Wire safe text reads, conditional replacement, changed-file lists, review, and rollback only after the backend checkpoint verifies their typed client and production composition.
- [ ] Show before/after identity conflicts and external changes as explicit reconciliation states; never overwrite because a local editor model is newer.
- [ ] Build side-by-side and inline diff views with change and agent attribution, review state, rollback availability, and clear unsupported states for binary files.
- [ ] Add save, format, search, go-to-line, breadcrumbs, diagnostics, and accessible editor commands in separately verifiable slices.
- [ ] Keep Git status and mutations out of this checkpoint until their read model and approval-bearing command surface exist.

**Exit gate:** tab and Monaco model lifecycles are leak-free; controlled replace is conditional and conflict-aware; review and rollback reconcile from backend events; external changes never cause silent data loss.

## Checkpoint 10 — Integrate Git

**Status: Blocked.** Internal Git services and content-free affinity evidence exist, but the renderer has no authoritative Git query, projection, or mutation contract. `git.workspace.observed` is not a Git UI read model.

- [ ] Define the right-pane branch, head, worktree, staged, unstaged, untracked, conflicted, and aggregate diff projection for the one visible checkout.
- [ ] Add bounded status and diff queries plus subscriptions that reconcile with Artisan's changed-file ledger.
- [ ] Add explicit approval-bearing commands for branch creation and checkout, stage/unstage, commit, push, pull-request creation, and any destructive action that V1 deliberately supports.
- [ ] Show external Git changes and stale snapshots as reconciliation states rather than guessing from filesystem watchers.
- [ ] Keep Git functional in non-Git workspaces by clearly separating Artisan changes from optional Git integration.

**Exit gate:** branch, changed files, diff summary, and review surfaces describe the same checkout after external changes and reconnect; no Git mutation is smuggled into a read API or executed from the renderer.

## Checkpoint 11 — Integrate Terminals, Processes, and External Previews

**Status: Ready for terminal lifecycle and bytes; fixture-ready for processes; blocked for production preview integration.**

- [ ] Render live terminal identity, thread/workspace IDs, executable and arguments, cwd, dimensions, generation, pid, lifecycle, exit/failure details, timestamps, and bounded output.
- [ ] Keep friendly display names, agent/run ownership, pinning, and handoff unavailable until those fields and commands are public.
- [ ] Wire input, resize, clear, restart, kill, and close through canonical terminal commands and a dedicated stream port; keep terminal handoff unavailable until it has a public command.
- [ ] Virtualize or otherwise bound terminal and log rendering so sustained output cannot stall control actions.
- [ ] Model process/dev-server rows and ports against fixtures without reading the process table from the renderer.
- [ ] Add fixture states for preview-target discovery, health, logs, URL state, and external-browser launch; keep them disabled until public preview routes exist.
- [ ] Add rich-link metadata, cache, redirect, fallback, private-network blocking, favicon, and asset states; `OpenAsset` is usable only after a public projection supplies an asset ID.
- [ ] Keep native file, Markdown, image, and diff previews in the main pane; do not embed application WebViews.

**Exit gate:** real MessagePort tests prove stream ordering, overflow/gap behavior, reconnect, input, resize, restart, kill, and control responsiveness under output load; preview controls stay disabled until backend health and launch contracts are live.

## Checkpoint 12 — Build Guidance and Curated Settings

**Status: Ready for global guidance, Model Behaviour, and retention; fixture-ready for remaining preferences.**

- [ ] Build Global Guidance selection, editing, provider status, drift comparison, import, overwrite, ignore, retry, and privacy-aware errors from the typed snapshot.
- [ ] Build Model Behaviour settings from registry metadata, including provider support, scope, activation timing, unavailable/unsupported state, drift actions, and retry.
- [ ] Build thread-retention settings from the canonical policy.
- [ ] Add appearance, motion, keyboard shortcut, notification, agent name-bank, and local pane preferences as explicitly frontend- or backend-owned settings.
- [ ] Do not expose arbitrary provider configuration as a generated settings form.

**Exit gate:** live tests prove update receipts, drift races, reconnect replay, unsupported providers, and error recovery; secrets and full provider config never enter renderer logs or persisted frontend state.

## Checkpoint 13 — Build Marketplace

**Status: Blocked.** The Routine and Capability registries, installation previews, approval, sync, drift, auth, health, and invocation contracts are missing.

- [ ] Design the Marketplace landing view directly under `New chat`, with separate `Routines` and `Capabilities` sections using canonical Artisan vocabulary.
- [ ] Fixture Routine details for source, version, scope, permissions, compatibility, files, progressive disclosure, enable/disable, remove, sync, and drift.
- [ ] Fixture Capability details for stdio/HTTP transport, auth, tools/resources, scope, health, policy, approvals, connection, sync, and uninstall.
- [ ] Fixture install preview and approval with exact source, file writes, permissions, trust, scope, provider effects, and rollback plan.
- [ ] Do not execute package-manager, Git, OAuth, MCP, or provider-config actions from the renderer.

**Exit gate:** design and fixture tests may complete, but the checkpoint remains unchecked until every mutation is approval-bearing, ledgered, rollback-aware, and integrated through a public backend contract.

## Checkpoint 14 — Add Resilience, Accessibility, and Activity Personality

**Status: Fixture-ready.** `ArtisanClient` exposes errors and cursors and handles reconnect internally, but it does not expose an authoritative negotiating, reconnecting, replaying, current, or stale lifecycle stream.

- [ ] Add one connection-state model for negotiating, connected, reconnecting, replaying, stale, and failed states after the typed client exposes those facts; do not infer them from timers or absence of events.
- [ ] Preserve drafts, tabs, selections, and scroll state through backend reconnect without presenting stale projections as current.
- [ ] Surface sequence gaps and snapshot fallback as quiet recovery UI, with destructive actions disabled only when an authoritative client state says projections are not current.
- [ ] Add global keyboard navigation, command palette, skip/focus landmarks, accessible names for icon buttons, live regions for meaningful status, and non-color state cues.
- [ ] Test reduced motion, screen-reader order, high contrast, 200% zoom, long localized text, empty data, large data, and rapid event updates.
- [ ] Implement the PRD's status-personality precedence with safe, observable labels, a low-frequency pulsing Artisan mark, a declared sprite manifest, and deterministic reduced-motion still frames.
- [ ] Keep Dock, taskbar, overlay, and tray activity fixture-only until an aggregate backend activity projection and Electron shell capability exist.

**Exit gate:** keyboard-only workflow covers new chat through send/steer/review; automated accessibility checks are clean; reconnect tests prove local-state preservation and projection reconciliation.

## Checkpoint 15 — Wire the Electron Shell and Package Boundary

**Status: Blocked on the real shell bootstrap.** The structural MessagePort adapter exists, but Electron main, utility, renderer, packaging, single-instance ownership, and restart equivalence do not.

- [ ] Add a desktop-shell module that brokers control and stream MessagePorts between the renderer and one backend utility process.
- [ ] Keep Electron types and APIs out of the frontend module; expose only the shell-neutral typed session bridge.
- [ ] Add secure window defaults, external-link policy, protocol negotiation, crash reporting boundaries, single-instance behavior, and deterministic shutdown.
- [ ] Add a shell-owned activity indicator for macOS Dock, Windows taskbar, and best-effort Linux/tray integration only after the backend exposes one aggregate activity projection; keep detailed truth inside the app.
- [ ] Prove backend utility-process restart, renderer reconnect, replay, stream recovery, and resource cleanup in packaged tests.
- [ ] Add icons, installer/update policy, platform metadata, and release artifacts only after process-boundary equivalence is green.

**Exit gate:** the same protocol fixtures pass in process and across packaged Electron; no renderer code gains filesystem, process, shell, or arbitrary IPC authority.

## Checkpoint 16 — First Complete Frontend Milestone

**Status: Blocked on Checkpoints 0–15 and backend completion.**

- [ ] Run the complete repository validation and frontend-specific type, lint, unit, component, accessibility, layout, and packaged-process suites.
- [ ] Exercise one public-protocol scenario from thread creation through chat, orchestration, terminal activity, controlled file change, review, rollback, reconnect, and final summary.
- [ ] Verify that sidebar, main tabs, right pane, changed files, Git state, terminal state, and orchestrator all describe the same thread and workspace.
- [ ] Verify that every unsupported backend capability is visibly unavailable rather than simulated.
- [ ] Perform an independent standards review, PRD review, import-boundary review, keyboard pass, reduced-motion pass, and resource-leak pass.

**Exit gate:** the frontend is a truthful client of the production backend, survives process restart without contradictory state, and completes the core developer workflow without a terminal or external editor standing in for unfinished UI.

## Current Critical Path

The productive frontend sequence is `0 → 1 → 2 → 3 → 4 → 5`, then `6`, `7`, and the live portions of `8` can proceed in parallel by file ownership. Monaco can begin behind fixtures during that work, but live file/change integration waits for the backend session's verified mutation-service checkpoint. Marketplace and Electron packaging should stay behind their missing contracts rather than shaping the foundation around guesses.
