# Visible-wait interface (frozen)

Status: root decision recorded. Shape (b) implemented: explicit per-turn
engine labels, no fabricated transition facts. Base `5c3c60ba`.

## 1. Verified facts (in-tree, not inferred)

- The renderer already consumes an engine label for the waiting row:
  `TurnStatusBlock.engine_label: Option<String>`
  (`conversation_scene.rs`), rendered by `provider_wait_copy` as
  `"Waiting for {label}…"` when `Some`, `"Waiting for provider to
  respond."` when `None` (`conversation_surface.rs`, read-only).
- The label was previously populated only from `ModelTransition` facts in
  session mode. For a pre-response `Pending` turn there is normally no
  transition fact, so the row rendered the generic copy.
- The conversation domain carries no engine attribution:
  `ConversationTurn` is `{turn_id, ordinal, lifecycle, created_at,
  updated_at}` and no conversation item carries an engine id. Engine
  selection at send time lives outside the conversation lanes
  (`native_application`, submission path).
- Reference (`activity-status.ts`): before the provider accepts the turn,
  the request waits on the provider (`waiting_label_for`); the engine name
  is optional display metadata supplied by the caller.

## 2. Frozen contract (projection lane owns this side)

- `TurnNarrationEntry.engine_label: Option<String>` with additive
  `with_engine_label(label: String)` (infallible carrier, like the sibling
  builders) and `pub validate_engine_label(&str)` enforcing the existing
  scene label limits: non-blank, at most `SCENE_MAX_STEERING_LABEL_BYTES`
  (1024) UTF-8 bytes. New `SceneBuildError::{EmptyEngineLabel,
  EngineLabelTooLong}`; `ConversationScene::build` validates every
  supplied entry label, so direct build callers cannot bypass dispatch
  validation.
- `ConversationStateEvent::SetTurnEngineLabel { turn_id: TurnId,
  engine_label: Option<String> }`, dispatched via
  `ConversationStateController::{on_turn_engine_label,
  set_turn_engine_label}`.
- Storage `turn_engine_labels: BTreeMap<TurnId, String>`, keyed only by
  turns already present in the controller map (bounded by
  `MAX_TURN_CONTROLLERS`, no separate ceiling, no unbounded map).
- Semantics: unknown turn rejects with `UnknownTurn`; invalid labels
  reject with `Scene(..)`; repeated identical label (including clearing an
  absent one) is a no-op success with no effects; accepted mutations push
  exactly one `SceneInvalidated`; labels for turns that leave the
  authoritative snapshot are pruned during synchronization (turn retirement
  from view), invalidating once iff anything dropped.
- `scene()` copies stored labels onto narration entries; the scene build
  prefers the explicit label over transition-derived ones everywhere,
  including outside session mode.
- Labels are display metadata only: no fabricated work, sessions,
  lifecycles, or canonical mutations of any kind.

## 3. Producer contract (native_application lane, root coordinated)

- After send receipt whose `message_id` matches the canonical user
  item/turn (correlation key: durable `source_message_id: Option<MessageId>`
  on both user item types, filled by DB snapshot/replay from the existing
  column; fifth worker owns domain/protocol/DB), dispatch
  `SetTurnEngineLabel` with the captured validated send-time engine display
  label.
- Send only validated, non-blank, bounded display names actually routed;
  never defaults or guesses. Absence keeps the generic copy, which is the
  honest fallback.
- Namingfollow-up: tell root if `SetTurnEngineLabel` / `engine_label`
  naming needs change; the projection side renames mechanically.
