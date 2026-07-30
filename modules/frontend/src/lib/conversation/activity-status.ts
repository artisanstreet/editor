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

export const thinking_word_at = (index: number): string =>
	artisan_thinking_words[index % artisan_thinking_words.length] ?? fallback_thinking_words[0];

/**
 * Chooses one thinking word per work session and keeps it there. The word is
 * derived from the session's own identity rather than a timer, so a turn reads
 * as one continuous thought instead of a slideshow, and the choice survives
 * every re-render of the same session.
 */
export const thinking_word_for = (seed: string): string => {
	let hash = 2_166_136_261;
	for (const character of seed) {
		hash ^= character.codePointAt(0) ?? 0;
		hash = Math.imul(hash, 16_777_619) >>> 0;
	}
	return thinking_word_at(hash);
};

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
 * True while any detail item is visibly doing something — a running activity,
 * streaming reasoning, or streaming prose. While that holds, the latest live
 * item is the status; only a fully quiet trace earns a standalone status line.
 */
export const conversation_work_is_live = (items: ReadonlyArray<ConversationItem>): boolean =>
	items.some((item) =>
		item.type === "activity"
			? conversation_activity_is_live(item)
			: (item.type === "assistant_message" || item.type === "reasoning_summary") &&
				live_lifecycles.has(item.lifecycle),
	);
