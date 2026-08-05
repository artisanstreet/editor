import { Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	ConversationAlignedScrollTop,
	ConversationBaseEndSpacePixels,
	ConversationBottomScrollTop,
	ConversationEndSpaceHeight,
	ConversationIsFollowing,
	ConversationUserMessageIds,
	ConversationUserMessageWithSourceReference,
	NewestConversationUserMessage,
} from "../../modules/frontend/src/lib/conversation/scroll-position";

type ScrollItem = Parameters<typeof ConversationUserMessageIds>[0][number];

const Item = (
	id: string,
	ordinal: number,
	type: ScrollItem["type"] = "user_message",
	source_reference = id,
): ScrollItem => ({
	id,
	ordinal,
	source_refs: [{ reference: source_reference }],
	type,
});

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

	it("waits for this client's accepted message when another client projects first", () => {
		const current = [
			Item("user-existing", 1),
			Item("user-other-client", 2, "user_message", "command-other"),
		];

		expect(
			Option.isNone(ConversationUserMessageWithSourceReference(current, "command-local")),
		).toBe(true);

		const with_local_message = [
			...current,
			Item("user-local", 3, "user_message", "command-local"),
		];
		expect(
			Option.getOrUndefined(
				ConversationUserMessageWithSourceReference(with_local_message, "command-local"),
			),
		).toBe("user-local");
	});

	it("positions initial navigation at the bottom without animation state", () => {
		expect(ConversationBottomScrollTop(1_600, 900)).toBe(700);
		expect(ConversationBottomScrollTop(600, 900)).toBe(0);
	});

	it("stops following as soon as the reader leaves the live-tail zone", () => {
		expect(ConversationIsFollowing(1_400, 2_000, 600)).toBe(true);
		expect(ConversationIsFollowing(1_398.5, 2_000, 600)).toBe(true);
		/** The tail zone is wide enough that one frame of streamed growth stays inside it. */
		expect(ConversationIsFollowing(1_390, 2_000, 600)).toBe(true);
		expect(ConversationIsFollowing(1_300, 2_000, 600)).toBe(false);
	});

	it("aligns a sent turn to the top inset and creates only the missing end space", () => {
		expect(ConversationAlignedScrollTop(500, 100, 420)).toBe(804);
		expect(ConversationEndSpaceHeight(900, 420, 620)).toBe(684);
		expect(ConversationEndSpaceHeight(900, 420, 1_200)).toBe(ConversationBaseEndSpacePixels);
	});
});
