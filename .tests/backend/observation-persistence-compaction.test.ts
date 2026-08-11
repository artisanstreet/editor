import { describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";

import { CompactObservationBatch } from "../../modules/backend/src/orchestration/observation-persistence";

const Delta = (
	sequence: number,
	delta = "x",
	phase: "commentary" | "final" | "unspecified" = "unspecified",
): EngineObservation => ({
	_tag: "agent_message_delta",
	artisan_run_id: "run_1",
	delta,
	item_id: "message_1",
	observation_id: `observation_${sequence}`,
	phase,
	raw: { engine_id: "codex", frame: {}, transport: "test" },
	sequence,
	turn_id: "turn_1",
});

describe("observation persistence compaction", () => {
	it("turns token-frequency text into one render-cadence durable update", () => {
		const compacted = CompactObservationBatch(
			Array.from({ length: 256 }, (_, index) => Delta(index + 1)),
		);

		expect(compacted).toHaveLength(1);
		expect(compacted[0]).toMatchObject({
			_tag: "agent_message_delta",
			delta: "x".repeat(256),
			observation_id: "observation_256",
			sequence: 256,
		});
	});

	it("does not merge across semantic or completion boundaries", () => {
		const compacted = CompactObservationBatch([
			Delta(1, "commentary"),
			Delta(2, "final", "final"),
			{
				_tag: "agent_message_completed",
				artisan_run_id: "run_1",
				item_id: "message_1",
				message: "commentaryfinal",
				observation_id: "observation_3",
				phase: "final",
				raw: { engine_id: "codex", frame: {}, transport: "test" },
				sequence: 3,
				turn_id: "turn_1",
			},
		]);

		expect(compacted).toHaveLength(3);
	});

	it("bounds interleaved telemetry without reordering durable observations", () => {
		const observations = Array.from({ length: 128 }, (_, index) => [
			Delta(index * 3 + 1),
			{
				_tag: "usage" as const,
				artisan_run_id: "run_1",
				basis: "delta" as const,
				...(index === 0 ? { context_tokens: 50 } : {}),
				input_tokens: 1,
				observation_id: `usage_${index}`,
				raw: { engine_id: "codex", frame: {}, transport: "test" as const },
				sequence: index * 3 + 2,
				turn_id: "turn_1",
			},
			{
				_tag: "tool" as const,
				action: "progress" as const,
				artisan_run_id: "run_1",
				call_id: "call_1",
				observation_id: `progress_${index}`,
				raw: { engine_id: "codex", frame: {}, transport: "test" as const },
				sequence: index * 3 + 3,
				tool_id: "tool_1",
				tool_name: "test",
				turn_id: "turn_1",
			},
		]).flat() satisfies ReadonlyArray<EngineObservation>;

		const compacted = CompactObservationBatch(observations);

		expect(compacted).toHaveLength(2);
		expect(compacted.map(({ sequence }) => sequence)).toEqual([382, 383]);
		expect(compacted[0]).toMatchObject({ delta: "x".repeat(128) });
		expect(compacted[1]).toMatchObject({
			_tag: "usage",
			context_tokens: 50,
			input_tokens: 128,
		});
		expect(compacted[1]).not.toHaveProperty("output_tokens");
	});
});
