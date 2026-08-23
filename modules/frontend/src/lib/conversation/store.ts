import {
	ApplyConversationPatch,
	InitializeConversation,
	type ConversationItem,
	type ConversationLifecycle,
	type ConversationPatch,
	type ConversationRebuildState,
	type ConversationSnapshot,
	type ConversationTurn,
} from "@artisan/protocol";

import { conversation_progress_phase, type ConversationProgressPhase } from "./activity-status";

export interface ConversationViewState {
	readonly items_by_id: ReadonlyMap<string, ConversationItem>;
	readonly ordered_item_ids: ReadonlyArray<string>;
	readonly phase: "ready" | "resync_required";
	readonly projection: ConversationRenderProjection;
	readonly rebuild: ConversationRebuildState;
}

type ConversationWorkSession = Extract<ConversationItem, { type: "work_session" }>;
type ConversationChangeSet = Extract<ConversationItem, { type: "change_set" }>;
type ConversationFileChange = Extract<ConversationItem, { type: "file_change" }>;
type ConversationAssistantMessage = Extract<ConversationItem, { type: "assistant_message" }>;
type ConversationStreamingText = Extract<
	ConversationItem,
	{ type: "assistant_message" | "reasoning_summary" }
>;
type ConversationModelTransition = Extract<ConversationItem, { type: "model_transition" }>;

export type ConversationRenderBlock =
	| {
			readonly id: string;
			readonly item: ConversationItem;
			readonly turn_id: string;
			readonly type: "item";
	  }
	| {
			readonly change_sets: ReadonlyArray<ConversationChangeSet>;
			readonly files: ReadonlyArray<ConversationFileChange>;
			readonly id: string;
			readonly turn_id: string;
			readonly type: "changes";
	  }
	| {
			readonly details: ReadonlyArray<ConversationItem>;
			readonly duration_kind: "thought" | "worked";
			readonly id: string;
			/** Newest prose-or-work phase before detail visibility is projected. */
			readonly progress_phase: ConversationProgressPhase;
			/** Full source group retained for status facts after reply prose is reparented. */
			readonly progress_items?: ReadonlyArray<ConversationItem>;
			readonly session: ConversationWorkSession;
			/**
			 * Whether later blocks of the same turn render below this session. A
			 * steered turn lifts its post-steer work out of the session and into
			 * blocks beneath the user's message, so the session is no longer the end
			 * of the turn's flow and must not narrate live status from up there.
			 */
			readonly superseded?: boolean;
			/** The engine handoff that started this run, shown in the session header. */
			readonly transition?: ConversationModelTransition;
			readonly turn_id: string;
			readonly type: "work_group";
	  }
	| {
			readonly id: string;
			readonly settled_at: string;
			readonly text: string;
			readonly turn_id: string;
			readonly type: "turn_footer";
	  };

interface ConversationRenderGroup {
	readonly blocks: ReadonlyArray<ConversationRenderBlock>;
	readonly id: string;
	readonly item_locations: ReadonlyMap<string, ConversationRenderItemLocation>;
	readonly source_turn_ids: ReadonlySet<string>;
	readonly turn_id: string;
}

/** Locates a source item within its already-rendered group without regrouping it. */
type ConversationRenderItemLocation =
	| {
			readonly assistant_fragment_start?: number;
			readonly block_index: number;
			readonly type: "item";
	  }
	| { readonly block_index: number; readonly detail_index: number; readonly type: "work_detail" };

interface FragmentedConversationItems {
	readonly frozen_fragment_ids: ReadonlySet<string>;
	readonly fragmented_source_ids: ReadonlySet<string>;
	readonly items: ReadonlyArray<ConversationItem>;
	readonly latest_fragment_by_source_id: ReadonlyMap<string, string>;
	readonly post_steering_fragment_ids: ReadonlySet<string>;
}

interface ConversationRenderProjection {
	readonly first_steering_ordinal_by_run: ReadonlyMap<string, number>;
	readonly groups_by_id: Map<string, ConversationRenderGroup>;
	readonly group_id_by_item: Map<string, string>;
	readonly group_ids_by_participant_agent_id: ReadonlyMap<string, ReadonlyArray<string>>;
	/** A turn's segments in timeline order, so patches touch only their own turn. */
	readonly group_ids_by_turn_key: Map<string, Array<string>>;
	readonly ordered_group_ids: ReadonlyArray<string>;
	readonly root_group_ids: ReadonlyArray<string>;
	/** Legacy provider-turn aliases, retained so patches can find their render key. */
	readonly turn_aliases: ReadonlyMap<string, string>;
	readonly turns_by_id: Map<string, ConversationTurn>;
}

export interface ConversationRenderWindow {
	readonly blocks: ReadonlyArray<ConversationRenderBlock>;
	readonly hidden_group_count: number;
}

export type ConversationViewPatchResult =
	| { readonly _tag: "applied" | "duplicate"; readonly state: ConversationViewState }
	| {
			readonly _tag: "resync_required";
			readonly expected_sequence: number;
			readonly received_sequence: number;
			readonly state: ConversationViewState;
	  }
	| {
			readonly _tag: "invariant_error";
			readonly message: string;
			readonly state: ConversationViewState;
	  };

/**
 * Snapshot queries and live patches race by design. A delayed query may never
 * replace a projection that has already advanced further on the live stream.
 */
export const CanReplaceConversationSnapshot = (
	current: ConversationSnapshot,
	next: ConversationSnapshot,
): boolean =>
	current.thread_id === next.thread_id &&
	current.conversation_id === next.conversation_id &&
	next.last_patch_sequence >= current.last_patch_sequence;

/** The lowest loaded turn ordinal — the floor an older hydration range starts below. */
export const ConversationTurnFloor = (snapshot: ConversationSnapshot): number | undefined =>
	snapshot.turns.reduce<number | undefined>(
		(floor, turn) => (floor === undefined || turn.ordinal < floor ? turn.ordinal : floor),
		undefined,
	);

/** Whether the durable thread still holds turns older than the loaded window. */
export const ConversationHasRemoteOlderTurns = (snapshot: ConversationSnapshot): boolean => {
	const window = snapshot.window;
	if (window === undefined) return false;
	if (snapshot.turns.length < window.total_turn_count) return true;
	const floor = ConversationTurnFloor(snapshot);
	return floor !== undefined && window.markers.some((marker) => marker.turn_ordinal < floor);
};

/**
 * Merges one hydrated older range beneath the loaded window and rebuilds the
 * view. Only entities the view does not hold join, so a range read that raced
 * live patches can never regress a newer entity, and the view keeps its own
 * patch cursor — hydration never disturbs streaming continuity.
 */
export const MergeConversationRange = (
	state: ConversationViewState,
	range: ConversationSnapshot,
): ConversationViewPatchResult => {
	const current = state.rebuild.snapshot;
	if (
		range.thread_id !== current.thread_id ||
		range.conversation_id !== current.conversation_id
	) {
		return {
			_tag: "invariant_error",
			message: "Range belongs to another conversation",
			state,
		};
	}
	const known_turn_ids = new Set(current.turns.map((turn) => turn.id));
	const added_turns = range.turns.filter((turn) => !known_turn_ids.has(turn.id));
	const merged_turn_ids = new Set([...known_turn_ids, ...added_turns.map((turn) => turn.id)]);
	const known_item_ids = new Set(current.items.map((item) => item.id));
	const added_items = range.items.filter(
		(item) => !known_item_ids.has(item.id) && merged_turn_ids.has(item.turn_id),
	);
	if (added_turns.length === 0 && added_items.length === 0) {
		return { _tag: "applied", state };
	}
	const merged: ConversationSnapshot = {
		...current,
		items: [...current.items, ...added_items].sort(
			(left, right) => left.ordinal - right.ordinal || left.id.localeCompare(right.id),
		),
		turns: [...current.turns, ...added_turns].sort(
			(left, right) => left.ordinal - right.ordinal || left.id.localeCompare(right.id),
		),
	};
	return MakeConversationViewState(merged);
};

const MakeViewState = (
	rebuild: ConversationRebuildState,
	phase: ConversationViewState["phase"] = "ready",
): ConversationViewState => {
	const items_by_id = new Map(rebuild.snapshot.items.map((item) => [item.id, item]));
	const ordered_item_ids = [...items_by_id.values()]
		.sort((left, right) => left.ordinal - right.ordinal || left.id.localeCompare(right.id))
		.map((item) => item.id);

	const state = {
		items_by_id,
		ordered_item_ids,
		phase,
		rebuild,
	} satisfies Omit<ConversationViewState, "projection">;
	return { ...state, projection: MakeConversationRenderProjection(state) };
};

const CompareConversationItemOrder = (left: ConversationItem, right: ConversationItem) =>
	left.ordinal - right.ordinal || left.id.localeCompare(right.id);

/** Inserts a new item without re-sorting the entire history for an out-of-order replay. */
const InsertOrderedItemId = (
	ordered_item_ids: ReadonlyArray<string>,
	items_by_id: ReadonlyMap<string, ConversationItem>,
	item: ConversationItem,
): ReadonlyArray<string> => {
	const tail_id = ordered_item_ids.at(-1);
	const tail = tail_id === undefined ? undefined : items_by_id.get(tail_id);
	if (tail === undefined || CompareConversationItemOrder(tail, item) < 0)
		return [...ordered_item_ids, item.id];

	let start = 0;
	let end = ordered_item_ids.length;
	while (start < end) {
		const middle = Math.floor((start + end) / 2);
		const middle_id = ordered_item_ids[middle];
		const middle_item = middle_id === undefined ? undefined : items_by_id.get(middle_id);
		if (middle_item === undefined || CompareConversationItemOrder(middle_item, item) < 0)
			start = middle + 1;
		else end = middle;
	}
	return [...ordered_item_ids.slice(0, start), item.id, ...ordered_item_ids.slice(start)];
};

/** Protocol validation permits append patches only for these two text-bearing item kinds. */
const AppendConversationItemText = (
	item: ConversationItem,
	text: string,
	revision: number,
): ConversationItem => {
	if (item.type !== "assistant_message" && item.type !== "reasoning_summary") return item;
	return { ...item, revision, text: item.text + text };
};

/**
 * `ConversationRebuildState` already deliberately shares its applied patch set
 * across reducer states. The renderer index follows the same reducer-chain
 * ownership: only the newest state is observable, and an applied item patch
 * mutates the shared map entry after the protocol reducer accepts it. This
 * avoids copying the whole transcript map once per streamed token while the
 * returned state object still gives Svelte a fresh reactive value.
 */
const ApplyViewCollections = (
	state: ConversationViewState,
	rebuild: ConversationRebuildState,
	patch: ConversationPatch,
): ConversationViewState => {
	const items_by_id = state.items_by_id as Map<string, ConversationItem>;
	const projection = state.projection;
	if (patch.type === "turn_upsert" || patch.type === "turn_lifecycle") {
		const turn_id = patch.type === "turn_upsert" ? patch.turn.id : patch.turn_id;
		const previous_turn = projection.turns_by_id.get(turn_id);
		let turn: ConversationTurn;
		if (patch.type === "turn_upsert") turn = patch.turn;
		else {
			if (previous_turn === undefined) return MakeViewState(rebuild);
			turn = {
				...previous_turn,
				lifecycle: patch.lifecycle,
				revision: patch.revision,
			};
		}
		projection.turns_by_id.set(turn_id, turn);
		const next = { ...state, rebuild };
		/** A turn arriving ahead of its items renders nothing; only the index changes. */
		if (previous_turn === undefined) return next;
		const stable_identity =
			previous_turn.parent_id === turn.parent_id &&
			previous_turn.agent_id === turn.agent_id;
		const render_key = projection.turn_aliases.get(turn_id) ?? turn_id;
		if (stable_identity && TryUpdateStructuralRenderTurn(next, render_key, undefined)) {
			return next;
		}
		return { ...next, projection: MakeConversationRenderProjection(next) };
	}

	const previous =
		patch.type === "item_upsert"
			? items_by_id.get(patch.item.id)
			: items_by_id.get(patch.item_id);
	let item: ConversationItem;
	if (patch.type === "item_upsert") item = patch.item;
	else {
		if (previous === undefined) return MakeViewState(rebuild);
		item =
			patch.type === "item_append"
				? AppendConversationItemText(previous, patch.text, patch.revision)
				: { ...previous, lifecycle: patch.lifecycle, revision: patch.revision };
	}
	items_by_id.set(item.id, item);
	const ordered_item_ids =
		patch.type === "item_upsert" && previous === undefined
			? InsertOrderedItemId(state.ordered_item_ids, items_by_id, item)
			: state.ordered_item_ids;
	const next = {
		...state,
		rebuild,
		ordered_item_ids,
	};
	if (
		patch.type !== "item_upsert" &&
		previous !== undefined &&
		UpdateNonStructuralRenderItem(next, item, previous, patch)
	) {
		return next;
	}
	/**
	 * A structural item patch reshapes at most its own turn's segments unless it
	 * moves run-wide grouping inputs. The dirty-turn path rebuilds only that
	 * turn; anything it cannot prove safe falls through to the canonical full
	 * rebuild.
	 */
	const render_key = projection.turn_aliases.get(item.turn_id) ?? item.turn_id;
	const appended_item =
		patch.type === "item_upsert" && previous === undefined ? item : undefined;
	if (
		CanUpdateStructuralRenderItem(item, previous, ordered_item_ids) &&
		TryUpdateStructuralRenderTurn(next, render_key, appended_item)
	) {
		return next;
	}
	return { ...next, projection: MakeConversationRenderProjection(next) };
};

/** Normalizes a validated snapshot into identity-keyed, ordinal-ordered renderer state. */
export const MakeConversationViewState = (
	snapshot: ConversationSnapshot,
): ConversationViewPatchResult => {
	const result = InitializeConversation(snapshot);
	return result._tag === "invariant_error"
		? {
				_tag: "invariant_error",
				message: result.error.message,
				state: MakeEmptyConversationViewState(snapshot),
			}
		: { _tag: "applied", state: MakeViewState(result.state) };
};

const MakeEmptyConversationViewState = (snapshot: ConversationSnapshot): ConversationViewState => ({
	items_by_id: new Map(),
	ordered_item_ids: [],
	phase: "resync_required",
	projection: {
		first_steering_ordinal_by_run: new Map(),
		groups_by_id: new Map(),
		group_id_by_item: new Map(),
		group_ids_by_participant_agent_id: new Map(),
		group_ids_by_turn_key: new Map(),
		ordered_group_ids: [],
		root_group_ids: [],
		turn_aliases: new Map(),
		turns_by_id: new Map(),
	},
	rebuild: { applied_patch_ids: new Set(), snapshot },
});

const collapsible_work_types = new Set<ConversationItem["type"]>([
	"reasoning_summary",
	"activity",
	"compaction",
	"native_event",
]);

/** Provider commentary is intermediate work, not a second final response. */
const item_is_explicit_commentary = (item: ConversationItem): boolean =>
	item.type === "assistant_message" && item.phase === "commentary";

/** JSON tuple encoding keeps synthetic fragment identities collision-free. */
const AssistantFragmentId = (item_id: string, text_offset: number): string =>
	JSON.stringify(["assistant-fragment", item_id, text_offset]);

const AssistantFragmentStart = (item_id: string): number | undefined => {
	try {
		const value: unknown = JSON.parse(item_id);
		return Array.isArray(value) &&
			value[0] === "assistant-fragment" &&
			typeof value[2] === "number"
			? value[2]
			: undefined;
	} catch {
		return undefined;
	}
};

const MakeAssistantFragment = <Item extends ConversationStreamingText>(
	item: Item,
	start: number,
	end?: number,
	frozen = false,
): Item => ({
	...item,
	id: AssistantFragmentId(item.id, start),
	lifecycle: frozen ? "completed" : item.lifecycle,
	text: item.text.slice(start, end),
});

/** Splits one straddled provider message into source-ordered and user-anchored ranges. */
const FragmentAssistantMessages = (
	ordered_items: ReadonlyArray<ConversationItem>,
): FragmentedConversationItems => {
	const anchored = new Map<string, Array<ConversationStreamingText>>();
	const prefix_by_source_id = new Map<string, ConversationStreamingText>();
	const frozen_fragment_ids = new Set<string>();
	const fragmented_source_ids = new Set<string>();
	const latest_fragment_by_source_id = new Map<string, string>();
	const post_steering_fragment_ids = new Set<string>();
	const item_ids = new Set(ordered_items.map((item) => item.id));
	for (const item of ordered_items) {
		if (
			(item.type !== "assistant_message" && item.type !== "reasoning_summary") ||
			(item.type === "assistant_message" && item.phase === "commentary") ||
			item.steering_fragment_boundaries === undefined ||
			item.steering_fragment_boundaries.length === 0
		)
			continue;
		const boundaries = [...item.steering_fragment_boundaries]
			.filter((boundary) => boundary.text_offset <= item.text.length)
			.sort((left, right) => left.text_offset - right.text_offset);
		const first = boundaries[0];
		if (
			first === undefined ||
			!boundaries.every((boundary) => item_ids.has(boundary.after_item_id))
		)
			continue;
		fragmented_source_ids.add(item.id);
		const prefix = MakeAssistantFragment(item, 0, first.text_offset, true);
		if (prefix.text.length > 0) {
			prefix_by_source_id.set(item.id, prefix);
			frozen_fragment_ids.add(prefix.id);
		}
		for (const [index, boundary] of boundaries.entries()) {
			const fragment = MakeAssistantFragment(
				item,
				boundary.text_offset,
				boundaries[index + 1]?.text_offset,
				index < boundaries.length - 1,
			);
			if (fragment.text.length > 0) {
				const values = anchored.get(boundary.after_item_id) ?? [];
				values.push(fragment);
				anchored.set(boundary.after_item_id, values);
				post_steering_fragment_ids.add(fragment.id);
				if (index < boundaries.length - 1) frozen_fragment_ids.add(fragment.id);
			}
			if (index === boundaries.length - 1 && fragment.text.length > 0)
				latest_fragment_by_source_id.set(item.id, fragment.id);
		}
	}
	const items: Array<ConversationItem> = [];
	for (const item of ordered_items) {
		const prefix = prefix_by_source_id.get(item.id);
		if (prefix !== undefined) items.push(prefix);
		else if (!fragmented_source_ids.has(item.id)) items.push(item);
		if (item.type === "user_message") items.push(...(anchored.get(item.id) ?? []));
	}
	return {
		frozen_fragment_ids,
		fragmented_source_ids,
		items,
		latest_fragment_by_source_id,
		post_steering_fragment_ids,
	};
};

const item_is_change = (
	item: ConversationItem,
): item is ConversationChangeSet | ConversationFileChange =>
	item.type === "change_set" || item.type === "file_change";

interface LegacyWorkAliases {
	readonly canonical_sessions: ReadonlyMap<string, ConversationWorkSession>;
	readonly turn_aliases: ReadonlyMap<string, string>;
}

/**
 * Older exec projections persisted one Artisan run session and one provider-turn
 * session for the same prompt. Alias only that exact historical pair; arbitrary
 * provider turns and user messages that happen to share a run ID stay untouched.
 */
const MakeLegacyWorkAliases = (
	sessions: ReadonlyArray<ConversationWorkSession>,
): LegacyWorkAliases => {
	const sessions_by_id = new Map(sessions.map((session) => [session.id, session]));
	const canonical_sessions = new Map<string, ConversationWorkSession>();
	const turn_aliases = new Map<string, string>();

	for (const session of sessions) {
		const run_id = session.run_id;
		if (
			run_id === undefined ||
			session.id !== `work:run:${run_id}` ||
			session.turn_id !== `run:${run_id}`
		)
			continue;
		const provider_session = sessions_by_id.get(`work:exec:${run_id}:turn`);
		if (
			provider_session?.run_id !== run_id ||
			provider_session.turn_id !== `exec:${run_id}:turn`
		)
			continue;
		canonical_sessions.set(session.turn_id, session);
		turn_aliases.set(provider_session.turn_id, session.turn_id);
	}

	return { canonical_sessions, turn_aliases };
};

const ConversationRenderKey = (
	item: ConversationItem,
	turn_aliases: ReadonlyMap<string, string>,
): string => turn_aliases.get(item.turn_id) ?? item.turn_id;

/**
 * A queued prompt precedes its run session; a steer arrives after one already
 * exists. That durable ordering is the renderer's delimiter between prior work
 * and the live response to the acknowledged steer.
 */
const MakeFirstSteeringOrdinalByRun = (
	ordered_items: ReadonlyArray<ConversationItem>,
): ReadonlyMap<string, number> => {
	const first_work_ordinal_by_run = new Map<string, number>();
	for (const item of ordered_items) {
		if (item.type !== "work_session" || item.run_id === undefined) continue;
		const first_ordinal = first_work_ordinal_by_run.get(item.run_id);
		if (first_ordinal === undefined || item.ordinal < first_ordinal) {
			first_work_ordinal_by_run.set(item.run_id, item.ordinal);
		}
	}

	const first_steering_ordinal_by_run = new Map<string, number>();
	for (const item of ordered_items) {
		if (item.type !== "user_message" || item.run_id === undefined) continue;
		const work_ordinal = first_work_ordinal_by_run.get(item.run_id);
		if (
			work_ordinal === undefined ||
			work_ordinal >= item.ordinal ||
			first_steering_ordinal_by_run.has(item.run_id)
		)
			continue;
		first_steering_ordinal_by_run.set(item.run_id, item.ordinal);
	}
	return first_steering_ordinal_by_run;
};

/**
 * Groups renderer-owned intermediate work beneath its typed session marker.
 * Commentary stays in its source ordinal between activity blocks; only final
 * assistant replies remain independently visible.
 */
/** Turn outcomes that mean no further work will land in the turn. */
const settled_turn_lifecycles: ReadonlySet<ConversationLifecycle> = new Set([
	"completed",
	"failed",
	"cancelled",
]);

const MakeConversationRenderBlocksForItems = (
	source_ordered_items: ReadonlyArray<ConversationItem>,
	turns_by_id: ReadonlyMap<string, ConversationTurn>,
	first_steering_ordinal_by_run = MakeFirstSteeringOrdinalByRun(source_ordered_items),
	fragments = FragmentAssistantMessages(source_ordered_items),
): ReadonlyArray<ConversationRenderBlock> => {
	const ordered_items = fragments.items;
	const legacy_work = MakeLegacyWorkAliases(
		ordered_items.filter(
			(item): item is ConversationWorkSession => item.type === "work_session",
		),
	);
	const sessions_by_turn = new Map<string, Array<ConversationWorkSession>>();
	const details_by_turn = new Map<string, Array<ConversationItem>>();
	const change_sets_by_turn = new Map<string, Array<ConversationChangeSet>>();
	const files_by_turn = new Map<string, Array<ConversationFileChange>>();
	const transition_by_turn = new Map<string, ConversationModelTransition>();
	const concrete_work_turns = new Set<string>();
	const final_message_by_turn = new Map<string, ConversationAssistantMessage>();
	const latest_reply_by_turn = new Map<string, ConversationAssistantMessage>();
	const items_by_turn = new Map<string, Array<ConversationItem>>();

	for (const item of ordered_items) {
		const group_key = ConversationRenderKey(item, legacy_work.turn_aliases);
		const turn_items = items_by_turn.get(group_key) ?? [];
		turn_items.push(item);
		items_by_turn.set(group_key, turn_items);
		if (item.type === "activity" || item_is_change(item)) {
			concrete_work_turns.add(group_key);
		}
		if (item.type === "work_session") {
			const sessions = sessions_by_turn.get(group_key) ?? [];
			sessions.push(item);
			sessions_by_turn.set(group_key, sessions);
		} else if (item.type === "change_set") {
			const change_sets = change_sets_by_turn.get(group_key) ?? [];
			change_sets.push(item);
			change_sets_by_turn.set(group_key, change_sets);
		} else if (item.type === "file_change") {
			const files = files_by_turn.get(group_key) ?? [];
			files.push(item);
			files_by_turn.set(group_key, files);
		} else if (item.type === "model_transition" && !transition_by_turn.has(group_key)) {
			transition_by_turn.set(group_key, item);
		}
		if (
			item.type === "assistant_message" &&
			!fragments.frozen_fragment_ids.has(item.id) &&
			item.phase !== "commentary" &&
			item.text.length > 0
		) {
			latest_reply_by_turn.set(group_key, item);
			if (item.phase === "final") final_message_by_turn.set(group_key, item);
		}
	}

	const work_session_by_turn = new Map(
		[...sessions_by_turn.entries()].flatMap(([group_key, sessions]) => {
			const canonical = legacy_work.canonical_sessions.get(group_key);
			if (canonical !== undefined) return [[group_key, canonical] as const];
			const only_session = sessions.length === 1 ? sessions.at(0) : undefined;
			return only_session === undefined ? [] : [[group_key, only_session] as const];
		}),
	);
	const ItemFollowsSteering = (item: ConversationItem): boolean => {
		if (fragments.post_steering_fragment_ids.has(item.id)) return true;
		if (item.run_id === undefined) return false;
		const steering_ordinal = first_steering_ordinal_by_run.get(item.run_id);
		return steering_ordinal !== undefined && item.ordinal > steering_ordinal;
	};
	/**
	 * The latest model prose is the visible reply while it remains the newest
	 * phase, even for providers that call every message `unspecified`. If a tool
	 * or reasoning item lands afterwards, the next rebuild returns that prose to
	 * work history and reopens the disclosure.
	 */
	for (const [group_key, candidate] of latest_reply_by_turn) {
		if (conversation_progress_phase(items_by_turn.get(group_key) ?? []) === "reply") {
			final_message_by_turn.set(group_key, candidate);
		}
	}

	/**
	 * Historical exec projections can label every assistant message as final. In
	 * a real work turn, the last completed non-commentary message is the settled
	 * reply; all prior assistant messages remain progress in their source order.
	 */
	const last_item_id_by_turn = new Map<string, string>();
	for (const item of ordered_items) {
		last_item_id_by_turn.set(ConversationRenderKey(item, legacy_work.turn_aliases), item.id);
	}

	for (const item of ordered_items) {
		const group_key = ConversationRenderKey(item, legacy_work.turn_aliases);
		const work_session = work_session_by_turn.get(group_key);
		if (
			work_session === undefined ||
			item.type !== "assistant_message" ||
			fragments.frozen_fragment_ids.has(item.id) ||
			item.phase === "commentary" ||
			item.lifecycle !== "completed" ||
			item.text.length === 0
		)
			continue;
		const turn_completed = turns_by_id.get(group_key)?.lifecycle === "completed";
		/**
		 * Some providers (Claude) never emit phase "final" and can leave their
		 * turn's completion patch dangling behind a stuck reasoning item that is
		 * fixed upstream. When the work session itself has already settled and
		 * this message is the last item anywhere in the turn — no later tool
		 * call or message queued behind it — promote the reply without waiting
		 * on the turn's own lifecycle patch. Requiring "last item" is what keeps
		 * a mid-turn tool sequence from promoting an interim message early: a
		 * later item in the same turn withholds promotion until it, in turn,
		 * becomes the settled last item.
		 */
		const session_settled = work_session.lifecycle === "completed";
		const is_last_item_in_turn = last_item_id_by_turn.get(group_key) === item.id;
		if (turn_completed || (session_settled && is_last_item_in_turn)) {
			final_message_by_turn.set(group_key, item);
		}
	}

	const ItemIsWorkDetail = (item: ConversationItem, group_key: string): boolean => {
		const work_session = work_session_by_turn.get(group_key);
		if (work_session === undefined) return false;
		if (ItemFollowsSteering(item)) return false;
		if (collapsible_work_types.has(item.type) || item_is_explicit_commentary(item)) return true;
		if (fragments.frozen_fragment_ids.has(item.id)) return true;
		return (
			item.type === "assistant_message" &&
			item.id !== final_message_by_turn.get(group_key)?.id
		);
	};

	for (const item of ordered_items) {
		const group_key = ConversationRenderKey(item, legacy_work.turn_aliases);
		if (!ItemIsWorkDetail(item, group_key)) continue;
		const details = details_by_turn.get(group_key) ?? [];
		details.push(item);
		details_by_turn.set(group_key, details);
	}

	const content_blocks = ordered_items.flatMap(
		(
			item,
		): ReadonlyArray<
			Exclude<ConversationRenderBlock, { type: "changes" } | { type: "turn_footer" }>
		> => {
			const group_key = ConversationRenderKey(item, legacy_work.turn_aliases);
			if (item.type === "plan") return [];
			if (item_is_change(item)) return [];
			if (ItemIsWorkDetail(item, group_key)) return [];
			/**
			 * A run's engine handoff renders inside its work session header, not
			 * as a standalone timeline row. It stays standalone only when its
			 * turn produced no session to host it.
			 */
			if (
				item.type === "model_transition" &&
				transition_by_turn.get(group_key) === item &&
				work_session_by_turn.has(group_key)
			)
				return [];
			if (item.type === "work_session") {
				const session = work_session_by_turn.get(group_key);
				if (session === undefined || session.id !== item.id) return [];
				const transition = transition_by_turn.get(group_key);
				const turn_items = items_by_turn.get(group_key) ?? [];
				const progress_phase = conversation_progress_phase(turn_items);
				return [
					{
						details: details_by_turn.get(group_key) ?? [],
						duration_kind: concrete_work_turns.has(group_key) ? "worked" : "thought",
						id: `work:${item.id}`,
						progress_phase,
						progress_items: turn_items,
						session,
						...(transition === undefined ? {} : { transition }),
						turn_id: group_key,
						type: "work_group",
					},
				];
			}
			return [{ id: item.id, item, turn_id: group_key, type: "item" }];
		},
	);
	const last_block_by_turn = new Map<string, string>();

	for (const block of content_blocks) {
		last_block_by_turn.set(block.turn_id, block.id);
	}

	return content_blocks.flatMap((block): ReadonlyArray<ConversationRenderBlock> => {
		const { turn_id } = block;
		const files = files_by_turn.get(turn_id) ?? [];
		const change_sets = change_sets_by_turn.get(turn_id) ?? [];
		const is_last_block = last_block_by_turn.get(turn_id) === block.id;
		/**
		 * A turn narrates itself in one place, at the end of its flow. Marking the
		 * session as superseded is what keeps that true once steering has moved
		 * later work below the user's message.
		 */
		const positioned: ConversationRenderBlock =
			block.type === "work_group" ? { ...block, superseded: !is_last_block } : block;
		if (!is_last_block) {
			return [positioned];
		}
		const trailing_blocks: Array<ConversationRenderBlock> = [positioned];
		const final_message = final_message_by_turn.get(turn_id);
		const turn =
			final_message === undefined
				? turns_by_id.get(turn_id)
				: (turns_by_id.get(final_message.turn_id) ?? turns_by_id.get(turn_id));
		/**
		 * Held back until the turn settles. A change set is a summary of what the
		 * turn did, and a summary that appears while the turn is still working
		 * keeps growing under the reader — and reads as a finished result when it
		 * is not one. Any terminal outcome qualifies, not just success: a
		 * cancelled run still changed the files it changed.
		 */
		const turn_settled = turn !== undefined && settled_turn_lifecycles.has(turn.lifecycle);
		if (turn_settled && (files.length > 0 || change_sets.length > 0)) {
			trailing_blocks.push({
				change_sets,
				files,
				id: `changes:${turn_id}`,
				turn_id,
				type: "changes",
			});
		}
		if (turn?.lifecycle === "completed" && final_message?.lifecycle === "completed") {
			trailing_blocks.push({
				id: `footer:${turn_id}`,
				settled_at: turn.updated_at,
				text: final_message.text,
				turn_id,
				type: "turn_footer",
			});
		}
		return trailing_blocks;
	});
};

/** Retained full projection for non-virtual consumers and semantic regression tests. */
export const MakeConversationRenderBlocks = (
	state: ConversationViewState,
): ReadonlyArray<ConversationRenderBlock> =>
	MakeConversationRenderBlocksForItems(
		state.ordered_item_ids
			.map((item_id) => state.items_by_id.get(item_id))
			.filter((item): item is ConversationItem => item !== undefined),
		new Map(state.rebuild.snapshot.turns.map((turn) => [turn.id, turn])),
	);

/** Builds durable, state-owned group indexes once for a snapshot replacement. */
const MakeConversationRenderGroup = (
	blocks: ReadonlyArray<ConversationRenderBlock>,
	id: string,
	latest_fragment_by_source_id: ReadonlyMap<string, string>,
	source_turn_ids: ReadonlySet<string>,
	turn_id: string,
): ConversationRenderGroup => {
	const item_locations = new Map<string, ConversationRenderItemLocation>();
	for (const [block_index, block] of blocks.entries()) {
		if (block.type === "item") {
			item_locations.set(block.item.id, { block_index, type: "item" });
			continue;
		}
		if (block.type !== "work_group") continue;
		for (const [detail_index, detail] of block.details.entries())
			item_locations.set(detail.id, { block_index, detail_index, type: "work_detail" });
	}
	for (const [source_id, fragment_id] of latest_fragment_by_source_id) {
		const location = item_locations.get(fragment_id);
		const assistant_fragment_start = AssistantFragmentStart(fragment_id);
		if (location?.type !== "item" || assistant_fragment_start === undefined) continue;
		item_locations.set(source_id, { ...location, assistant_fragment_start });
	}
	return { blocks, id, item_locations, source_turn_ids, turn_id };
};

const MakeConversationRenderProjection = (
	state: Omit<ConversationViewState, "projection">,
): ConversationRenderProjection => {
	const ordered_items = state.ordered_item_ids
		.map((item_id) => state.items_by_id.get(item_id))
		.filter((item): item is ConversationItem => item !== undefined);
	const legacy_work = MakeLegacyWorkAliases(
		ordered_items.filter(
			(item): item is ConversationWorkSession => item.type === "work_session",
		),
	);
	const group_id_by_item = new Map<string, string>();
	const source_turn_ids_by_group = new Map<string, Set<string>>();
	for (const item of ordered_items) {
		const group_id = ConversationRenderKey(item, legacy_work.turn_aliases);
		const source_turn_ids = source_turn_ids_by_group.get(group_id) ?? new Set<string>();
		source_turn_ids.add(item.turn_id);
		source_turn_ids_by_group.set(group_id, source_turn_ids);
	}
	const groups_by_id = new Map<string, ConversationRenderGroup>();
	const ordered_group_ids: Array<string> = [];
	const root_group_ids: Array<string> = [];
	const group_ids_by_participant_agent_id = new Map<string, Array<string>>();
	const group_ids_by_turn_key = new Map<string, Array<string>>();
	const turns_by_id = new Map(state.rebuild.snapshot.turns.map((turn) => [turn.id, turn]));
	const first_steering_ordinal_by_run = MakeFirstSteeringOrdinalByRun(ordered_items);
	const fragments = FragmentAssistantMessages(ordered_items);
	const blocks = MakeConversationRenderBlocksForItems(
		ordered_items,
		turns_by_id,
		first_steering_ordinal_by_run,
		fragments,
	);
	let segment_start = 0;
	while (segment_start < blocks.length) {
		const first_block = blocks[segment_start];
		if (first_block === undefined) break;
		let segment_end = segment_start + 1;
		while (blocks[segment_end]?.turn_id === first_block.turn_id) segment_end += 1;
		const segment_blocks = blocks.slice(segment_start, segment_end);
		/** Stable first-block identity prevents earlier paging/inserts from remounting this segment. */
		const segment_id = JSON.stringify([first_block.turn_id, first_block.id]);
		const group = MakeConversationRenderGroup(
			segment_blocks,
			segment_id,
			fragments.latest_fragment_by_source_id,
			source_turn_ids_by_group.get(first_block.turn_id) ?? new Set(),
			first_block.turn_id,
		);
		groups_by_id.set(segment_id, group);
		ordered_group_ids.push(segment_id);
		const key_group_ids = group_ids_by_turn_key.get(first_block.turn_id) ?? [];
		key_group_ids.push(segment_id);
		group_ids_by_turn_key.set(first_block.turn_id, key_group_ids);
		for (const item_id of group.item_locations.keys())
			group_id_by_item.set(item_id, segment_id);
		const source_turns = [...group.source_turn_ids].map((turn_id) => turns_by_id.get(turn_id));
		if (
			source_turns.length > 0 &&
			source_turns.every((turn) => turn !== undefined && turn.parent_id === undefined)
		) {
			root_group_ids.push(segment_id);
		} else {
			const participant_agent_id = source_turns[0]?.agent_id;
			if (
				participant_agent_id !== undefined &&
				source_turns.every(
					(turn) =>
						turn !== undefined &&
						turn.parent_id !== undefined &&
						turn.agent_id === participant_agent_id,
				)
			) {
				const group_ids = group_ids_by_participant_agent_id.get(participant_agent_id) ?? [];
				group_ids.push(segment_id);
				group_ids_by_participant_agent_id.set(participant_agent_id, group_ids);
			}
		}
		segment_start = segment_end;
	}
	return {
		first_steering_ordinal_by_run,
		groups_by_id,
		group_id_by_item,
		group_ids_by_participant_agent_id,
		group_ids_by_turn_key,
		ordered_group_ids,
		root_group_ids,
		turn_aliases: legacy_work.turn_aliases,
		turns_by_id,
	};
};

/**
 * Replaces an item in its existing render slot. Append and lifecycle patches
 * cannot change grouping except at final-message/footer boundaries; those
 * structural cases intentionally fall through to the canonical rebuild below.
 */
const UpdateNonStructuralRenderItem = (
	state: ConversationViewState,
	item: ConversationItem,
	previous: ConversationItem,
	patch: Exclude<ConversationPatch, { readonly type: "item_upsert" }>,
): boolean => {
	if (
		patch.type === "item_append" &&
		item.type !== "assistant_message" &&
		item.type !== "reasoning_summary"
	)
		return false;
	if (item.type !== previous.type) return false;
	const group_id = state.projection.group_id_by_item.get(item.id);
	const group = group_id === undefined ? undefined : state.projection.groups_by_id.get(group_id);
	if (group === undefined) return false;
	if (
		patch.type === "item_append" &&
		(item.type === "assistant_message" || item.type === "reasoning_summary") &&
		(previous.type === "assistant_message" || previous.type === "reasoning_summary") &&
		previous.text.length === 0
	)
		/** First visible text can change both disclosure phase and item ownership. */
		return false;
	if (
		patch.type === "item_lifecycle" &&
		item.type === "assistant_message" &&
		([...group.source_turn_ids].some((turn_id) => {
			const lifecycle = state.projection.turns_by_id.get(turn_id)?.lifecycle;
			return lifecycle !== undefined && settled_turn_lifecycles.has(lifecycle);
		}) ||
			group.blocks.some(
				(block) => block.type === "work_group" && block.session.lifecycle === "completed",
			))
	)
		return false;
	const location = group.item_locations.get(item.id);
	if (location === undefined) return false;
	const blocks = [...group.blocks];
	const block = blocks[location.block_index];
	if (block === undefined) return false;
	const rendered_item =
		location.type === "item" &&
		location.assistant_fragment_start !== undefined &&
		item.type === "assistant_message"
			? MakeAssistantFragment(item, location.assistant_fragment_start)
			: item;
	if (location.type === "item" && block.type === "item") {
		blocks[location.block_index] = { ...block, item: rendered_item };
	} else if (location.type === "work_detail" && block.type === "work_group") {
		/** The reducer chain owns historical detail entries; replace only this message slot. */
		const details = block.details as Array<ConversationItem>;
		if (details[location.detail_index] === undefined) return false;
		details[location.detail_index] = item;
		blocks[location.block_index] = { ...block, details };
	} else return false;
	state.projection.groups_by_id.set(group.id, { ...group, blocks });
	/**
	 * Status metadata retains the full group after visible prose moves outside
	 * its details — including a work group split into an earlier segment than
	 * the promoted reply it narrates, so every segment of the turn is checked.
	 */
	const key_group_ids = state.projection.group_ids_by_turn_key.get(group.turn_id) ?? [
		group.id,
	];
	for (const key_group_id of key_group_ids) {
		const key_group = state.projection.groups_by_id.get(key_group_id);
		if (key_group === undefined) continue;
		let progress_blocks: Array<ConversationRenderBlock> | undefined;
		for (const [block_index, candidate] of key_group.blocks.entries()) {
			if (candidate.type !== "work_group") continue;
			const prior_progress_items = candidate.progress_items ?? [
				candidate.session,
				...candidate.details,
			];
			const progress_index = prior_progress_items.findIndex(
				(progress_item) =>
					progress_item.id === item.id || progress_item.id === rendered_item.id,
			);
			if (progress_index < 0) continue;
			const progress_items = [...prior_progress_items];
			progress_items[progress_index] = rendered_item;
			progress_blocks = progress_blocks ?? [...key_group.blocks];
			progress_blocks[block_index] = { ...candidate, progress_items };
		}
		if (progress_blocks !== undefined)
			state.projection.groups_by_id.set(key_group_id, {
				...key_group,
				blocks: progress_blocks,
			});
	}
	return true;
};

/**
 * Whether a structural item patch is provably confined to its own turn.
 * A new steering prompt or work session moves run-wide grouping inputs, a
 * pre-fragmented new message can anchor outside its arrival order, and only a
 * tail arrival keeps every earlier segment's global position. Replacements
 * keep type, ordinal, and turn by protocol invariant; a changed run identity
 * would stale the cached steering ordinals.
 */
const CanUpdateStructuralRenderItem = (
	item: ConversationItem,
	previous: ConversationItem | undefined,
	ordered_item_ids: ReadonlyArray<string>,
): boolean => {
	if (previous !== undefined) return item.run_id === previous.run_id;
	if (item.type === "user_message" || item.type === "work_session") return false;
	if (
		(item.type === "assistant_message" || item.type === "reasoning_summary") &&
		(item.steering_fragment_boundaries?.length ?? 0) > 0
	)
		return false;
	return ordered_item_ids.at(-1) === item.id;
};

/**
 * Rebuilds one turn's blocks and splices them into that turn's existing
 * segments, leaving every other cached group untouched. Segment identities are
 * anchored to their first block, so the rebuilt blocks are partitioned at the
 * previous first-block ids — sound because a turn's blocks map onto its
 * segments monotonically and other turns' interleave points cannot move while
 * only this turn changes. Any shape this partition cannot prove — a vanished
 * boundary, a first render landing mid-history, a steering boundary anchored
 * outside the turn — returns false and the caller performs the canonical full
 * rebuild.
 */
const TryUpdateStructuralRenderTurn = (
	state: ConversationViewState,
	render_key: string,
	appended_item: ConversationItem | undefined,
): boolean => {
	const projection = state.projection;
	const turn_items: Array<ConversationItem> = [];
	const turn_item_ids = new Set<string>();
	for (const item_id of state.ordered_item_ids) {
		const candidate = state.items_by_id.get(item_id);
		if (candidate === undefined) continue;
		if ((projection.turn_aliases.get(candidate.turn_id) ?? candidate.turn_id) !== render_key)
			continue;
		turn_items.push(candidate);
		turn_item_ids.add(candidate.id);
	}
	const previous_segment_ids = projection.group_ids_by_turn_key.get(render_key) ?? [];
	if (turn_items.length === 0) return previous_segment_ids.length === 0;
	for (const item of turn_items) {
		if (item.type !== "assistant_message" && item.type !== "reasoning_summary") continue;
		const boundaries = item.steering_fragment_boundaries ?? [];
		/** A boundary anchored outside this turn regroups across segments. */
		if (boundaries.some((boundary) => !turn_item_ids.has(boundary.after_item_id)))
			return false;
	}
	const fragments = FragmentAssistantMessages(turn_items);
	const blocks = MakeConversationRenderBlocksForItems(
		turn_items,
		projection.turns_by_id,
		projection.first_steering_ordinal_by_run,
		fragments,
	);
	if (blocks.length === 0) return previous_segment_ids.length === 0;

	const previous_groups: Array<ConversationRenderGroup> = [];
	for (const group_id of previous_segment_ids) {
		const group = projection.groups_by_id.get(group_id);
		if (group === undefined) return false;
		previous_groups.push(group);
	}
	const segments: Array<ReadonlyArray<ConversationRenderBlock>> = [];
	let created_segment_blocks: ReadonlyArray<ConversationRenderBlock> | undefined;
	if (previous_groups.length === 0) {
		/** A first render for this turn is positionally valid only at the global tail. */
		if (appended_item === undefined) return false;
		created_segment_blocks = blocks;
	} else {
		const boundary_ids = previous_groups.map((group) => group.blocks.at(0)?.id);
		if (boundary_ids.some((id) => id === undefined)) return false;
		if (blocks.at(0)?.id !== boundary_ids.at(0)) return false;
		let cursor = 0;
		let current: Array<ConversationRenderBlock> = [];
		for (const block of blocks) {
			if (cursor + 1 < boundary_ids.length && block.id === boundary_ids[cursor + 1]) {
				segments.push(current);
				current = [];
				cursor += 1;
			}
			current.push(block);
		}
		segments.push(current);
		/** A vanished segment boundary means this turn's interleave shape changed. */
		if (segments.length !== previous_groups.length) return false;
		const last_owns_global_tail =
			projection.ordered_group_ids.at(-1) === previous_segment_ids.at(-1);
		if (appended_item !== undefined && !last_owns_global_tail) {
			/**
			 * Another turn owns the global tail, so the appended item cannot extend
			 * this turn's last segment in place — its own block must open a new
			 * tail segment, and everything before it must be shape-identical to
			 * the previous last segment.
			 */
			const last_segment = segments.at(-1) ?? [];
			const previous_last_blocks = previous_groups.at(-1)?.blocks ?? [];
			const split_index = last_segment.findIndex(
				(block) => block.id === appended_item.id,
			);
			const retained =
				split_index < 0 ? last_segment : last_segment.slice(0, split_index);
			if (
				retained.length !== previous_last_blocks.length ||
				retained.some(
					(block, index) => block.id !== previous_last_blocks[index]?.id,
				)
			)
				return false;
			if (split_index >= 0) {
				segments[segments.length - 1] = retained;
				created_segment_blocks = last_segment.slice(split_index);
			}
		}
		/**
		 * A patch on an existing item can still materialize a block that never
		 * rendered before — reply promotion pulls a message out of its work
		 * details. Such a block is positionally safe only where its global
		 * placement is provable: trailing turn summaries follow the turn's last
		 * content block, and anything else must land in the turn's final segment
		 * while that segment owns the global tail. Every other new id may belong
		 * between other turns' groups, which only the full rebuild can see.
		 */
		const previous_block_ids = new Set(
			previous_groups.flatMap((group) => group.blocks.map((block) => block.id)),
		);
		const trailing_ids = new Set([`changes:${render_key}`, `footer:${render_key}`]);
		for (const [index, segment_blocks] of segments.entries()) {
			const is_last = index === segments.length - 1;
			for (const block of segment_blocks) {
				if (previous_block_ids.has(block.id)) continue;
				if (is_last && trailing_ids.has(block.id)) continue;
				if (is_last && block.id === appended_item?.id) continue;
				if (is_last && last_owns_global_tail) continue;
				return false;
			}
		}
		if (created_segment_blocks !== undefined) {
			for (const block of created_segment_blocks) {
				if (block.id === appended_item?.id || trailing_ids.has(block.id)) continue;
				return false;
			}
		}
	}
	const created_first_block = created_segment_blocks?.at(0);
	const created_segment_id =
		created_first_block === undefined
			? undefined
			: JSON.stringify([render_key, created_first_block.id]);
	if (created_segment_id !== undefined && projection.groups_by_id.has(created_segment_id))
		return false;

	const source_turn_ids = new Set(turn_items.map((item) => item.turn_id));
	for (const group of previous_groups) {
		for (const item_id of group.item_locations.keys())
			projection.group_id_by_item.delete(item_id);
	}
	const replace_group = (
		segment_blocks: ReadonlyArray<ConversationRenderBlock>,
		segment_id: string,
	) => {
		const group = MakeConversationRenderGroup(
			segment_blocks,
			segment_id,
			fragments.latest_fragment_by_source_id,
			source_turn_ids,
			render_key,
		);
		projection.groups_by_id.set(segment_id, group);
		for (const item_id of group.item_locations.keys())
			projection.group_id_by_item.set(item_id, segment_id);
	};
	for (const [index, segment_blocks] of segments.entries()) {
		const segment_id = previous_segment_ids[index];
		if (segment_id === undefined) return false;
		replace_group(segment_blocks, segment_id);
	}
	if (created_segment_blocks !== undefined && created_segment_id !== undefined) {
		replace_group(created_segment_blocks, created_segment_id);
		(projection.ordered_group_ids as Array<string>).push(created_segment_id);
		const key_group_ids = projection.group_ids_by_turn_key.get(render_key);
		if (key_group_ids === undefined)
			projection.group_ids_by_turn_key.set(render_key, [created_segment_id]);
		else key_group_ids.push(created_segment_id);
		const source_turns = [...source_turn_ids].map((turn_id) =>
			projection.turns_by_id.get(turn_id),
		);
		if (
			source_turns.length > 0 &&
			source_turns.every((turn) => turn !== undefined && turn.parent_id === undefined)
		) {
			(projection.root_group_ids as Array<string>).push(created_segment_id);
		} else {
			const participant_agent_id = source_turns[0]?.agent_id;
			if (
				participant_agent_id !== undefined &&
				source_turns.every(
					(turn) =>
						turn !== undefined &&
						turn.parent_id !== undefined &&
						turn.agent_id === participant_agent_id,
				)
			) {
				const group_ids =
					projection.group_ids_by_participant_agent_id.get(participant_agent_id);
				if (group_ids === undefined) {
					(
						projection.group_ids_by_participant_agent_id as Map<
							string,
							Array<string>
						>
					).set(participant_agent_id, [created_segment_id]);
				} else (group_ids as Array<string>).push(created_segment_id);
			}
		}
	}
	return true;
};

/**
 * Returns the newest group window plus any explicitly paged older groups. The
 * cache is updated only for the patched group, so streamed words never regroup
 * historical turns or allocate historical block arrays.
 */
export const MakeConversationRenderWindow = (
	state: ConversationViewState,
	group_limit: number,
	older_group_count = 0,
): ConversationRenderWindow => {
	const visible_count = Math.max(1, group_limit) + Math.max(0, older_group_count);
	const start = Math.max(0, state.projection.ordered_group_ids.length - visible_count);
	const group_ids = state.projection.ordered_group_ids.slice(start);
	return {
		blocks: group_ids.flatMap(
			(group_id) => state.projection.groups_by_id.get(group_id)?.blocks ?? [],
		),
		hidden_group_count: start,
	};
};

/**
 * The extra history window required to mount the group containing an item.
 *
 * Turn navigation is built from the complete durable snapshot, while the DOM
 * deliberately keeps only a bounded tail mounted. Returning an absolute older
 * count lets a caller reveal exactly the missing prefix before looking for the
 * item's element, without paging one screen at a time or mounting all history.
 */
export const ConversationOlderGroupCountForItem = (
	state: ConversationViewState,
	participant_agent_id: string | undefined,
	group_limit: number,
	item_id: string,
): number | undefined => {
	const group_id = state.projection.group_id_by_item.get(item_id);
	if (group_id === undefined) return undefined;
	const group_ids =
		participant_agent_id === undefined
			? state.projection.root_group_ids
			: (state.projection.group_ids_by_participant_agent_id.get(participant_agent_id) ?? []);
	const group_index = group_ids.indexOf(group_id);
	if (group_index < 0) return undefined;

	return Math.max(0, group_ids.length - Math.max(1, group_limit) - group_index);
};

/**
 * Keeps one shared conversation subscription while rendering a participant's
 * own turns. Root turns are parentless; adopted workers are parented and carry
 * their durable agent identity, so no provider identity leaks into the view.
 */
export const MakeParticipantConversationRenderWindow = (
	state: ConversationViewState,
	participant_agent_id: string | undefined,
	group_limit: number,
	older_group_count = 0,
): ConversationRenderWindow => {
	const group_ids =
		participant_agent_id === undefined
			? state.projection.root_group_ids
			: (state.projection.group_ids_by_participant_agent_id.get(participant_agent_id) ?? []);
	const visible_count = Math.max(1, group_limit) + Math.max(0, older_group_count);
	const start = Math.max(0, group_ids.length - visible_count);
	const visible_group_ids = group_ids.slice(start);
	return {
		blocks: visible_group_ids.flatMap(
			(group_id) => state.projection.groups_by_id.get(group_id)?.blocks ?? [],
		),
		hidden_group_count: start,
	};
};

/** Applies only contiguous protocol patches. A gap leaves the prior view intact and requests resync. */
export const ApplyConversationViewPatch = (
	state: ConversationViewState,
	patch: ConversationPatch,
): ConversationViewPatchResult => {
	if (state.phase === "resync_required") {
		return {
			_tag: "resync_required",
			expected_sequence: state.rebuild.snapshot.last_patch_sequence + 1,
			received_sequence: patch.sequence,
			state,
		};
	}

	const result = ApplyConversationPatch(state.rebuild, patch);
	if (result._tag === "applied" || result._tag === "duplicate") {
		return {
			_tag: result._tag,
			state:
				result._tag === "applied"
					? ApplyViewCollections(state, result.state, patch)
					: state,
		};
	}
	if (result.error.code === "patch_gap") {
		const resync_state = { ...state, phase: "resync_required" as const };
		return {
			_tag: "resync_required",
			expected_sequence: state.rebuild.snapshot.last_patch_sequence + 1,
			received_sequence: patch.sequence,
			state: resync_state,
		};
	}
	return { _tag: "invariant_error", message: result.error.message, state };
};
