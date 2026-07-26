import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationItem } from "@artisan/protocol";
import { make_conversation_trace_segments } from "../../modules/frontend/src/lib/conversation/trace";

const base = {
	created_at: "2026-07-26T00:00:00.000Z",
	lifecycle: "completed",
	ordinal: 1,
	references: [],
	revision: 0,
	run_id: "run_1",
	source_refs: [],
	turn_id: "run:run_1",
	updated_at: "2026-07-26T00:00:00.000Z",
};

const item = (value: unknown) => Schema.decodeUnknownSync(ConversationItem)(value);

describe("conversation trace", () => {
	it("hides diagnostics by default without suppressing active reasoning", () => {
		const segments = make_conversation_trace_segments(
			[
				item({
					...base,
					id: "diagnostic_1",
					summary: "Provider warning",
					type: "native_event",
				}),
				item({
					...base,
					id: "reasoning_1",
					lifecycle: "active",
					ordinal: 2,
					text: "Checking the provider",
					type: "reasoning_summary",
				}),
			],
			false,
		);

		expect(segments).toEqual([expect.objectContaining({ id: "reasoning_1", type: "item" })]);
	});

	it("groups every diagnostic behind one disclosure when enabled", () => {
		const segments = make_conversation_trace_segments(
			[
				item({
					...base,
					id: "diagnostic_1",
					summary: "Provider warning",
					type: "native_event",
				}),
				item({
					...base,
					id: "activity_1",
					kind: "terminal_activity",
					label: "Ran a command",
					ordinal: 2,
					status: "completed",
					type: "activity",
				}),
				item({
					...base,
					id: "diagnostic_2",
					ordinal: 3,
					summary: "Usage update",
					type: "native_event",
				}),
			],
			true,
		);

		expect(segments.filter((segment) => segment.type === "diagnostic_group")).toEqual([
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "diagnostic_1" }),
					expect.objectContaining({ id: "diagnostic_2" }),
				],
				type: "diagnostic_group",
			}),
		]);
	});
});
