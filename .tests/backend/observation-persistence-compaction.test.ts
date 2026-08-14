import { describe, expect, it } from "vitest";
import { Cause, Effect, Exit } from "effect";

import type { EngineObservation } from "@artisan/engines";

import {
	CompactObservationBatch,
	MakeObservationPersistence,
} from "../../modules/backend/src/orchestration/observation-persistence";
import { OrchestrationFailure } from "../../modules/backend/src/persistence/orchestration/contracts";

const Delta = (
	sequence: number,
	delta = "x",
	phase: "commentary" | "final" | "unspecified" = "unspecified",
): Extract<EngineObservation, { readonly _tag: "agent_message_delta" }> => ({
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
	it("preserves an interrupt-only batch failure", async () => {
		const persistence = MakeObservationPersistence({
			continuation_repository: {
				RecordObservationMetadata: () => Effect.void,
				RecordObservationsMetadata: () => Effect.void,
			},
			graph_repository: {
				RecordObservedSubagent: () => Effect.succeed([]),
				ReconcileObservedRoot: () => Effect.succeed([]),
			},
			repository: {
				RecordObservation: () => Effect.die("individual fallback must not run"),
				RecordObservations: () => Effect.interrupt,
			},
		});

		const exit = await Effect.runPromiseExit(
			persistence.PersistCompactedBatch({ run_id: "run_1" }, [Delta(1)]),
		);

		expect(Exit.isFailure(exit)).toBe(true);
		if (Exit.isFailure(exit)) expect(Cause.hasInterruptsOnly(exit.cause)).toBe(true);
	});

	it("fails rather than silently succeeding when individual fallback also fails", async () => {
		const persisted: Array<number> = [];
		const persistence = MakeObservationPersistence({
			continuation_repository: {
				RecordObservationMetadata: () => Effect.void,
				RecordObservationsMetadata: () => Effect.void,
			},
			graph_repository: {
				RecordObservedSubagent: () => Effect.succeed([]),
				ReconcileObservedRoot: () => Effect.succeed([]),
			},
			repository: {
				RecordObservation: (observation) =>
					Effect.sync(() => {
						persisted.push(observation.sequence);
					}).pipe(
						Effect.andThen(
							observation.sequence === 2
								? Effect.fail(
										new OrchestrationFailure({
											cause: new Error("individual persistence failed"),
										}),
									)
								: Effect.succeed([]),
						),
					),
				RecordObservations: () =>
					Effect.fail(
						new OrchestrationFailure({ cause: new Error("batch persistence failed") }),
					),
			},
		});

		await expect(
			Effect.runPromise(
				persistence.PersistCompactedBatch({ run_id: "run_1" }, [
					{ ...Delta(1), phase: "commentary" },
					{ ...Delta(2), phase: "final" },
					Delta(3),
				]),
			),
		).rejects.toMatchObject({
			_tag: "OrchestrationFailure",
			cause: expect.objectContaining({ message: "individual persistence failed" }),
		});
		expect(persisted).toEqual([1, 2]);
	});

	it("preserves ordered individual fallback when every observation succeeds", async () => {
		const persisted: Array<number> = [];
		const persistence = MakeObservationPersistence({
			continuation_repository: {
				RecordObservationMetadata: () => Effect.void,
				RecordObservationsMetadata: () => Effect.void,
			},
			graph_repository: {
				RecordObservedSubagent: () => Effect.succeed([]),
				ReconcileObservedRoot: () => Effect.succeed([]),
			},
			repository: {
				RecordObservation: (observation) =>
					Effect.sync(() => {
						persisted.push(observation.sequence);
					}).pipe(Effect.as([])),
				RecordObservations: () =>
					Effect.fail(
						new OrchestrationFailure({ cause: new Error("batch persistence failed") }),
					),
			},
		});

		await Effect.runPromise(
			persistence.PersistCompactedBatch({ run_id: "run_1" }, [
				{ ...Delta(1), phase: "commentary" },
				{ ...Delta(2), phase: "final" },
				Delta(3),
			]),
		);
		expect(persisted).toEqual([1, 2, 3]);
	});

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

	it("coalesces adjacent native noise but retains classified native failures", () => {
		const compacted = CompactObservationBatch([
			{
				_tag: "native_action",
				action: "item/reasoning/textDelta",
				artisan_run_id: "run_1",
				detail: "Reasoning privately",
				observation_id: "private_reasoning",
				raw: { engine_id: "codex", frame: {}, transport: "test" },
				sequence: 1,
			},
			{
				_tag: "native_action",
				action: "item/reasoning/textDelta",
				artisan_run_id: "run_1",
				detail: "Reasoning privately",
				observation_id: "private_reasoning_repeated",
				raw: { engine_id: "codex", frame: {}, transport: "test" },
				sequence: 2,
			},
			{
				_tag: "native_action",
				action: "error",
				artisan_run_id: "run_1",
				error_ref: { artisan_code: "AE-PROVIDER-201" },
				observation_id: "classified_failure",
				raw: { engine_id: "codex", frame: {}, transport: "test" },
				sequence: 3,
			},
		] satisfies ReadonlyArray<EngineObservation>);

		expect(compacted).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				observation_id: "private_reasoning_repeated",
			}),
			expect.objectContaining({
				_tag: "native_action",
				error_ref: { artisan_code: "AE-PROVIDER-201" },
				observation_id: "classified_failure",
			}),
		]);
	});
});
