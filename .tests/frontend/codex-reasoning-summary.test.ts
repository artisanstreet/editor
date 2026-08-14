import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationItem } from "@artisan/protocol";
import { group_conversation_trace_blocks } from "../../modules/frontend/src/lib/conversation/trace";

const base = {
	created_at: "2026-08-14T00:00:00.000Z",
	lifecycle: "completed",
	references: [],
	revision: 0,
	run_id: "run_1",
	source_refs: [],
	turn_id: "run:run_1",
	updated_at: "2026-08-14T00:00:00.000Z",
};

const item = (value: unknown) => Schema.decodeUnknownSync(ConversationItem)(value);

describe("Codex reasoning summary presentation", () => {
	it("routes post-steer reasoning through the same trace disclosure as adjacent work", () => {
		const activity = (id: string, ordinal: number) =>
			item({
				...base,
				id,
				kind: "terminal_activity",
				label: "Ran a command",
				ordinal,
				status: "completed",
				type: "activity",
			});
		const reasoning = item({
			...base,
			id: "reasoning_after_steer",
			ordinal: 2,
			text: "Checking the steered request",
			type: "reasoning_summary",
		});
		const as_block = (entry: typeof reasoning) => ({
			id: entry.id,
			item: entry,
			turn_id: entry.turn_id,
			type: "item" as const,
		});

		expect(
			group_conversation_trace_blocks([
				as_block(activity("activity_before_reasoning", 1)),
				as_block(reasoning),
				as_block(activity("activity_after_reasoning", 3)),
			]),
		).toEqual([
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "activity_before_reasoning" }),
					expect.objectContaining({ id: "reasoning_after_steer" }),
					expect.objectContaining({ id: "activity_after_reasoning" }),
				],
				type: "trace_group",
			}),
		]);
	});
});
