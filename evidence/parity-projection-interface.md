# Parity projection interface — footer + active-elapsed contract (projection lane → renderer lane)

Source: projection worker, session ses_f7474b6d0ffeMCF2XloTlFYhG7, base 029ea0d0.
Renderer lane session: ses_f7474b666ffexY8Muc5zs4qW7I (root relays).
Read against Electron truth: `conversation-turn-footer.svelte` (hover/focus-only
absolute footer, `top-[calc(100%+0.25rem)]`, Copy-response button, `<time>`
relative age refreshed on hover/focus only) and `conversation-work-session.svelte`
(header counts from durable `item.started_at` on a 1s scoped tick while unsettled;
settled labels carry their own durations).

## Out of scope for this lane (no change, by root finding)

- The floating `hi` / `who are you` duplicate is the turn navigator's
  always-visible text (`conversation_surface.rs` `render_turn_navigator`), NOT a
  duplicate canonical message. No canonical dedup change is made here: one
  durable item id still projects to exactly one scene block (`scene()` maps the
  snapshot 1:1, `build()` rejects duplicate ids/ordinals as typed errors), the
  optimistic steering lip is composer-placed and released on durable anchor, and
  replay keeps the last-good snapshot (duplicate/old/gap batches request a
  resnapshot, never a second render).
- `naturally.I'm`-style joins are a segment-block separation concern, never a
  reason to mutate token text. `apply_item_append` stays byte-exact; adjacent
  durable assistant items stay distinct scene blocks in ordinal order. Domain
  currently has no `steering_fragment_boundaries` field, so Electron-style
  post-steer fragment placement needs a domain/backend contract extension (see
  below) — this packet does not invent one.

## Additive scene contract (this packet, projection lane owns)

All additions are optional data on existing blocks; no enum shape changes, no
`ConversationScene::build` / `SceneTurn::new` / `TurnNarrationEntry::new`
signature changes. Existing exhaustive matches keep compiling.

1. `TurnFooterBlock { turn_id, settlement: Option<TurnFooterSettlement> }`
   - `TurnFooterSettlement::new(response_text: String, settled_at_ms: i64)` —
     text bounded by `SCENE_MAX_MESSAGE_BODY_BYTES`, timestamp is authoritative
     Forge `turn.updated_at` millis (never a local clock).
   - Eligibility (mirrors `store.ts`: footer iff turn completed AND final reply
     completed): turn lifecycle is exactly `Completed`, plus the latest-by-ordinal
     non-empty assistant item in that turn with domain phase `Final` AND item
     lifecycle `Completed`. Text is that item's body byte-for-byte. Backend
     already persists `Final` on `Completed` terminal settlement
     (`native_run_dispatch.rs` `settle_terminal`), so eligible completed turns
     settle their footers without any further extension.
   - Otherwise `settlement` is `None`: renderer renders NO footer. No fake
     inactive footer. (If a completed turn carries no `Final`-phase completed
     reply, its footer stays unsettled — truthful; the backend `Final` path
     above is what makes the eligible case fire.)
   - `ConversationScene::set_turn_footer_settlement(&mut self, turn_id,
     settlement) -> bool` applies it to that turn's one footer.
   - Renderer wiring: copy payload = `settlement.response_text()`; `<time
     datetime>` = `settled_at_ms`; request one clock sample on hover/focus only
     (existing `ConversationTurnFooterPolicy` already models exactly this).

2. `TurnStatusBlock { narration, active_started_at_ms: Option<i64> }`
   - `TurnNarrationEntry::new` unchanged, plus additive
     `.with_active_started_at_ms(i64)` builder. `build()` copies the basis onto
     the turn's status block and rejects it with typed
     `SceneBuildError::ActiveBasisWithoutActiveNarration` on any non-active-work
     narration (`Quiet`, terminal labels, failures).
   - Source: turn-controller `view().started_at` (first active-entry event time,
     caller-supplied, monotonic; survives resume and snapshot refresh — never
     reset on rerender). The state machine attaches it only while the turn view
     `state.is_active()`. This module never reads a clock.
   - Renderer: `elapsed = frame_now - active_started_at_ms`, ticking only while
     the narration is active work (`Thinking` → `Thinking for Xs`, `Working` →
     `Working for Xs`). Terminal `WorkedFor`/`ThoughtFor` millis are unchanged
     and take over on settlement. Millis formatting stays whole-second floor,
     matching `FormatElapsed`.

## Delivery→turn ownership wire (this packet, projection lane owns)

Production gap fixed: `NativeApplication` only delivers Snapshot/Batch through
`ConversationHost`, and turn controllers previously existed only via explicit
`register_turn` (tests), so the real app always fell back to `Quiet` and the
active-elapsed basis never fired outside manual drive.

Bounded interface (all inside `conversation_state_machine.rs`; renderer
`active_started_at_ms` contract unchanged):

- After every accepted delivery event, `synchronize_turn_controllers` drives
  each durable turn that has NO explicitly registered controller: auto-create
  (skipped at `MAX_TURN_CONTROLLERS`) + dispatch events derived from canonical
  lifecycle/item/fact evidence. No host or delivery-machine change was needed.
- Explicit `register_turn` REPLACES any delivery-derived controller with a
  fresh one and marks the turn explicitly owned; sync skips explicit turns
  forever after. Duplicate/explicit semantics and manual-drive tests are
  unchanged by construction.
- Derivation (`derive_turn_events`, pure): terminal lifecycles settle
  (Completed/Failed/Cancelled/Interrupted) via a work/thought pre-step so the
  settled kind stays truthful; active lifecycles report streaming reply (live
  non-commentary reply text), else Activity/ChangedFiles facts → Working,
  Reasoning facts → Thinking, else WaitingForProvider; Pending derives nothing.
  Never text content, never speculation.
- Clocks: first activation uses `turn.created_at` (send-time basis, reference
  parity, never resets); later drives use the window watermark (monotonic);
  terminal events use `max(turn.updated_at, watermark)` so redelivery freezes
  the first settlement. Revisions ride the controller lane (`rev + 1/2`).
- Best-effort: sealed/stale/regressed derivations are swallowed (already
  covered). At most one `SceneInvalidated` per delivery event, only on real
  leaf-state change. Registry-full or effect-full skips sync; accepted delivery
  always stands (idempotence/replay/backpressure preserved).

## Needed contract extensions (reported, not implemented here)

1. Renderer lane: hover/focus-only footer visual + navigator label visibility +
   active-elapsed ticking + separation between adjacent assistant blocks.
2. Domain lane (if ever needed): surface steering fragment boundaries (Electron
   keys post-steer placement on `steering_fragment_boundaries`). Not required
   for the joining fix below.

## Actual joining fix (this packet, backend lane slice owned)

Root cause, traced end to end: Codex `item/agentMessage/delta` carries an
explicit `itemId` (`codex.rs`), each chunk is tagged `part_id = item_id`, and
`OrderedAssistantText` (`native_run_dispatch.rs`) retained those part ids but
`body()` concatenated all part texts with no separator — joining sentences
across distinct provider messages (`naturally.I'm`).

Fix, helper-only (no dispatch/usage/queue/recovery change):
- Same `part_id` appends stay byte-exact (token continuity; keeps the
  `previous + delta == next_body` streaming fast path in `handle_text_delta`
  valid).
- A first contribution that makes a new part nonempty earns exactly one `"\n\n"`
  separator when another nonempty part exists — faithful paragraph/block
  separation in the single committed body. Empty parts earn no text and no
  separator (no leading/trailing/doubled separators).
- `total_bytes` now tracks the exact `body()` length including separators, so
  the `AssistantBody::MAX_BYTES` bound stays honest; replacement recharges the
  previous bytes plus both separator shares before admitting the new text.
- `handle_text_delta` already routes a divergent join (`previous + delta !=
  next_body`, which is now the new-part case) to `replace_assistant_body`, so
  no consumption change was needed.
- Focused tests updated: `multipart_byte_bound_tracks_all_parts_separators_and_replacements`
  (bound fills exactly at MAX with separators counted), correction expectations
  now `"A-1A-2\n\nB-1"` / `"A-1A-2\n\nB-corrected"`, plus
  `empty_parts_contribute_no_text_and_no_separator`.
