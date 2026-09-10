# Reference scene interface (projection lane → renderer lane) — FROZEN v1

Source: reference-session-projection worker, base 5886dd2c.
Implements audit R1/R7/R8 conclusion sentences + H through the existing
state+scene architecture. Renderer worker consumes exactly this surface; no
other projection output is contractual.

Ground truth: `C:/Users/sander/Desktop/artisan-editor`
(`modules/frontend/src/lib/conversation/store.ts`,
`trace.ts`, `presentation.ts`, `activity-status.ts`;
components under `routes/components/`). Audit:
`E:/artisan-editor-worktrees/wt-parity-transcript/docs/reference-conversation-audit.md`.

## Frozen compatibility promises

- `ConversationScene::build(turns, items, narrations, steerings)` keeps its
  4-argument signature and stays total over legacy inputs.
- `SceneItem::new`, `SceneTurn::new`, `TurnNarrationEntry::new` unchanged.
- `TurnBlock`, `TurnNarration`, `SceneItemKind`, `WorkItem` gain NO variants
  and lose none the renderer consumes. `TurnFooterBlock` shape frozen
  (renderer constructs it literally).
- `AssistantPhase::Streaming` is REMOVED (it was the collapse H forbids; the
  renderer never constructs or matches it — verified). New shape:
  `Unspecified | Commentary | Final`, a 1:1 copy of the domain phase. Never
  inferred from text.
- New struct FIELDS are additive-only on structs the renderer only reads
  (`WorkGroupBlock`, `AssistantMessageBlock`, `TurnStatusBlock`, `SceneItem`
  via builder, `SceneFact` via builder). No renderer construction site for
  these exists outside `conversation_scene.rs`.

## Session anchor (R1/H)

- One session per turn at most, EXACT mirror of the reference grouping rule:
  the reference emits one work group only for a turn with exactly one work
  session; zero-session turns render positional blocks, and multi-session
  turns dissolve grouping the same way (no group; content top-level in
  order). Natively a "session" is a run with content, derived from exact
  evidence only — assistant `run_id`s and fact `run_id`s (new additive
  attribution, never parsed text). No architecture-driven visible invention.
- Stable anchor id `session-{turn_id}` as `SceneId` (turn ids admit no
  whitespace/control; overlong anchors are a typed `SceneBuildError`, never
  a fallback id). The id keys disclosure controllers and scroll anchors;
  renderer must treat it as opaque. The group also carries the session
  `run_id` as data (see frozen fields).
- A turn HAS a session iff exactly one run carries content — any assistant
  item of that run, or any Reasoning/Activity/Compaction fact attributed to
  it. Zero runs: legacy positional layout. Several runs: dissolved flat
  positional layout — no group, every assistant top-level in ordinal order.
  This is exact, not a deviation: `ItemIsWorkDetail` returns false without a
  canonical session (`store.ts:704-707`), so dissolved turns never void prose
  into an unrendered map; all prose stays top-level. The promoted-reply rule
  still selects the single footer/copy reply.
- Legacy alias override (`store.ts:637-644` `canonical_sessions`): no native
  equivalent evidence exists (it keys on durable `work_session` items with
  legacy `work:run:` / `work:exec:` id shapes, which the domain has no kind
  for), so the rule is dormant. If such evidence ever arrives, the alias map
  applies before the sole-session rule.
- No synthetic persisted rows: without domain work-session items, the anchor
  is derived from canonical evidence only. `WorkSession` facts, when
  supplied, contribute only the session title signal, never an item row.

## Detail collection (R1)

`WorkGroupBlock.items` (visible details, durable ordinal order) contains,
per turn with a session:

- Activity facts, Compaction facts, NativeFact facts;
- explicit commentary assistants (`phase == Commentary`);
- non-final assistants (every assistant that is NOT the one promoted reply).
- Frozen fragments: EXPLICIT LIMITATION. The domain carries no
  `steering_fragment_boundaries`, so frozen/post-steer fragment splitting
  cannot be implemented here. Post-steering USER messages and assistant
  items after an exact steering anchor are real and handled below; fragment
  ranges are not claimed.

Top-level per turn: user messages, the ONE promoted reply, approvals,
questions, errors, usage interruptions, post-steering items (below), and the
model transition ONLY when the turn has no session (with a session it folds
into the group header — see fields).

`WorkItem::Reasoning` is NEVER emitted by the builder (R2). Reasoning text
reaches the renderer ONLY through `reasoning_summary` (below). The variant
stays for exhaustive matches; matching it may treat it as unreachable from
this builder.

Post-steering rule (exact evidence only): steering placements carry exact
anchor `ItemId`s; an item in the same turn with a greater ordinal than any
anchor is post-steering and renders top-level in ordinal order after its
anchor. `superseded = true` iff the session group is not the turn's last
content block. A superseded session never narrates (renderer hides its live
line); the turn-level status row at turn end narrates current work.

## Final promotion (mirrors store.ts:626-702)

Exactly one `final_message_by_turn` per turn over non-empty assistants:

1. Latest non-frozen, non-commentary, non-empty assistant; `Final` phase
   wins immediately.
2. If conversation progress phase is `reply` (newest reply ordinal beats
   newest work ordinal over durable items + facts), the latest reply is
   promoted even when `Unspecified` (phaseless providers).
3. Settled-last-item promotion: turn lifecycle exactly `Completed`
   selects the latest completed non-commentary non-empty message.
   Failed/Cancelled turns never promote here — without an independent
   session lifecycle that limitation is stated, not equated away.

Progress and work ordinals use durable item ordinals + fact ordinals;
commentary never counts as reply OR work for progress (it is prose-like
history, matching the reference).

## Status inputs (R7/C1-C6)

`TurnStatusBlock` gains two renderer-read fields (exact names/types in the
frozen list below):

- `reasoning_summary: Option<String>` — newest non-empty Reasoning fact
  body for the turn, exposed ONLY while the narration is active work.
  Scoped to the session run when fact `run_id`s allow it, else newest
  overall; settled rows never carry it. Renderer shows it in place of the
  verb line.
- `engine_label: Option<String>` — handoff target label from the turn's
  model-transition fact, for `Waiting for {engine}` and the header far end.

Suppression is computed INSIDE the build (no new row either way):

- genuine live reply suppresses — non-empty assistant, live lifecycle,
  phase `Final` or `Unspecified` ONLY, AND progress phase `Reply` (the
  reply must be the newest phase; a stale stream behind newer work does
  not). Commentary NEVER suppresses: it is intermediate work, not a reply,
  so the live reasoning/status line stays while commentary streams;
- waiting-for-activity suppresses — a live Activity fact (typed lifecycle
  in the live set; unknown or settled tools never wait) newer than the
  newest model prose (non-empty assistants of any phase plus non-empty
  reasoning summaries) means the tool chain carries progress;
- `StreamingSuppression` narration behaves as before.

Thinking-word epochs stay renderer-side (`seed` = session anchor id).
`has_live_status_detail` (DOM observation) stays renderer-side.
Compaction-wait and background-agent-name inputs do NOT exist: no producer
carries that evidence today (context lane / unstructured fact bodies);
adding them would invent data. Recorded as follow-ups, not placeholders.

## Disclosure reconcile (R8)

Disclosure VALUES still come from the aggregate `DisclosureController`
registry keyed by scene id — now including session anchor ids. Aggregate
rules mirror `presentation.ts` + `ReconcileStatus`/`ReconcileReplyDisclosure`:

- initial open = working (settled starts closed, even failed/with details);
- failure observed live opens once; explicit user choice is authoritative
  afterwards and survives refreshes;
- reply-phase folding: confirmed reply (completed non-commentary prose, or
  any settlement) folds an open live session unless the user chose;
  newer work reopens;
- retired controllers project static (last) values; removal drops the key.

Renderer keeps ownership of the control itself (visible-details gating,
chevron, mount policy); scene exposes `session`, `items`,
`reasoning_summary`, `progress_phase`, and per-block `disclosure` so the
renderer never re-derives them.

## Footer (unchanged contract)

Eligibility exactly `turn Completed + completed final reply`,
`settled_at = turn.updated_at`, copy text = final body (audit §E;
`store.ts:812-820`). This packet only repoints the lookup at the scene's
single promoted reply (same rule, single source); contract unchanged.

## Frozen new API (exact names/types/methods)

`conversation_scene.rs`:

- `pub enum AssistantPhase { Unspecified, Commentary, Final }` —
  `Streaming` removed. 1:1 copy of the domain phase, never inferred.
- `pub struct ItemProvenance { pub run_id: Option<RunId>, pub lifecycle: Option<ConversationLifecycle> }` —
  `run_id` is `Some` for assistant items in production, `None` for user
  items and undisclosed facts; `lifecycle` is `Some` for assistant items,
  `None` for fact-derived cards (never treated as live).
- `SceneItem { …, pub provenance: Option<ItemProvenance> }` +
  `pub fn with_provenance(self, provenance: ItemProvenance) -> Self` —
  `::new` unchanged (provenance `None` = legacy positional behavior).
- `AssistantMessageBlock { …, pub provenance: Option<ItemProvenance> }` —
  same shared struct, renderer reads `Option`. Session mode always populates
  it from durable evidence; legacy inputs carry `None` (never a fabricated
  sentinel run); renderer treats `None` as settled/unknown, never as live.
- `pub enum ProgressPhase { None, Reply, Work }` —
  newest-phase computation over durable + fact ordinals (commentary excluded
  from both sides).
- `WorkGroupBlock { …, pub session: Option<SceneId>, pub session_run: Option<RunId>, pub superseded: bool, pub reasoning_summary: Option<String>, pub progress: ProgressPhase, pub transition: Option<ModelTransitionBlock>, pub session_details: Vec<SessionDetail> }` —
  `session` is the `session-{turn_id}` anchor (disclosure/scroll key) when
  this group is a session, `None` for legacy positional groups (which never
  carry the other new fields); `reasoning_summary` is the one live line
  (never settled); `transition` folds the handoff into the header;
  commentary/non-final-assistant/activity/compaction/native details live in
  `session_details` in ordinal order (every variant carries its exact
  `ordinal: u64` for merge); session mode leaves `items` empty and legacy
  positional groups leave `session_details` empty — never both, no duplicate
  sources.
- `pub enum SessionDetail { Assistant { id: SceneId, body: String, phase: AssistantPhase, ordinal: u64, provenance: Option<ItemProvenance>, disclosure: Option<SceneDisclosure> }, Activity { id: SceneId, body: String, ordinal: u64, disclosure: Option<SceneDisclosure> }, Compaction { id: SceneId, summary: String, ordinal: u64, disclosure: Option<SceneDisclosure> }, NativeFact { id: SceneId, text: String, ordinal: u64, disclosure: Option<SceneDisclosure> } }` —
  the ONE ordered detail list including Activity. The renderer matches
  nothing on it yet and must adopt it (with `items`) as the detail source.
- `TurnStatusBlock { …, pub reasoning_summary: Option<String>, pub engine_label: Option<String> }`.
- `ConversationScene::promoted_reply_id(&self, turn_id: &TurnId) -> Option<SceneId>` —
  exactly one reply id per turn at most, selected in reference loop order:
  latest explicit final recorded independently of the latest reply (so
  Final@2 beats newer Unspecified@3 unless progress overrides); newest-phase
  reply promotes phaseless prose while current; settled-last promotes the
  latest completed non-commentary message of a `Completed` turn only.
  Failed/Cancelled turns never settled-last promote — without an independent
  session lifecycle that limitation is stated, not equated away. Dissolved
  and legacy turns still promote (footer/copy source); only top-level
  placement differs.

`conversation_state_machine.rs`:

- Phase mapping is 1:1 (`Unspecified→Unspecified`, `Commentary→Commentary`,
  `Final→Final`); provenance attached to every assistant item and every
  fact-derived item carrying a `run_id`.
- `SceneFact { …, pub run_id: Option<RunId>, pub activity_lifecycle: Option<ConversationLifecycle> }` +
  `pub fn with_run_id(self, run_id: RunId) -> Self` +
  `pub fn with_activity_lifecycle(self, lifecycle: ConversationLifecycle) -> Self`.
  Upsert preserves `run_id` and prefers incoming lifecycles (incoming `None`
  keeps existing); only an explicit live lifecycle ever counts as waiting.
- Footer annotation consumes `promoted_reply_id` (single source with the
  scene) instead of its own latest-Final scan.

`conversation_observation_projection.rs`:

- `project_activities` fills `.with_run_id(run)` from row attribution on
  every projected fact (reasoning/tool/terminal/approval/question/timeline),
  plus `.with_activity_lifecycle` from the row's own report on tool rows
  (`Started|Progress→Active`, `Completed→Completed`, `Failed→Failed`) and
  terminal rows (`Started|Output→Active`, else settled). Timeline rows carry
  no lifecycle report, so they never count as live. No new arms, no
  synthetic session rows.

## Renderer consumption recipes

- Session group: header = elapsed basis (`TurnStatusBlock.active_started_at_ms`
  while active) + terminal `label` when settled; chevron/disclosure from
  `disclosure` + `session_details.is_empty()`; details = `session_details`
  in ordinal order (assistant/commentary/activity/compaction/native, never
  reasoning; `items` stays empty in session mode); thinking line =
  `reasoning_summary` else verb; superseded ⇒ no live line; `transition`
  ⇒ header far-end handoff.
- Top-level reply: single `AssistantMessage` with `phase == Final` (or
  promoted `Unspecified`); `provenance` (run + lifecycle) rides the block
  for streaming treatment and attribution without text inference.
- Status row: `TurnStatusBlock` as before, plus summary/engine label above;
  absent exactly when suppressed.
- Ordinal collision + replay rules unchanged: duplicate ids/ordinals are
  typed build errors; equal-cursor reinstalls are no-ops; the lip never
  becomes a bubble.

## Explicit non-goals (other lanes)

- Reasoning inline-fragment rendering, bubble radii/card layer, type scale,
  transitions/motion, navigator/composer/shell (R2-render/R3/R4/R5/R6/R9,
  audit §B/C/D/F).
- Domain `steering_fragment_boundaries` / durable work-session items (would
  unlock fragment placement + multi-session fidelity).
- Structured background-agent names, compaction-wait producer, engine
  display-name resolution.
- Scroll/endspace contracts: block order changes (session before reply) flow
  through existing id-keyed anchors; endspace linkage per shell audit §7 is
  renderer/shell business.
