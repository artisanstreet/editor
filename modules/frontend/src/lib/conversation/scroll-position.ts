import type { ConversationItem } from "@artisan/protocol";
import { Option } from "effect";

export const ConversationBaseEndSpacePixels = 192;
export const ConversationTurnTopInsetPixels = 16;

type ConversationScrollItem = Pick<ConversationItem, "id" | "ordinal" | "type">;

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

/** Computes an immediate bottom position without inheriting smooth-scroll CSS. */
export const ConversationBottomScrollTop = (scroll_height: number, viewport_height: number) =>
	Math.max(0, scroll_height - viewport_height);

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
