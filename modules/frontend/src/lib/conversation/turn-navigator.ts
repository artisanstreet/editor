import type { ConversationSnapshot } from "@artisan/protocol";

/** One reachable point in the transcript: a thing you said, and where it sits. */
export interface ConversationTurnMarker {
	/** The conversation item id, which is also its `data-conversation-item-id`. */
	readonly id: string;
	/** The opening words, as the navigator lists them. */
	readonly label: string;
}

/**
 * A transcript is navigated by what you asked, not by what came back. Assistant
 * output is long, plural per turn, and phrased in the model's words; your own
 * messages are short, one per turn, and already the way you remember the
 * conversation — so they are the only anchors worth offering.
 */
const navigator_label_length = 120;

/** Collapses a message to one line so a preview can never break the row. */
const single_line = (text: string) => text.replaceAll(/\s+/gu, " ").trim();

/**
 * The points a reader can jump to, oldest first.
 *
 * Empty for a transcript with nothing to move between: a single turn is already
 * on screen, and offering to navigate to it is a control that does nothing.
 */
export const ConversationTurnMarkers = (
	items: ConversationSnapshot["items"],
): ReadonlyArray<ConversationTurnMarker> => {
	const markers = items
		.filter((item) => item.type === "user_message")
		.map((item) => ({
			id: item.id,
			label: single_line(item.text).slice(0, navigator_label_length),
		}))
		.filter((marker) => marker.label.length > 0);

	return markers.length > 1 ? markers : [];
};

/**
 * Which marker the reader is currently at, given each one's offset from the top
 * of the viewport.
 *
 * The turn you are reading is the last one that has already passed the top
 * edge, so a turn scrolled just out of sight above still owns the position
 * until the next one arrives. Before the first has passed — at the very top of
 * a transcript — the first owns it, because no reading position is better
 * described as "nowhere".
 */
export const ActiveConversationTurn = (
	offsets: ReadonlyArray<{ readonly id: string; readonly top: number }>,
	/** Where a turn counts as reached; a little below the top edge reads best. */
	threshold = 96,
): string | undefined => {
	if (offsets.length === 0) return undefined;
	const passed = offsets.filter((offset) => offset.top <= threshold).at(-1);

	return (passed ?? offsets[0])?.id;
};
