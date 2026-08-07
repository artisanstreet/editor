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
			readonly session: ConversationWorkSession;
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
	readonly item_locations: ReadonlyMap<string, ConversationRenderItemLocation>;
	readonly source_turn_ids: ReadonlySet<string>;
	readonly turn_id: string;
}

/** Locates a source item within its already-rendered group without regrouping it. */
type ConversationRenderItemLocation =
	| { readonly block_index: number; readonly type: "item" }
	| { readonly block_index: number; readonly detail_index: number; readonly type: "work_detail" };

interface ConversationRenderProjection {
	readonly groups_by_id: Map<string, ConversationRenderGroup>;
	readonly group_id_by_turn: Map<string, string>;
	readonly item_ids_by_turn: Map<string, ReadonlyArray<string>>;
	readonly ordered_group_ids: ReadonlyArray<string>;
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
		let turn: ConversationTurn;
		if (patch.type === "turn_upsert") turn = patch.turn;
		else {
			const previous_turn = projection.turns_by_id.get(turn_id);
			if (previous_turn === undefined) return MakeViewState(rebuild);
			turn = {
				...previous_turn,
				lifecycle: patch.lifecycle,
				revision: patch.revision,
			};
		}
		projection.turns_by_id.set(turn_id, turn);
		const group_id = projection.group_id_by_turn.get(turn_id);
		const next = { ...state, rebuild };
		if (group_id !== undefined) RefreshConversationRenderGroup(next, group_id);
		return next;
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
	const existing_group_id = projection.group_id_by_turn.get(item.turn_id);
	if (patch.type === "item_upsert" && previous === undefined) {
		const ids = projection.item_ids_by_turn.get(item.turn_id) ?? [];
		projection.item_ids_by_turn.set(item.turn_id, InsertOrderedItemId(ids, items_by_id, item));
	}
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
	 * A legacy session can merge source turns, while the first item for a turn
	 * establishes its globally ordered group. Both are rare structural changes;
	 * rebuild the projection so aliases and replay ordering remain canonical.
	 */
	if (
		patch.type === "item_upsert" &&
		(item.type === "work_session" ||
			(previous === undefined && existing_group_id === undefined))
	) {
		return { ...next, projection: MakeConversationRenderProjection(next) };
	}
	const group_id = existing_group_id ?? item.turn_id;
	RefreshConversationRenderGroup(next, group_id);
	return next;
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
		groups_by_id: new Map(),
		group_id_by_turn: new Map(),
		item_ids_by_turn: new Map(),
		ordered_group_ids: [],
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
	ordered_items: ReadonlyArray<ConversationItem>,
	turns: ReadonlyArray<ConversationRebuildState["snapshot"]["turns"][number]>,
): ReadonlyArray<ConversationRenderBlock> => {
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
	const turns_by_id = new Map(turns.map((turn) => [turn.id, turn] as const));

	for (const item of ordered_items) {
		const group_key = ConversationRenderKey(item, legacy_work.turn_aliases);
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
		if (item.type === "assistant_message" && item.phase === "final" && item.text.length > 0) {
			final_message_by_turn.set(group_key, item);
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
		if (collapsible_work_types.has(item.type) || item_is_explicit_commentary(item)) return true;
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
				return [
					{
						details: details_by_turn.get(group_key) ?? [],
						duration_kind: concrete_work_turns.has(group_key) ? "worked" : "thought",
						id: `work:${item.id}`,
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
		if (last_block_by_turn.get(turn_id) !== block.id) {
			return [block];
		}
		const trailing_blocks: Array<ConversationRenderBlock> = [block];
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
		state.rebuild.snapshot.turns,
	);

/** Builds durable, state-owned group indexes once for a snapshot replacement. */
const MakeConversationRenderGroup = (
	blocks: ReadonlyArray<ConversationRenderBlock>,
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
	return { blocks, item_locations, source_turn_ids, turn_id };
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
	const group_id_by_turn = new Map<string, string>();
	const item_ids_by_turn = new Map<string, ReadonlyArray<string>>();
	const source_turn_ids_by_group = new Map<string, Set<string>>();
	for (const item of ordered_items) {
		const group_id = ConversationRenderKey(item, legacy_work.turn_aliases);
		group_id_by_turn.set(item.turn_id, group_id);
		const source_turn_ids = source_turn_ids_by_group.get(group_id) ?? new Set<string>();
		source_turn_ids.add(item.turn_id);
		source_turn_ids_by_group.set(group_id, source_turn_ids);
		const ids = item_ids_by_turn.get(item.turn_id);
		if (ids === undefined) item_ids_by_turn.set(item.turn_id, [item.id]);
		else (ids as Array<string>).push(item.id);
	}
	const blocks_by_group_id = new Map<string, Array<ConversationRenderBlock>>();
	const ordered_group_ids: Array<string> = [];
	for (const block of MakeConversationRenderBlocksForItems(
		ordered_items,
		state.rebuild.snapshot.turns,
	)) {
		const blocks = blocks_by_group_id.get(block.turn_id);
		if (blocks === undefined) {
			blocks_by_group_id.set(block.turn_id, [block]);
			ordered_group_ids.push(block.turn_id);
		} else blocks.push(block);
	}
	const groups_by_id = new Map(
		[...blocks_by_group_id.entries()].map(([turn_id, blocks]) => [
			turn_id,
			MakeConversationRenderGroup(
				blocks,
				source_turn_ids_by_group.get(turn_id) ?? new Set(),
				turn_id,
			),
		]),
	);
	return {
		groups_by_id,
		group_id_by_turn,
		item_ids_by_turn,
		ordered_group_ids,
		turns_by_id: new Map(state.rebuild.snapshot.turns.map((turn) => [turn.id, turn])),
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
	const group_id = state.projection.group_id_by_turn.get(item.turn_id);
	const group = group_id === undefined ? undefined : state.projection.groups_by_id.get(group_id);
	if (group === undefined) return false;
	if (
		patch.type === "item_append" &&
		item.type === "assistant_message" &&
		previous.type === "assistant_message" &&
		item.phase === "final" &&
		previous.text.length === 0
	)
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
	if (location.type === "item" && block.type === "item") {
		blocks[location.block_index] = { ...block, item };
	} else if (location.type === "work_detail" && block.type === "work_group") {
		/** The reducer chain owns historical detail entries; replace only this message slot. */
		const details = block.details as Array<ConversationItem>;
		if (details[location.detail_index] === undefined) return false;
		details[location.detail_index] = item;
		blocks[location.block_index] = { ...block, details };
	} else return false;
	state.projection.groups_by_id.set(group.turn_id, { ...group, blocks });
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

/** Rebuilds one cached render group from its indexed source turns only. */
const RefreshConversationRenderGroup = (state: ConversationViewState, group_id: string): void => {
	const projection = state.projection;
	const previous = projection.groups_by_id.get(group_id);
	const source_turn_ids =
		previous?.source_turn_ids ??
		new Set(
			[...projection.group_id_by_turn.entries()]
				.filter(([, candidate]) => candidate === group_id)
				.map(([turn_id]) => turn_id),
		);
	const items = [...source_turn_ids]
		.flatMap((turn_id) => projection.item_ids_by_turn.get(turn_id) ?? [])
		.map((item_id) => state.items_by_id.get(item_id))
		.filter((item): item is ConversationItem => item !== undefined)
		.sort(CompareConversationItemOrder);
	const turns = [...source_turn_ids]
		.map((turn_id) => projection.turns_by_id.get(turn_id))
		.filter((turn): turn is ConversationTurn => turn !== undefined);
	const blocks = MakeConversationRenderBlocksForItems(items, turns);
	if (blocks.length === 0) {
		projection.groups_by_id.delete(group_id);
		return;
	}
	projection.groups_by_id.set(
		group_id,
		MakeConversationRenderGroup(blocks, source_turn_ids, group_id),
	);
	if (!projection.ordered_group_ids.includes(group_id)) {
		(projection.ordered_group_ids as Array<string>).push(group_id);
	}
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
