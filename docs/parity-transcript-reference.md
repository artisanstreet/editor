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
| `conversation-work-session.svelte`: plain `section.t-acc` (no card), header with elapsed label + disclosure chevron, details panel, single status line at flow end | work groups render as plain sections with no card chrome and no generic `Work` title: header is only the terminal duration label; live groups show items with the turn status row below carrying the elapsed line. Reasoning bodies go through shared markdown; session titles render muted base. Controlled disclosure (open/closed/static) and scroll anchors are preserved exactly; a labelless group renders mounted without a collapsible (no reference header exists to hang the control on) |
| user prompt with markdown source (`#`, `-`, backticks, fences, links) | confirmed intentional: the reference renders `{item.text}` plain with no `MarkdownContent` for user messages, so raw source in the bubble matches. No change. |
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
- Status shimmer defaults to `MotionPolicy::Full` (`status_motion`,
  `set_status_motion` override); settled rows stay immediate via the
  component's inactive path. The fork exposes no OS reduced-motion query
  (only `WindowAppearance`), so a reduced preference arrives through the
  setter when the application layer owns one.
- Host (`conversation_host.rs`) serves the surface footer actions through the
  existing `ConversationTurnFooterPolicy`: reveal takes one `SystemTime` clock
  sample, formats it with `conversation_relative_age`, and mirrors the text;
  copy writes the scene settlement bytes via the platform clipboard and
  mirrors the actual outcome. Settlement lookups always come from the accepted
  scene, never the action echo.
- Live elapsed: the host runs one 1s task only while the accepted scene
  carries active-work status, pushes the first sample synchronously on start
  (so capture sees `Thinking/Working for X` immediately), mirrors `None` and
  drops the task on settlement, and ends with host drop. Footer ages refresh
  on hover/focus only, per reference.
- Shell: the transcript viewport paints no opaque fill (thread-screen black
  shows through); cards, bubbles, panels, and popovers keep their faces.
- Turn rhythm: inter-turn gap is the reference `gap-8` (32px); the absolute
  footer reveals inside that room. No per-turn pad is added, so unsettled
  turns carry no phantom gap. Intra-turn block gap stays 16px against the
  reference `1lh`; capture will judge.
- Projection `c7892511` adds no new public scene API (internal turn-chart
  drive); this packet builds on `e6020f4` only. Root integrates `c7892511`
  separately.
- Production currently derives `ProviderWait` while no scene fact has arrived,
  so the elapsed clock counts it with the reference default verb (`Thinking`,
  as the work-session header does with unknown duration kind) while the
  waiting sentence stays retained as the row's accessible name. No tool or
  reasoning facts are invented to fill the row.
- Known limitation, no full runtime-activity parity claimed: actual
  tool/reasoning detail rows are not delivered to the scene yet, so live turns
  show the honest waiting/elapsed status rather than the reference's detailed
  activity chain. That data path stays upstream work.
- Work-group correction (capture-verified fault): no card chrome, no generic
  `Work`/`Activity`/`Reasoning` headings; live labelless groups render mounted
  without a collapsible (documented edge: a closed labelless group has no
  reference header for the control). Intra-turn block gap stays 16px against
  reference `1lh`; capture judges next.
- Delegated adjustment included: `attribution: None` on the surface test
  `EngineObservationEvent` constructor for the integrated domain row; that
  field does not exist in this tree's domain yet and resolves at integration.
