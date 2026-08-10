import raw_thinking_words from "@artisan/data/activity-status/thinking-words.json";
import type { ConversationItem, ConversationLifecycle, ThreadWorkItem } from "@artisan/protocol";
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
 * provider accepts the turn, the request is waiting on the provider. Otherwise,
 * an intentionally content-free thinking verb can describe private work without
 * exposing it.
 */
export const active_work_label_for = (input: {
	readonly engine_name: string | undefined;
	readonly provider_responded: boolean;
	readonly seed: string;
	readonly thinking_visibility_generation?: number;
	/** True while a provider-started command or tool has not emitted its terminal item event. */
	readonly waiting_for_activity: boolean;
}): string =>
	input.waiting_for_activity
		? "Waiting"
		: !input.provider_responded
			? (waiting_label_for(input.engine_name) ??
				thinking_word_for(input.seed, input.thinking_visibility_generation))
			: thinking_word_for(input.seed, input.thinking_visibility_generation);

/**
 * The durable coordinator's verdict on the run behind one work session — the
 * input a session header needs before it may claim an end.
 *
 * - `pending`: the run is expected to produce work it has not started yet.
 * - `active`: the durable work item says the run is running or waiting.
 * - `settled`: the authority holds nothing live for this run. Whatever the
 *   transcript still claims, the run is over.
 */
export type WorkSessionRunAuthority = "pending" | "active" | "settled";

/**
 * Reconciles one projected session against the durable work item, which is the
 * authority on what is running. A thread-global boolean is insufficient twice
 * over: an older orphaned session would become live again whenever a later
 * turn starts, and the send gap — where the new session reaches the transcript
 * before the durable work item catches up — would read as a dead run.
 *
 * The mismatch branch resolves that gap: a session the work item does not
 * describe is settled history, unless it is the transcript's newest session —
 * then the authority simply has not caught up with a session this new (the
 * work item is created and refreshed on its own channel), and presuming death
 * there would flash a failure over every freshly sent message.
 */
export const work_session_run_authority = (input: {
	readonly session_run_id: string | undefined;
	readonly active_run_id: string | undefined;
	readonly active_run_status: ThreadWorkItem["status"] | undefined;
	readonly newest_session_run_id: string | undefined;
}): WorkSessionRunAuthority => {
	if (input.session_run_id === undefined) return "settled";
	if (input.session_run_id !== input.active_run_id) {
		return input.session_run_id === input.newest_session_run_id ? "pending" : "settled";
	}
	return input.active_run_status === "queued"
		? "pending"
		: input.active_run_status === "running" || input.active_run_status === "waiting"
			? "active"
			: "settled";
};

/** The instant a session's header presents as its end, and on whose word. */
export interface WorkSessionSettlement {
	readonly ended_at: string;
	/**
	 * True when the run died before the provider ever answered: there is no
	 * terminal event and nothing to show for the wait, so the only honest
	 * header is a failure. A dead run that did respond keeps its ordinary
	 * header — its work is real; the loss is only the terminal event.
	 */
	readonly presumed_failed: boolean;
}

/**
 * Resolves whether a session's header may present an end — the only source a
 * duration header can be formatted from.
 *
 * A genuine terminal `ended_at` always settles the session. Without one, the
 * durable authority decides: while it holds the run as pending or active the
 * session keeps waiting, so the send gap can never flash a fabricated
 * "Thought for 0s". Once the authority has nothing live for the run, the
 * session settles on when it last changed — as a presumed failure when the
 * provider never responded, because a run that died before its first byte
 * did fail, and a waiting line that never resolves would claim otherwise.
 */
export const work_session_settlement = (input: {
	readonly ended_at: string | undefined;
	readonly updated_at: string;
	readonly run_authority: WorkSessionRunAuthority;
	readonly provider_responded: boolean;
}): WorkSessionSettlement | undefined =>
	input.ended_at !== undefined
		? { ended_at: input.ended_at, presumed_failed: false }
		: input.run_authority !== "settled"
			? undefined
			: { ended_at: input.updated_at, presumed_failed: !input.provider_responded };

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
