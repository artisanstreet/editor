import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeCommandEnvelope, OrchestrationFanoutLimits } from "@artisan/protocol";

const assignment = (assignment_id: string) => ({
	assignment_id,
	engine_id: "engine_codex",
	expected_result: "Return a concise result.",
	instructions: "Inspect the assigned scope.",
	parent_node_id: "group_1",
	permission_policy: {
		approval: "never" as const,
		network_access: false,
		write_access: false,
	},
	profile: "default",
	role: "reviewer",
	scope: { kind: "repo" as const, value: "artisan-editor", write_access: false },
	summary_contract: "Summarize the finding.",
	workspace: {
		isolation: "isolated" as const,
		working_directory: "C:/workspace/artisan-editor",
		workspace_id: "workspace_1",
	},
});

const command = (
	assignments: ReadonlyArray<ReturnType<typeof assignment>>,
	max_concurrency: number,
) => ({
	kind: "command",
	message_id: "message_1",
	origin: "frontend",
	payload: {
		assignments,
		group_id: "group_1",
		max_concurrency,
		type: "orchestration.group.start" as const,
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-07-10T08:00:00.000Z",
	thread_id: "thread_1",
});

describe("orchestration fan-out protocol bounds", () => {
	it("accepts the documented assignment and concurrency ceilings", async () => {
		const input = command(
			Array.from({ length: OrchestrationFanoutLimits.max_assignments }, (_, index) =>
				assignment(`assignment_${index + 1}`),
			),
			OrchestrationFanoutLimits.max_concurrency,
		);

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).resolves.toEqual(input);
	});

	it("rejects a graph command that exceeds either fan-out ceiling", async () => {
		const assignments = Array.from(
			{ length: OrchestrationFanoutLimits.max_assignments + 1 },
			(_, index) => assignment(`assignment_${index + 1}`),
		);

		await expect(
			Effect.runPromise(
				DecodeCommandEnvelope(
					command(assignments, OrchestrationFanoutLimits.max_concurrency),
				),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeCommandEnvelope(
					command(assignments.slice(0, 2), OrchestrationFanoutLimits.max_concurrency + 1),
				),
			),
		).rejects.toBeDefined();
	});
});
