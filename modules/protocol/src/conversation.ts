import { Effect, Schema } from "effect";

import { ArtisanErrorCode } from "@artisan/catalog";

import { Identifier, IsoDateTime, JournalSequence, SchemaVersion } from "./common";
import { ImageAttachmentReference, UserMessageContentPart } from "./attachments";

/** A lifecycle shared by canonical turns and items. Terminal values never transition again. */
export const ConversationLifecycle = Schema.Literals([
	"pending",
	"streaming",
	"active",
	"waiting",
	"completed",
	"failed",
	"cancelled",
]);
export type ConversationLifecycle = typeof ConversationLifecycle.Type;

export const ConversationTerminalLifecycle = Schema.Literals(["completed", "failed", "cancelled"]);
export type ConversationTerminalLifecycle = typeof ConversationTerminalLifecycle.Type;

/** Distinguishes provider-disclosed progress commentary from a settled assistant reply. */
export const ConversationAssistantMessagePhase = Schema.Literals([
	"commentary",
	"final",
	"unspecified",
]);
export type ConversationAssistantMessagePhase = typeof ConversationAssistantMessagePhase.Type;

const NonNegativeInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));

export const conversation_maximum_steering_fragment_boundaries = 256;

/** A durable cut in one provider message, anchored after an acknowledged user item. */
export const ConversationSteeringFragmentBoundary = Schema.Struct({
	after_item_id: Identifier,
	text_offset: NonNegativeInt,
});
export type ConversationSteeringFragmentBoundary = typeof ConversationSteeringFragmentBoundary.Type;

/** Bounds renderer-visible text while still allowing an initially empty streaming delta. */
export const ConversationText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length <= 4_096 ? undefined : "Expected at most 4096 characters",
	),
);

/**
 * What a message body may hold, as opposed to the label-sized bound above.
 *
 * A label truncated at 4096 characters is still a label; a reply truncated there
 * stops mid-sentence and the turn ends looking answered, which is the one
 * failure a transcript must never present as a completed thought. The bound
 * stays finite because every delta rewrites the whole stored entity, and 64 KiB
 * clears any reply a provider can produce in one turn.
 */
export const conversation_body_text_limit = 65_536;

export const ConversationBodyText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length <= conversation_body_text_limit
			? undefined
			: `Expected at most ${conversation_body_text_limit} characters`,
	),
);

/** Bounds a renderer-visible, non-empty label or message. */
export const ConversationSafeText = ConversationText.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 ? undefined : "Expected at least one character",
	),
);

/** Bounds a renderer-visible, non-empty message body. */
export const ConversationSafeBodyText = ConversationBodyText.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 ? undefined : "Expected at least one character",
	),
);

/**
 * An engine failure in Artisan's custody: a stable `AE-*` code the renderer
 * resolves against the error catalog, with the provider's own code and
 * message riding along as evidence. The code is the durable artifact — a
 * future docs page is keyed on it — so the renderer must tolerate codes it
 * does not recognize and fall back to the catalog's unknown definition.
 */
export const ConversationErrorRef = Schema.Struct({
	code: ArtisanErrorCode,
	detail: Schema.optional(ConversationSafeText),
	provider_code: Schema.optional(ConversationSafeText),
	/** When a limit-class failure clears, so the card can say so. */
	resets_at: Schema.optional(IsoDateTime),
});
export type ConversationErrorRef = typeof ConversationErrorRef.Type;

/** Attributes a renderer-safe entity to durable or provider-origin evidence. */
export const ConversationSourceRef = Schema.Struct({
	event_id: Schema.optional(Identifier),
	journal_sequence: Schema.optional(JournalSequence),
	provider: Schema.optional(Identifier),
	reference: Identifier,
});
export type ConversationSourceRef = typeof ConversationSourceRef.Type;

/** The stable fields every normalized conversation entity carries. */
export const ConversationEntityBase = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	created_at: IsoDateTime,
	id: Identifier,
	lifecycle: ConversationLifecycle,
	ordinal: NonNegativeInt,
	parent_id: Schema.optional(Identifier),
	references: Schema.Array(Identifier),
	revision: NonNegativeInt,
	run_id: Schema.optional(Identifier),
	source_refs: Schema.Array(ConversationSourceRef),
	updated_at: IsoDateTime,
});
export type ConversationEntityBase = typeof ConversationEntityBase.Type;

export const ConversationTurn = Schema.Struct({
	...ConversationEntityBase.fields,
	type: Schema.Literal("turn"),
});
export type ConversationTurn = typeof ConversationTurn.Type;

/** The Artisan-owned worker identity one conversation activity refers to. */
export const ConversationActivitySubagent = Schema.Struct({
	agent_id: Identifier,
	display_name: ConversationSafeText,
});
export type ConversationActivitySubagent = typeof ConversationActivitySubagent.Type;

const ConversationItemFields = {
	...ConversationEntityBase.fields,
	turn_id: Identifier,
};

/** Describes the user-visible action bound to an opaque approval interaction. */
export const ConversationApprovalRequest = Schema.Union([
	Schema.Struct({
		command: Schema.optional(ConversationSafeText),
		cwd: Schema.optional(ConversationSafeText),
		kind: Schema.Literal("command"),
		reason: Schema.optional(ConversationSafeText),
	}),
	Schema.Struct({
		kind: Schema.Literal("file_change"),
		reason: Schema.optional(ConversationSafeText),
	}),
	Schema.Struct({
		kind: Schema.Literal("action"),
		reason: Schema.optional(ConversationSafeText),
	}),
]);
export type ConversationApprovalRequest = typeof ConversationApprovalRequest.Type;

/** A normalized, renderer-ready entity. Its `type` is a domain discriminator, not a UI label. */
export const ConversationItem = Schema.Union([
	Schema.Struct({
		attachments: Schema.optional(Schema.Array(ImageAttachmentReference)),
		content: Schema.optional(Schema.Array(UserMessageContentPart)),
		...ConversationItemFields,
		text: ConversationSafeBodyText,
		type: Schema.Literal("user_message"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		phase: ConversationAssistantMessagePhase.pipe(
			Schema.optional,
			Schema.withDecodingDefault(Effect.succeed("unspecified")),
		),
		/**
		 * Renderer-safe cuts recorded when a live run acknowledges a steer while
		 * this provider item is still streaming. They keep later deltas below the
		 * steer without fabricating a second provider item.
		 */
		steering_fragment_boundaries: Schema.optional(
			Schema.Array(ConversationSteeringFragmentBoundary).check(
				Schema.isMaxLength(conversation_maximum_steering_fragment_boundaries),
			),
		),
		text: ConversationBodyText,
		type: Schema.Literal("assistant_message"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		text: ConversationBodyText,
		type: Schema.Literal("reasoning_summary"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		ended_at: Schema.optional(IsoDateTime),
		/** When the provider accepted the turn and began producing run activity. */
		responded_at: Schema.optional(IsoDateTime),
		started_at: IsoDateTime,
		status: ConversationLifecycle,
		title: ConversationSafeText,
		type: Schema.Literal("work_session"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		detail: Schema.optional(ConversationSafeText),
		kind: Identifier,
		label: ConversationSafeText,
		result_ref: Schema.optional(Identifier),
		status: ConversationLifecycle,
		/** Present only when this row describes delegation to one Artisan-owned worker. */
		subagent: Schema.optional(ConversationActivitySubagent),
		type: Schema.Literal("activity"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		file_count: NonNegativeInt,
		file_ids: Schema.Array(Identifier),
		state: Schema.Literals(["pending", "applied", "reverted", "failed"]),
		summary: ConversationSafeText,
		type: Schema.Literal("change_set"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		change_set_id: Identifier,
		diff: Schema.Union([
			Schema.Struct({
				additions: NonNegativeInt,
				deletions: NonNegativeInt,
				kind: Schema.Literal("known"),
			}),
			Schema.Struct({ kind: Schema.Literal("unavailable") }),
		]),
		operation: Schema.Literals(["created", "modified", "deleted", "renamed"]),
		path: ConversationSafeText,
		type: Schema.Literal("file_change"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		entries: Schema.Array(
			Schema.Struct({
				id: Identifier,
				state: Schema.Literals(["pending", "active", "completed", "skipped"]),
				text: ConversationSafeText,
			}),
		),
		state: Schema.Literals(["draft", "active", "completed", "abandoned"]),
		type: Schema.Literal("plan"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		interaction_id: Identifier,
		prompt: ConversationSafeText,
		request: Schema.optional(ConversationApprovalRequest),
		requested_at: IsoDateTime,
		resolution: Schema.optional(ConversationSafeText),
		resolved_at: Schema.optional(IsoDateTime),
		state: Schema.Literals(["requested", "approved", "rejected", "cancelled"]),
		type: Schema.Literal("approval"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		interaction_id: Identifier,
		prompt: ConversationSafeText,
		requested_at: IsoDateTime,
		resolution: Schema.optional(ConversationSafeText),
		resolved_at: Schema.optional(IsoDateTime),
		state: Schema.Literals(["requested", "answered", "cancelled"]),
		type: Schema.Literal("question"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		message: ConversationSafeText,
		retry: Schema.Union([
			Schema.Struct({ kind: Schema.Literal("none") }),
			Schema.Struct({
				after_ms: NonNegativeInt,
				attempt: NonNegativeInt,
				kind: Schema.Literal("scheduled"),
				max_attempts: NonNegativeInt,
			}),
		]),
		type: Schema.Literal("error"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		portability: Schema.Literals(["portable", "provider_bound", "unknown"]),
		state: Schema.Literals(["started", "completed", "failed"]),
		summary: Schema.optional(ConversationSafeText),
		type: Schema.Literal("compaction"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		/**
		 * How loudly the renderer may present the event: an error is a failure the
		 * reader must see, a warning is worth noticing, and info is quiet
		 * provenance such as usage reports.
		 *
		 * Defaulted on decode rather than required: rows written before this field
		 * existed carry no severity, and the only honest fallback for an unknown
		 * row is the quiet tier.
		 */
		/**
		 * The failure in Artisan custody, when this event reports one. Optional
		 * rather than required for the same reason as `severity`: rows written
		 * before the error system existed must still decode, and they simply
		 * render as the plain diagnostics they always were.
		 */
		error: Schema.optional(ConversationErrorRef),
		severity: Schema.Literals(["error", "warning", "info"]).pipe(
			Schema.optional,
			Schema.withDecodingDefault(Effect.succeed("info")),
		),
		summary: ConversationSafeText,
		type: Schema.Literal("native_event"),
	}),
	Schema.Struct({
		...ConversationItemFields,
		continuation: Schema.Literals(["native", "portable"]),
		source_engine_id: Identifier,
		source_model_id: Schema.optional(Schema.NonEmptyString),
		/**
		 * Whether the handoff is still in flight or the next engine has taken the
		 * thread.
		 *
		 * Defaulted on decode rather than required: rows written before this field
		 * existed carry no `state`, and a required key would fail the decode of
		 * every stored item — which fails the whole snapshot read, not just the
		 * handoff row. A legacy row was only ever written once the transition had
		 * landed, which is the same fallback the journal projection applies.
		 */
		state: Schema.Literals(["started", "completed"]).pipe(
			Schema.optional,
			Schema.withDecodingDefault(Effect.succeed("completed")),
		),
		target_engine_id: Identifier,
		target_model_id: Schema.optional(Schema.NonEmptyString),
		type: Schema.Literal("model_transition"),
	}),
]);
export type ConversationItem = typeof ConversationItem.Type;

/** A versioned renderer snapshot. Patch sequence zero means no patches have been applied. */
export const ConversationSnapshot = Schema.Struct({
	conversation_id: Identifier,
	items: Schema.Array(ConversationItem),
	journal_sequence: JournalSequence,
	last_patch_sequence: NonNegativeInt,
	schema_version: SchemaVersion,
	thread_id: Identifier,
	turns: Schema.Array(ConversationTurn),
	updated_at: IsoDateTime,
});
export type ConversationSnapshot = typeof ConversationSnapshot.Type;

/** A validated renderer cursor used to resume a conversation subscription without a snapshot. */
export const ConversationSubscriptionCursor = Schema.Struct({
	conversation_id: Identifier,
	last_patch_sequence: NonNegativeInt,
});
export type ConversationSubscriptionCursor = typeof ConversationSubscriptionCursor.Type;

/** Requests the current canonical conversation projection for one thread. */
export const ConversationQuery = Schema.Struct({
	thread_id: Identifier,
});
export type ConversationQuery = typeof ConversationQuery.Type;

const ConversationPatchBase = {
	patch_id: Identifier,
	sequence: Schema.Int.check(Schema.isGreaterThan(0)),
};

/** A sequenced mutation against a conversation snapshot. */
export const ConversationPatch = Schema.Union([
	Schema.Struct({
		...ConversationPatchBase,
		item: ConversationItem,
		type: Schema.Literal("item_upsert"),
	}),
	Schema.Struct({
		...ConversationPatchBase,
		item_id: Identifier,
		revision: NonNegativeInt,
		text: Schema.String,
		type: Schema.Literal("item_append"),
	}),
	Schema.Struct({
		...ConversationPatchBase,
		item_id: Identifier,
		lifecycle: ConversationLifecycle,
		revision: NonNegativeInt,
		type: Schema.Literal("item_lifecycle"),
	}),
	Schema.Struct({
		...ConversationPatchBase,
		turn: ConversationTurn,
		type: Schema.Literal("turn_upsert"),
	}),
	Schema.Struct({
		...ConversationPatchBase,
		lifecycle: ConversationLifecycle,
		revision: NonNegativeInt,
		turn_id: Identifier,
		type: Schema.Literal("turn_lifecycle"),
	}),
]);
export type ConversationPatch = typeof ConversationPatch.Type;

/** Carries one contiguous patch batch after a known conversation sequence. */
export const ConversationPatchBatch = Schema.Struct({
	conversation_id: Identifier,
	from_sequence: NonNegativeInt,
	patches: Schema.Array(ConversationPatch),
	thread_id: Identifier,
	to_sequence: NonNegativeInt,
});
export type ConversationPatchBatch = typeof ConversationPatchBatch.Type;

/**
 * Reducer-owned replay state. It is intentionally separate from the renderer
 * snapshot.
 *
 * The collections and indexes are owned by one linear reducer chain and mutate
 * in place. A long run applies tens of thousands of patches, so copying every
 * entity array or rebuilding every lookup index per streamed word made the
 * allocation cost grow with transcript history. Consumers must advance with
 * the returned state; retained older states share the chain and are rejected
 * before they can apply a delayed competing patch.
 */
interface ConversationReducerChain {
	readonly item_ids: Set<string>;
	readonly item_index: Map<string, number>;
	readonly items: Array<ConversationItem>;
	last_patch_sequence: number;
	readonly ordinals: Set<number>;
	readonly turn_ids: Set<string>;
	readonly turn_index: Map<string, number>;
	readonly turns: Array<ConversationTurn>;
}

export interface ConversationRebuildState {
	readonly applied_patch_ids: Set<string>;
	/**
	 * Internal linear-chain owner. Optional only for legacy resync placeholders;
	 * normal initialization always creates it. Never reuse an older state after
	 * an accepted patch.
	 */
	readonly reducer?: ConversationReducerChain;
	readonly snapshot: ConversationSnapshot;
}

/**
 * Recent exact-replay window for streamed patch IDs. Four thousand ninety-six
 * entries cover a long burst of token patches without retaining one identifier
 * per token for the lifetime of an open conversation.
 */
export const conversation_patch_dedupe_window = 4_096;

/** Records one accepted patch with O(1) insertion-order eviction. */
const remember_applied_patch_id = (patch_ids: Set<string>, patch_id: string) => {
	patch_ids.add(patch_id);
	if (patch_ids.size <= conversation_patch_dedupe_window) return;
	const oldest = patch_ids.values().next().value;
	if (oldest !== undefined) patch_ids.delete(oldest);
};

export type ConversationInvariantError =
	| {
			readonly _tag: "conversation.invariant_error";
			readonly code:
				| "duplicate_entity_id"
				| "duplicate_ordinal"
				| "invalid_lifecycle_transition"
				| "invalid_parent"
				| "invalid_reference"
				| "invalid_turn"
				| "ordinal_changed"
				| "patch_gap"
				| "revision_not_monotonic"
				| "terminal_immutable"
				| "type_changed";
			readonly message: string;
	  }
	| {
			readonly _tag: "conversation.invariant_error";
			readonly code: "missing_item" | "missing_turn";
			readonly message: string;
	  };

export type ConversationPatchResult =
	| { readonly _tag: "applied"; readonly state: ConversationRebuildState }
	| { readonly _tag: "duplicate"; readonly state: ConversationRebuildState }
	| { readonly _tag: "invariant_error"; readonly error: ConversationInvariantError };

const terminal_lifecycles = new Set<ConversationLifecycle>(["completed", "failed", "cancelled"]);
const invariant_error = (
	code: ConversationInvariantError["code"],
	message: string,
): ConversationPatchResult => ({
	_tag: "invariant_error",
	error: { _tag: "conversation.invariant_error", code, message },
});

const entity_is_terminal = (entity: ConversationEntityBase) =>
	terminal_lifecycles.has(entity.lifecycle);

const legal_lifecycle_transitions: Readonly<
	Record<ConversationLifecycle, ReadonlySet<ConversationLifecycle>>
> = {
	active: new Set(["waiting", "completed", "failed", "cancelled"]),
	cancelled: new Set(),
	completed: new Set(),
	failed: new Set(),
	pending: new Set(["streaming", "active", "waiting", "completed", "failed", "cancelled"]),
	streaming: new Set(["active", "waiting", "completed", "failed", "cancelled"]),
	waiting: new Set(["active", "streaming", "completed", "failed", "cancelled"]),
};

const lifecycle_transition_is_legal = (from: ConversationLifecycle, to: ConversationLifecycle) =>
	from === to || legal_lifecycle_transitions[from].has(to);

const validate_references = (
	entity: ConversationEntityBase,
	item_ids: ReadonlySet<string>,
	turn_ids: ReadonlySet<string>,
): ConversationPatchResult | undefined => {
	if (
		entity.parent_id !== undefined &&
		!item_ids.has(entity.parent_id) &&
		!turn_ids.has(entity.parent_id)
	) {
		return invariant_error("invalid_parent", `Unknown parent ${entity.parent_id}`);
	}
	for (const reference_id of entity.references) {
		if (!item_ids.has(reference_id) && !turn_ids.has(reference_id)) {
			return invariant_error("invalid_reference", `Unknown reference ${reference_id}`);
		}
	}
};

/** Takes ownership of snapshot collections once, with indexes maintained by accepted upserts. */
const make_reducer = (snapshot: ConversationSnapshot): ConversationReducerChain => {
	const turns = [...snapshot.turns];
	const items = [...snapshot.items];
	return {
		item_ids: new Set(items.map((item) => item.id)),
		item_index: new Map(items.map((item, index) => [item.id, index])),
		items,
		last_patch_sequence: snapshot.last_patch_sequence,
		ordinals: new Set([
			...turns.map((turn) => turn.ordinal),
			...items.map((item) => item.ordinal),
		]),
		turn_ids: new Set(turns.map((turn) => turn.id)),
		turn_index: new Map(turns.map((turn, index) => [turn.id, index])),
		turns,
	};
};

/** Creates reducer state after validating the snapshot's internal graph. */
export const InitializeConversation = (snapshot: ConversationSnapshot): ConversationPatchResult => {
	const turn_ids = new Set(snapshot.turns.map((turn) => turn.id));
	const item_ids = new Set(snapshot.items.map((item) => item.id));
	if (turn_ids.size !== snapshot.turns.length || item_ids.size !== snapshot.items.length) {
		return invariant_error("duplicate_entity_id", "Conversation contains duplicate entity IDs");
	}
	const ordinals = new Set([
		...snapshot.turns.map((turn) => turn.ordinal),
		...snapshot.items.map((item) => item.ordinal),
	]);
	if (ordinals.size !== snapshot.turns.length + snapshot.items.length) {
		return invariant_error("duplicate_ordinal", "Conversation contains duplicate ordinals");
	}
	for (const item of snapshot.items) {
		if (!turn_ids.has(item.turn_id))
			return invariant_error("invalid_turn", `Unknown turn ${item.turn_id}`);
		const validation = validate_references(item, item_ids, turn_ids);
		if (validation !== undefined) return validation;
	}
	for (const turn of snapshot.turns) {
		const validation = validate_references(turn, item_ids, turn_ids);
		if (validation !== undefined) return validation;
	}
	const reducer = make_reducer(snapshot);
	return {
		_tag: "applied",
		state: {
			applied_patch_ids: new Set(),
			reducer,
			snapshot: { ...snapshot, items: reducer.items, turns: reducer.turns },
		},
	};
};

/** Applies one patch deterministically, preserving stable identity and stream ordering. */
export const ApplyConversationPatch = (
	state: ConversationRebuildState,
	patch: ConversationPatch,
): ConversationPatchResult => {
	const reducer = state.reducer ?? make_reducer(state.snapshot);
	/** A stale branch shares collections but not the chain cursor, so never let it mutate newer data. */
	if (reducer.last_patch_sequence !== state.snapshot.last_patch_sequence) {
		return invariant_error(
			"patch_gap",
			"Conversation reducer state is stale; continue from the latest accepted state",
		);
	}
	if (state.applied_patch_ids.has(patch.patch_id)) return { _tag: "duplicate", state };
	if (patch.sequence !== reducer.last_patch_sequence + 1) {
		return invariant_error(
			"patch_gap",
			`Expected patch sequence ${reducer.last_patch_sequence + 1}, received ${patch.sequence}`,
		);
	}
	const { items, turns } = reducer;

	if (patch.type === "turn_upsert") {
		const existing_index = reducer.turn_index.get(patch.turn.id);
		if (existing_index === undefined) {
			if (reducer.ordinals.has(patch.turn.ordinal))
				return invariant_error(
					"duplicate_ordinal",
					`Duplicate ordinal ${patch.turn.ordinal}`,
				);
			const validation = validate_references(patch.turn, reducer.item_ids, reducer.turn_ids);
			if (validation !== undefined) return validation;
			turns.push(patch.turn);
			reducer.turn_ids.add(patch.turn.id);
			reducer.turn_index.set(patch.turn.id, turns.length - 1);
			reducer.ordinals.add(patch.turn.ordinal);
		} else {
			const existing = turns[existing_index];
			if (existing === undefined)
				return invariant_error("missing_turn", `Unknown turn ${patch.turn.id}`);
			if (existing.type !== patch.turn.type)
				return invariant_error("type_changed", "Turn type is immutable");
			if (existing.ordinal !== patch.turn.ordinal)
				return invariant_error("ordinal_changed", "Turn ordinal is immutable");
			if (entity_is_terminal(existing))
				return invariant_error("terminal_immutable", "Terminal turn is immutable");
			if (patch.turn.revision <= existing.revision)
				return invariant_error("revision_not_monotonic", "Turn revision must increase");
			if (!lifecycle_transition_is_legal(existing.lifecycle, patch.turn.lifecycle))
				return invariant_error(
					"invalid_lifecycle_transition",
					"Illegal turn lifecycle transition",
				);
			const validation = validate_references(patch.turn, reducer.item_ids, reducer.turn_ids);
			if (validation !== undefined) return validation;
			turns[existing_index] = patch.turn;
		}
	} else if (patch.type === "item_upsert") {
		const existing_index = reducer.item_index.get(patch.item.id);
		if (existing_index === undefined) {
			if (reducer.ordinals.has(patch.item.ordinal))
				return invariant_error(
					"duplicate_ordinal",
					`Duplicate ordinal ${patch.item.ordinal}`,
				);
			if (!reducer.turn_ids.has(patch.item.turn_id))
				return invariant_error("invalid_turn", `Unknown turn ${patch.item.turn_id}`);
			const validation = validate_references(patch.item, reducer.item_ids, reducer.turn_ids);
			if (validation !== undefined) return validation;
			items.push(patch.item);
			reducer.item_ids.add(patch.item.id);
			reducer.item_index.set(patch.item.id, items.length - 1);
			reducer.ordinals.add(patch.item.ordinal);
		} else {
			const existing = items[existing_index];
			if (existing === undefined)
				return invariant_error("missing_item", `Unknown item ${patch.item.id}`);
			if (existing.type !== patch.item.type)
				return invariant_error("type_changed", "Item type is immutable");
			if (existing.ordinal !== patch.item.ordinal)
				return invariant_error("ordinal_changed", "Item ordinal is immutable");
			if (existing.turn_id !== patch.item.turn_id)
				return invariant_error("invalid_turn", "Item turn is immutable");
			if (entity_is_terminal(existing))
				return invariant_error("terminal_immutable", "Terminal item is immutable");
			if (patch.item.revision <= existing.revision)
				return invariant_error("revision_not_monotonic", "Item revision must increase");
			if (!lifecycle_transition_is_legal(existing.lifecycle, patch.item.lifecycle))
				return invariant_error(
					"invalid_lifecycle_transition",
					"Illegal item lifecycle transition",
				);
			const validation = validate_references(patch.item, reducer.item_ids, reducer.turn_ids);
			if (validation !== undefined) return validation;
			items[existing_index] = patch.item;
		}
	} else if (patch.type === "item_append") {
		const existing_index = reducer.item_index.get(patch.item_id);
		if (existing_index === undefined)
			return invariant_error("missing_item", `Unknown item ${patch.item_id}`);
		const existing = items[existing_index];
		if (existing === undefined)
			return invariant_error("missing_item", `Unknown item ${patch.item_id}`);
		if (existing.lifecycle !== "streaming")
			return invariant_error(
				"invalid_lifecycle_transition",
				"Only streaming items can be appended",
			);
		if (existing.type !== "assistant_message" && existing.type !== "reasoning_summary")
			return invariant_error("type_changed", "This item type has no appendable text");
		if (patch.revision <= existing.revision)
			return invariant_error("revision_not_monotonic", "Item revision must increase");
		items[existing_index] = {
			...existing,
			revision: patch.revision,
			text: existing.text + patch.text,
		};
	} else if (patch.type === "item_lifecycle") {
		const existing_index = reducer.item_index.get(patch.item_id);
		if (existing_index === undefined)
			return invariant_error("missing_item", `Unknown item ${patch.item_id}`);
		const existing = items[existing_index];
		if (existing === undefined)
			return invariant_error("missing_item", `Unknown item ${patch.item_id}`);
		if (entity_is_terminal(existing))
			return invariant_error("terminal_immutable", "Terminal item is immutable");
		if (patch.revision <= existing.revision)
			return invariant_error("revision_not_monotonic", "Item revision must increase");
		if (!lifecycle_transition_is_legal(existing.lifecycle, patch.lifecycle))
			return invariant_error(
				"invalid_lifecycle_transition",
				"Illegal item lifecycle transition",
			);
		items[existing_index] = {
			...existing,
			lifecycle: patch.lifecycle,
			revision: patch.revision,
		};
	} else if (patch.type === "turn_lifecycle") {
		const existing_index = reducer.turn_index.get(patch.turn_id);
		if (existing_index === undefined)
			return invariant_error("missing_turn", `Unknown turn ${patch.turn_id}`);
		const existing = turns[existing_index];
		if (existing === undefined)
			return invariant_error("missing_turn", `Unknown turn ${patch.turn_id}`);
		if (entity_is_terminal(existing))
			return invariant_error("terminal_immutable", "Terminal turn is immutable");
		if (patch.revision <= existing.revision)
			return invariant_error("revision_not_monotonic", "Turn revision must increase");
		if (!lifecycle_transition_is_legal(existing.lifecycle, patch.lifecycle))
			return invariant_error(
				"invalid_lifecycle_transition",
				"Illegal turn lifecycle transition",
			);
		turns[existing_index] = {
			...existing,
			lifecycle: patch.lifecycle,
			revision: patch.revision,
		};
	}
	remember_applied_patch_id(state.applied_patch_ids, patch.patch_id);
	reducer.last_patch_sequence = patch.sequence;
	return {
		_tag: "applied",
		state: {
			applied_patch_ids: state.applied_patch_ids,
			reducer,
			snapshot: { ...state.snapshot, items, last_patch_sequence: patch.sequence, turns },
		},
	};
};

/** Replays ordered patches from a snapshot, retaining the first deterministic invariant failure. */
export const RebuildConversation = (
	snapshot: ConversationSnapshot,
	patches: ReadonlyArray<ConversationPatch>,
): ConversationPatchResult => {
	const initial = InitializeConversation(snapshot);
	if (initial._tag === "invariant_error") return initial;
	let state = initial.state;
	for (const patch of patches) {
		const result = ApplyConversationPatch(state, patch);
		if (result._tag === "invariant_error") return result;
		state = result.state;
	}
	return { _tag: "applied", state };
};
