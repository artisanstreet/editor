import type { ConversationItem } from "@artisan/protocol";
import { Option } from "effect";

export const ConversationBaseEndSpacePixels = 192;
export const ConversationTurnTopInsetPixels = 16;

type ConversationScrollItem = Pick<ConversationItem, "id" | "ordinal" | "source_refs" | "type">;

/** Captures the durable user turns visible before a local submission begins. */
export const ConversationUserMessageIds = (
	items: ReadonlyArray<ConversationScrollItem>,
): ReadonlySet<string> =>
	new Set(items.filter((item) => item.type === "user_message").map((item) => item.id));

/** Selects the newest projected user turn that was absent at submission time. */
export const NewestConversationUserMessage = (
	items: ReadonlyArray<ConversationScrollItem>,
	previous_ids: ReadonlySet<string>,
): Option.Option<string> => {
	const candidates = items
		.filter((item) => item.type === "user_message" && !previous_ids.has(item.id))
		.toSorted((left, right) => right.ordinal - left.ordinal || right.id.localeCompare(left.id));

	return Option.fromUndefinedOr(candidates[0]?.id);
};

/** Resolves one accepted command to its exact canonical user-message item. */
export const ConversationUserMessageWithSourceReference = (
	items: ReadonlyArray<ConversationScrollItem>,
	source_reference: string,
): Option.Option<string> =>
	Option.fromUndefinedOr(
		items.find(
			(item) =>
				item.type === "user_message" &&
				item.source_refs.some(
					(source) =>
						source.reference === source_reference ||
						source.event_id === source_reference,
				),
		)?.id,
	);

/** Computes an immediate bottom position without inheriting smooth-scroll CSS. */
export const ConversationBottomScrollTop = (scroll_height: number, viewport_height: number) =>
	Math.max(0, scroll_height - viewport_height);

/**
 * How close to the bottom still counts as following the transcript.
 *
 * This decides when the transcript may move the reader, so it is bounded on
 * both sides. Too generous and auto-scroll fights you: scroll up a little to
 * reread something and it drags you back. Too tight and streaming breaks it —
 * content grows between the frame that measures and the frame that corrects, so
 * a reader who never moved reads as several pixels adrift and following silently
 * switches off for the rest of the turn.
 *
 * Roughly two lines of prose is the band that satisfies both: wider than any
 * slip one frame of growth can open, and far narrower than the smallest
 * deliberate scroll a wheel notch or a drag produces.
 */
export const ConversationFollowTolerance = (viewport_height: number) =>
	Math.max(64, viewport_height * 0.06);

/**
 * Whether the reader is parked at the bottom and so wants new content followed.
 *
 * Derived from position rather than tracked as a mode, which is what keeps it
 * honest: a programmatic scroll that parks the reader elsewhere turns following
 * off by consequence, with no separate state to keep in sync.
 */
export const ConversationIsFollowing = (
	scroll_top: number,
	scroll_height: number,
	viewport_height: number,
) => scroll_height - viewport_height - scroll_top < ConversationFollowTolerance(viewport_height);

/** Aligns a turn to the viewport's top inset in scroll-content coordinates. */
export const ConversationAlignedScrollTop = (
	current_scroll_top: number,
	viewport_top: number,
	item_top: number,
	inset = ConversationTurnTopInsetPixels,
) => Math.max(0, current_scroll_top + item_top - viewport_top - inset);

/**
 * Leaves enough content after the anchored turn to reach the top inset. As
 * streamed content grows above the spacer, the required spacer shrinks.
 */
export const ConversationEndSpaceHeight = (
	viewport_height: number,
	item_top: number,
	end_space_top: number,
	base_height = ConversationBaseEndSpacePixels,
	inset = ConversationTurnTopInsetPixels,
) => Math.max(base_height, item_top + viewport_height - inset - end_space_top);
