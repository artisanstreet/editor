# Transcript lane reference mapping (parity-transcript)

Worker: `conversation_surface.rs` + this note. Base `029ea0d0` plus dependency
cherry-pick `e6020f4` (projection packet, own commit `f62b6cce`, reported
separately). Scene additive contract is now in-tree; renderer consumes it here.

(Located under `docs/` because this worktree's sparse checkout excludes the
lane `evidence/` path; content is the requested parity-LANE reference mapping.)

## Electron truth → native mapping

| Electron reference | Native renderer |
|---|---|
| `conversation-message.svelte` user branch: right-aligned `max-w-xl rounded-2xl px-4 py-3` gradient bubble, `whitespace-pre-wrap text-base leading-7 [overflow-wrap:anywhere]` | `render_user_message`: `max_w(576)` (=`max-w-xl`) `rounded(16) px(16) py(12)` solid surface-800 (no GPUI gradient fill; documented); body keeps `body_text` base/leading |
| assistant branch: chromeless markdown at `max-w-(--prose-body-width)`, `MarkdownContent` | `render_assistant_message`: shared `MarkdownRenderer::render_source`, `max_w(672)`, body verbatim — never joins token chunks; `naturally.I’m` joins originate in backend `OrderedAssistantText` concatenation (projection packet `e6020f4` separates distinct parts; same-part deltas stay byte-exact) |
| `reasoning_summary`: `ShimmerText active` muted base | work items keep controlled `Collapsible`; reasoning/activity bodies verbatim |
| `conversation-activity.svelte`: `flex items-baseline gap-3 text-sm`, label + truncated detail | activity rows render the single scene body at text-sm foreground with no kind heading and no content truncation (the scene carries no label/detail split; split is a future scene extension, not invented here) |
| `conversation-work-session.svelte`: plain `section.t-acc` (no card), header with elapsed label + disclosure chevron, details panel, single status line at flow end | work groups render as plain sections with no card chrome and no generic `Work` title: header is the terminal duration label when attached, otherwise the latest group headers the turn's live Thinking/Working line once from the same accepted narration and clock (earlier groups and the separate status row stand down). One shared header row (`relative`, `gap-3`, `pb-2`, stable `{block}-header` selector) serves plain and disclosable renders: the collapsible always wraps header plus items with only its disabled flag following control, so registering disclosure later never remounts the row (a host toggle test asserts identical header bounds across open/close); the 1 px settled divider (`t-settle-underline` base rule is unconditional in the reference, so it always paints via shared `separator`, absolute bottom, zero layout impact; only the label-width grow is conditional and unimplemented — no text-measurement primitive — reported below). The `status-swap-enter` entrance (150 ms hold + 150 ms EaseInOut via `MotionRecipe::TextSwap` chain carrying opacity, relative 4 px rise, and 2 px blur) plays only for groups frozen as mounted-working in retained window state on first mount — history arriving settled, or settled history that later goes live, stays static. Chevron is the reference `size-4` right glyph with a static down swap when open (rotation needs shared `AssetGlyph::with_transformation` forwarding, specified below — no tween faked); reduced motion rests on static end states via the framework guarantee. Controlled open/closed/static disclosure, collapsed defaults, scroll anchors, keyboard/focus, and the disclosure action are preserved in every case; a headerless controlled group uses a chevron-only affordance with an honest accessible name, never invented content |
| user prompt with markdown source (`#`, `-`, backticks, fences, links) | confirmed intentional: the reference renders `{item.text}` plain with no `MarkdownContent` for user messages, so raw source in the bubble matches. No change. |
| user-message text selection | the bubble body renders through shared `SelectableText::retained` under the stable body id (retained framework state, no caller maps); container font/line-height/width/gradient/attachments unchanged, status and control labels untouched. A mounted drag-select plus `ctrl-c` test copies the exact body bytes with no surface actions; generic selection behavior belongs to the shared module suite. |
| `conversation-work-session.svelte`: header counts from durable `started_at` on a 1s scoped tick while unsettled; settled `Thought/Worked/Stopped/Failed after X`; single status line at flow end with shimmer (`delay 1.5 dur 3` verb, `delay 0 dur 2` summary) | live `Thinking`/`Working` + `active_started_at_ms` (authoritative controller `started_at`, never frontend clock) + mirrored `frame_now` → `Thinking for Xs` / `Working for Xs` via `live_status_copy` (whole-second floor, `FormatElapsed` parity); terminal `WorkedFor`/`ThoughtFor` attach to the work-group header; status row suppresses the duplicate when the group carries it (`status_row_visible`); shimmer via existing `artisan_ui::shimmer_text` under `MotionPolicy::Reduced` (static, reduced-motion-safe; animates if policy flips to Full) |
| idle quiet: no visible row | `TurnNarration::Quiet` → `turn_status_copy` returns `None`; no row, no gap slot |
| `conversation-turn-footer.svelte`: `absolute top-[calc(100%+0.25rem)] left-0`, `opacity-0` → `group-hover/turn` + `group-focus-within/turn` reveal, ghost icon copy button + `<time>` relative age refreshed on hover/focus only | turn root carries `.group(TURN_GROUP)` + `.relative()`; footer paints **only** when `settlement` is `Some` (eligible = completed turn + latest non-empty completed `Final` reply, per projection packet); `.top_full()` + `mt(4px)` = `calc(100%+0.25rem)`; `opacity-0` → `group_hover` reveal + per-turn focus reveal; copy emits `TurnFooterCopyRequested` with the exact settlement bytes; hover/focus emits `TurnFooterRevealed` (host takes one clock sample via existing `TurnFooterInput::Hover`/`Focus` + `conversation_relative_age`, mirrors text back via setters); no timer, no clock read, no always-visible controls |
| `conversation-turn-navigator.svelte`: labels `hidden` until `group-hover`/`focus-within`; at rest `h-px` ticks (`w-6` active, `w-4` rest); rows always buttons with accessible names | rail renders ticks at rest (1px, 24px active / 16px rest, `/50` muted); labels expand into a panel on rail hover or row focus; every row is a focusable `role=Button` with `aria-label` = message text, Enter/Space + click → existing `ScrollIntent`; active tick = latest marker (viewport-turn tracking is a future host extension) |
| literal `Turn footer` placeholder | removed; `render_footer` returns `None` without settlement, and footer contributes no scroll-identity slot |

## Doubling diagnosis (projection lane owns data; renderer owns paint)

- Renderer paints each scene block exactly once in scene order (`render_turn` loop
  over `turn.blocks()`); bodies are verbatim scene text, never concatenated.
- The floating `hi` / `who are you` duplicates were the navigator rail's
  always-visible `ButtonContent::text(marker.label)` rows — fixed here (ticks at
  rest). No canonical data was mutated to hide them, per root finding.

## Host integration still owned outside this lane (reported, not implemented)

- `set_active_now_ms`: host/viewport controller mirrors one clock sample while a
  turn is active (same tick that drives relative ages, or scene-update cadence).
  Without it, live rows truthfully show the bare verb. The renderer never reads
  a clock and never resets the authoritative basis on rerender.
- `TurnFooterRevealed` → host `TurnFooterInput::Hover`/`Focus` → clock sample →
  `set_footer_relative_age`; `TurnFooterCopyRequested` → clipboard write →
  `set_footer_copy_message` (existing `ConversationTurnFooterPolicy` models this).
- `set_turn_footer_settlement` application stays in the state machine
  (projection packet); renderer only paints.
- Needed extensions reported upstream: steering fragment boundaries +
  `AssistantMessagePhase::Final` disclosure for complete footer coverage;
  viewport active-turn identity for the navigator active tick; activity
  label/detail split.

## Follow-up packet (host clock/footer wiring, gradient, motion, shell)

- User bubble uses the existing two-stop GPUI gradient
  (`artisan_ui::gradient::vertical_gradient`, S775 top → S850 bottom for
  reference `bg-linear-to-t from-surface-850 to-surface-775`); the earlier
  "no gradient fill" comment was stale and is removed.
- Message selection: the fork exposes no text-selection primitive, so there is
  nothing faithful to wire; assistant/user bodies keep rendering through the
  shared `MarkdownRenderer` / `body_text` path verbatim with no regression.
- Status shimmer defaults to `MotionPolicy::Full` but follows the live
  `cx.reduce_motion()` window signal at render time; `set_status_motion` is an
  explicit override that always wins. Settled rows stay immediate via the
  component's inactive path. (Historical note: an earlier revision hardcoded
  `Reduced` with no system wiring; that is superseded.)
- Host (`conversation_host.rs`) serves the surface footer actions through the
  existing `ConversationTurnFooterPolicy`: reveal takes one `SystemTime` clock
  sample, formats it with `conversation_relative_age`, and mirrors the text;
  copy writes the scene settlement bytes via the platform clipboard (a void
  API, so success settles unconditionally with no speculative failure path)
  and mirrors the actual outcome. Settlement lookups always come from the
  accepted scene, never the action echo. Stale cached policies retire when the
  canonical settlement revises.
- Live elapsed: the host runs one 1s task only while the accepted scene
  carries active-work status, pushes the first sample synchronously on start
  (so capture sees `Thinking/Working for X` immediately), mirrors `None` and
  drops the task on settlement, and ends with host drop. Footer ages refresh
  on hover/focus only, per reference.
- Shell: the transcript viewport paints no opaque fill (thread-screen black
  shows through); cards, bubbles, panels, and popovers keep their faces.
- Turn rhythm: inter-turn gap is the reference `gap-8` (32px); the absolute
  footer reveals inside that room. No per-turn pad is added, so unsettled
  turns carry no phantom gap. Intra-turn block gap is the reference `gap-[1lh]`
  resolved to 24px (no app line-height override in lib/styles, so Tailwind
  preflight 1.5 on the 16px base applies); inter-turn gap stays 32px.
- Projection `c7892511` adds no new public scene API (internal turn-chart
  drive); this packet builds on `e6020f4` only. Root integrates `c7892511`
  separately.
- Production currently derives `ProviderWait` while no scene fact has arrived,
  so the elapsed clock counts it with the reference default verb (`Thinking`,
  as the work-session header does with unknown duration kind) while the
  waiting sentence stays retained as the row's accessible name. No tool or
  reasoning facts are invented to fill the row.
- Current activity-pipeline state (verified in-tree, no visual parity claimed
  without root capture evidence): the scene delivers Reasoning, Activity, and
  WorkSession items and this renderer paints them (markdown summaries,
  text-sm rows, muted titles); the turn-chart drive feeds that evidence into
  the state machine. Still absent from the scene contract: per-activity
  label/detail splits, per-session live elapsed basis (group headers derive
  the turn-level line instead), and engine attribution details. Live turns
  therefore show honest waiting/elapsed status rather than the reference's
  full per-operation detail rows.
- Work-group correction (capture-verified fault): no card chrome, no generic
  `Work`/`Activity`/`Reasoning` headings; the latest group owns the live
  Thinking/Working header once while `Closed` is honored in every case through
  the existing disclosure action (headerless controlled groups use a
  chevron-only affordance). Intra-turn block gap is the resolved reference
  `gap-[1lh]` (24px); inter-turn gap stays 32px.
- Error card (capture-verified fault): the renderer painted an outer wrapper
  card with a second `Error` heading around the destructive alert. It now
  paints the reference's single destructive card through a specialized
  `AlertStyle` (rounded-xl 14px per the v4 ramp, destructive/25 border,
  destructive/5 tint, 14/12px paddings, 6px content gap, 8px icon gap, CircleX
  icon, muted description; no shared `Alert` global change, no copy action
  since the scene block carries only the message), always mounted with stable
  anchor and debug selectors. No visual parity claimed without root capture
  evidence.
- Delegated adjustment included: `attribution: None` on the surface test
  `EngineObservationEvent` constructor for the integrated domain row; that
  field does not exist in this tree's domain yet and resolves at integration.

## Session details packet (frozen scene v185)

- Detail rows come from exactly one ordered source per group: session mode
  carries session_details (assistant/commentary, activity, compaction,
  native fact, each ordinal-keyed, stably sorted); legacy positional groups
  carry items in vec order with reasoning stripped. Never both, so no
  interleave can scramble chronology. Anchors and {group}-detail-{ordinal}
  selectors follow painted rows one-to-one.
- Assistant details render full markdown at prose width; compaction and
  native facts reuse the native card presentation statically (no nested
  toggles — visibility follows the group control, matching the reference
  grouping). Per-row disclosure stays data-only.
- The thinking line is the status row's reduced scene summary
  (TurnStatusBlock.reasoning_summary, headline/sentence reduced,
  unfinished phases fall back to narration); Full sweeps it through the
  shared shimmer text-runs builder (faces survive the band, selection
  retained per stable id), Reduced resolves to static inline fragments.
  ProviderWait narrates the engine-named wait, never elapsed.
- Type scale behind ProseTypography: body 16/28/410/-0.64, text-sm 14,
  error card 14px radius, bubble 18px radius with the card shadow stack
  beneath the gradient. Assistant markdown body stays markdown-lane owned.
- Endspace applies the reference anchoring formula to live prepaint
  geometry (192px base, 16px inset) with a change-guarded scalar; overlay
  rail sits at right-2 per reference.
- Group anchors prefer the session id with legacy fallback; superseded
  groups never own the live line; engine handoffs fold into the header far
  end. Footer/host wiring unchanged.
- Blockers resolved at integration (all doc-exact per frozen v1):
  SessionDetail, AssistantPhase::Commentary, ProgressPhase,
  TurnStatusBlock.reasoning_summary/engine_label, WorkGroupBlock
  session fields, ItemProvenance, ProseTypography, text-runs compiler
  items, attribution row. No visual parity claimed without root capture
  evidence.
## Session details + inline summary packet

- Group details come from exactly one ordered source: session
  session_details sorted stably by durable ordinal (assistant prose,
  activities, compactions, native facts), else legacy items in vec order
  with reasoning stripped. Never both, so chronology cannot scramble.
  Anchors and \{group}-detail-{ordinal}\ selectors follow painted rows
  one-to-one; per-row disclosure stays data-only under the group control.
- Assistant details render full markdown at prose width; compaction and
  native facts reuse the native card statically. Group anchors prefer the
  session id; superseded groups never own the live line; engine handoffs
  fold into the header far end.
- The thinking line is the status row's reduced scene summary (headline or
  first finished sentence; unfinished falls back to narration) with inline
  faces through the frozen text-runs contract — mono family plus zero
  tracking compile at layout, identically under Full (sweeping) and Reduced
  (static), selection retained per stable id. ProviderWait narrates the
  engine-named wait, never elapsed.
- Type scale behind ProseTypography (16/28/410/-0.64 body, 14 workspace,
  18px bubble radius with card shadow, 14px error radius). The footer copy
  control stays 32px IconSmall against reference 24px icon-xs; icon sizing
  belongs to button/shell lanes.
- Endspace applies the reference anchoring formula to live prepaint geometry
  (192px base, 16px inset, change-guarded convergence); overlay rail sits at
  right-2 per reference (vertical centering still top-anchored, pending a
  translate primitive).
- Remaining reference styling explicitly deferred (no claim of completion):
  work-session header pb-2 and settle-underline growth, disclosure chevron
  rotation animation, status-enter/text-swap entrances, navigator expand
  scale, model-handoff icon marks, per-activity label/detail split,
  per-session elapsed basis, engine display-name resolution, assistant
  streaming treatment and provenance display, markdown body/plan/prompt
  sizes, steering labels. Capture judges each.
- Blockers resolved at integration (all doc-exact per frozen v1):
  SessionDetail, AssistantPhase::Commentary, ProgressPhase,
  TurnStatusBlock reasoning_summary and engine_label, WorkGroupBlock session
  fields, ItemProvenance, ProseTypography, text-runs compiler items, the
  attribution row, inline module and test registration. No visual parity
  claimed without root capture evidence.
## Final renderer packet (frozen v1 consumer)

- Status summary reduces raw scene text (headline/first-sentence, unfinished falls back); Full sweeps faces through ShimmerText text-runs (combine merge, selectable retained id), Reduced resolves static. ProviderWait narrates engine-named waits, never elapsed.
- Render and scroll identities share turn_status_paints/turn_status_copy_text decision points; endspace measures live prepaint geometry with change-guarded convergence; bubble carries card shadow beneath gradient.
