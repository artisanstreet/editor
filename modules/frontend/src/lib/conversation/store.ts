import {
	ApplyConversationPatch,
	InitializeConversation,
	type ConversationItem,
	type ConversationLifecycle,
	type ConversationPatch,
	type ConversationRebuildState,
	type ConversationSnapshot,
} from "@artisan/protocol";

export interface ConversationViewState {
	readonly items_by_id: ReadonlyMap<string, ConversationItem>;
	readonly ordered_item_ids: ReadonlyArray<string>;
	readonly phase: "ready" | "resync_required";
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

	return { items_by_id, ordered_item_ids, phase, rebuild };
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

export const MakeConversationRenderBlocks = (
	state: ConversationViewState,
): ReadonlyArray<ConversationRenderBlock> => {
	const ordered_items = state.ordered_item_ids
		.map((item_id) => state.items_by_id.get(item_id))
		.filter((item): item is ConversationItem => item !== undefined);
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
	const turns_by_id = new Map(
		state.rebuild.snapshot.turns.map((turn) => [turn.id, turn] as const),
	);

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
		return { _tag: result._tag, state: MakeViewState(result.state) };
	}
	if (result.error.code === "patch_gap") {
		const resync_state = MakeViewState(state.rebuild, "resync_required");
		return {
			_tag: "resync_required",
			expected_sequence: state.rebuild.snapshot.last_patch_sequence + 1,
			received_sequence: patch.sequence,
			state: resync_state,
		};
	}
	return { _tag: "invariant_error", message: result.error.message, state };
};
