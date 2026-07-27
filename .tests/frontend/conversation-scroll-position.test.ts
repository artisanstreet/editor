import { Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	ConversationAlignedScrollTop,
	ConversationBaseEndSpacePixels,
	ConversationBottomScrollTop,
	ConversationEndSpaceHeight,
	ConversationUserMessageIds,
	NewestConversationUserMessage,
} from "../../modules/frontend/src/lib/conversation/scroll-position";

type ScrollItem = Parameters<typeof ConversationUserMessageIds>[0][number];

const Item = (
	id: string,
	ordinal: number,
	type: ScrollItem["type"] = "user_message",
): ScrollItem => ({ id, ordinal, type });

describe("conversation scroll position", () => {
	it("finds only the newest user message projected after local submission", () => {
		const existing = [Item("user-1", 1), Item("assistant-1", 2, "assistant_message")];
		const previous_ids = ConversationUserMessageIds(existing);
		const current = [
			...existing,
			Item("user-2", 3),
			Item("activity-1", 4, "activity"),
			Item("user-3", 5),
		];

		expect(Option.getOrUndefined(NewestConversationUserMessage(current, previous_ids))).toBe(
			"user-3",
		);
		expect(Option.isNone(NewestConversationUserMessage(existing, previous_ids))).toBe(true);
	});

	it("positions initial navigation at the bottom without animation state", () => {
		expect(ConversationBottomScrollTop(1_600, 900)).toBe(700);
		expect(ConversationBottomScrollTop(600, 900)).toBe(0);
	});

	it("aligns a sent turn to the top inset and creates only the missing end space", () => {
		expect(ConversationAlignedScrollTop(500, 100, 420)).toBe(804);
		expect(ConversationEndSpaceHeight(900, 420, 620)).toBe(684);
		expect(ConversationEndSpaceHeight(900, 420, 1_200)).toBe(ConversationBaseEndSpacePixels);
	});
});
