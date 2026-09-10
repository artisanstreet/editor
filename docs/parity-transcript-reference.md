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
| `conversation-activity.svelte`: `flex items-baseline gap-3 text-sm`, label + truncated detail | activity rows keep whole-body text (scene carries no label/detail split); split is a future scene extension, not invented here |
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
