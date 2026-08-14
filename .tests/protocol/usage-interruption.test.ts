import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	CommandPayload,
	ConversationItem,
	EventPayload,
	UsageInterruption,
} from "@artisan/protocol";

const interruption = {
	affected_model_id: "gpt-5.6-sol",
	alternatives: [
		{
			display_name: "GPT-5.3-Codex-Spark",
			engine_id: "codex",
			model_id: "gpt-5.3-codex-spark",
			verified_at: "2026-08-14T10:01:00.000Z",
		},
	],
	auto_continue: true,
	created_at: "2026-08-14T10:00:00.000Z",
	interruption_id: "usage-interruption:run_source",
	limit_id: "primary",
	limit_scope: "model" as const,
	resets_at: "2026-08-14T12:00:00.000Z",
	resume_not_before: "2026-08-14T12:00:00.000Z",
	revision: 2,
	source_agent_id: "agent_root",
	source_engine_id: "codex",
	source_model_id: "gpt-5.6-sol",
	source_run_id: "run_source",
	state: "scheduled" as const,
	thread_id: "thread_usage",
	updated_at: "2026-08-14T10:01:00.000Z",
};

describe("usage interruption protocol", () => {
	it("decodes the bounded renderer-safe interruption snapshot", () => {
		expect(Schema.decodeUnknownSync(UsageInterruption)(interruption)).toEqual(interruption);
		expect(() =>
			Schema.decodeUnknownSync(UsageInterruption)({
				...interruption,
				alternatives: Array.from({ length: 17 }, () => interruption.alternatives[0]),
			}),
		).toThrow();
	});

	it("routes revisioned recovery commands and complete update events", () => {
		const command = Schema.decodeUnknownSync(CommandPayload)({
			action: {
				target_engine_id: "codex",
				target_model_id: "gpt-5.3-codex-spark",
				type: "continue",
			},
			expected_revision: 2,
			interruption_id: interruption.interruption_id,
			type: "usage.interruption.resolve",
		});
		const event = Schema.decodeUnknownSync(EventPayload)({
			interruption,
			type: "usage.interruption.updated",
		});

		expect(command.type).toBe("usage.interruption.resolve");
		expect(event.type).toBe("usage.interruption.updated");
	});

	it("keeps the recovery decision as a first-class conversation item", () => {
		const item = Schema.decodeUnknownSync(ConversationItem)({
			created_at: interruption.created_at,
			id: "item_usage",
			interruption,
			lifecycle: "active",
			ordinal: 10,
			references: [],
			revision: 11,
			run_id: interruption.source_run_id,
			source_refs: [{ reference: "usage-interruption:run_source" }],
			turn_id: "run:run_source",
			type: "usage_interruption",
			updated_at: interruption.updated_at,
		});

		expect(item.type).toBe("usage_interruption");
	});
});
