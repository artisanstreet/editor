import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";
import { describe, expect, it } from "vitest";

import { make_engine_registry_layer, type Engine, type EngineCapabilities } from "@artisan/engines";

import {
	ThreadContinuationService,
	ThreadContinuationServiceLive,
} from "../../modules/backend/src/orchestration/thread-continuation-service";
import { render_portable_checkpoint_context } from "../../modules/backend/src/orchestration/thread-continuation-model";
import type { CanonicalTranscriptEntry } from "../../modules/backend/src/orchestration/thread-continuation-model";
import {
	ThreadContinuationRepository,
	type ContinuationLaunch,
	type ThreadContinuationContext,
} from "../../modules/backend/src/persistence/thread-continuation-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const capabilities: EngineCapabilities = {
	approval: { state: "unsupported" },
	auth: { state: "supported" },
	cancel: { state: "supported" },
	close: { state: "supported" },
	continuation_export: { state: "unsupported" },
	events: { state: "supported" },
	global_guidance: { state: "supported" },
	model_selection: { state: "supported" },
	native_continuation: { state: "unsupported" },
	native_tools: { state: "unsupported" },
	probe: { state: "supported" },
	question: { state: "unsupported" },
	raw_frames: { state: "supported" },
	resume: { state: "supported" },
	start: { state: "supported" },
	steer: { state: "unsupported" },
	subagents: { state: "unsupported" },
};

const engine = (
	id: string,
	operations: Partial<Pick<Engine, "CheckNativeContinuation" | "ExportContinuation">> = {},
): Engine => ({
	...operations,
	Descriptor: {
		capabilities: {
			...capabilities,
			continuation_export: {
				state: operations.ExportContinuation === undefined ? "unsupported" : "experimental",
			},
			native_continuation: {
				state:
					operations.CheckNativeContinuation === undefined
						? "unsupported"
						: "experimental",
			},
		},
		display_name: id,
		id,
		transport: "test",
	},
	Open: () => Effect.die("Open is outside this service test"),
	Probe: () => Effect.die("Probe is outside this service test"),
});

const message_entry = (
	journal_sequence: number,
	role: "assistant" | "user",
	text: string,
): CanonicalTranscriptEntry => ({
	journal_sequence,
	logical_sequence: journal_sequence,
	role,
	text,
});

const base_context = (
	overrides: Partial<ThreadContinuationContext> = {},
): ThreadContinuationContext => ({
	first_target_journal_sequence: 20,
	native_compaction: Option.none(),
	request: {
		command_id: "command-target",
		message_id: "message-target",
		text: "Continue.",
	},
	source: Option.some({
		engine_id: "claude",
		last_native_turn_id: "turn-source",
		last_observation_sequence: 12,
		model_id: "claude-sonnet",
		native_thread_id: "native-source",
		resume_token: Option.some({ native_thread_id: "native-source" }),
		run_id: "run-source",
		status: "completed",
		working_directory: "C:\\workspace",
	}),
	source_cut_journal_sequence: 19,
	target: {
		engine_id: "codex",
		model_id: "gpt-5",
		run_id: "run-target",
		thread_id: "thread-1",
	},
	...overrides,
});

function make_test_layer(input: {
	readonly context: ThreadContinuationContext;
	readonly earliest?: ReadonlyArray<CanonicalTranscriptEntry>;
	readonly engines: ReadonlyArray<Engine>;
	readonly latest?: ReadonlyArray<CanonicalTranscriptEntry>;
	readonly launches: Array<ContinuationLaunch>;
}) {
	const repository = ThreadContinuationRepository.of({
		BindTarget: () => Effect.void,
		FailLaunch: () => Effect.void,
		IsDispatchReady: () => Effect.succeed(true),
		MarkOpening: () => Effect.void,
		PrepareLaunch: (_target_run_id, launch) =>
			Effect.sync(() => {
				input.launches.push(launch);
				return "prepared" as const;
			}),
		ReadContext: () => Effect.succeed(input.context),
		ReadCanonicalHistory: () => {
			const entries = [...(input.latest ?? [])];
			const first_user = (input.earliest ?? entries).find((entry) => entry.role === "user");
			return Effect.succeed({
				entries: entries.map((entry) => ({ ...entry, run_id: "run-source" })),
				first_user_objective:
					first_user === undefined
						? Option.none()
						: Option.some({ ...first_user, run_id: "run-source" }),
				total_entries: entries.length,
			});
		},
		ReconcileStranded: () => Effect.succeed([]),
		RecordNativeCompaction: () => Effect.void,
		RecordObservationMetadata: () => Effect.void,
	});
	let next_id = 0;
	const runtime = RuntimeMetadata.of({
		instance_id: "backend-test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}-${++next_id}`),
		Now: Effect.succeed("2026-07-30T10:00:00.000Z"),
	});
	const dependencies = Layer.mergeAll(
		Layer.succeed(ThreadContinuationRepository, repository),
		Layer.succeed(RuntimeMetadata, runtime),
		make_engine_registry_layer(input.engines).pipe(Layer.orDie),
		NodeCrypto.layer,
	);

	return ThreadContinuationServiceLive.pipe(Layer.provide(dependencies));
}

const prepare = (
	layer: Layer.Layer<ThreadContinuationService>,
	target_model: string | null = "target-model",
) =>
	Effect.runPromise(
		ThreadContinuationService.pipe(
			Effect.flatMap((service) =>
				service.Prepare({
					command_id: "command-target",
					...(target_model === null ? {} : { target_model }),
					target_model_id: "target-model-base",
					target_run_id: "run-target",
				}),
			),
			Effect.provide(layer),
		),
	);

describe("thread continuation service", () => {
	it("prepares a genuinely new thread without manufacturing history", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const layer = make_test_layer({
			context: base_context({ source: Option.none() }),
			engines: [engine("codex")],
			launches,
		});

		await expect(prepare(layer)).resolves.toEqual({
			_tag: "fresh",
			launch_state: "prepared",
		});
		expect(launches).toEqual([
			{
				_tag: "fresh",
				request_id: "command-target",
				target_model_id: "target-model-base",
			},
		]);
	});

	it("uses native resume only after an explicit target-model compatibility decision", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const decisions: Array<unknown> = [];
		const claude = engine("claude", {
			CheckNativeContinuation: (input) =>
				Effect.sync(() => {
					decisions.push(input);
					return { state: "compatible" as const };
				}),
		});
		const context = base_context({
			target: { ...base_context().target, engine_id: "claude" },
		});

		const result = await prepare(make_test_layer({ context, engines: [claude], launches }));

		expect(result).toMatchObject({
			_tag: "native",
			resume_token: { native_thread_id: "native-source" },
			source_run_id: "run-source",
		});
		expect(decisions).toEqual([
			{
				resume_token: { native_thread_id: "native-source" },
				source_model: "claude-sonnet",
				target_model: "target-model",
			},
		]);
		expect(launches[0]).toMatchObject({ _tag: "native", source_run_id: "run-source" });
	});

	it("falls back to a portable checkpoint when the executable target model is absent", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const decisions: Array<unknown> = [];
		const claude = engine("claude", {
			CheckNativeContinuation: (input) =>
				Effect.sync(() => {
					decisions.push(input);
					return { state: "compatible" as const };
				}),
		});
		const context = base_context({
			target: { ...base_context().target, engine_id: "claude" },
		});

		const result = await prepare(
			make_test_layer({ context, engines: [claude], launches }),
			null,
		);

		expect(result).toMatchObject({ _tag: "portable" });
		expect(decisions).toEqual([]);
		expect(launches[0]).toMatchObject({ _tag: "portable" });
	});

	it("turns Claude PostCompact output into a bounded portable checkpoint with post-boundary tail", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const context = base_context({
			native_compaction: Option.some({
				through_journal_sequence: 10,
				through_run_id: "run-source",
				value: {
					boundary_id: "boundary-secret",
					method: "claude_post_compact",
					observation_id: "observation-secret",
					source_native_thread_id: "native-source",
					summary: "Claude compacted state.",
					summary_sha256: "a".repeat(64),
					trigger: "auto",
				},
			}),
		});
		const result = await prepare(
			make_test_layer({
				context,
				engines: [engine("claude"), engine("codex")],
				latest: [message_entry(12, "assistant", "After compaction.")],
				launches,
			}),
		);

		expect(result._tag).toBe("portable");
		if (result._tag !== "portable") return;
		expect(result.checkpoint.method).toBe("claude_post_compact");
		expect(result.checkpoint.tail).toEqual([{ role: "assistant", text: "After compaction." }]);
		expect(result.lineage).toMatchObject({
			boundary_id: "boundary-secret",
			kind: "claude",
			through_run_id: "run-source",
		});
		const checkpoint_json = JSON.stringify(result.checkpoint);
		expect(checkpoint_json).not.toContain("boundary-secret");
		expect(checkpoint_json).not.toContain("native-source");
		expect(render_portable_checkpoint_context(result.checkpoint)).not.toContain(
			"observation-secret",
		);
	});

	it("exports a settled Codex fork and keeps every native identifier in private lineage", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const export_inputs: Array<unknown> = [];
		const codex = engine("codex", {
			ExportContinuation: (input) =>
				Effect.sync(() => {
					export_inputs.push(input);
					return {
						export_native_item_id: "item-export-secret",
						export_native_thread_id: "thread-export-secret",
						export_native_turn_id: "turn-export-secret",
						message: JSON.stringify({ summary: "Codex portable state." }),
						method: "codex_fork_summary" as const,
						source_native_thread_id: "native-source",
						source_native_turn_id: "turn-source",
					};
				}),
		});
		const context = base_context({
			source: Option.some({
				...Option.getOrThrow(base_context().source),
				engine_id: "codex",
				model_id: "gpt-5",
			}),
			target: { ...base_context().target, engine_id: "claude" },
		});
		const result = await prepare(
			make_test_layer({ context, engines: [codex, engine("claude")], launches }),
		);

		expect(result._tag).toBe("portable");
		if (result._tag !== "portable") return;
		expect(result.checkpoint.method).toBe("codex_fork_summary");
		expect(result.checkpoint.summary).toBe("Codex portable state.");
		expect(result.lineage).toMatchObject({
			export_native_item_id: "item-export-secret",
			kind: "codex",
		});
		expect(JSON.stringify(result.checkpoint)).not.toMatch(
			/(native-source|thread-export-secret|turn-export-secret|item-export-secret)/,
		);
		expect(export_inputs[0]).toMatchObject({
			settled_native_turn_id: "turn-source",
			source_model: "gpt-5",
			source_resume_token: { native_thread_id: "native-source" },
		});
	});

	it("falls back to the canonical transcript when provider export is malformed", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const codex = engine("codex", {
			ExportContinuation: () =>
				Effect.succeed({
					export_native_item_id: "item",
					export_native_thread_id: "fork",
					export_native_turn_id: "export-turn",
					message: '{"summary":42}',
					method: "codex_fork_summary" as const,
					source_native_thread_id: "native-source",
					source_native_turn_id: "turn-source",
				}),
		});
		const context = base_context({
			source: Option.some({
				...Option.getOrThrow(base_context().source),
				engine_id: "codex",
			}),
			target: { ...base_context().target, engine_id: "claude" },
		});
		const result = await prepare(
			make_test_layer({
				context,
				earliest: [message_entry(1, "user", "Original objective.")],
				engines: [codex, engine("claude")],
				latest: [message_entry(18, "assistant", "Most recent verified work.")],
				launches,
			}),
		);

		expect(result._tag).toBe("portable");
		if (result._tag !== "portable") return;
		expect(result.checkpoint.method).toBe("canonical_transcript_summary");
		expect(result.checkpoint.summary).toContain("Original objective.");
		expect(result.checkpoint.tail).toContainEqual({
			role: "assistant",
			text: "Most recent verified work.",
		});
		expect(result.lineage).toEqual({ kind: "canonical" });
	});
});
