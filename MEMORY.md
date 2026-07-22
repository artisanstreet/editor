# Artisan Editor Memory

Last updated: 2026-07-22

## Mission

Build the complete Artisan Editor backend/core and deep testing harness described by `artisan-editor-prd.md`. The backend is provider-neutral, Effect service/layer based, SQLite/Drizzle backed, and connected to a UI-only frontend through typed MessagePort RPC/events. Codex CLI is the sole production engine during prototyping; the generic Engine seam and fake harness remain so future adapters can be added deliberately after the editor/core is proven. Provider-native data is normalized while raw origin remains attributable.

This project is not complete until the final audit in `backend-completion-matrix.md` is proven requirement by requirement.

## Working State

- [x] Repository: `C:\Users\Sander\Desktop\artisan-editor`
- [x] Branch: `codex/backend-services`
- [x] Package manager: pnpm 11.7.0
- [x] Stack: TypeScript 7, Effect 4 beta, Drizzle 1 RC, SQLite
- [x] The last pushed checkpoint before the full completion program was `9360425 docs: record backend git checkpoint`; the final completion checkpoint is recorded after the integration commit below.
- [x] Local `HEAD`, upstream, and `origin/codex/backend-services` were equal at `93604252b0fede84a67bd30f603b2b95d38eeb7a` before the full completion program began.
- [x] `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 pnpm --filter @artisan/bounded-file-store-native verify:local` is the canonical native gate and passes locally for production reads/replacement, test-hook races, and process-crash recovery.
- [x] Routine development verification is local-first. Do not recreate temporary GitHub Actions, remote runners, or similar testing detours; future CI is a real clean-checkout/release gate and never a substitute for local validation.
- [x] Reconfirmed on 2026-07-13: the leftover empty `.github/workflows` directory was removed and the canonical native gate passed locally. The earlier BSOD remains classified as a one-off storage-driver incident, not a reason to move routine testing off-machine.
- [x] Model Behaviour is committed as focused protocol, persistence, provider, service, composition, transport, and security changes.
- [x] Local GitHub account: `sandersonstabo`; `origin` -> `https://github.com/sandersonstabo/artisan-editor.git`; GitHub visibility was verified as `PRIVATE` on 2026-07-12.
- [x] Commit every coherent, verified checkpoint as a small, focused, independently understandable change. Never bundle unrelated dirty work into the same commit and never push `main` or `master` without explicit approval.
- [x] Push the current feature branch to `origin` after every coherent commit and at natural checkpoints. Local worktrees are temporary working state, not durable project storage.
- [x] Check local and remote state at session start, after each push, and before handoff. Confirm the branch tracks `origin` and that local `HEAD` equals its upstream after every push.
- [x] The PRD now makes one visible shared checkout and selected branch a product guarantee. Artisan must coordinate writers through durable mutation claims and must never create hidden worktrees, per-agent branches, or temporary commits.

## Native Mutation Incident And Local Authorization

- [x] The user explicitly classified the BSOD as a one-off on 2026-07-12 and authorized local native execution. The prior host safety hold is lifted; keep the explicit `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1` gate so ordinary validation never loads the addon silently.
- [x] Windows bugchecked at 08:45:50 with `0xD1 DRIVER_IRQL_NOT_LESS_OR_EQUAL`: `0x000000d1 (0x0, 0x2, 0x1, 0xfffff8052abb750f)`. WinDbg analysis of the preserved copy at `C:\Users\Sander\Desktop\071226-17781-01.dmp` attributes the null write directly to `rcbottom.sys+0x750f`, AMD-RAID Bottom Service version `9.3.3-00218`; the captured stack contains no Node, N-API, NTFS, or Artisan frame.
- [x] The dump's failure bucket is `AV_rcbottom!unknown_function`. `rcbottom.sys` is the boot storage driver used by this machine's actual RAID-backed disks, so it must not be disabled or removed as a workaround. Driver or BIOS changes require an explicit human decision, a current backup, and the motherboard/AMD recovery procedure.
- [x] Correlation with the native harness remains strong: generated addon files were written at 08:45:41, the first concurrent replacement root was created at 08:45:43, and that unfinished root retained a valid 1 MiB stage/backup/published-target receipt after reboot. Preserve `C:\Users\Sander\AppData\Local\Temp\artisan-native-replace-5hTN29`, the copied dump, and `C:\Users\Sander\Desktop\071226-bounded-file-store-native-gnu.node` (`SHA256 2FA0F3DE54816E5DABDD84E4A392E18E3A57BF558F48BBBDD6D43919C480E2E3`) as incident evidence.
- [x] Independent static ABI review found no Rust memory corruption, dangling handle, double close, or malformed EA record. It found four-byte-short `FILE_RENAME_INFORMATION` and `FILE_LINK_INFORMATION` allocations plus an incorrect secondary read of `IO_STATUS_BLOCK.Status` after synchronous `NtFlushBuffersFile`; all three are fixed in the accepted native implementation.
- [x] The corrected native crate passes `cargo fmt`, GNU default/test-hook `cargo check`, and GNU default/test-hook Clippy with warnings denied without loading or executing the addon.
- [x] Fresh independent reviews of the corrected native implementation, authenticated foreign-operation handling, and local build gate report no remaining P0-P2 finding.
- [x] `build:local:gnu` uses isolated GNU release outputs. `verify:local` runs production read/full replacement first, then one test-hook build for deterministic races and every crash window; all addon execution remains explicit opt-in.
- [x] The authenticated native mutation implementation is accepted for fixed local NTFS and process-crash recovery through the local gate. This does not claim power-loss atomicity or universal filesystem support.

## Active Work

- [x] The full authorized Barekey frontend style foundation is now vendored verbatim from `usebarekey/barekey` checkpoint `6b51c66`: all 13 style files (global theme, sidebar, prose, markdown, snippets, and shared Tailwind reference), all 26 font assets, and the typography plugin dependency. SHA-256 comparison confirms every copied style file matches upstream. Artisan-only editor/status aliases are isolated in `artisan-compatibility.css` so the copied foundation remains pristine and updateable.

- [x] Second installed-app iteration is complete: the original three-column prototype is replaced by a persistent Barekey-derived inset sidebar, a recent-thread welcome route at `/`, routed workspaces at `/thread/<id>`, and one anchored scrolling settings document at `/settings`. The identity dropdown links to settings, thread inspectors expose only contextual Session/Changes/Tools, and the user-authorized Barekey mark is paired with the Artisan Editor name. The signed installer was rebuilt, passed the isolated packaged smoke with a real `/thread/thread_...` route, installed silently, opened from `C:\Users\sander\AppData\Local\Programs\artisan-editor\Artisan Editor.exe`, and was visually inspected in the running installed app.
- [x] The desktop dependency-bundling defect is closed systematically: Vite bundles non-Electron dependencies into privileged desktop output, the release gate rejects unresolved bare package imports in the real ASAR, and packaged smoke copies the unpacked app outside the checkout before launch so parent `node_modules` cannot mask missing dependencies. TypeScript, focused gates, desktop/preload builds, and the isolated packaged smoke pass.
- [x] The stuck initial transcript cause is fixed in the live store: thread-list subscription snapshots are authoritative, an empty list enters the empty phase, first upsert selection becomes ready, and selection changes hydrate work/transcript/groups plus subscriptions. The focused live-store suite, TypeScript, and frontend build pass.

- [x] The live renderer completion pass now covers authoritative Codex-only session policy, fresh first-message and strict-clarification routing, scoped terminal-output watcher ownership/fencing, session-policy-derived built-in tool availability, Monaco file discovery/read/conditional save, live transcript interactions and rich links, orchestration fan-out controls, Marketplace Routine/Capability lifecycle, terminal/preview/Git/change/settings controls, and desktop identity. An independent P0-P2 review found and the implementation fixed three release defects: first messages were blocked without an existing run, watcher fibers survived selection changes, and tool discovery used an over-permissive hard-coded policy. The focused live/frontend/policy batch passes 30 tests and root TypeScript is green.
- [x] The PRD working-state personality now has an original generated bitmap sprite, a validated RGBA manifest, observable canonical-surface labels, safe playful fallback words, compacting/waiting modes, and an explicit reduced-motion still/plain label. It is rendered through shadcn Card/Badge primitives and existing semantic tokens. The asset/safe-label/component-scope/tool-policy suites pass 10 tests; the production frontend build passes.
- [x] Packaged renderer diagnostics proved the custom protocol serves the full static payload and exposed three real integration faults: the entry URL routed SvelteKit to `/index.html` 404, the preload build needed CommonJS, and Electron primary-frame validation had to use `senderFrame === sender.mainFrame`. The accepted packaged gate now passes trusted keyboard and native mouse interaction, taskbar activity, focus restoration, responsive layout, 200% zoom, utility replacement, reconnect/replay, forward progress, and two-epoch native loading.
- [x] SER component effects now receive one explicit app-lifetime `Scope.Scope` from the production ManagedRuntime layer. `LiveWorkspaceStoreLive` remains `Layer.effect`, which already supplies its construction scope in Effect 4 beta; the nonexistent `Layer.scoped` API was rejected after inspecting the installed Effect source. This closes the packaged `Service not found: effect/Scope` startup crash without leaking direct Effect runners into components.
- [x] Final local acceptance on 2026-07-22: `pnpm run validate` passed formatting, lint, root TypeScript, the production frontend build, 155 test files, 1,205 passing tests, and 2 explicit skips. `pnpm run package:desktop` produced the signed unpacked application and `Artisan Editor Setup 0.1.0.exe`; `pnpm run verify:desktop-package` then passed against that exact artifact with mandatory temporary-data cleanup.
- [x] The complete Codex-only backend, Electron, frontend, and release checkpoint is committed at `f8a6efa` (`feat: complete Codex-only desktop editor`). The evidence follow-up is `1f57c63`; immediately after push, local `HEAD` and `origin/codex/backend-services` were equal at `1f57c63c38756ee665c30f5beed9fcf2e8886164`. GitHub reports this branch as the repository default, so there is no distinct base branch for a pull request.

- [x] The six completion sections are integrated: protocol ledger/rebuild, Artisan tools, preview, orchestration/canonical surfaces, Routine/MCP Marketplace, and deep release gates. The desktop uses main-brokered MessagePorts with backend/PTY/Codex lifecycle in one utility process, a narrow preload bridge, and renderer-only shell-neutral client imports. Monaco is attached to production file discovery/read/conditional save. The live three-pane frontend uses the existing shadcn-svelte primitives and semantic tokens for interaction rather than introducing a second styling system.

- [x] Root integration includes the M2 tool control plane, preview targets/assets, M5 transcript/session/surface/conflict routes, and complete Routine/MCP lifecycle APIs in one renderer-safe `ArtisanClient` and Effect Layer graph. Live frontend subscriptions/actions and the final package evidence are integrated.

- [x] M9/M10 include deep public-surface traversal, generated Engine state machines, joined public-protocol workspace/rebuild integration, Windows clean-checkout workflows, stable authority identities, canonical temp roots, and fixture isolation. The former string-only desktop assertion is replaced by actual ASAR/unpacked inspection plus a forced packaged utility restart/reconnect/replay/native-load/mounted-UI gate, so M8-M10 are complete.

- [x] M1 protocol/ledger/rebuild coordination completed the owned implementation on branch `m1-protocol-ledger-rebuild` from shared checkpoint `9360425`. Live protocol ingress is exhaustive, the generic command transaction contract is explicit, and the supported public projections rebuild deterministically through a backend/admin-only Effect Service. Marketplace, preview behavior, Electron UI, and orchestration product semantics remain outside this section.
- [x] M1 rebuild/equivalence status and failures are an operational backend/admin concern. The PRD does not currently require a user-visible maintenance state, so the rebuild service must remain backend-owned and test-visible without a renderer protocol/client route. If a future PRD requirement makes recovery state visible, expose only a renderer-safe projection rather than persistence or administrative internals.
- [x] Deterministic public-projection rebuild is accepted after resolving every independent P0-P2 finding. `ProjectionRebuildService` verifies/rebuilds Threads, WorkspaceChanges, and GitWorkspaceProjections inside a connection-held Drizzle transaction whose first write acquires the singleton SQLite rebuild lock. It validates global/stream continuity, payload schemas/types, Git journal cursors, thread activity, legacy provenance, erasure, and exact stored equivalence; corruption/interruption rolls back. Eight focused rebuild scenarios prove deletion repair, legacy recovery, stale cleanup, Git corruption refusal, true two-runtime contention, restart/private-state preservation, and deterministic interruption. Erasure removes legacy provenance and cannot resurrect content. The service is backend/admin-only and intentionally does not claim operational/private/external state as a rebuildable projection.
- [x] The generic transactional command acceptance gate proves one SQLite transaction commits command dedupe, canonical event, stream cursor, and thread projection; exact retries replay without state change and rejected changed intent rolls back. Existing specialized workspace/Git/guidance/Model Behaviour repositories retain their domain-specific durable operation identities and transaction tests; no production defect was found in the accepted implemented-domain audit.
- [x] A time-bound existing workspace-evidence restart test was stabilized by injecting a fixed retention clock after its 2026-07-11 fixture crossed the seven-day policy boundary. The isolated evidence/rebuild/transaction/erasure batch passes 19 tests; TypeScript and formatting checks pass. The first full validation attempt exposed this clock regression, and the corrected full rerun passes.
- [x] M1 rebuild implementation is committed and pushed at `1f8bd6f`; the deterministic retention-clock regression fix is committed and pushed at `821c2ed`. Final `pnpm run validate` passes formatting, Oxlint, TypeScript, the production frontend build, 106 test files, 898 passing tests, and 2 intentional skips. Independent final P0-P2 re-review is clean. Immediately after each push, local `HEAD` matched the tracked upstream; after the clock-fix push both resolved to `821c2ed14f111aa348f9ae1a1efec26b715a854b`.
- [x] M1 envelope-router ownership is committed and pushed at `3b7a7ac`. `ProtocolServer.Receive` now decodes every inbound control frame through `ProtocolRouter.ClassifyInbound`, routes classified commands without a second decode, and uses a compile-time exhaustive ready-envelope handler instead of silent fallthrough. Focused protocol verification passed 21 tests; backend TypeScript and diff checks passed; independent P0-P2 re-review is clean. Immediately after push, local `HEAD` and `origin/m1-protocol-ledger-rebuild` both resolved to `3b7a7ac56ed5de34ec5ee4695c7dd6d8c8b32d52`.
- [x] M5 orchestration and canonical-surface coordination began from checkpoint `9360425`, audited the intake, steering, usage, and conflict/review gaps, and completed all four slices with independent review and final integration evidence.
- [x] M5 intake has provider-neutral risk assessments, pre-run questions with no engine outbox, low-risk assumptions, restart-safe clarification resolution, project mentions/raw origin, thread-local auto-steer preference, strict answer authorization, durable constraints, erasure cleanup, independent review, and renderer wiring. M5 is complete.
- [x] Final canonical-surface P2 fixes add SQLite-assigned cross-run projection ordering and validate every public item before persistence. Unsafe provider provenance becomes a schema-validated opaque native-action surface while the raw ledger retains forensic data. Unknown usage bases invalidate only their claimed metrics, and aggregate reads reject corrupt counts. The focused suites, root checks, migration inspection, integration, and final full validation pass.
- [x] Fresh renderers now have typed authoritative transcript history/live append and thread-scoped orchestration-group discovery/live replacement projections. Latest-safe pagination filters before limiting, continuation cursors are explicit, transcript and group erasure clear active subscribers, exact-thread filters prevent cross-thread patches, per-subscription and atomic snapshot watermarks close replay/setup races, and rapid notifications cannot duplicate future rows. Renderer access stays behind `ArtisanClient`; fixture pagination and terminal filtering match live semantics. Independent final P0-P2 review is clean after the atomic group-snapshot boundary regression passed.
- [x] M5 intake was independently re-reviewed and fixed for terminal-run clarification replacement. Strict Work/Time/Guidance/Routine/Capability/Process/Change/Permission/native-action schemas retain safe raw-origin references; Codex app-server totals are cumulative, exec usage is delta, and invalid token counts fail. Durable surface/usage projections and renderer client wiring are complete.
- [x] M2 Artisan-owned tool control-plane slice is complete on branch `m2-artisan-tool-control-plane`. The canonical 20-tool registry, policy/permission routing, durable invocation and approval recovery, observable lifecycle/usage/evidence projections, controlled files, terminal/process controls, narrow Git stage/unstage, questions, assumptions, native-action normalization, and truthful unavailable preview/language seams are composed through Effect Services and Layers. Public protocol routes and the renderer-safe `ArtisanClient` expose registry, execute, approval resolution, invocation/approval lists, bounded root-confined workspace discovery, and language capabilities; MessagePort and import-boundary tests prove the renderer never enumerates the filesystem. Preview implementation internals and Marketplace/MCP registries remain separate owned slices. Verification on 2026-07-18: focused control-plane/evidence/protocol/transport coverage passed 8 files and 31 tests; full `pnpm run validate` passed formatting, lint, TypeScript, the production frontend build, 115 test files, and 922 tests with 2 intentional skips. Independent P0-P2 review and post-fix re-review are clean.

- [x] Preview backend completion is integrated on `preview-backend-completion`. Durable SQLite targets, inspection sessions, exact commands, journal projections, restart recovery, and owner-bound cross-runtime dispatch leases cover process/terminal source attribution, declared ports/routes, health, external launch, reconnect/error states, cleanup, and erasure fencing. The fail-closed journal trigger admits post-claim completion only when correlation and causation match the active lease and its event kind.
- [x] Production preview runtime composition uses a literal-loopback-only bounded HTTP(S) health probe, shell-free OS external-browser launch, and an explicit injectable inspection connector lifecycle. Artisan never embeds a browser or WebView. Rich-link resolution retains pinned-address redirect/private-network defenses and content-addressed public favicon assets.
- [x] Renderer-safe protocol and `ArtisanClient` operations expose preview listing/query/mutation, health probes, state/removal, rich links, asset metadata/bytes, external launch, and inspection open/inspect/close. Durable live events are exactly `preview.target.updated` and `preview.inspection.updated`; launch state is carried by the target projection/event. Asset not-found and source-failure frames are distinct and do not poison the MessagePort session.
- [x] Preview verification includes deterministic restart/idempotency, failed-launch replay without a second opener call, four two-runtime lease barriers, claimed-thread erasure fencing, connector cleanup, contiguous live events, loopback security, binary capability transport, and continued-stream error recovery. Independent final review is clean after the raw unrelated-event bypass regression was fixed and covered.
- [x] Final preview integration verification after rebasing through shared checkpoint `69869a1`: root TypeScript and a combined preview/erasure/protocol/transport/evidence batch pass 71/71. Exact `pnpm run validate` passes formatting, lint (two pre-existing warnings), TypeScript, and the production frontend build, then completes 109/115 test files; the remaining failures are cross-section dependencies outside preview ownership (three Model Behaviour filesystem tests exceeding the 15-second Windows timeout and six deterministic frontend/source-layout assertions against concurrent UI integration). Preview completion status is based on the green owned/integration evidence, not those unrelated failures; the root coordinator must reconcile the frontend checkpoint and rerun the full gate.
- [x] Preview implementation is committed at `3e1dc43`; completion evidence/integration formatting is committed at `a7cb1ab`. Both commits were pushed to `origin/codex/backend-services`, and local `HEAD` equaled the upstream ref at `a7cb1ab475ce8cfdab152c462bc4dc2aba52bd11` immediately after push.
- [x] Marketplace M7 implementation is complete. Canonical SQLite Routine and MCP Capability registries now own scoped summaries/details, source/version/files/permissions/compatibility/trust, lifecycle/approval/replay, rollback/uninstall, sync/drift/runtime-only fallback, OAuth/token status, health/discovery/policy, invocation metadata/artifacts/ledger, and explicit crash recovery. Production Routine inspection uses bounded no-follow identity-checked reads and atomic receipt-bound local install/rollback; `npx skills` is an inert, bounded discovery adapter whose returned candidate includes the opaque fingerprint required for canonical reinspection/import. MCP stdio and HTTP sessions are scoped, bounded, secret-reference-only, and never acquired at Layer construction or backend startup. Renderer access is only through the scoped protocol and `ArtisanClient`; invocation approvals return correlated visibility metadata, OAuth begin returns a safe URL/opaque continuation, and OAuth completion/refresh/revoke atomically update canonical secret-reference auth state. On 2026-07-19, format, lint, TypeScript, frontend production build, and the explicit 13-file Marketplace/fixture suite passed (97 tests); independent re-review closed all P1 findings. Provider-native file writers and OS vault/browser presentation remain injectable platform/cross-section adapters; absent adapters fail closed or report `runtime_only`, never silently write/connect.
- [x] The former cross-section workspace-evidence retry regression is resolved. Final full validation on 2026-07-22 passed 155 files with 1,205 tests and 2 explicit skips.
- [x] M7 Effect capability research against pinned `effect@4.0.0-beta.97` found reusable Scope/acquire-release, Schema, Stream/Sink/Queue, Schedule/retry/timeout, Redacted/Config, experimental HTTP client, and generic experimental process APIs. Effect ships MCP server support but no outbound MCP client, OAuth/token lifecycle, OS secret vault, or Marketplace policy/ledger. The stdio MCP client is therefore a custom scoped Effect Service over Artisan's hardened process boundary; HTTP, OAuth, secret storage, source installers, provider translators, and MCP client protocol remain narrow injectable boundaries with deterministic fakes.
- [x] The renderer-safe Marketplace protocol foundation is committed and pushed at `531ffb1`. It defines scoped Routine/Capability summaries and selected detail, category/search filters, source/trust/permissions/compatibility/files, pure fingerprint-bound install/connect previews, npx discovery/import without canonicalizing its format, lifecycle/rollback/sync/drift/OAuth/invocation requests, secret-reference-only MCP auth, and canonical `marketplace.lifecycle` event payloads. Focused codec coverage passed 8 tests; local `HEAD` and `origin/m7-marketplace` were equal at `531ffb16d209a2c363b625cae971e30864cf64fd` immediately after push. This is a protocol checkpoint only and does not mark any M7 matrix row implemented.
- [x] Secure MCP transport infrastructure started at `24df8b1` and is now fully integrated with the registry/service/protocol/client/frontend lifecycle. Default Layers remain inert and deny unavailable secret/OAuth/transport access. Explicit stdio and HTTP sessions are bounded, scoped, policy-checked, and covered for crash, timeout, pagination, cleanup, and reconnect behavior.

- [x] M9 architecture/surface normalization gate added on `m9-deep-harness-release`: the deep test traverses complete protocol, Engine, and renderer-safe transport-client public import graphs, rejects layer inversions and host/database leakage, verifies unique normalized renderer surface ownership, and keeps unavailable M7/M8/Electron domains explicitly blocked or fixture-only. Focused verification passes 5 tests and `pnpm run check`.

- [x] Restored a deterministic green Windows validation gate without raising the 15-second test timeout or weakening coverage. The affinity restart fixture now injects its fixed retention clock, timing-free binary-stream tests deterministically prove both server logical-queue and client consumer-queue overflow, and Windows runs Vitest files through one worker so PowerShell ACL operations and Codex process hosts cannot starve each other. The formerly failing eight-file regression batch passes 78 tests with two intentional skips, and full `pnpm run validate` passes formatting, lint, TypeScript, the production frontend build, 93 test files, and 788 passing tests plus two intentional skips.

- [x] Removed the concrete Claude Code execution adapter, its exports, fake CLI, transcript, and adapter-specific tests. Codex app-server with exec fallback is now the sole production Engine integration during prototyping. The provider-neutral Engine contract, process boundary, deterministic fake adapter, transcript replay, and Codex conformance coverage remain. Claude guidance-file discovery/import remains a separate provider-config feature and does not imply executable engine support. Verification on 2026-07-18: the focused engine suite passed 13 files and 102 tests with one intentional live probe skipped; after the deterministic Windows gate repair, full `pnpm run validate` passes 93 files and 788 tests with two intentional skips.

- [x] Added the private `@artisan/data` workspace module with 268 curated one-word Norwegian female agent names and 70 independently curated Artisan thinking words, including the exact semi-viral `Muhammading` reference spelling. The names include `Linnea` and `Elise`, use `Martha` instead of `Marta`, and pass JSON, uniqueness, NFC, single-word, package-discovery, formatting, and diff checks. Claude spinner references remain taste/provenance only because the published compilations are unlicensed or non-commercial and the PRD forbids blindly shipping them. Full `pnpm run validate` passes formatting, lint, TypeScript, the production frontend build, and 98 test files with 837 passing tests plus 3 intentional skips.

- [x] Immutable workspace diff generation, persistence, recovery, validation, and migration safety are committed and pushed at `07033aa`; the public query route and truthful unavailable/corrupt errors are committed and pushed at `3dadcaa`.

- [x] Durable workspace change command/projection persistence is committed and pushed at `e7048cb`.
- [x] Transactional SQLite rollback snapshots are committed and pushed at `3eba329`.
- [x] Transactional run/agent/workspace authority is committed and pushed at `7cbe507`.
- [x] Recovery-only access to staged rollback bytes is committed and pushed at `b81fa19`.
- [x] The narrow `BoundedRegularFileStore` seam and explicitly non-adversarial Node adapter are committed and pushed at `98ef0b3`, `011dff5`, and `720cfaa`. Two-phase receipts retain the exact stage and original backup until SQLite durably records `applied`, then finalization removes them idempotently. Crash, corruption, mode, artifact, and concurrency coverage is green, including 50 synchronized exact-ID and 50 competing-ID stress runs. Independent final P0-P2 review is clean.
- [x] The private `@artisan/bounded-file-store-native` N-API package scaffold is committed and pushed at `dfc08bb`. Rust/NAPI versions are pinned, production output targets Windows x64 MSVC under `.dist`, and the pinned GNU development build proves direct binding load, generated CommonJS loader load, and generated declaration consumption without exposing any filesystem method. Independent P0-P2 review is clean.
- [x] Native Windows pinned-root bounded reads are committed and pushed at `e915763` and `d5ec0bc`. The N-API class accepts only absolute fixed-drive local NTFS roots, resolves children through open directory handles, rejects reparse/private/alias/multiply-linked paths, denies concurrent writers/deletion, and gives in-flight async reads an exact root lease across `close()`.
- [x] Native exact-handle conditional replacement and finalization on Windows local NTFS is complete and accepted with authenticated receipts, process-crash recovery, metadata preservation, deterministic race coverage, and best-effort flush hardening. The contract does not claim power-loss atomicity or universal filesystem support. Koffi remains rejected for this security-critical adapter because maintaining Windows NT ABI structures and syscalls in TypeScript is too fragile.
- [x] The scoped Effect adapter and opaque workspace bounded-store registry are committed and pushed at `78164fd`. The production addon loads only during Layer acquisition, validates the exact Windows x64 MSVC non-test descriptor, adapts native results through the existing `BoundedRegularFileStore` Service, closes pinned roots through `Effect.acquireRelease`, accepts only a caller-owned stable redacted 32-byte key, and canonicalizes every workspace root before native acquisition.
- [x] Terminal optimistic-concurrency rejection is committed and pushed at `247e7b5`. `Changed` transitions remain source-free, replay exactly after restart, reject forged journal/projection state, and never return a filesystem capability through mutation authority.
- [x] Transient expected/replacement byte-pair persistence is committed and pushed at `c143986` with its deep recovery harness at `dade5c0`. The private payload table is hash/length constrained, consumed into non-resurrectable tombstones, deleted during thread erasure, and never copied into commands, events, operations, or public projections.
- [x] Thread erasure now treats every unsettled or still-available workspace mutation as a durable retention fence at `bc37306`. Complete claim and erasure transactions retry bounded SQLite contention, stale post-quiescence claims release without erasing live work, and deterministic two-runtime barriers cover Stage and MarkApplied overlap.
- [x] Rejected mutations now discard exact payload and rollback-snapshot bytes into non-resurrectable tombstones at `54feea9`, after validating canonical operation/projection identity and private-row integrity. Exact retries and concurrent two-runtime cleanup converge without exposing content.
- [x] Controlled UTF-8 reads and recoverable attributed replacements are composed into the production backend runtime at `566ceb2`, with real SQLite/repository/authority/payload/snapshot/evidence coverage at `47c28e8` and `a3c9bbd`.
- [x] The real composition harness proves accepted replacement, exact committed retry after a terminal run, applied/finalization recovery after restart without a second replacement, terminal changed replay without a projection/evidence/snapshot, portable empty-registry denial, full fake-native option forwarding, and scoped acquisition/release counts.
- [x] Review and guarded rollback execution are committed at `e68c484`, `72cd1af`, and `f8ea18c`. Rollback reuses the original replace's pinned authority after its run terminates, binds all I/O to event-validated source data, and never issues a store for duplicate or rejected outcomes.
- [x] Real SQLite composition proves review/rollback success, applied restart recovery, committed cleanup recovery, native rejection after payload staging, byte-free tombstones, snapshot preservation on rejection, and exact duplicates without a second native replacement.
- [x] Typed public workspace list/read/replace/review/rollback protocol routes and the renderer-safe `ArtisanClient` surface are committed at `c072a82` and `a333d78`.
- [x] Durable cross-process workspace-evidence idempotency is committed at `3e6302e`. New events use a nullable journal idempotency key with a SQLite unique index, legacy exact duplicates remain immutable and resolve to their earliest event, conflicting legacy intent fails closed, and a prior-schema migration fixture proves upgrade safety.
- [x] Synchronized two-runtime replacement convergence, preflight publication races, native `Changed` reconciliation, committed cleanup, and thread-erasure overlap are committed at `f5ba7e1`.

## Completed Immutable Workspace Diffs

- [x] `WorkspaceChangeDiffService` prepares strict UTF-8 unified patches before any snapshot or native replacement, verifies both source identities, copies caller bytes, and applies V1 source, line, patch-byte, rendered-line, and deterministic edit-distance limits. The synchronous jsdiff path is bounded by `min(16_384, floor(4_000_000 / max(1, before_lines + after_lines)))`; the worst accepted 1,000-by-1,000-line full rewrite has a 750 ms local regression guard.
- [x] Replacement preparation failure durably rejects and settles the operation before filesystem mutation. Applied/finalization recovery reconstructs the exact artifact from retained private payload bytes, while projection, immutable patch, journal command/event, and committed lifecycle transition are inserted atomically.
- [x] Stored patches are SHA-256 and byte-count bound to operation/projection identities, path, thread, workspace, command, format version, and context. jsdiff parse-and-canonical-format validation handles quoted Unicode paths and fails malformed, missing, duplicate, or corrupt state closed without exposing source or patch bytes in errors.
- [x] Prior committed projections upgrade to explicit `legacy_unavailable`. The migration uses inline checked `ADD COLUMN` statements instead of Drizzle's generated parent-table rebuild, preserving existing authority/payload children while migrations execute transactionally; fresh foreign keys, legacy preservation, and invalid-state checks are covered.
- [x] Thread erasure explicitly removes private diff artifacts. Public diff queries return a V1 immutable result, `workspace.diff_unavailable` for missing/legacy state, `workspace.unavailable` for erased state, and `workspace.invariant_failed` for corruption.
- [x] Focused verification passed 8 files with 115 tests. Full local `pnpm run validate` passed formatting, Oxlint, TypeScript, the production Svelte build, and 98 test files with 837 passing tests plus 3 intentional skips. Independent P0-P2 review and re-review are clean after fixing quoted-path validation and reducing/proving the synchronous work ceiling.

## Completed Concurrent Workspace Mutation Convergence

- [x] `WorkspaceChangeRepository.ReconcileChanged` resolves preflight and native changed observations transactionally against claimed, applied, committed, rejected, and exactly staged durable state.
- [x] Replace and rollback callers resume the exact private payload after another runtime publishes, finalize an applied operation, replay a committed event, or settle a terminal rejection without issuing a second native write.
- [x] Evidence publication is process-safe through one generic nullable journal idempotency key; SQLite arbitrates stale-read races while bounded Effect retry re-reads the canonical event.
- [x] The upgrade migration preserves legacy journal rows and stream continuity. Multiple exact legacy evidence rows resolve logically to the earliest event, while any conflicting row remains a hard `WorkspaceEvidenceConflict`.
- [x] Deterministic gates prove both runtimes read stale state before publication, exact replacements converge through preflight and native changed races, committed cleanup emits one evidence event, and erasure cannot bypass unsettled mutation fences.
- [x] Both synchronized replacement scenarios and both evidence/cleanup scenarios passed ten consecutive stress iterations.
- [x] Fresh independent P0-P2 review is clean after replacing the unsafe legacy partial-index migration with the nullable idempotency-key policy and upgrade fixture.
- [x] An isolated staged snapshot passed `pnpm run validate`: formatting, lint, TypeScript, production frontend build, and 95 test files with 799 passing tests plus 3 intentional skips.
- [x] Focused commits: `3e6302e fix: make workspace evidence idempotent` and `f5ba7e1 fix: reconcile concurrent workspace replacements`.

## Completed Controlled Workspace Read And Replace Composition

- [x] `WorkspaceFileServiceLive` is built at the runtime composition root from explicit Effect Services and Layers; dependency Layers remain private to service construction while the existing repositories stay independently available from the backend runtime.
- [x] Production reads use only the registry's restricted reader. Production replacements use atomic run authority, the pinned bounded mutation capability, strict UTF-8 identities, private payload/snapshot recovery, native conditional replacement/finalization, durable change projection, and attributed evidence.
- [x] The deterministic fake native module validates the complete replacement/finalization option set and preserves receipt state across runtime recreation, while real SQLite migrations and production repositories own every durable transition.
- [x] Restart tests terminalize the original base run before exact duplicate, applied, and rejected retries. Duplicate/recovery paths issue no second native replacement, and rejected retry remains source-free and side-effect free.
- [x] Local verification passed TypeScript, scoped Oxfmt/Oxlint, 24 focused service/authority/composition tests, and the full suite with 90 test files, 726 passing tests, and 3 intentional skips.
- [x] Fresh independent P0-P2 re-review is clean after strengthening durable settlement assertions, native interface fidelity, runtime disposal, portable read denial, and acquisition/release checks.
- [x] Focused commits: `566ceb2 feat: compose controlled workspace files`, `47c28e8 test: compose workspace file service`, and `a3c9bbd test: harden workspace file recovery harness`.

## Completed Controlled Review And Rollback Execution

- [x] `WorkspaceFileService` now owns idempotent review and complete rollback recovery behind the same small Effect Service interface as controlled reads and replacements.
- [x] Rollback authority strictly decodes the source operation/projection, exact-replays the original committed replace against its journal event, revalidates graph file scope, fences thread erasure, and returns event-validated source path/identities with no store on duplicate or rejected outcomes.
- [x] Rollback claims authority before reading content and uses only admission-bound source data for snapshot reads, bounded file reads, payload identity, native replacement/finalization, and anonymous frontend evidence.
- [x] The durable order is payload Stage, conditional native Replace, MarkApplied, native Finalize, CommitRolledBack, snapshot Consume, anonymous filesystem evidence, MarkEvidenceRecorded, then payload Consume. Every boundary resumes idempotently after restart.
- [x] Rejected rollback preserves the original snapshot. A native `Changed` after payload staging produces a consumed private row whose byte, count, and hash columns are all null; exact retry receives no store and performs no second replacement.
- [x] A deterministic SQLite trigger proves recovery when CommitRolledBack succeeds but snapshot consumption aborts: restart duplicate cleanup consumes snapshot/payload, records evidence, and never reruns native mutation.
- [x] Local `pnpm run validate` passes formatting, lint, TypeScript, the frontend production build, and 92 test files with 755 passing tests plus 3 intentional skips. The explicit local native addon gate also passes and no remote test workflow is retained.
- [x] Fresh independent P0-P2 review is clean after closing jointly forged source-state and pre-claim projection TOCTOU findings.
- [x] Focused commits: `e68c484 feat: authorize pinned workspace rollbacks`, `72cd1af feat: review and rollback workspace changes`, and `f8ea18c test: prove workspace rollback recovery`.

## Completed Checkpoints

- [x] Versioned protocol envelopes, negotiation, receipts, events, ACKs, replay, heartbeats, and bounded stream transport.
- [x] SQLite WAL persistence, Drizzle migrations, journal command deduplication, stream sequencing, and core projections.
- [x] Codex app-server engine with exec JSONL fallback and raw-origin normalization.
- [x] Shared engine conformance, fake process, transcript replay, lifecycle, cancellation, timeout, and cleanup harnesses.
- [x] Durable thread identity, default seven-day retention, erasure fences, restart recovery, and activity tracking.
- [x] Canonical global guidance with Codex/Claude provider-file discovery, first-run selection, backups, drift handling, Codex runtime handoff, and privacy tests. Claude provider-file support is configuration import/sync only, not an executable Engine.
- [x] First-class orchestration graph with playful names, roles, assignments, fan-out, DAG dependencies, joins, retries, steering, stop, recovery, artifacts, and heartbeats.
- [x] Terminal session service with real/fake PTY coverage, lifecycle persistence, input, resize, clear, restart, kill, and cleanup.
- [x] Engine-neutral filesystem, Git/process, preview target, rich-link, and network-policy service foundations.
- [x] Live thread metadata refinement with bounded context, latest-wins scheduling, locks, source idempotency, and restart replay.
- [x] Project affinity scoring/rehome core with Git-root discovery, recency windows, locks, suggestions, linked projects, and journal coordination.
- [x] Curated Model Behaviour registry with Codex config reconciliation, capability probing, drift handling, private backups, and typed MessagePort control.

## Completed Non-Adversarial Conditional Replacement Checkpoint

Implemented, independently reviewed, committed, and verified:

- [x] `BoundedRegularFileStore` is a narrow Effect Service for bounded reads, recoverable conditional replacement, and receipt finalization; ordinary `Filesystem` and `WorkspaceFilesystemRegistry` expose none of its methods.
- [x] The Node adapter creates and syncs a private same-directory stage, preserves the original mode, moves the exact expected target to a unique backup, and publishes with no-overwrite hard-link semantics.
- [x] Successful publication retains stage and backup receipts until explicit finalization; exact retries recover staged, moved, and published crash windows without overwriting external targets.
- [x] Finalization is idempotent only for valid receipt states, fails closed for missing, external, ambiguous, corrupt, or impossible receipts, and preserves the sole backup when proof is incomplete.
- [x] Owned artifacts are reserved from direct reads and hidden from ordinary list/watch surfaces; unrelated similarly named files remain visible.
- [x] Focused conditional and registry suites pass 31 tests, plus 50 synchronized exact-ID stress runs and 50 synchronized competing-ID stress runs. Full validation passes 78 test files, 567 tests, and 3 intentional skips.
- [x] Independent final P0-P2 review is clean after fixing stage-missing/backup-present finalization.
- [x] Focused commits: `98ef0b3 feat: define bounded regular file store`, `011dff5 feat: add recoverable node file replacement`, and `720cfaa test: prove conditional file replacement recovery`.
- [x] Production composition depends on the scoped Effect adapter around the accepted N-API class; the Node adapter is deliberately named `non_adversarial` and remains referenced only by focused deterministic harnesses.

## Completed Native Package Scaffold

Implemented, independently reviewed, committed, and verified:

- [x] Private workspace package `@artisan/bounded-file-store-native` uses NAPI-RS v3 and a pinned Rust 1.97.0 toolchain; Rust source, Cargo state, and package metadata remain isolated under `modules/bounded-file-store-native`.
- [x] Production build metadata targets `x86_64-pc-windows-msvc`; generated loader, declarations, native artifacts, and Cargo intermediates are written only beneath `.dist/bounded-file-store-native`.
- [x] A user-scoped GNU verification path uses WinLibs UCRT without treating its binary as production. It smoke-loads the direct binding and generated `index.cjs` package loader, then typechecks a consumer against the generated declarations.
- [x] The scaffold checkpoint exported only `getNativeBuildDescriptor`; filesystem capability was added later as a separately reviewed slice.
- [x] `cargo fmt --check`, GNU Clippy with warnings denied, frozen pnpm install, full repository validation, and independent P0-P2 review pass.
- [x] Local machine state: rustup 1.29.0, Rust 1.97.0 GNU/MSVC toolchains, and portable WinLibs UCRT 16.1 are installed. Visual Studio Build Tools did not install because its elevated bootstrapper could not proceed; production MSVC load remains an explicit CI/platform gate.
- [x] Focused commit: `dfc08bb build: scaffold native file store package`.

## Completed Native Pinned-Root Read Slice

Implemented, independently reviewed, committed, and verified:

- [x] `NativeBoundedRegularFileStore` pins one exact Windows root through `NtCreateFile`, permits only absolute fixed-drive roots on exact local NTFS, and owns its handle through an `Arc` lease.
- [x] Every child component is validated before normalization and opened relative to the preceding directory handle with `OBJ_DONT_REPARSE` and `FILE_OPEN_REPARSE_POINT`; traversal, absolute/device/UNC paths, ADS, invalid segments, trailing dot/space, and private artifact names fail closed.
- [x] Normalized handle names reject 8.3 aliases for `.artisan-trash` and `.artisan-conditional-*`. Multiply-linked leaves are rejected so private artifacts cannot be read through a hard-link alias and later replacement semantics remain unambiguous.
- [x] Leaf reads require a single-link non-directory, non-reparse file; keep every parent handle alive, deny write/delete sharing on the leaf, bound allocation and `ReadFile`, and compare same-handle identity and metadata before and after every read, including empty files.
- [x] N-API validates JavaScript numeric bounds before narrowing, runs disk I/O through `AsyncTask`, generates `Promise<Uint8Array>`, rejects new reads after deterministic `close()`, and lets an already-created read finish through its cloned root lease.
- [x] The opt-in GNU harness covers raw bytes, exact/invalid/oversize bounds, nested and empty files, missing/directory paths, traversal/device/ADS syntax, junction roots and parents, real 8.3 aliases, hard-link aliases, existing writers, opaque errors, repeated close, and an in-flight read surviving close.
- [x] Ten consecutive native build/loader/type/runtime smoke runs pass. Rust format, GNU Clippy with warnings denied, and full repository validation pass with 78 Vitest files, 567 passing tests, and 3 intentional skips.
- [x] Independent review found and closed initial P1 gaps for 8.3/hard-link aliases and explicit local-volume admission; final P0-P2 re-review is clean.
- [x] Focused commits: `e915763 feat: read files through pinned native roots` and `d5ec0bc test: prove native root confinement`.
- [x] Production MSVC compilation/loading passed in the historical one-off runner before that temporary workflow was removed. The maintained runtime gate is local GNU; MSVC and Electron packaging remain future release-platform checks.

## Completed Native Exact-Handle Replacement And Finalization

Implemented, independently reviewed, committed, and verified:

- [x] The N-API constructor binds one pinned root to an exact 32-byte receipt key. Replacement and finalization validate the operation id, normalized relative path, expected/replacement bytes, and byte limit before asynchronous native work begins.
- [x] Root-derived HMAC-SHA256 NTFS EA receipts authenticate the namespace, expected/replacement digests, exact file identities, peer identities, and lifecycle role. Wrong keys, corrupt markers, replayed artifacts, cross-root artifacts, and ambiguous state fail closed.
- [x] Exact-handle no-overwrite rename/link operations preserve the original backup and replacement stage until SQLite can durably record application and request idempotent finalization. Recovery covers creating-stage, staged, backup, published, finalizing, and restoring states.
- [x] Stages inherit ordinary file attributes and semantically equivalent owner/group/DACL security. Raw descriptors remain available for creation/application while canonical SDDL comparison avoids false failures from equivalent Windows descriptor encodings.
- [x] Production replacement coverage passes changed targets, validation, authenticated recovery, corrupt/unmarked collisions, tamper/replay, wrong keys, Windows hidden/read-only attributes, protected DACLs, close leases, and case-insensitive namespaces.
- [x] A deterministic foreign-operation regression proves that a second operation returns `Changed` only after authenticating the first operation's root receipt and exact self identity; wrong-key, corruption, replay, and same-operation changed-intent paths remain failures.
- [x] The test-hook build retains only proof-bearing two-party race rendezvous and crash hooks. Five same-operation and five competing-operation races pass with exact outcomes, and all eleven distinct process-crash windows recover correctly.
- [x] Local command `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 pnpm --filter @artisan/bounded-file-store-native verify:local` builds dedicated production/test-hook GNU release outputs, validates direct and generated loaders plus declarations, then passes reads, full replacement/recovery, deterministic races, and process-crash recovery.
- [x] Diagnosis-only phase tracing was removed after acceptance. Fresh independent review found no P0-P2 cleanup regression and confirmed all 13 crash call sites plus the replacement rendezvous remain unchanged.
- [x] GNU default/test-hook checks and Clippy pass with warnings denied. The local verifier restores its caller environment, rejects reparse output children, never clobbers MSVC output, and removes temporary loader aliases on success or failure.
- [x] Focused commits: `68fb9ed fix: recognize competing native receipts` and `fc3e818 test: verify native mutation locally`.
- [x] The temporary GitHub Actions workflow was removed at the user's request and must not be recreated for routine testing. The canonical native gate passed locally again on 2026-07-12 without another bugcheck; the original `rcbottom.sys` incident evidence remains preserved as history.

## Completed Scoped Native Effect Adapter

Implemented, independently reviewed, committed, and verified:

- [x] `BuildNativeBoundedRegularFileStore` dynamically loads the addon only when its scoped Layer is acquired; ordinary backend imports and validation never evaluate the native package.
- [x] The adapter rejects GNU, test-hook, wrong-platform, malformed module, malformed instance, malformed read/replace/finalize result, and invalid key states before exposing a bounded store.
- [x] Every opened native root is released exactly once through `Effect.acquireRelease`, including partial multi-root registry construction failures. Temporary JavaScript key copies are zeroed without mutating the caller-owned redacted key.
- [x] `WorkspaceBoundedRegularFileStoreRegistry` uses Effect `FileSystem.realPath` and `stat`, rejects malformed registrations, duplicate IDs, duplicate canonical roots, and non-directory roots before native acquisition, and exposes only opaque workspace IDs plus bounded store capabilities.
- [x] A clean-checkout simulation removed `.dist/bounded-file-store-native` while typecheck and the public adapter suite passed, then restored the output in `finally`. The focused suites pass 14 tests without loading the real addon.
- [x] Full validation passes 80 test files, 581 tests, and 3 intentional skips. Fresh independent P0-P2 review is clean.
- [x] Focused commit: `78164fd feat: adapt native workspace file stores`.

## Completed Durable Mutation Recovery Prerequisites

Implemented, independently reviewed, committed, and verified:

- [x] `WorkspaceChangeRepository` now has a terminal `rejected` lifecycle for native `Changed`. Exact replace/rollback retries return the durable rejection without creating a command, event, or change projection.
- [x] Rejected rows fail closed if forged command, event, replace projection, consumed rollback projection, journal sequence, evidence, or review state is present. `WorkspaceMutationAuthority` maps the terminal retry to typed `operation_rejected` and returns no capability-bearing admission.
- [x] `WorkspaceMutationPayloadStore` privately stages the exact expected/replacement byte pair for replace and rollback recovery, binds both identities to the canonical operation/projection, recomputes SHA-256 on read, and returns fresh byte copies.
- [x] The generated SQLite table constrains available/consumed shape, byte lengths, four-MiB bounds, and lowercase hashes. Committed consumption nulls every sensitive field while retaining a tombstone that prevents resurrection.
- [x] Thread erasure preserves pending, applied, corrupt, or byte-bearing workspace mutations, releases stale claims after quiescence, and erases only fully settled terminal rows whose private bytes are gone.
- [x] Rejected replace and rollback cleanup consumes payloads and replace snapshots only after exact lifecycle, action, thread, identity, and projection checks; absence and tombstones are idempotent without permitting resurrection.
- [x] Focused recovery coverage includes malformed and corrupt rows, restart, erasure claims/tombstones, real thread erasure, source-free public persistence, exact two-runtime staging, deterministic Stage/MarkApplied erasure overlap, and synchronized two-runtime consumption races.
- [x] Full validation passes 82 test files, 674 tests, and 3 intentional skips. Drizzle generation is idempotent, migration integrity passes, and fresh independent P0-P2 reviews are clean.
- [x] Focused commits: `247e7b5 feat: persist rejected workspace changes`, `c143986 feat: store workspace mutation payloads`, `dade5c0 test: prove mutation payload recovery`, `bc37306 fix: preserve pending workspace mutations`, and `54feea9 feat: discard rejected mutation bytes`.

## Completed Rollback Snapshot Foundation

Implemented, independently reviewed, committed, and verified:

- [x] SQLite-backed private rollback bytes with strict size, state, content-length, and lowercase SHA-256 constraints.
- [x] Snapshots bind transactionally to the canonical replace claim, thread, and recorded before identity; committed claims are cross-checked against their projection.
- [x] Read and Exists require a committed replace with rollback still available; Consume requires a matching applied or committed rollback operation.
- [x] Stage is allowed before replace commit and cannot recreate a missing or consumed snapshot afterward.
- [x] Consumed snapshots erase bytes and identity metadata, remain non-resurrectable through the service, and are deleted atomically during thread erasure.
- [x] Effect-classified SQLite lock contention uses a bounded exponential retry around the complete write transaction, rechecking thread erasure and authority on every attempt.
- [x] Deterministic transaction-attempt barriers prove real Stage and Consume retries; cross-runtime races and erasure-during-retry passed 20 repeated runs.
- [x] The production backend runtime exposes both `WorkspaceChangeRepository` and `WorkspaceSnapshotStore`; a real composition test claims and stages through that runtime.
- [x] Independent final P0-P2 review is clean.
- [x] Last full validation: 76 test files, 533 passed, 3 intentionally skipped; format, lint, typecheck, migration generation, and Drizzle integrity checks passed.
- [x] Focused commit: `3eba329 feat: store rollback snapshots transactionally`.

## Completed Snapshot Recovery Read Checkpoint

Implemented, independently reviewed, committed, and verified:

- [x] `WorkspaceSnapshotStore.Resume` exposes staged private bytes only to the exact live canonical replace while its lifecycle is `claimed` or `applied` and no committed projection exists.
- [x] The recovery read binds thread, change, canonical before identity, snapshot metadata, content length, and a fresh SHA-256 digest; missing, consumed, corrupt, cross-thread, committed, erased, or malformed state fails closed without leaking content.
- [x] Ordinary `Read` remains committed-only, so recovery access does not broaden the public rollback read contract.
- [x] Focused lifecycle, absent-snapshot, wrong-identity, wrong-thread, consumed, erasure, corruption, and null-input coverage passes 19 tests.
- [x] Independent P0-P2 review is clean. Residual harness work is to add deterministic two-runtime `Resume` races against replace commit and thread-erasure admission, plus direct lifecycle/projection-corruption cases.
- [x] Full worktree validation passed with 78 test files, 553 passing tests, and 3 intentionally skipped tests; format, lint, and typecheck passed. The run also included the still-uncommitted conditional-filesystem worker files, which remain under review rather than accepted by this checkpoint.
- [x] Focused commit: `b81fa19 feat: resume uncommitted workspace snapshots`.

## Completed Workspace Mutation Authority Foundation

Implemented, independently reviewed, committed, and verified:

- [x] One deep `WorkspaceMutationAuthority.ClaimReplace` interface hides base-run, graph-run, registry, repository, and SQLite transaction details from callers.
- [x] Base authority requires the live coordinator, active run, thread, agent, running/waiting state, and a registered canonical workspace root.
- [x] Graph authority derives assignment/group identity from the database and requires a running group, active dispatch, live run/assignment, matching agent/thread/workspace, write policy, and `repo` or `files` scope.
- [x] Effect `FileSystem.realPath` proves canonical workspace roots while returned capabilities omit absolute-path resolution; repo aliases are accepted only when they resolve to the registered root.
- [x] The workspace operation and immutable authority pin commit in one outer SQLite transaction; the existing repository transaction nests as an Effect SQL savepoint.
- [x] Exact retries use the authority pin after a run becomes terminal, reject changed authority/intent and unpinned legacy claims, revalidate pinned scope, and acquire a same-row write fence before liveness admission.
- [x] Bounded SQLite contention retry re-runs the complete proof. Deterministic stale-run, new-claim erasure, exact-retry erasure, and two-runtime convergence scenarios passed 10 consecutive final runs.
- [x] Malformed assignment and pinned-authority state fails closed; denial/conflict/failure errors do not expose workspace roots or requested paths.
- [x] Thread erasure deletes authority pins before their parent operations, and the production backend runtime exposes the registry and authority service.
- [x] The reviewer-discovered exact-retry/erasure stale-snapshot P1 was fixed with the write fence; independent final P0-P2 re-review is clean.
- [x] Last full validation: 77 test files, 542 passed, 3 intentionally skipped; format, lint, typecheck, migration generation, and Drizzle integrity checks passed.
- [x] Focused prerequisite commits: `4ff58ec refactor: share sqlite write retries` and `2615311 feat: authorize registered workspace roots`.
- [x] Focused authority commit: `7cbe507 feat: authorize workspace mutation claims`.

## Completed Model Behaviour Milestone

Implemented, independently reviewed, committed, and verified:

- [x] Curated provider-neutral capability schema for auto-compaction trigger tokens.
- [x] Provider support, scope, activation timing, unavailable/unsupported states, and `drift_ignored` projection.
- [x] Structured Codex TOML parsing/patching with `@decimalturn/toml-patch` owned by `@artisan/backend`.
- [x] Canonical SQLite setting and metadata-only provider reconciliation repository/migration.
- [x] Content-free hashes; full provider config remains ephemeral and is never persisted to SQLite/events.
- [x] Codex behavioral capability probe using isolated `CODEX_HOME`; non-zero exits fail closed.
- [x] Query/update/drift import/overwrite/ignore/retry service with exact operation fingerprints.
- [x] Public protocol routes and typed `ArtisanClient` MessagePort methods.
- [x] Desktop composition, unsupported-provider disabling, exact retry, stale drift, crash-window recovery, and reconnect replay tests.
- [x] External edit races before claim, after claim, and immediately after publication have regression coverage.
- [x] Exact bigint file identities and descriptor-bound POSIX/Windows permission mutation prevent path-substitution races.
- [x] Empty files are created privately and atomically before bytes are written; Windows uses the `FileSecurity` creation constructor.
- [x] Replacements are fully written and synced in a private staging path before no-overwrite publication.
- [x] The dedicated backups directory is current-user-only before backups, permission records, or publication anchors are created.
- [x] Rollback restores or erases through pinned identities and never deletes an external replacement.
- [x] Last full validation: 70 test files, 466 passed, 3 intentionally skipped; format, lint, and typecheck passed.

Closed review and delivery gates:

- [x] External replacement, backup collision, recovery, permission failure, write failure, rollback, and Windows ACL regressions are covered.
- [x] Focused Model Behaviour suites pass.
- [x] `pnpm run validate` passes across the complete repository.
- [x] Independent P0-P2 re-review of staged publication, hard-link restoration, and backup-directory privacy is clean.
- [x] The milestone was split into focused commits from `917ed8e` through `1ecade0`.

## Completed Project Affinity Evidence Milestone

Implemented, independently reviewed, committed, and verified:

- [x] Canonical filesystem mutation, process ownership, Git root/worktree/branch/diff, explicit project mention, and metadata evidence kinds.
- [x] `WorkspaceEvidenceRecorder` validates tool input with Effect Schema, preserves run/agent/raw-origin attribution, rejects changed operation intent, and returns exact duplicates across restart.
- [x] Public project mentions are resolved through `ProjectLocator`; forged project identities are discarded and only the canonical reference is persisted as affinity evidence.
- [x] Automatic rehome requires corroborating high-integrity journal events, so one dirty Git inspection cannot move a thread by itself.
- [x] Metadata evidence is emitted through the real refinement worker/repository path rather than fabricated directly in tests.
- [x] Historical affinity evidence replays after restart without treating its old projection basis as changed intent.
- [x] A real MessagePort subscription and list query observe the multi-source rehome projection.
- [x] Independent P0-P2 review is clean after the replay fix; the two-runtime regression passed 20 repeated runs.
- [x] Last full validation: 71 test files, 483 passed, 3 intentionally skipped; format, lint, and typecheck passed.
- [x] Focused commits: `3da881a feat: complete project affinity evidence` and `7defe4b feat: record attributed workspace evidence`.

## Completed V1 Backend Work

### Project Affinity Follow-Through

- [x] Built-in controlled filesystem, narrow Git stage/unstage, and terminal/process tool adapters bind to `WorkspaceEvidenceRecorder`; raw watcher activity remains deliberately non-authoritative. Preview stays truthfully unavailable until its separately owned production adapter exists.

### Marketplace: Skills And MCP

- [x] Built the canonical Routine/skill registry with source, version, scope, permissions, compatibility, files, enable/disable/remove, and progressive disclosure.
- [x] Added skill install preview, approval, rollback, provider sync, drift detection, and runtime-only fallback.
- [x] Integrated first-class inert `npx skills` discovery/import without making its format canonical.
- [x] Built the canonical MCP Capability registry for stdio and HTTP transports.
- [x] Added MCP auth/secrets references, OAuth/token lifecycle seams, health, tools/resources, scope, approvals, policy, sync/drift, invocation events, crash/restart, and uninstall.
- [x] Exposed Marketplace below the sidebar `New chat` action through the shadcn Dialog primitive.

### Files, Git, Surfaces, And Orchestration

- [x] Add attributed controlled-write operation history, changed-file/diff projections, review/rollback state, and private rollback snapshot persistence.
- [x] Add atomic run/agent/workspace authorization and durable mutation-claim pinning.
- [x] Add controlled filesystem read/replace execution and real restart/recovery composition through the production Layer graph.
- [x] Add review/rollback execution with pinned source authority, full recovery, anonymous evidence, and private-byte settlement.
- [x] Add public read/replace/list/review/rollback protocol commands and typed client methods.
- [x] Added Git worktree inventory, durable change/session projections, approval-bound stage/unstage mutations, and public protocol commands.
- [x] Completed the canonical surface taxonomy: Work, Time, Guidance, Routines, Capabilities, Processes, Changes, Permissions, and native actions.
- [x] Added intake risk classification, assumptions, usage aggregation, and workspace conflict/review handling.

### Preview And Electron

- [x] Added durable preview projections, production local health probe, binary asset routes, and browser inspection lifecycle.
- [x] Built the real Electron main/utility/renderer bootstrap around the shell-neutral MessagePort transport.
- [x] Added packaged-process restart/equivalence tests and single-instance ownership.
- [x] Built and connected the SvelteKit/Vite+ frontend and Monaco editor to live renderer-safe contracts.

### Deep Harness And Release

- [x] Added deterministic public-projection rebuild from the event ledger and equivalence tests without overwriting private operational/external state.
- [x] Added generated property/state-machine scenarios spanning retries, exits, reconnects, rebuilds, and concurrent agents.
- [x] Added a deep public-protocol workspace/Git/SQLite/fake-engine integration scenario.
- [x] Added complete architecture-boundary and surface-normalization suites.
- [x] Added CI/release workflows; live CLI probes remain explicit opt-in and never run silently in ordinary CI.

## Product And Architecture Decisions

- [x] Electron is the V1 desktop shell; SvelteKit/Vite+ is the frontend; Monaco is the editor.
- [x] Artisan never embeds a browser/WebView. Local web previews open in the user's external browser; Artisan owns only preview lifecycle, URL state, health, and attributable automation.
- [x] Frontend is UI-only. Backend owns provider processes, terminals, filesystem/Git, persistence, policy, and reconciliation.
- [x] Control uses typed MessagePort request/result plus durable events; streams use separate bounded MessagePorts.
- [x] SQLite is the journal/projection store. Full provider configs, credentials, and secrets never enter journal payloads.
- [x] Production conditional file mutation uses a focused Rust N-API adapter with handle-relative platform operations. Windows x64 on local NTFS is the first target and guarantees process-crash recovery, not power-loss atomicity. The path-based Node implementation remains a non-adversarial development and deterministic-test adapter only.
- [x] Provider CLIs remain the optimized native harnesses. Artisan adapts their I/O rather than replacing them with a generic API-key harness.
- [x] Canonical Artisan surfaces and registries map to provider-native capabilities with truthful unsupported/runtime-only states.
- [x] Model picker, behavior controls, Git state, and other lower-frequency controls belong in the right pane.
- [x] Threads have live titles/status, default seven-day inactivity deletion, and project affinity/rehome behavior.
- [x] Orchestration is first-class and uses user-supplied playful agent names rather than exposing provider monotony.
- [x] Keep settings curated and opinionated. Do not expose every provider config key as a generic form.

## Required Development Method

- [ ] Read `AGENTS.md` and this file at the start of every session.
- [ ] Always apply `C:\Users\Sander\.codex\skills\sanders-skill\SKILL.md`.
- [ ] Main Sol coordinates architecture, decomposition, integration, communication, and final verification.
- [ ] Never use fast mode or priority service tier for subagents.
- [ ] Use Terra medium for implementation/debugging/integration/review grunt work.
- [ ] Use Luna medium for mechanical/focused grunt work.
- [ ] Use another Sol only for a critical review or to unblock work that Terra cannot complete after a better brief.
- [ ] Keep worker write scopes disjoint and require focused verification reports.
- [ ] Update this memory whenever checked state or remaining work changes.
- [ ] Check the tracked remote at session start and before handoff. Fetch when needed, report divergence, and never overwrite remote work implicitly.
- [ ] Keep commits small, focused, coherent, and independently understandable. Commit each verified checkpoint instead of accumulating an entire milestone or mixing unrelated changes.
- [ ] Push the current feature branch to `origin` immediately after every verified coherent commit and at every natural checkpoint. Do not leave completed commits only on the local machine.
- [ ] After every push, verify that local `HEAD` equals the upstream branch head and record the new checkpoint in this file.

## Frontend Design Planning

- [x] Added and independently reviewed `frontend-checkpoints.md`, a dependency-ordered frontend build plan that marks slices as ready, fixture-ready, blocked, or moving with the backend. It keeps `ArtisanClient` as the only production renderer gateway and does not claim integration where only protocol envelopes or fixtures exist.
- [x] Added and independently reviewed `barekey-design-language.md`, a source-backed inspection of `usebarekey/barekey` at commit `2811067`. Barekey is treated as visual research only because the public repository has no detected license; Artisan must not copy its code, bundled fonts, logos, or other assets without permission.
- [x] Completed the contract map and initial `modules/frontend` scaffold. Live workspace-change integration remains gated on the backend loop recording the controlled file/change service, typed client, production composition, and verification as complete.

## Frontend Scaffold Milestone

- [x] Added a SvelteKit/Vite+ renderer module with Svelte Effect Runtime 4, Effect 4, Tailwind CSS, shadcn-svelte conventions, mode-watcher, the repository's composer stack, and static production output under `.dist/frontend`.
- [x] Added a source-tested frontend contract registry that classifies every initial interaction as live, fixture, backend-moving, or blocked and keeps command identity, correlation, retries, and subscriptions owned by `ArtisanClient`.
- [x] Added Artisan-owned dark/light semantic tokens, density and reduced-motion tokens, the canonical OFL Artisan Neo variable WOFF2, Cal Sans for the `-0.05em` wordmark, and JetBrains Mono for code.
- [x] Added the fixture-first three-pane editor shell with the `272px minmax(720px, 1fr) 340px` desktop grid, independent pane scrolling, separate editor/chat/orchestrator modes and file tabs, a dense session pane, right-first responsive collapse, a left rail, and keyboard-dismissable edge overlays.
- [x] All new interactive behavior is SER-owned and composed with `Effect.gen`; the frontend source contains no direct Effect runner.
- [x] Verification: `pnpm --filter @artisan/frontend run build` passed; three frontend architecture files passed 15 tests; the full `pnpm run validate` passed 87 files with 711 tests and 3 intentional skips.
- [x] Independent frontend review findings were resolved before the milestone: responsive controls no longer cover mode buttons, duplicated responsive pane instances share shell-owned state and use unique ARIA IDs, file tabs no longer advertise a fake close action, and the button groups use honest keyboard semantics.
- [x] Root `pnpm run validate` now includes the production frontend build, so future SER/Svelte compiler regressions fail the monorepo gate.
- [x] Frontend scaffold checkpoint `9312359` was pushed to `origin/codex/backend-services`; local `HEAD` and the upstream branch both resolved to `9312359a147e97dc011ec8c6b4b704ab0d14683b` immediately after the push.
- [x] `better-svelte-check` is not available as a portable published package; the accepted V1 gate uses the production Svelte build, root TypeScript, import-boundary, source-layout, and mounted-package tests instead.
- [x] Added the test-only typed `ArtisanClient` fixture Layer and complete visual-fixture route. Persisted pane preferences and browser-level layout/accessibility coverage remain the next frontend foundation slices.

## Frontend Fixture Runtime And Visual Lab

- [x] Added an explicitly fixture-only `Layer.succeed(ArtisanClient, ...)` that satisfies the complete current renderer-safe client contract with deterministic protocol-shaped threads, orchestration, terminals, settings, streams, receipts, and not-found failures. Production source tests prove that no composition root silently selects it.
- [x] Fixture values decode through the public Effect Schemas; the guidance fixture's byte count and SHA-256 are verified against its actual content.
- [x] Added `/visual-fixtures` as a compositional, renderer-only lab covering typography, controls, inputs, disclosure actions, tooltips, navigation groups, badges, status rows, empty/loading states, banners, permission prompts, diffs, terminal chrome, and explicit unavailable states.
- [x] The lab provides local dark/light, high-contrast, reduced-motion, whole-interface 200% scale, and long-label stress modes. These are design affordances, not a substitute for the still-open browser zoom and accessibility gate.
- [x] Independent review findings were resolved: every Tabler component is imported under its rendered name, the popup uses honest disclosure semantics, closing restores focus only while open, and all visual radii, shadows, focus rings, and ambient durations use shared semantic tokens.
- [x] All fixture behaviors remain `Effect.gen` programs, streams use Effect's Stream API for the concrete streaming contract, SER owns visual-lab execution, and no direct Effect runner exists in the new source or tests.
- [x] Focused verification: the frontend production build passed, all five frontend test files passed 26 tests, and the fixture/runtime-focused suite passed 11 tests after review fixes.
- [x] Full `pnpm run validate` passed after the backend owner formatted its concurrent work: formatting, lint, root TypeScript, the production SER/Svelte build, and 90 test files passed with 732 tests and 3 intentional skips.
- [x] Frontend fixture/runtime checkpoint `c8011a0` was pushed to `origin/codex/backend-services`; local `HEAD` and upstream both resolved to `c8011a0e366488538e262daea2cd0a64120dcb9e` immediately after the push.

## Frontend Shell Presentation Preferences

- [x] Added a versioned `ShellPresentationPreferences` Effect Service backed by Effect's browser `KeyValueStore` Layer; malformed or unsupported persisted state repairs to safe defaults.
- [x] Browser storage acquisition defects fall back to Effect's memory `KeyValueStore`, and an Effect-owned regression test proves startup remains available without `localStorage`.
- [x] Explicit desktop left/right collapse and expand actions persist, while responsive overlay open, Escape, and backdrop closure remain ephemeral.
- [x] Desktop collapse uses the 56px left rail and removes the right column; responsive breakpoints ignore desktop collapse state without exposing controls that appear to do nothing.
- [x] The frontend production build, focused lint, diff check, and 16 preference/layout tests pass. Independent P0-P2 review and re-review are clean.
- [x] Full `pnpm run validate` passed after the backend loop settled: formatting, lint, root TypeScript, the production SER/Svelte build, and 92 test files passed with 754 tests and 3 intentional skips.
- [x] Shell-presentation checkpoint `14a7d9d` was pushed to `origin/codex/backend-services`; local `HEAD` and upstream both resolved to `14a7d9d9d83e98a82fc712a4419471b07423b659` immediately after the push.

## Frontend Workspace And Tab Model

- [x] Added an immutable, renderer-owned workspace model for editor/chat/orchestrator mode state, user-owned file tabs, recents, changed files, and bounded overflow without introducing a capability Service or mutable singleton.
- [x] The model uses explicit ADTs for preview/open/pinned ownership, file/diff content, clean/dirty state, agent-change badges, tab mutation outcomes, and revision/incarnation-bound dirty-close consent.
- [x] Preview replacement, explicit-open/double-click/edit promotion, monotonic pinning, dirty close/reopen races, injective diff identities, changed-file isolation, badge replacement, mode preservation, recent ordering, active overflow, successor selection, and duplicate opens pass 19 focused Effect tests.
- [x] Independent P0-P2 review and two re-review rounds are clean after closing preview ownership, dirty-consent, stale-diff, pinned-demotion, diff-edit, tab-incarnation, and composite-ID findings.
- [x] Brought the fixture `ArtisanClient` forward to the backend's newly published list/read/replace/review/rollback workspace methods, with protocol-shaped fixture data and Effect-owned contract tests; the combined model/fixture suites pass 24 tests and root TypeScript is green.
- [x] Integrated the model into the main pane with simultaneous open/pinned/dirty/diff/preview/agent-change states, exact dirty-close confirmation, truthful before/after fixture diffs, breadcrumbs, repeatable recent/changed/overflow navigation, and an active-first horizontally scrollable tab strip.
- [x] Added Ctrl/Cmd+P fixture quick open with search, arrow/Enter/Escape/Tab handling, focus containment and prior-focus restoration across close/reopen races, keyboard-selection reveal, transitions.dev modal motion, and reduced-motion fallback.
- [x] Text Editor, Chat, and Orchestrator retain real viewport positions plus active file, draft, and selected node through mode switches. All UI behavior remains SER-owned and no direct Effect runner, timer, Promise, or fake backend command was introduced.
- [x] The focused model/UI/fixture suite passes 32 tests, the production frontend build passes, and final independent P0-P2 re-review is clean after truthful-diff, physical-overflow, state-label, select-reset, focus-return, and keyboard-reveal fixes.
- [x] Browser/package-level layout and accessibility verification now covers focus containment/restoration, accessible names, trusted native controls, wide/narrow layouts, and real 200% zoom. The development server was also explicitly requested and browser-checked during the completion pass.
- [x] Full `pnpm run validate` passed from an isolated detached snapshot containing current `HEAD` plus exactly this staged frontend checkpoint: formatting, lint, root TypeScript, the production SER/Svelte build, and 95 test files passed with 787 tests and 3 intentional skips. The backend loop's unrelated unfinished files were excluded without modification.
- [x] User-owned workspace/tab checkpoint `08e7eca` was pushed to `origin/codex/backend-services`; local `HEAD` and upstream both resolved to `08e7ecac6e2745bcf40f16977182840dda723400` immediately after the push.

## Frontend Typeface Prototype

- [x] Built `Artisan Neo` v0.1 as an OFL-licensed derivative of the verified Inter 4.1 release. This is the usable application and visual prototype; it is not yet an original proprietary typeface.
- [x] The primary variable font preserves Inter's `wght` 100–900 and `opsz` 14–32 axes, 2,937 glyphs, 2,852 mapped codepoints, OpenType features, and language coverage. The build also emits WOFF2 and eight correctly named static instances.
- [x] The primary build is deterministic. After the packaging audit fixes, two clean builds of `ArtisanNeo-Variable.ttf` produced SHA-256 `028839C365C896CD7202FD100157A293B231B21AAA049E28A6D81EC497C7678D`; every glyph has a variation record, WOFF2 reopens successfully, all variable/static PostScript names are Artisan-owned, and static weight/style metadata was checked.
- [x] The primary builder verifies both the cached official Inter 4.1 archive and extracted variable-font digests before using upstream data, so a changed cached source cannot silently alter the derivative.
- [x] Kept `modules/artisan-font/src/build-font.py` only as a rejected from-scratch construction experiment. Its stencil-like texture did not meet the neo-grotesk visual brief and must not be presented as the product font.
- [ ] The next type-design pass should preserve the stable text texture while redrawing `a`, `e`, `g`, `r`, `s`, `R`, `Q`, `&`, and `@`, then improve `I`/`l`/`1` differentiation. Until that pass, describe the family as Artisan's modified Inter prototype.
- [x] Added five reproducible comparison variants—Edge, Soft, Round, Grotesk, and Wink—as topology-preserving coordinate experiments built directly from the verified Inter source plus the Artisan base treatment. Every variant retains the two axes, glyph order, cmap, variation coverage, features, and OFL metadata; corner/default instances reopen successfully and consecutive amplified TTF/WOFF2 builds are byte-identical.
- [x] The variant pipeline materializes IUP-implied deltas, transforms every point through a fixed affine plan, re-optimizes tuples, constrains expanded outlines inside deliberate sidebearings, recomputes `hmtx`/`hhea`, and distinguishes derivative producer metadata from upstream attribution. An independent corner-coordinate oracle reports at most two units of ordinary rounding drift at `opsz=32`/weight endpoints, with no remaining P0-P2 finding.
- [x] Added the hosted Barekey `/artisan-neo` variant lab with one shared specimen, synchronized `wght`/`opsz`/size/tracking controls, honest same-setting rows, focused UI/display proofs, and individual WOFF2 downloads. The variants were visually amplified after the first comparison proved too subtle; base Neo remains unchanged.

## Completed Git Backend Milestone

- [x] Audited the PRD, existing adapter, Effect 4 process APIs, workspace authority, journal/projection conventions, public protocol, transport client, and restart semantics before implementation.
- [x] Fixed the V1 mutation contract at explicit, approval-bound `stage` and `unstage` operations over exact paths from a known workspace snapshot. Checkout, commit, push, reset, clean, and every worktree mutation remain outside this prototype surface and fail closed rather than leaking into the read adapter.
- [x] Added the generated SQLite migration foundation for workspace-owned Git projections and approval-bound, at-most-once mutation lifecycles, including exact-intent identities and a cross-runtime one-dispatch-per-workspace constraint.
- [x] Extended thread-retention fencing so pending Git approvals/dispatches prevent erasure and terminal Git operation rows are erased with their owning thread; root TypeScript passed after the schema and erasure changes.
- [x] Replaced the production Git execution path with an exact-root-authorized Effect `ChildProcess` adapter. Git is spawned as the installed CLI with argv arrays, no shell or agent SDK, bounded stdin/stdout/stderr, cancellation, porcelain-v2/NUL parsing, coherent snapshot hashing, worktree inventory, bounded diffs, and literal pathspec stage/unstage primitives.
- [x] Added durable Git workspace and mutation repositories with transactional journal ordering, exact request/decision idempotency, cross-runtime dispatch claiming, synchronous workspace-before-mutation success events, ambiguous crash recovery without replay, and approved-operation restart continuation. Repository recovery tests pass 5 scenarios.
- [x] Added the Git orchestration Service, production runtime Layers, attributed workspace evidence, public query/diff/stage/unstage/resolve envelopes, sanitized protocol routes, and typed `ArtisanClient` plus frontend fixture methods.
- [x] The first real CLI/SQLite/restart integration run found and fixed a same-snapshot/later-observation timestamp bug. The public regression now proves `ArtisanClient -> MessagePort transport -> ProtocolServer -> SQLite -> Git CLI`, one approved stage, durable version 2, actual staged index state, empty pending state, and unchanged state after backend restart. Repository/service tests separately prove exact resolution retries never redispatch.
- [x] Independent review findings were resolved: inherited `GIT_*`/`GCM_*` variables are stripped; subprocesses have hard byte caps and deadlines; snapshots double-sample status, worktrees, numstat, tracked content, and NUL-safe untracked hashes; patches are full-snapshot bracketed; exact Git filenames are never separator-normalized; POSIX literal-backslash and Windows UNC worktrees remain canonical; lock reasons omit invalid empty strings; path ordering is locale-independent.
- [x] Mutation dispatch is fenced by the durable expected snapshot/version immediately before spawn. A transaction-level workspace-busy fence prevents version advancement while the mutation holds a 60-second owner lease; healthy overlapping runtimes do not recover each other's work, expired/legacy dispatches become ambiguous, and every post-claim driver uncertainty is terminally ambiguous rather than falsely failed or replayed.
- [x] V1 remains deliberately narrow: only explicit approval-bound `stage` and `unstage` exact paths are mutable. Checkout, commit, push, reset, clean, branch mutation, and worktree mutation are unsupported and fail closed while the prototype is Codex-only.
- [x] Final independent re-review is clean through P2. Focused review verification passed 86 tests before the final POSIX/UNC/coherence fixes; targeted re-review then passed 62 service/protocol tests with no remaining P0-P2 finding.
- [x] Completion matrix now marks the verified Git status/branch/worktree/diff/session slice implemented. Full repository validation passed: formatting, lint, root TypeScript, production frontend build, 104 test files, 886 passing tests, and 2 intentional skips. One pre-existing Windows ACL integration test received an explicit 30-second timeout because its successful isolated execution takes just over the global 15-second default; no Model Behaviour production code changed.
- [x] Git backend implementation checkpoint `079157d` was pushed to `origin/codex/backend-services`; immediately after the push, local `HEAD` and the upstream branch both resolved to `079157d8b12f339af926b3022355bf619943139a`.

## Frontend Barekey Docs Shell Reset

- [x] Intentionally removed the previous routed Artisan presentation (`/`, `/settings`, `/thread/[id]`, `/visual-fixtures`) and its route-level workspace/settings/fixture components at the user's direction. Backend services, transport contracts, state models, and low-level UI primitives remain intact.
- [x] Copied the Barekey docs sidebar primitive family, mobile hook, sidebar motion helper, and the exact Barekey `logo-40.png` asset into the frontend. Added the primitive's `cuelume` runtime dependency.
- [x] Added `SectionedPanel`, a snippet-driven shell whose layout owns the sidebar and gradient card surfaces while route pages provide the primary and optional secondary card content.
- [x] The first rendered milestone is deliberately sparse: the docs-style left sidebar contains only the Barekey mark, `Artisan Editor` wordmark, and circular collapse trigger; `/` supplies no card content yet.
- [x] Browser verification at `http://localhost:5173/` confirmed the expanded shell and successful collapse to the icon rail. The copied logo SHA-256 matches Barekey's source asset.
- [x] Verification passed: formatting, lint, root TypeScript, production frontend build, 13 frontend test files with 84 tests, and the full Vitest suite with 153 files, 1,184 passing tests, and 2 intentional skips.
- [x] Barekey docs shell reset checkpoint `b32410b` was pushed to `origin/codex/backend-services`; local `HEAD` and the upstream branch both resolved to `b32410bcb1f565c7d3f2d6001a968d9c73d36a77` immediately after the push.
- [x] Corrected the sectioned panel surface to match the Barekey desktop docs frame: removed the invented opaque `bg-background` inner wrappers so the `from-foreground/5` to `to-foreground/2.5` gradient remains visible across the card. Browser verification confirmed the exposed gradient in the expanded shell.
- [x] Added the conditional third shell surface through the existing secondary snippet contract. The layout passes it only for concrete `/thread/<id>` paths, and the restored dynamic thread route is deliberately content-empty pending the user's next step. Browser verification confirmed two gradient content cards on `/thread/demo`; `/` retains one.
- [x] Added the first thread-panel content: a compact top model identity row using Barekey's `@selemondev/svgl-svelte` source, the foreground-aware OpenAI mark, hard-coded `OpenAI GPT 5.6 Sol`, and muted `OpenAI` lab label. Browser verification confirmed dark-mode contrast and spacing.
- [x] Refined the thread model identity hierarchy: the heading is now `text-2xl` and reads only `GPT 5.6 Sol`; the separate muted `OpenAI` lab label carries the provider identity without duplication.
- [x] Reduced the thread model heading to `text-xl` after visual review.
- [x] Sized the OpenAI mark from the full two-line identity row with `aspect-square h-full w-auto` and increased the mark-to-copy spacing to `gap-4`.
- [x] Replaced the percentage-height OpenAI mark after it resolved against the full panel height. The model identity now uses an optically balanced fixed `size-10` mark while retaining `gap-4`.
- [x] Reduced the fixed OpenAI model mark to `size-8` after visual review; the `gap-4` spacing remains unchanged.
- [x] Tightened the vertical model/provider stack with `-space-y-1`; horizontal `-space-x-1` would not affect the intended axis on `flex-col`.
- [x] Reduced the thread model heading to `text-lg` after visual review.
- [x] Replaced the static thread identity with a local-only model selector built from the Barekey popover, tabs, and ScrollArea primitives plus a semantic table. Its compact icon tabs cover Codex, Claude Code, Grok, OpenCode, and Google's current Antigravity naming; model rows reuse the compact icon/name/lab hierarchy and close the popover after selection.
- [x] The selector is explicitly a frontend prototype and does not restore non-Codex production adapters or claim backend availability. Codex is the default harness and its lab label remains `Codex`; its visual mark is the OpenAI SVGL logo. OpenCode uses the official `currentColor` provider glyph from `anomalyco/opencode`, and Cline is deliberately absent.
- [x] Focused verification passed: the shell source suite has 6 tests, the production frontend build passes, and browser interaction proved open, engine-tab switching, row selection, header replacement, and popover closure.
- [x] Reset the model-name/lab stacks inside dropdown rows to `space-y-0`. The next agreed visual direction is to treat the provider tabs as elevated selector chrome and the model list as the quieter `bg-background` content surface; the color split itself remains pending the next explicit step.
- [x] Corrected the selector surface hierarchy after visual review: the entire popover now uses `bg-background` rather than painting only the model ScrollArea black. The provider navbar uses the exact prose-card treatment (`rounded-3xl`, `bg-linear-to-b from-foreground/5 to-foreground/2.5`, `p-1`, and `card`), while its triggers retain neutral hover/active styling and dropdown copy remains `text-base`/`text-sm`.
- [ ] Continue only from the user's next visual direction; do not restore or invent the removed navigation, welcome, thread, settings, or workspace UI without an explicit next step.
