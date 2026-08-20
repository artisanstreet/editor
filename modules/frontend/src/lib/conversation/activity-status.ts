import raw_thinking_words from "@artisan/data/activity-status/thinking-words.json";
import type { ConversationItem, ConversationLifecycle } from "@artisan/protocol";
import { Schema } from "effect";

const ThinkingWordVocabulary = Schema.NonEmptyArray(Schema.NonEmptyString);
const fallback_thinking_words = [
	"Pondering",
	"Percolating",
	"Recombobulating",
	"Puttering",
	"Zesting",
] as const;

const load_thinking_words = (): ReadonlyArray<string> => {
	try {
		const decoded = Schema.decodeUnknownSync(ThinkingWordVocabulary)(raw_thinking_words);
		const unique = [...new Set(decoded)];
		return unique.length === decoded.length ? unique : fallback_thinking_words;
	} catch {
		return fallback_thinking_words;
	}
};

export const artisan_thinking_words = load_thinking_words();

/**
 * What a work session says before its engine has said anything. A thinking verb
 * would claim thought that has not started — the request is simply out — so the
 * status names the side the wait is on instead. Work with no engine to name
 * keeps its verb, which is the honest answer when nobody knows who is answering.
 *
 * Takes the engine's display name rather than its id: resolving an id pulls in
 * the provider marks, and those cannot load outside a browser.
 */
export const waiting_label_for = (engine_name: string | undefined): string | undefined =>
	engine_name === undefined ? undefined : `Waiting for ${engine_name} to respond…`;

/**
 * What a full window says while the engine rewrites it. Names the work rather
 * than the wait, because a compaction is the engine doing something — and one
 * that has to summarize a million tokens holds the stream silent for minutes,
 * which under a waiting line is indistinguishable from a run that has died.
 */
export const compacting_label_for = (awaiting_compaction: boolean): string | undefined =>
	awaiting_compaction ? "Compacting the conversation…" : undefined;

/**
 * The honest status once the reply has landed and delegated work is the only
 * thing keeping the turn open: the model is not thinking, it is waiting on the
 * named workers. Two names still read as a sentence; a crowd reads as a count.
 */
export const background_work_label_for = (names: ReadonlyArray<string>): string | undefined =>
	names.length === 0
		? undefined
		: names.length === 1
			? `Waiting for ${names[0]} to finish…`
			: names.length === 2
				? `Waiting for ${names[0]} and ${names[1]} to finish…`
				: `Waiting for ${names.length} background agents…`;

export const thinking_word_at = (index: number): string =>
	artisan_thinking_words[index % artisan_thinking_words.length] ?? fallback_thinking_words[0];

/**
 * Chooses one thinking word for a rendered quiet-status epoch. The session
 * identity picks the starting point and the visibility generation advances
 * through the same vocabulary deterministically, so a mounted line never
 * changes underneath the reader and its later reappearance is distinct.
 */
export const thinking_word_for = (seed: string, visibility_generation = 0): string => {
	let hash = 2_166_136_261;
	for (const character of seed) {
		hash ^= character.codePointAt(0) ?? 0;
		hash = Math.imul(hash, 16_777_619) >>> 0;
	}
	return thinking_word_at(hash + visibility_generation);
};

/**
 * Names the genuinely different quiet phases of a live turn. A provider-started
 * external activity means the model is waiting for that operation. Before the
 * provider accepts the turn, the request is waiting on the provider. Delegated
 * workers still running after the model has spoken own the quiet stretch they
 * cause. Otherwise the model's own latest summary of what it is doing is the
 * best possible line, and an intentionally content-free thinking verb is the
 * fallback for every model and moment that has no summary to offer.
 */
export const active_work_label_for = (input: {
	/**
	 * True when the engine has to compact before it can answer. It outranks the
	 * wait it replaces because it is the more specific truth about the same
	 * silence: the request is indeed out to the provider, but naming only that
	 * turns a compaction — minutes long, with nothing on the stream — into what
	 * reads as a stalled turn.
	 */
	readonly awaiting_compaction?: boolean;
	/** Delegated workers still running behind the model's latest words. */
	readonly background_agent_names?: ReadonlyArray<string>;
	readonly engine_name: string | undefined;
	readonly provider_responded: boolean;
	/**
	 * The model's own words for the thought it is having now, from a model that
	 * publishes summaries. It supersedes the verb and nothing else: the waits
	 * above are facts about the run that outrank whatever the model last said,
	 * and it is only ever the verb that this replaces with something true.
	 */
	readonly reasoning_summary?: string | undefined;
	readonly seed: string;
	readonly thinking_visibility_generation?: number;
	/** True while a provider-started command or tool has not emitted its terminal item event. */
	readonly waiting_for_activity: boolean;
}): string =>
	input.waiting_for_activity
		? "Waiting"
		: !input.provider_responded
			? (compacting_label_for(input.awaiting_compaction ?? false) ??
				waiting_label_for(input.engine_name) ??
				thinking_word_for(input.seed, input.thinking_visibility_generation))
			: (background_work_label_for(input.background_agent_names ?? []) ??
				input.reasoning_summary ??
				thinking_word_for(input.seed, input.thinking_visibility_generation));

/** The durable statuses after which no further work arrives for a session. */
const settled_statuses: ReadonlySet<ConversationLifecycle> = new Set([
	"completed",
	"failed",
	"interrupted",
	"cancelled",
]);

/**
 * Whether a session's own durable status says its run is over. Exported so the
 * few places that gate on liveness read the same fact the header settles on,
 * rather than deriving a second opinion from somewhere else.
 */
export const work_session_is_settled = (status: ConversationLifecycle): boolean =>
	settled_statuses.has(status);

/** The instant a session's header presents as its end. */
export interface WorkSessionSettlement {
	readonly ended_at: string;
}

/**
 * Resolves whether a session's header may present an end — the only source a
 * duration header can be formatted from.
 *
 * The session's own projected status is the whole input, and that is the
 * point. This used to reconcile the transcript against a separately fetched
 * work item, and because that item rode its own transport it could disagree
 * with the transcript for as long as nothing happened to refresh it: the run
 * ended, the thread list lit its attention dot, and this header went on
 * thinking for as long as the reader left it alone. One fact carried on one
 * sequence cannot disagree with itself, so the disagreement is now
 * unrepresentable rather than merely unlikely.
 *
 * A run killed without emitting its own terminal event — a Forge restart takes
 * the engine process with it — is closed by startup recovery, which journals
 * the interruption and reaches this through the same patches as everything
 * else. The header then settles on what actually happened, instead of on a
 * guess the renderer made about a silence.
 */
export const work_session_settlement = (input: {
	readonly ended_at: string | undefined;
	readonly status: ConversationLifecycle;
	readonly updated_at: string;
}): WorkSessionSettlement | undefined =>
	input.ended_at !== undefined
		? { ended_at: input.ended_at }
		: work_session_is_settled(input.status)
			? /** Terminal without an `ended_at` is a row from before that field. */
				{ ended_at: input.updated_at }
			: undefined;

const live_lifecycles: ReadonlySet<ConversationLifecycle> = new Set([
	"pending",
	"streaming",
	"active",
	"waiting",
]);

/**
 * A live activity must be live on both axes: `lifecycle` tracks the item's
 * stream state and `status` the work's outcome. Engines can leave a failed
 * command's lifecycle dangling open, and that ghost must not read as running.
 */
export const conversation_activity_is_live = (
	activity: Extract<ConversationItem, { type: "activity" }>,
): boolean => live_lifecycles.has(activity.lifecycle) && live_lifecycles.has(activity.status);

/**
 * True between a canonical activity's provider-started and terminal events.
 * The activity item is patched in place, so this flips without polling when a
 * command, tool, search, or other external operation starts or completes.
 */
export const conversation_has_live_activity = (items: ReadonlyArray<ConversationItem>): boolean =>
	items.some((item) => item.type === "activity" && conversation_activity_is_live(item));

/**
 * True when the newest relevant detail is an external operation that remains
 * live. A later model-text item proves the model is active again even if an
 * earlier long-running command is still open; a still-later operation restores
 * the wait. Ordinals are the durable cross-item order — timestamps and revisions
 * are local to one item and cannot establish replay-safe recency.
 */
export const conversation_waiting_for_activity = (
	items: ReadonlyArray<ConversationItem>,
): boolean => {
	let newest_live_activity_ordinal: number | undefined;
	let newest_model_text_ordinal: number | undefined;

	for (const item of items) {
		if (item.type === "activity" && conversation_activity_is_live(item)) {
			newest_live_activity_ordinal = Math.max(
				newest_live_activity_ordinal ?? -1,
				item.ordinal,
			);
			continue;
		}
		if (
			(item.type === "assistant_message" || item.type === "reasoning_summary") &&
			item.text.trim().length > 0
		) {
			newest_model_text_ordinal = Math.max(newest_model_text_ordinal ?? -1, item.ordinal);
		}
	}

	return (
		newest_live_activity_ordinal !== undefined &&
		(newest_model_text_ordinal === undefined ||
			newest_live_activity_ordinal > newest_model_text_ordinal)
	);
};

/**
 * Names the delegated workers this turn has visibly handed work to and then
 * spoken past: a live subagent row older than the newest non-empty model text
 * means the model said its piece and the run is held open only by delegated
 * work. That is the one quiet phase a thinking verb misreports — the private
 * work it claims is already finished. A live subagent newer than the text is
 * an ordinary foreground wait and stays with the activity wait above; ordinals
 * are the durable cross-item order there and here for the same reason.
 */
export const conversation_background_agent_names = (
	items: ReadonlyArray<ConversationItem>,
): ReadonlyArray<string> => {
	let newest_model_text_ordinal: number | undefined;
	for (const item of items) {
		if (
			(item.type === "assistant_message" || item.type === "reasoning_summary") &&
			item.text.trim().length > 0
		) {
			newest_model_text_ordinal = Math.max(newest_model_text_ordinal ?? -1, item.ordinal);
		}
	}
	if (newest_model_text_ordinal === undefined) return [];
	const text_ordinal = newest_model_text_ordinal;

	const names = items
		.filter(
			(item): item is Extract<ConversationItem, { type: "activity" }> =>
				item.type === "activity" &&
				item.subagent !== undefined &&
				item.ordinal < text_ordinal &&
				conversation_activity_is_live(item),
		)
		.sort((first, second) => first.ordinal - second.ordinal)
		.flatMap((item) => (item.subagent === undefined ? [] : [item.subagent.display_name]));

	return [...new Set(names)];
};

export type ConversationProgressPhase = "none" | "reply" | "work";

/**
 * The newest visible kind of progress in a work session. Durable ordinals,
 * rather than item lifecycles, decide whether prose has retired the tool chain
 * or work has resumed after prose: engines can leave a mid-turn message
 * streaming for the rest of the run, and a completed final paragraph can
 * arrive in the same projection batch as settlement.
 */
export const conversation_progress_phase = (
	items: ReadonlyArray<ConversationItem>,
): ConversationProgressPhase => {
	let newest_reply_ordinal = -1;
	let newest_work_ordinal = -1;
	for (const item of items) {
		if (item.type === "assistant_message" && item.text.trim().length > 0) {
			newest_reply_ordinal = Math.max(newest_reply_ordinal, item.ordinal);
		}
		if (
			item.type === "activity" ||
			(item.type === "reasoning_summary" && item.text.trim().length > 0)
		) {
			newest_work_ordinal = Math.max(newest_work_ordinal, item.ordinal);
		}
	}
	if (newest_reply_ordinal === -1 && newest_work_ordinal === -1) return "none";
	return newest_reply_ordinal > newest_work_ordinal ? "reply" : "work";
};

/**
 * True while the assistant's own prose is arriving and has something to show.
 * A reply speaks for itself, so it takes over as the status.
 *
 * Nothing else qualifies, and each exclusion is a way the status line went
 * missing over a live turn:
 *
 * - Running activities are not a reply. A tool chain is activities separated by
 *   gaps in which nothing is live, so counting them tore the line out and
 *   replayed its entrance between every call.
 * - Reasoning is not a reply either. It is the thinking the word stands for, and
 *   it opens and closes repeatedly inside one chain, which made the line blink
 *   in and out for the length of the run.
 * - Text still empty is not a reply. A streamed item exists from its first
 *   delta, so an item that has yet to carry a character used to silence the
 *   status line while rendering nothing — a turn that reads as frozen for as
 *   long as the provider stays quiet.
 */
export const conversation_reply_is_live = (items: ReadonlyArray<ConversationItem>): boolean =>
	items.some(
		(item) =>
			item.type === "assistant_message" &&
			live_lifecycles.has(item.lifecycle) &&
			item.text.trim().length > 0,
	);
