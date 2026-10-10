# Frontend, UI, catalog and assets review evidence

Review scope: entire current working tree, including existing code and uncommitted edits. No source changes are part of this review. This is the delegated strict maintainability and frontend correctness pass.

## Inventory and review limits

Initial inventory on 2026-10-04: frontend 360 Rust files / 162,021 lines; UI 63 / 36,263; assets 2 / 877; catalog 8 / 4,470. These counts include tests nested in module source trees. The root reviewer owns the function inventory. A file/function inventory is not a claim that every execution path was semantically verified. Coverage notes below record the paths actually traced.

## Initial execution map

- `modules/frontend/src/main.rs::main` installs mimalloc and calls `artisan_frontend::run`, which calls `native_application::run`.
- `native_application/app_entry.rs` initializes the GPUI application, vendored assets/fonts, window and application entities. `native_application.rs::NativeApplication` owns connection-scoped state; `native_application/workspace.rs::NativeWorkspace` owns the one selected host and its view/connection lifetime.
- `native_transport_service` connects the selected Forge host (or the explicitly requested local development endpoint), then serializes protocol requests over authenticated transport. UI entities receive Forge event snapshots and reconcile them into presentation and conversation state machines.
- Composer input and attachment acquisition feed persisted draft state, queue/dispatch tracking and message submission. Forge authoritative state pushes reconcile those optimistic UI changes.
- Conversation observations are projected into scenes and visible transcript blocks. Transcript windowing, markdown preparation/cache/reveal and selectable text render those blocks into GPUI.
- `artisan-ui` owns shared interaction and rendering primitives. Assets embeds SVG/fonts and license/provenance records. Catalog parses and validates checked-in harness/model policy and wire data.

| Crate / entry point | Internal dependencies | Major external dependencies / features |
| --- | --- | --- |
| `artisan-frontend`, binary `editor` (`src/main.rs`), library `run` | assets, build_info, catalog, domain, cli, protocol, transport, ui, tracing | GPUI/platform with profiler, Tokio, statig, image/resvg, serde_json, url, sha2, mimalloc; optional debug-tools, flight-recorder and visual-proof. Windows adds native-dialog. |
| `artisan-ui`, `src/lib.rs` exported components | assets | GPUI, pulldown-cmark, syntect, thiserror; GPUI test-support only under dev dependency. |
| `artisan-assets`, `src/lib.rs` AssetId and `fonts.rs` | none | No runtime dependencies; compile-time embedded SVG/font bytes. Asset tests use roxmltree/sha2/toml. |
| `artisan-catalog`, `rust/lib.rs` manifest/policy/wire | none | serde_json and thiserror. Harness descriptors are embedded; actual model rows come from host discovery. |

## Review checkpoints

- Initialized this durable record before semantic review. Read sanders-skill and all references relevant to Rust repository/UI review, thermo-nuclear-code-quality-review, and unslop.

## Confirmed findings, checkpoint 1

### FQ-01. Failed autosave lets Send submit an older prompt

- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application/composer_drafts.rs::sync_composer_draft`, lines 115-120; `submit_draft_save`, lines 307-318; `receive_draft_event`, lines 146-161. `modules/frontend/src/composer_draft_sync.rs::begin_submit`, lines 208-226; `settle`, lines 259-280. `modules/frontend/src/native_application/draft_send.rs::drive_draft_submission`, lines 31-58.
- What is wrong: a rejected autosave is treated as definitively settled and its body is discarded. The composer change counter has already been marked consumed, so no retry occurs. `begin_submit` ignores the supplied body whenever any previous revision is known and there is no pending/in-flight save. It consequently calls the old revision ready even though that revision contains different text.
- Why it matters: the app can send instructions the user did not intend, then clear the newer prompt. This is more serious than a missing retry or a misleading error banner.
- Concrete failure scenario: Forge stores "old prompt" at revision 1. The user types "new prompt". The 64-command queue rejects that save as Busy, or the Forge returns a non-connection SaveFailed. The user presses Send after capacity returns. No save is queued; the request names revision 1 and sends "old prompt". The submission token still describes the current draft, so accepting the receipt can consume the newer text.
- Validation: `.dist/review/draft-sync-repro.rs` imports the actual `composer_draft_sync.rs` with minimal domain type stubs. `rustc --edition=2024 ...` succeeded; the reproduction asserts that, after the new-body save settles unsuccessfully, `begin_submit("new prompt")` returns no save and `submit_readiness` returns Ready(revision 1).
- Recommended fix: retain the latest unsaved body on admission/backpressure failure and distinguish failed-to-admit from definitive refusal. Track the acknowledged body together with its revision. `begin_submit` may reuse that revision only when it names the supplied body; otherwise save the supplied body first. Keep a user-visible unsaved/error state for definitive failures.
- Example code: `if acknowledged.as_ref().is_some_and(|saved| saved.body == body) { Ready(saved.revision) } else { save(body) }`.
- Tests to add: successful revision 1, rejected/failed save of body 2, then Send must save body 2 and await its acknowledgement. Run both Busy admission and a non-connection SaveFailed. Ensure failed autosaves remain pending across idle polling, host switch and shutdown.

### FQ-02. Failed image upload permanently suppresses retry

- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application/composer_drafts.rs::sync_composer_draft`, lines 92-113; `receive_draft_event` Uploaded arm, lines 181-210; `flush_composer_drafts`, lines 245-257. `modules/frontend/src/native_application/impl_message_flight.rs::begin_message_submission`, lines 199-209. `modules/frontend/src/native_composer/forge_draft.rs::unstored_attachments`, lines 213-228; `draft_body_with_uploads`, lines 188-216.
- What is wrong: the attachment ID is inserted into `uploads` before a request ID is minted or transport admission succeeds. A Busy/Stopped admission finishes the hold but leaves the ID in this dedup set. Uploaded Err also leaves it there without setting a retryable attachment failure. Later syncs skip that same unstored attachment forever until connection bookkeeping resets.
- Why it matters: a transient queue-full or upload failure makes a usable image impossible to send. Closing the app treats the same ID as a live upload and can synthesize a draft reference for an object the Forge never stored.
- Concrete failure scenario: a ready screenshot enters the tray while the command queue is full. Its upload is refused once. The queue drains; repeated sync/Send skips the upload and reports "Images are still uploading" forever. The user must remove/re-add the image or reconnect. Shutdown can save the digest of the never-uploaded image.
- Recommended fix: model attachment upload state explicitly as queued/in-flight/stored/failed, with correlation and a retry action. Mark in-flight only after admission. Roll back on admission failure; terminal Uploaded errors must leave a visible failure and allow retry. Only acknowledged or genuinely admitted uploads may enter the shutdown draft reference set.
- Example code: keep the ID in a local variable; on `submit_draft` Err, `uploads.remove(&attachment_id)` and retain pending work. On Uploaded Err, remove it and set a typed failure rather than silently finishing.
- Tests to add: Busy admission then capacity recovery uploads the same image exactly once; request-ID failure does not poison dedup; Uploaded Err becomes retryable; shutdown never references an upload refused at admission. Current `submit_draft` test path (`composer_drafts.rs`, lines 329-332) bypasses the scripted command outcomes and always returns Ok, so include a realistic bounded service or a failure-capable draft sink.

### FQ-03. Rich-link retention can create a recurring fetch/repaint loop

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/rich_link_titles.rs::queue`, lines 108-113; `insert`, lines 192-206; `modules/frontend/src/conversation_surface.rs::SurfaceRichLinkTitles::resolved_title`, lines 460-473; `modules/frontend/src/conversation_surface/interactions.rs::flush_rich_link_requests`, lines 493-512.
- What is wrong: the table's 64-record LRU evicts Pending and Failed entries alike. Every missing link rendered next frame becomes a new candidate. A visible reply with at least 65 distinct links exceeds retention, so newly queued misses evict other still-visible pending/resolved links. `flush_rich_link_requests` then notifies another paint.
- Why it matters: entry count is bounded, but total admitted work is not. Many links can repeatedly refill the 128-preview lane, re-fetch evicted URLs, churn favicon/title state and repaint continuously. A generated answer can trigger this without user clicks.
- Concrete failure scenario: one on-screen Markdown body contains 65 distinct HTTP(S) links. First flush queues all 65 and evicts the first. The following paint sees the first as missing and evicts the second when queueing it; further frames cycle. Preview completions also notify and reorder retention.
- Recommended fix: separate durable per-surface attempted/in-flight identity from the LRU display cache; never evict in-flight requests as if they were never attempted. Cap admissions per surface or by the visible set, and reconcile visible references before adding work. Prefer a generation-aware bounded coordinator that knows Pending/Resolved/Failed states and admission acknowledgements.
- Tests to add: render 65 and 128 distinct URLs for several unchanged frames and drain completions; request count must stop growing and each URL may have at most one live request. Verify failed URLs stay suppressed after other URLs are retained.

### FQ-04. Evaluation-only fonts are part of every production binary

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/assets/src/fonts.rs::ALL`, lines 105-116; `bundled_fonts`, lines 133-137; `modules/assets/licenses/founders-grotesk-test.txt`, lines 1-3; `modules/frontend/src/native_application/app_entry.rs::run`, lines 112-115.
- What is wrong: the production catalog unconditionally embeds and registers both Test Founders Grotesk OTFs. The checked-in provenance says they are included for local evaluation and explicitly does not grant redistribution or production rights. There is no evaluation-only feature/asset boundary.
- Why it matters: packaging a production editor redistributes these bytes unless separate rights have been obtained outside this tree. This is a release-readiness and provenance gap, not a claim that a particular license has been legally violated.
- Concrete failure scenario: build the default production editor and publish its payload; both test-font byte arrays are compiled into the binary as startup registration inputs.
- Recommended fix: gate evaluation assets and typography behind a development-only feature excluded by packaging, or replace them with a licensed production face. Document acquired rights if the asset is meant to ship.
- Tests to add: production asset inventory must exclude evaluation-only font IDs/bytes; debug evaluation builds may include them. Include a provenance/license allowlist check for shipped assets.

### FQ-05. Shared controls and the composer omit existing accessibility semantics

- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/ui/src/button.rs::Button::render`, lines 493-575; `modules/frontend/src/native_composer/render.rs::NativeComposer::render`, lines 321-348 and 436-461; `modules/ui/src/input.rs`, lines 349-364; `modules/ui/src/textarea.rs`, lines 339-354.
- What is wrong: the application has a GPUI fork with AccessKit support and several custom controls call `.role/.aria_label`, but the shared Button renderer drops the accessible label when rendering icon-only content and sets no role/label/disabled state. The real composer text editor tracks focus and renders StyledText without a textbox role, label, value, selection or accessibility actions. Surrounding wrappers do not add these semantics to those controls.
- Why it matters: the core input and shared actions do not become actionable, named accessibility nodes. Keyboard focus and retained test metadata cannot substitute for platform semantics.
- Concrete failure scenario: a screen-reader user tries to find and edit the composer textbox or activate an icon-only Send/code-copy button. The native input handler serves IME/keyboard input, but no accessible textbox/value is published, and the shared button's validated AccessibleLabel never reaches AccessKit.
- Validation: inspected exact pinned GPUI commit `8d8c07348532be87463aa666a01c1dbcb28651df` through the local sibling repository `/home/sander/projects/artisanstreet/gpui-ce-fallback`. Its `_accessibility.rs`, lines 64-68, requires both an element ID and role for a node to be reported. `elements/div.rs::a11y_role`, lines 1913-1919, uses only the explicit override_role. This rejects the initial broad candidate that GPUI has no accessibility tree. Several module comments claiming no tree are stale.
- Recommended fix: connect the existing metadata to the fork's role/label/value/disabled and text-selection APIs. Add textbox semantics and edit/selection actions for composer input. Update stale GPUI capability comments.
- Example code: shared Button root should include `.role(gpui::Role::Button).aria_label(self.content.accessible_label())` and the disabled state; the composer needs the TextInput role plus the fork's value/selection/action integration.
- Tests to add: assert platform accessibility-tree nodes for shared buttons, composer text/value/selection, and disabled state. Run manual Narrator/NVDA/VoiceOver smoke tests on shipped platforms.

## Confirmed findings, checkpoint 2

### FQ-06. Typing copies every pending image before upload deduplication

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_composer/forge_draft.rs::unstored_attachments`, lines 213-228; `modules/frontend/src/native_application/composer_drafts.rs::sync_composer_draft`, lines 73-94; `modules/frontend/src/native_application/impl_message_flight.rs::observe_composer_change`, lines 252-255 and `begin_message_submission`, lines 199-200; subscription in `modules/frontend/src/native_application/impl_lifecycle.rs::new`, lines 112-114.
- What is wrong: `unstored_attachments` deep-copies each encoded image into a new `ComposerImage`. It is called for every composer observation before the already-uploading IDs are filtered, so keystrokes and selection changes copy buffers that the caller immediately discards. Send also copies every buffer merely to test whether the vector is empty.
- Why it matters: these allocations/copies occur on the GPUI thread. The per-image 32 MiB and per-draft 320 MiB bounds prevent unlimited single-draft input, but they permit large repeated copies during ordinary typing while an upload is outstanding.
- Concrete failure scenario: attach a 32 MiB image over a slow connection and keep typing. Each composer notification clones the 32 MiB payload even though `uploads` already contains its ID; ten ready pending images permit about 320 MiB of copying per observation. This increases latency, allocator pressure and peak memory without doing new work.
- Recommended fix: identify/filter pending upload IDs before materializing payloads. Return borrowed/shared attachment descriptors from the composer and clone bytes only once at transport admission if the protocol requires owned bytes. Add a cheap `has_unstored_attachments` predicate for Send.
- Example code: `composer.unstored_attachments_excluding(&uploads)` should skip known IDs before constructing `ComposerImage`; a boolean scan of `stored.is_none() && bytes.is_some()` replaces the allocation used for `.is_empty()`.
- Tests to add: a large pending attachment followed by many text/caret edits must not construct or copy additional upload payloads. Benchmark UI-side allocation/copy volume, including multiple concurrent pending attachments.

### FQ-07. Remembered drafts retain image buffers without a window-wide budget

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_composer.rs::NativeComposer::scope_drafts`, lines 188-192; `switch_thread`, lines 573-599. `modules/frontend/src/native_composer/forge_draft.rs::ScopeDraft`, lines 44-50; `remember_scope_draft`, lines 71-95; `restore_scope_draft`, lines 107-113. Attachment buffers in `modules/frontend/src/native_composer_attachments.rs::ComposerAttachment`, lines 227-240.
- What is wrong: leaving each scope inserts a clone of its ready attachments into a HashMap. Those clones share `Arc<Vec<u8>>` payloads and thumbnails, so clearing the visible tray does not free them. Restoring clones the entry again without removing it. There is no entry-count, aggregate-byte, age or completed-upload eviction; the map persists for the whole host-scoped composer lifetime.
- Why it matters: limits apply to one draft, not all remembered drafts. A long session can retain gigabytes of already-persisted image data even when only an empty/new composer is shown.
- Concrete failure scenario: create unsent image drafts in ten different threads and navigate away from each. Each can retain up to the 320 MiB domain payload allowance; the same window can keep about 3.2 GiB of encoded bytes plus preview data until it is dropped or those individual entries are replaced.
- Recommended fix: introduce a byte/count budget for remembered scopes, pinning unsaved/pending data until it is acknowledged. Store lightweight Forge attachment references for saved inactive scopes and reload their bytes on demand. Evict clean least-recently-used drafts while preserving authoritative saved text/references.
- Tests to add: visit more scopes than the retention budget with distinct large image drafts; retained bytes stay bounded, dirty/pending scopes remain recoverable, and revisiting an evicted clean scope re-reads the Forge draft.

### FQ-08. Observation storage grows while every replay processes the full history

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/engine_observation_state/state.rs::EngineObservationState`, lines 82-134; `apply_attributed`, lines 478-528; `pair_tool`, lines 906-946; `push_timeline`, lines 1163-1184. `modules/frontend/src/conversation_observation_projection.rs::project_activities`, lines 210-315 and 527-537. `modules/frontend/src/native_application/impl_service_events.rs::replay_observation_activity`, lines 874-914. `modules/frontend/src/native_application/history_paging.rs::apply_turn_work`, lines 394-410.
- What is wrong: seen event IDs/sequences and paired activity/timeline rows accumulate for the selected thread without pruning. Each replay walks retained rows, constructs/clones candidate bodies, sorts all candidates, and only then discards entries outside the scene window. The tool/terminal ordering vectors also use linear membership scans per new row despite the accompanying maps.
- Why it matters: a bounded scene and bounded per-commit/channel batch do not bound retained source state or work per frame. A tool-heavy thread gradually uses more memory and CPU. Per-new-row linear membership plus repeated full-history projection makes cumulative processing quadratic or worse across a long stream.
- Concrete failure scenario: leave a long run with thousands of distinct tool/terminal events selected, or expand a large settled turn's work pages. The transcript renders at most its configured window, but every later observation still rebuilds/sorts all retained activity candidates and event dedup sets keep growing.
- Safeguards checked: held-back settled turns delay loading; earlier transcript pages have a count limit; live thinking retains one block; replay is coalesced once per UI drain. None bounds cumulative live activity or already-expanded work. The backend reviewer confirmed that the 4,096 observation bound is channel capacity and 64 is per-commit batching, not cumulative retained run history.
- Recommended fix: bound retained presentation rows by loaded turns/recent activity and keep unsettled approvals/questions pinned. Preserve replay correctness using a durable watermark plus bounded out-of-order gaps instead of storing every identity indefinitely. Select the visible candidate window before cloning bodies, and update changed facts incrementally. Use map Entry occupancy to maintain insertion order without vector scans.
- Tests to add: stream a large distinct-event run and repeatedly update one existing tool; retained bytes and projection time must stay bounded by the loaded window, with unchanged facts left untouched. Verify reconnect dedup, late history pages and pending interactions after pruning.

### FQ-09. One UI service poll can drain an unbounded batch

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application/impl_service_events.rs::poll_service`, lines 9-38; `modules/frontend/src/native_transport_service/service_lifecycle.rs::try_recv`, lines 230-235. The producer bridge has capacity 64 in `spawn_for_host`, lines 79-80.
- What is wrong: the polling loop keeps receiving until the channel happens to be empty, appending to an unconstrained Vec, then handles the entire collected batch synchronously. Channel capacity bounds instantaneous queued events, but a concurrent producer can refill it while the UI drains, so it does not impose a per-poll work limit.
- Why it matters: sustained delivery or replay bursts can monopolize the GPUI thread, defer rendering/input and allocate a batch larger than the channel capacity. Coalescing activity replay helps only the final projection, not event collection and dispatch.
- Concrete failure scenario: a fast Forge replay emits updates while the background service continually refills the channel. One UI tick handles hundreds/thousands of updates before other interaction runs, even though the channel advertises a 64-event bound.
- Recommended fix: cap events and/or elapsed work per UI tick; handle a bounded batch, leave the remainder queued, and promptly schedule another tick. Apply the same budget to shutdown-drain loops if they run on the application executor.
- Example code: receive at most `MAX_EVENTS_PER_TICK` rather than `loop`, then let the next polling wake continue; use a time budget if handling costs vary.
- Tests to add: a producer that refills faster than dispatch must not grow one batch beyond the cap; keyboard/render progress continues while a large history replay is drained in order.

### FQ-10. Settings persistence is unsafe between Editor processes

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/editor_settings/storage.rs::persist`, lines 150-161; process-local load/update contract in `modules/frontend/src/editor_settings.rs`, lines 3-9 and `update` (lines 233-247).
- What is wrong: every writer uses the same `settings.json.pending` sibling via truncate/write/rename, without interprocess locking or unique temporary files. Each process also persists its startup snapshot of the whole settings object, so independent updates overwrite other processes' values and unknown keys.
- Why it matters: two running Editor binaries may share one installation root. The fixed temporary path breaks the documented complete-document atomic-write guarantee under concurrent writers; stale snapshots lose preferences even when no corruption occurs.
- Concrete failure scenario: process A opens/truncates the pending file; B opens/truncates the same inode; A renames it over settings while B is still writing. B can write to the inode now visible at settings and its own rename fails. Separately, A changes the frame limit and B changes the reopen host from an old snapshot; B overwrites A's frame limit.
- Recommended fix: serialize update transactions with an install-root file lock, re-read/merge the current object under that lock, and use a unique same-directory create-new temporary file for each write. Preserve unknown keys from the latest on-disk object. If crash durability is promised, sync the file and parent directory as appropriate.
- Tests to add: synchronized concurrent writers cannot expose partial JSON, both independent changes survive, and failed temp-file operations cannot remove another writer's pending document. Include Windows replacement/rename behavior.

### FQ-11. A panicked transport thread never reports completion

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_transport_service/service_lifecycle.rs::spawn_for_host`, lines 85-112; `is_finished`, lines 239-242; `join`, lines 245-259. `modules/frontend/src/native_application/workspace.rs::close_connection`, lines 425-453; `modules/frontend/src/native_application/app_entry.rs::request_app_shutdown`, lines 38-51.
- What is wrong: `finished` is set only after the service closure returns normally. With panic unwinding, the thread exits without setting it. `join` refuses to inspect the completed JoinHandle until this flag is true, so its advertised Panicked result becomes unreachable for that case. Quit passes no stop deadline and waits forever on the false flag.
- Why it matters: a recoverable worker panic in development/debug builds prevents service replacement and graceful application exit; shutdown/recovery state no longer reflects the actual worker lifetime.
- Concrete failure scenario: in an unwind-enabled development build, service handling panics. The event channel closes and the UI notices failure, but the completion bit remains false. Switching/recovery reaches its deadline without joining; pressing Quit enters the unbounded close loop although the worker is already dead.
- Scope limit: the custom production and production-debug profiles use/inherit `panic = "abort"`, so a panic there terminates the process instead of taking this hang path. This finding concerns unwind-enabled ordinary dev/test builds; it does not assert panic recovery in either packaged stage.
- Recommended fix: use a completion guard whose Drop sets the bit, or derive completion from JoinHandle::is_finished and allow joining a finished panicked thread. Give Quit a bounded stop policy consistent with the module's documented bounded wait, and surface worker panic distinctly from generic channel closure.
- Tests to add: deliberately panic a test service thread and verify completion is observed, join returns Panicked, recovery may replace it, and Quit completes within its deadline.

### FQ-12. The window root still owns unrelated state machines and presentation internals

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application.rs::NativeApplication`, lines 337-563; `modules/frontend/src/native_application/impl_lifecycle.rs::new`, lines 21-293; `modules/frontend/src/native_application/impl_service_events.rs::handle_service_event`, lines 66-323 and subscription/switch machinery, lines 401-737 and 1115-1517; `modules/frontend/src/native_application/impl_profile_menu.rs` (1,641 lines).
- What is wrong: NativeApplication owns transport/subscription flights, drafts/uploads, per-scope policy, catalogs, queue/run decisions, route caches, project intake, profile provider usage, and detailed hover/tween/focus/scroll state in one mutable object. Numerous `impl_*` files import `super::*` and add parent-owned methods; splitting those methods into files has not established independent state ownership or narrower mutation interfaces.
- Why it matters: a change in one feature must account for root-wide lifecycle/reset ordering and cross-feature fields. Review/test boundaries are difficult to identify, and admission/reset invariants can be bypassed by nearby methods. The draft admission bugs above are examples of failure-state coordination being spread across root methods and helper state.
- Concrete failure scenario: adding a new profile-menu refresh animation or host-scoped action requires initializing fields in the large root constructor and knowing which shutdown/switch paths should cancel them. A forgotten clear/fence can retain old host state or affect the newly mounted view; file-level separation alone cannot enforce that invariant.
- Recommended fix: move complete state and lifetime ownership into focused entities/coordinators (profile menu, draft/upload submission, selected-thread subscription/navigation). Expose typed events and narrow commands to the root. Preserve NativeWorkspace's useful host-lifetime boundary and the existing pure state machines; avoid a mechanical file split or generic callback/event bus that leaves the root coupled.
- Tests to add: owner-level transition/admission/reset tests and a small set of root integration tests for host switch, send and navigation. Each owner should have one explicit drop/cancel/reset contract.

### FQ-13. Context-window formatting overflows for accepted u64 values

- Severity: Low
- Confidence: High
- Exact files/functions/lines: `modules/catalog/rust/manifest.rs::context_window_label`, lines 154-162; `NativeContextWindowOption::display_label`, lines 138-143. Input parsing in `modules/catalog/rust/wire/decode.rs`, lines 307-320, and validation in `modules/catalog/rust/wire/validate.rs::validate_model_capabilities`, lines 336-349.
- What is wrong: million-token rounding adds 500,000 to an unrestricted u64. The wire accepts values near u64::MAX and capability validation imposes no token upper bound, so the formatter's arithmetic can overflow.
- Why it matters: malformed provider/Forge catalog data can panic an overflow-checked editor or show a wrapped nonsense label in release. This is a robustness edge case, not a claim that realistic model capacities approach the limit.
- Concrete failure scenario: a context option carries tokens = u64::MAX; opening its picker calls display_label and panics in debug, or produces `0M` with wrapping arithmetic in release.
- Validation: `.dist/review/catalog-max-repro.rs` links the existing compiled catalog crate, modifies a real discovery fixture's context option to u64::MAX, successfully encodes and decodes it through the production wire API, and asserts that calling the production formatter panics with overflow checks. Reproduction passed.
- Recommended fix: round with quotient/remainder (`tokens / 1_000_000 + u64::from(tokens % 1_000_000 >= 500_000)`), and optionally validate an explicit supported capacity range at the protocol boundary.
- Tests to add: u64::MAX and values just above u64::MAX - 500,000; normal thousand/million rounding boundary cases must stay unchanged.

### FQ-14. Resolved rich-link titles never enter the expiry-refresh path

- Severity: Low
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/conversation_surface.rs::SurfaceRichLinkTitles::resolved_title`, lines 461-472; `modules/frontend/src/rich_link_titles.rs::lookup`, lines 76-83; `request_candidate`, lines 88-104; `queue`, lines 108-113.
- What is wrong: resolved_title returns the cached title before consulting request_candidate. lookup deliberately returns expired resolved titles, so normal rendering never requests a refresh for an expired entry. If a caller does queue an expired entry, queue replaces it with Pending and erases the displayed title, contradicting the stale-title/no-flicker contract in the module documentation.
- Why it matters: retained previews ignore the backend expiry and can show obsolete titles for the entire surface lifetime. A future refresh integration would introduce label flicker unless the state model changes too.
- Concrete failure scenario: a link title resolves with a short expiry and the linked page changes. Re-rendering after expiry returns the old title indefinitely and sends no new request.
- Recommended fix: check refresh candidacy independently of whether a display title exists, and retain the last resolved title while a refresh is pending. Model cached display data separately from request state.
- Tests to add: keep a title visible while one refresh is admitted after expiry; repeated frames admit no duplicate refresh, and failure retains the stale title according to an explicit policy.

### FQ-15. Transient error notices lack announcement and keyboard lifetime control

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/window_notice.rs::render_window_notice`, lines 95-177; `WindowNotices::push`, lines 315-335; `set_hovered`, lines 363-379; `tick`, lines 384-402. `modules/frontend/src/native_application/impl_window_notices.rs::window_notice_stack`, lines 45-65; callers `impl_recent_threads.rs`, line 248 and `impl_machines.rs`, line 94.
- What is wrong: the in-app display of failed-host/open-thread messages is a notice whose lifetime is four seconds. The card has no alert/status role or announcement text, its close button has no keyboard focus handle, and expiry pauses only while the pointer hovers. The application Tab route uses focus traversal, so it cannot enter or pause these controls.
- Why it matters: keyboard and screen-reader users can miss the reason an operation failed. Even a slow reader cannot retain the error using the available input path. The close button's existing Button role/label is a partial safeguard, but it does not make the surrounding message an announcement or provide ordinary keyboard lifetime control.
- Concrete failure scenario: a keyboard user opens a recent thread whose subscription is refused. The message disappears four seconds later while focus stays in the picker/composer. No announced error or persistent in-app error record is available to inspect the cause. Stderr and optional flight-recorder diagnostics do not provide this user-facing interaction.
- Recommended fix: publish alert/status semantics through the existing GPUI accessibility API, add focusable close/review controls, pause while keyboard focus is inside, and provide persistent error details or a repeatable status surface. Treat errors requiring user action differently from short informational notices.
- Tests to add: tree contains a named alert/status for the message; Tab reaches dismiss/review, focus pauses expiry, Escape/dismiss works, and an important error remains inspectable after the timer.

### FQ-16. Rich-link work is marked pending before its action is admitted

- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/conversation_surface/interactions.rs::flush_rich_link_requests`, lines 493-512; `enqueue_action`, lines 1344-1349; `modules/frontend/src/rich_link_titles.rs::request_candidate`, lines 88-104 and `queue`, lines 108-113.
- What is wrong: queue mutates every URL to Pending before enqueue_action checks the bounded surface action queue. If that queue is full, the request vector is dropped while Pending remains. Future frames suppress the URLs as already in-flight even though no host/service ever received them.
- Why it matters: transient local backpressure permanently loses preview work for otherwise ordinary links. This is distinct from FQ-03: it affects a single link below the retention limit and does not require LRU churn.
- Concrete failure scenario: a backed-up host effect outbox leaves the surface's action queue full. A new assistant message renders a link and the render-tail flush fails admission once. Once other actions drain, that link still reports Pending forever and never resolves.
- Recommended fix: admit one action transactionally before committing Pending, or retain the failed action for retry. Roll back only the entries owned by the failed admission, taking care not to undo an older live request.
- Tests to add: fill the surface action queue, render one new link, then drain actions and repaint; exactly one resolve action must eventually be emitted and Pending must correspond to admitted work.

## Coverage, safeguards, and validation checkpoint

### Semantically traced paths

| Area | Paths and contracts reviewed | Important boundaries / limits |
| --- | --- | --- |
| Window and connection lifetime | main/lib/app_entry; NativeWorkspace host switch, drain/seal/stop/recovery; service spawn, bounded queues, PreviewLane and command admission; root poll/dispatch | Host lifetime fences and bounded switches are real safeguards. Quit stop differs from switch deadlines; completion assumes normal return. |
| Composer and submission | native_composer editing, render, attachments and Forge restoration; ScopeDraft cache; DraftSync; application draft send, image upload and message flight; existing Forge draft/send tests | Text/selection boundaries and IME conversion are explicit. Latest body/revision acknowledgement and upload admission state need stronger invariants. |
| Conversation and activity | observation retention/pairing, activity projection, conversation host action/effect handling, surface render/interactions/shaping/windowing; history paging, settled work | Scene/window count and per-body limits exist, but earlier sources and cumulative work are not equivalently bounded. Current replay coalescing prevents one full projection per event within a drain. |
| Markdown and rich links | parse/prepared cache, renderer inline/table/code-copy, reveal and selectable text; URL policy, title cache, icon decode, title/service effects | Raw HTML is inert; clickable schemes are restricted. Parse/cache/body limits and bounded image decoding were checked. Resolver network security belongs to backend scope. |
| Model and settings UX | model catalog, selector state/select policy/menu rows, native command menu, project/thread picker, shared input/button primitives, settings load/persist | Local policy selection intentionally precedes installation; unsupported providers are a backend capability problem, confirmed reachable through picker. Disk settings transactions are process-local. |
| Shared UI | Button/Input/Textarea; dialog/sheet/popover/context/dropdown focus contracts; menu interaction primitives; markdown/motion/theme/asset seams | Some primitives are reusable-library exports used mainly by tests. Caller-owned focus/trapping is documented; no unsupported blanket modal bug is reported. Some custom production controls do publish role/label semantics. |
| Assets and catalog | embed catalog/font registration/license provenance; complete catalog Rust source/wire/policy/validation and catalog unit tests | Catalog identifiers/references/unknown fields/floats/wire size are validated. Token arithmetic still assumes a narrower numeric domain. Production font rights are not demonstrated by the evaluation provenance. |

This is a broad static semantic pass of the major flows and reusable boundaries, not a claim that all 433 files or every function branch received equally deep scrutiny. The root report supplies the complete structural function/file inventory. GPUI/native windows and assistive technology were not executed during this delegated pass; those remaining integration limits are explicit.

### Security and failure candidates rejected or narrowed

- Image read/preview code validates reference identity, MIME/digest/size/name and scope/generation, bounds encoded sources and decoded pixel/allocation budgets, and cancels stale previews. No generic image decompression-bomb or stale-host image-write vulnerability is asserted.
- Composer file selection validates regular files, reads with `take(limit + 1)`, enforces per-image/count/aggregate input bounds and does expensive preparation off the UI thread. Local file-picker paths are not assumed to be remote attacker-controlled paths.
- Favicon SVG decode disables external and embedded image href resolution; raster decoding and fixed-size thumbnails have bounds. No SVG filesystem/network fetch finding is asserted.
- Markdown raw HTML is not executed, and clickable URL policy restricts opening to supported HTTP(S)/mailto destinations. No JavaScript/file URL execution finding is asserted at this UI boundary.
- Exact pinned GPUI has AccessKit support. The earlier candidate that the whole app/framework lacks a platform tree was rejected. FQ-05 is the concrete missing semantics on shared controls and the real composer.
- Model selection intentionally supports choosing locally before an engine is installed. That alone is not a bug. Cursor/Grok being marked runnable despite incomplete adapters is reported by the backend reviewer; their rows are selectable when models exist because engine_usable short-circuits later runnable/readiness checks.
- Entry counts, per-batch bounds and per-body limits were not confused with a total working-set bound. FQ-06 through FQ-09 record the specific upstream retention/copy/drain gaps despite those existing safeguards.

### Checks actually run

- `cargo test -p artisan-catalog --lib --offline`: **19 passed, 0 failed**. Existing suite exercises policy/wire/provenance/reference validation, but no maximum-u64 label case. Cargo warned that a backend fixture is both an example and integration-test target; the root reviewer owns that build issue.
- `cargo test -p artisan-assets --offline --target-dir .dist/review/assets-target`: **24 passed, 0 failed** (8 unit + 16 asset-validation integration tests; 0 doctests). Existing license tests check that records exist, not that evaluation-only records are excluded from a production payload.
- `.dist/review/draft-sync-repro.rs`, compiled with `rustc --edition=2024` and run: **passed the failure-path reproduction** of FQ-01 using the actual DraftSync source and minimal domain type stubs. This artifact is review-only and does not modify production/tests.
- `.dist/review/catalog-max-repro.rs`, compiled with `rustc --edition=2024 --extern artisan_catalog=<existing rlib> -L dependency=target/debug/deps` and run: **passed the accepted-wire/formatter-overflow reproduction** for FQ-13, using the real catalog crate and discovery fixture.
- Earlier catalog/asset invocations were interrupted while blocked on the root Cargo lock; they are not counted as passed checks. Completed invocations above supersede them. The asset target directory isolates this small crate from the root GPUI build lock.
- No full frontend build, native launch, snapshot rendering or platform accessibility smoke test was run. No source refactor, commit, push or external communication was performed.

## Cross-cutting architecture findings

1. **Admission is part of the state transition.** Draft autosaves, attachment uploads and rich-link work can be marked settled/in-flight before their downstream queue has admitted the action (FQ-01, FQ-02, FQ-16). A queue refusal is transient availability, not an authoritative save/upload/resolve result. Prefer transaction-shaped methods that return accepted ownership or retain retryable work, with correlation tying terminal outcomes to that ownership.
2. **Draft authority has several copies.** The view holds text/attachment bytes, per-scope caches remember them, DraftSync knows revisions/pending bodies, and the Forge stores the authoritative draft. Acknowledged body identity must travel with the revision so local presentation cannot turn an old authoritative body into a send of newer text (FQ-01). A dedicated draft/upload coordinator can enforce this rather than root call ordering.
3. **Bound the full retention chain.** Transcript and markdown caches are bounded, but observation maps, scope draft image caches, pre-dedup payload construction and work-per-tick sit outside those caps (FQ-06 through FQ-09). Document budgets in bytes/count/time for each owner and for the whole window; count-only or per-message ceilings do not establish a session memory budget.
4. **Accessibility is a shared-component contract.** The framework now has native semantics, but reusable components still expose metadata-only label APIs and product code selectively uses direct GPUI roles (FQ-05, FQ-15). Wire semantics once at shared component/text-editor boundaries and test the emitted tree rather than only stored labels or visual selectors.
5. **Ownership must survive file extraction.** NativeWorkspace has a useful connection lifetime boundary and conversation logic has pure state machines. Many NativeApplication impl files still mutate one broad root object, including presentation internals (FQ-12). Move the state, tasks, reset rules and transitions together, preserving thin orchestration at the root.
6. **Lifecycle and persistence promises need cross-process/failure tests.** Atomics set on normal return and a fixed temporary file look safe in sequential tests, but fail under unwinding or concurrent processes (FQ-10, FQ-11). Verify crash/race conditions that the contract claims to handle, not only happy-path snapshots.

## Prioritized scoped findings

| Priority | ID | Severity | Confidence | Impact |
| --- | --- | --- | --- | --- |
| 1 | FQ-01 | High | High | Sends an older prompt and can consume newer text after failed autosave |
| 2 | FQ-02 | High | High | Transient upload failure blocks image sending indefinitely and can create phantom saved references |
| 3 | FQ-05 | High | High | Core composer and shared actions omit platform accessibility semantics |
| 4 | FQ-04 | Medium | High | Default production payload contains fonts whose checked-in provenance only supports evaluation |
| 5 | FQ-03 | Medium | High | More than 64 visible links churn fetches/cache/repaints repeatedly |
| 6 | FQ-16 | Medium | High | Local action backpressure loses link resolution permanently |
| 7 | FQ-08 | Medium | High | Cumulative activity storage and full-history projections degrade long runs |
| 8 | FQ-06 | Medium | High | UI notifications deep-copy large pending images before dropping duplicates |
| 9 | FQ-07 | Medium | High | Inactive scope drafts retain gigabytes without an aggregate cache budget |
| 10 | FQ-10 | Medium | High | Concurrent Editors can corrupt/overwrite shared settings |
| 11 | FQ-09 | Medium | High | Sustained replay can monopolize an unbounded UI polling tick |
| 12 | FQ-11 | Medium | High | Unwind worker panic leaves completion false and Quit waiting forever |
| 13 | FQ-15 | Medium | High | Short-lived errors lack announcement and keyboard retention |
| 14 | FQ-12 | Medium | High | Broad root state ownership multiplies reset/admission/review complexity |
| 15 | FQ-14 | Low | High | Expired cached rich-link titles never refresh; refresh state loses stale display |
| 16 | FQ-13 | Low | High | Accepted extreme token values overflow display rounding |

## Quick wins and larger refactors

Quick wins: make draft test sinks capable of Busy/Stopped outcomes; keep rejected saves dirty; remove/retry failed upload IDs; filter image IDs before cloning and replace Send's allocating existence check; make rich-link action admission transactional; connect Button label/role/disabled metadata; give the composer explicit textbox semantics; use safe context-label rounding; gate evaluation assets; cap poll batches; publish notice roles and keyboard focus; replace the completion assumption with a panic-safe guard/JoinHandle check; use unique settings temp files as the first part of a locked transaction.

Larger refactors: acknowledged-body/revision draft coordination and typed attachment upload state; byte-budgeted per-scope draft retention; loaded-window activity storage with incremental fact projection and reconnect-safe compact dedup; complete profile and subscription/navigation state ownership extracted from NativeApplication; interprocess settings merge/lock protocol; platform text-selection/edit actions and accessibility-tree integration tests. Do not delay local failure-path fixes until those refactors finish.

## Ordered remediation and regression plan

1. Fix FQ-01/FQ-02 with a failure-capable command sink and regression cases for previous revision + rejected new body + Send; Busy/upload Err + retry; and rejected uploads during shutdown. Establish a single invariant: any Ready revision identifies the displayed send body, and every in-flight attachment corresponds to an admitted command.
2. Wire core shared Button/composer accessibility and test the platform tree, text values/selection/edit actions, disabled controls and keyboard activation. Include manual platform screen-reader checks. Give error notices named alert/status semantics and focus-based lifetime controls.
3. Resolve production asset provenance before release packaging; gate/remove evaluation fonts or provide production rights. Add shipped-asset inventory enforcement.
4. Repair link retention/admission/expiry together: no eviction of live work as never-attempted, retries after action backpressure, one in-flight request per URL, and stale display while refreshing. Test 65/128-link unchanged renders and bounded-action refusal.
5. Eliminate pending-image clones and add byte-budgeted inactive draft retention. Benchmark a slow upload with large image drafts while typing; verify old scopes reload clean authoritative drafts and unsaved data stays safe.
6. Bound activity retention/projection and per-poll work. Stress a long distinct-tool stream, late pages and reconnects; measure memory, allocations and UI frame/input progress rather than only output equality.
7. Make settings a locked read/merge/write transaction and add coordinated two-process tests, preserving both known/unknown keys. Verify platform rename/replace and failure cleanup.
8. Add worker-panic completion and bounded-Quit tests in unwind builds. Keep production abort behavior explicitly documented; do not imply panic recovery there.
9. Extract independent state owners incrementally once their contracts have tests. Begin with draft/upload coordination and profile-menu state, then selected-thread subscription/navigation. Keep root integration cases for host switch, send, project/thread removal and queued retries.
10. Fix safe label rounding and stale capability/provenance comments. Update architecture/state diagrams to the current native GPUI/Forge ownership, including budgets, admission and persistence guarantees.
