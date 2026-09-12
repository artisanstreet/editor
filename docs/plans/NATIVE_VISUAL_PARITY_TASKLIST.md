# Native app visual parity task list

Requested by Sander, 2026-09-10. Status: baseline implementation and native render verification passed. Release, startup, and local integration passed; full parity coverage remains open.

## Target

Keep the state machine as the authority for UI state. Reproduce the existing Artisan Editor Electron app's visual presentation as nearly pixel perfectly as possible, at the standard already established by the native model picker and user dropdown. Logic tests alone do not establish visual completion. The explicit black-background request below overrides differing Electron background colors.

## Tasks, in execution order

- [x] Inspect the Electron implementation and record the relevant dimensions, spacing, typography, colors, borders, radii, breakpoints, animation timing, and conditional visibility. Map each reference component and visible state to its native renderer before changing presentation.
- [x] Make all app shell backgrounds black, including the title bar, navigation sidebar, main area, and right sidebar. Preserve intentional contrast for message bubbles, cards, controls, and overlays. Apply this within the app, not to the desktop wallpaper.
- [x] Render the right sidebar only when the window has sufficient width. Omit it and its reserved layout space at narrower widths, including the supplied screenshot's size. Choose the threshold from the reference layout and minimum usable conversation width; verify resizing in both directions.
- [x] Correct sidebar and composer containment. When the sidebar is visible, give it a distinct column and the reference separation. Keep the composer inside the conversation column so it cannot extend across or cover the sidebar. When hidden, let the conversation and composer use the reclaimed space.
- [x] Eliminate duplicate message rendering. Trace why prompts appear both as bubbles and floating text. Render each message once in chronological order, including during optimistic send, receipt reconciliation, streamed updates, snapshot refresh, and reconnect. Preserve message data and identity.
- [x] Fix the stale empty state. Show “No messages yet” only for a genuinely empty conversation; remove it immediately when the first message appears and reconcile it correctly on thread changes.
- [x] Remove exposed implementation labels. Idle “Quiet” should not produce a visible status row. Replace literal “Turn footer” with the reference footer's actual actions/metadata and visibility rules, without leaving placeholder gaps.
- [x] Match thinking and working presentation: the reference “Thinking/Working for X” wording, elapsed-time formatting, shimmer, typography, placement, transitions, and visibility. Keep elapsed time stable across rerenders and stop active effects when work settles. Follow the reference's reduced-motion behavior.
- [x] Match streaming presentation: progressive text layout, paragraph boundaries, progress indicators, scroll following, and transition into a completed reply. Fix joined text such as `naturally.I’m` at the appropriate segment boundary without inserting spaces into arbitrary streamed token chunks.
- [ ] Match message bubbles and assistant content: width, alignment, padding, radii, text selection, markdown, code blocks, links, lists, and attachments. Match tool/activity groups, approvals, errors, interruption states, and their disclosures where the reference renders them.
- [x] Match conversation rhythm and composer sizing: transcript gutters, maximum content width, inter-message spacing, footer spacing, composer minimum/maximum height, growth, bottom inset, and controls. Eliminate unexplained gaps and overlap while preserving scrolling and input behavior.
- [ ] Match surrounding shell presentation: meaningful thread title, conversation history/navigation, right-sidebar card density, hierarchy, and alignment. Avoid giving machine metadata more visual weight than it has in the reference.
- [ ] Verify visual parity against Electron at matching viewport size, display scale, content, and state. Compare narrow and wide layouts, resizing across the sidebar threshold, empty conversation, first send, thinking, working, streaming, completed replies, long content, and failure/recovery. Capture and inspect rendered results through the authorized verification workflow; document remaining differences instead of claiming completion from passing logic tests.

## Starting reference files

Electron/Svelte components under `modules/frontend/src/routes/components/`:

- `thread-workspace.svelte`, `thread-panel.svelte`, `thread-environment-card.svelte`, `thread-composer.svelte`
- `conversation-status.svelte`, `conversation-activity.svelte`, `conversation-work-session.svelte`
- `conversation-prompt.svelte`, `conversation-message.svelte`, `conversation-item.svelte`, `conversation-turn-footer.svelte`

Follow their imported styling and animation helpers. These are starting points, not a substitute for reading the complete rendering path.

Native starting points: `modules/frontend/src/thread_screen.rs`, `conversation_surface.rs`, `native_application.rs`, and `native_composer.rs`.

## Completion criteria

- Each visible message appears once, with no stale empty state or implementation placeholders.
- Black shell backgrounds and responsive sidebar behavior satisfy the explicit request.
- Conversation, composer, and sidebar remain correctly separated at supported widths.
- State-machine transitions drive the same visible behavior as the Electron reference, including elapsed status and shimmer.
- Visual comparisons substantiate the result. Focused behavior checks preserve the existing transport, draft, queue, and recovery guarantees.

## Verification checkpoint, 2026-09-10

The candidate at `42c24b57` includes the black shell, responsive inspector,
corrected empty state and navigator labels, stable elapsed status and shimmer,
footer actions, composer geometry, distinct provider-part separation, centered
reading column, Markdown lists and inline formatting, and native text selection.
The only change after rendered source `2812e1be` is a test drag endpoint.

Fourteen hidden GPUI captures at 1024×720 and 1536×900 logical pixels,
125% display scale, cover empty, thinking, working, streaming, completed,
error, and long content. Each has immutable source and binary hashes, a geometry
receipt, and an original PNG under `evidence/parity-captures/2812e1be-*`.
The final index is `evidence/parity-final-capture-matrix.json`. Root inspected
all fourteen originals. This is native renderer readback, not an OS screenshot.

The captures verify the narrow inspector omission, wide composer containment,
centered transcript, one error card, list preservation, plain work sections,
and absence of duplicate messages or stale empty-state copy. Resize tests cover
both directions. Interaction tests exercise actual pointer selection, keyboard
copying, link activation, drag suppression, and text replacement. The final
frontend suite passed 537 tests; shared Markdown passed 21 and selection 22.

The actual activity path runs through the existing provider, transactional
SQLite database, QUIC delivery, and conversation state machine. A real configured
Codex scratch turn executed a harmless command and persisted six ordered records,
including reasoning summaries and terminal activity, with the correct run,
thread, and canonical turn. That live test passed, along with 71 activity/state/
approval checks, 20 delivery checks, 27 database checks, 15 protocol checks,
and the focused backend and Codex ownership checks.

## Remaining differences and verification limits

- The capture fixture leaves the left navigation slots empty. Populated history,
  surrounding shell content, approvals, interruption/recovery, and attachment
  states have not received a matching Electron/native image comparison.
- Activity rows carry a single body rather than the reference's separate label
  and truncated detail. Per-operation elapsed and attribution presentation
  remain less detailed than the reference.
- Markdown lists, code, links, emphasis, and selection work. Dedicated table,
  blockquote, math, and diagram presentation is not established by this packet.
  Relative links remain inert without a project base; task markers are glyphs.
- Selection is retained per rendered text element. Cross-paragraph drag selection
  is not claimed.
- Static captures establish layout, not frame-by-frame animation parity.
  Motion and settlement behavior have focused state and component tests.

The unchecked tasks remain open. Pixel-perfect parity is not claimed. The release built and opened successfully. Startup reached authenticated initial queries with WGPU active; staged Editor and Forge hashes match the release binaries. Local master and desktop-workspace advanced to 42c24b57. The health receipts are in evidence/parity-release-health.json and its follow-up.


## Reference correction checkpoint, 2026-09-10 23:13

This checkpoint supersedes the earlier 42c24b57 baseline for the corrected
conversation, emoji, and send behavior. Candidate source is 5c3c60ba.
The reference is C:/Users/sander/Desktop/artisan-editor at 36ad69a8.

- Conversation projection preserves provider phase and run attribution,
  places settled work history before the final reply, defaults settled work
  to collapsed, and renders the live summary through InlineCodeText inside
  ShimmerText. Header spacing, divider, entrance, bubble geometry, and prose
  colors were checked against the reference components and imported CSS.
- Emoji use native platform color faces first. The bundled Twemoji color
  font supplies unsupported clusters. Real shaping/raster tests cover flags,
  skin tones, keycaps, ZWJ sequences, and text/emoji presentation selectors.
- Sends capture engine settings when accepted. An eligible same-engine live
  turn receives a steer, with acknowledgement required before success.
  Replay does not resend an acknowledged command. Provider rejection retains
  the payload. Unsupported named targets fail explicitly.
- Database tests include upgrading a populated pre-change database and
  retaining the accepted configuration when settings change before launch.

Verification: 554 frontend unit tests, 16 host tests, 24 surface tests,
72 request-handler tests, 7 interaction tests, 35 dispatch tests, and the
previously recorded 51 GPU/font tests passed. The dispatch suite includes
an actual Codex wire fixture emitting 64 deltas through a four-slot channel
before acknowledgement, with persistence, live-turn, replay, cancellation,
and rejection assertions. Three live-provider tests remain ignored in this
run; this correction did not make another paid inference request.

Fresh native captures cover the reported Whoopty conversation, settled and
thinking, at both widths. Settled images are under 9f5c1c1d capture folders;
only test/capture timing changes followed those product bytes. Thinking
images under 5c3c60ba wait for the header entrance to finish. These are
renderer readbacks, not OS screenshots or a complete Electron image diff.

Remaining fidelity limits include relative inline-code font sizing,
continuous chevron rotation, label-width underline growth, cross-paragraph
selection, and the broader rich-content/navigation comparison coverage
listed above. They are not marked complete. Release/startup verification
is recorded separately in the live handoff.

Release 5c3c60ba is now open and passed all seven startup stages. Staged binaries match the release. The existing database has a verified backup and passes integrity/foreign-key checks after upgrade. Local master and desktop-workspace are integrated. The idle Editor CPU issue remains about one core and is not claimed fixed.

Idle follow-up: a later paired process/thread sample used 0.109 seconds of CPU over five seconds with 15 stable threads. The earlier one-core sample was not sustained in that follow-up. Its cause remains unproven; no performance-fix claim is made.
