import raw_thinking_words from "@artisan/data/activity-status/thinking-words.json";
import { GetConversationActivityPresentation, type ConversationItem } from "@artisan/protocol";
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

export const latest_active_activity_label = (
	items: ReadonlyArray<ConversationItem>,
): string | undefined => {
	for (let index = items.length - 1; index >= 0; index -= 1) {
		const item = items[index];
		if (
			item?.type === "activity" &&
			(item.lifecycle === "active" ||
				item.lifecycle === "pending" ||
				item.lifecycle === "streaming")
		) {
			return GetConversationActivityPresentation(item).label;
		}
	}
};
