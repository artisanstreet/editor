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
 * Names the two genuinely different quiet phases of a live turn. Before the
 * provider accepts it, the request is waiting; after that, an intentionally
 * content-free thinking verb can describe private work without exposing it.
 */
export const active_work_label_for = (
	seed: string,
	engine_name: string | undefined,
	provider_responded: boolean,
	thinking_visibility_generation = 0,
): string =>
	provider_responded
		? thinking_word_for(seed, thinking_visibility_generation)
		: (waiting_label_for(engine_name) ??
			thinking_word_for(seed, thinking_visibility_generation));

/**
 * Reconciles one projected session against the durable run that is live now.
 * A thread-global boolean is insufficient: an older orphaned session would
 * otherwise become live again whenever a later turn starts.
 */
export const conversation_work_session_is_active = (
	session_run_id: string | undefined,
	active_run_id: string | undefined,
	thread_run_active: boolean,
): boolean => thread_run_active && session_run_id !== undefined && session_run_id === active_run_id;

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
