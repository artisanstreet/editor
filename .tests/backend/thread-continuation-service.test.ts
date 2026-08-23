import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";
import { describe, expect, it } from "vitest";

import { make_engine_registry_layer, type Engine, type EngineCapabilities } from "@artisan/engines";

import {
	ThreadContinuationCompactor,
	type ThreadCompactionRequest,
	type ThreadCompactionSummary,
} from "../../modules/backend/src/orchestration/thread-continuation-compactor";
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
} from "../../modules/backend/src/persistence/thread-continuation/repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const capabilities: EngineCapabilities = {
	approval: { state: "unsupported" },
	auth: { state: "supported" },
	cancel: { state: "supported" },
	close: { state: "supported" },
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
	operations: Partial<Pick<Engine, "CheckNativeContinuation">> = {},
): Engine => ({
	...operations,
	Descriptor: {
		capabilities: {
			...capabilities,
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
	request: {
		command_id: "command-target",
		message_id: "message-target",
		text: "Continue.",
	},
	source: Option.some({
		catalog_revision: "live-source",
		engine_id: "claude",
		last_native_turn_id: "turn-source",
		last_observation_sequence: 12,
		model_id: "claude-sonnet",
		native_thread_id: "native-source",
		profile_id: "work",
		provider_route_id: "provider-route",
		resume_token: Option.some({ native_thread_id: "native-source" }),
		run_id: "run-source",
		status: "completed",
		usage_interruption_resume: false,
		variant_id: "high",
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
	readonly compaction?: ThreadCompactionSummary;
	readonly compaction_requests?: Array<ThreadCompactionRequest>;
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
				/**
				 * `earliest` describes entries older than the retained window, so the
				 * thread total exceeds what this read returns — exactly as the
				 * repository reports it, where the count covers every canonical entry
				 * rather than the bounded page it hands back. Reporting only the page
				 * would claim nothing was omitted while the fixture posits otherwise.
				 */
				total_entries: (input.earliest?.length ?? 0) + entries.length,
			});
		},
		ReconcileStranded: () => Effect.succeed([]),
		RecordObservationMetadata: () => Effect.void,
		RecordObservationsMetadata: () => Effect.void,
	});
	const compactor = ThreadContinuationCompactor.of({
		Summarize: (request) =>
			Effect.sync(() => {
				input.compaction_requests?.push(request);
				return input.compaction === undefined
					? Option.none<ThreadCompactionSummary>()
					: Option.some(input.compaction);
			}),
	});
	let next_id = 0;
	const runtime = RuntimeMetadata.of({
		instance_id: "backend-test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}-${++next_id}`),
		Now: Effect.succeed("2026-07-30T10:00:00.000Z"),
	});
	const dependencies = Layer.mergeAll(
		Layer.succeed(ThreadContinuationCompactor, compactor),
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

	/**
	 * A stopped run's provider session is still alive; only Artisan's turn ended.
	 * Resuming it natively is what keeps the real context window instead of the
	 * summarized rebuild the user would otherwise pay for after every stop.
	 */
	it("resumes natively from a run the user stopped", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const claude = engine("claude", {
			CheckNativeContinuation: () => Effect.succeed({ state: "compatible" as const }),
		});
		const context = base_context({
			source: Option.some({
				...Option.getOrThrow(base_context().source),
				status: "cancelled",
			}),
			target: { ...base_context().target, engine_id: "claude" },
		});

		const result = await prepare(make_test_layer({ context, engines: [claude], launches }));

		expect(result).toMatchObject({ _tag: "native", source_run_id: "run-source" });
		expect(launches[0]).toMatchObject({ _tag: "native", source_run_id: "run-source" });
	});

	/**
	 * PrepareLaunch rejects a native launch from a failed source unless a usage
	 * interruption claimed it. Choosing native anyway does not degrade to a
	 * checkpoint — it kills the run at startup and costs the user a second send.
	 */
	it("refuses native resume from a failed run with no usage-interruption authority", async () => {
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
			source: Option.some({
				...Option.getOrThrow(base_context().source),
				status: "failed",
			}),
			target: { ...base_context().target, engine_id: "claude" },
		});

		const result = await prepare(make_test_layer({ context, engines: [claude], launches }));

		expect(result).toMatchObject({ _tag: "portable" });
		expect(decisions).toEqual([]);
		expect(launches[0]).toMatchObject({ _tag: "portable" });
	});

	it("resumes natively from a failed run the usage interruption claimed", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const claude = engine("claude", {
			CheckNativeContinuation: () => Effect.succeed({ state: "compatible" as const }),
		});
		const context = base_context({
			source: Option.some({
				...Option.getOrThrow(base_context().source),
				status: "failed",
				usage_interruption_resume: true,
			}),
			target: { ...base_context().target, engine_id: "claude" },
		});

		const result = await prepare(make_test_layer({ context, engines: [claude], launches }));

		expect(result).toMatchObject({ _tag: "native", source_run_id: "run-source" });
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

	it("builds a compactor-generated checkpoint with compactor lineage", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const compaction_requests: Array<ThreadCompactionRequest> = [];
		const result = await prepare(
			make_test_layer({
				compaction: {
					compactor: { engine_id: "claude", model_id: "claude-sonnet" },
					summary: "Model handoff summary.",
				},
				compaction_requests,
				context: base_context(),
				engines: [engine("claude"), engine("codex")],
				latest: [message_entry(12, "assistant", "Most recent settled work.")],
				launches,
			}),
		);

		expect(result._tag).toBe("portable");
		if (result._tag !== "portable") return;
		expect(result.checkpoint.method).toBe("compaction_model_summary");
		expect(result.checkpoint.summary).toBe("Model handoff summary.");
		expect(result.checkpoint.tail).toEqual([
			{ role: "assistant", text: "Most recent settled work." },
		]);
		expect(result.lineage).toEqual({
			compactor_engine_id: "claude",
			compactor_model_id: "claude-sonnet",
			kind: "compactor",
		});
		expect(compaction_requests).toEqual([
			{
				head: [],
				omitted_head_entries: 0,
				source: {
					catalog_revision: "live-source",
					engine_id: "claude",
					model_id: "claude-sonnet",
					profile_id: "work",
					provider_route_id: "provider-route",
					variant_id: "high",
				},
				working_directory: "C:\\workspace",
			},
		]);
		const checkpoint_json = JSON.stringify(result.checkpoint);
		expect(checkpoint_json).not.toContain("native-source");
		expect(render_portable_checkpoint_context(result.checkpoint)).not.toContain(
			"native-source",
		);
		expect(launches[0]).toMatchObject({
			_tag: "portable",
			lineage: { kind: "compactor" },
			source_run_id: "run-source",
		});
	});

	it("falls back to the canonical transcript when the compactor yields nothing", async () => {
		const launches: Array<ContinuationLaunch> = [];
		const context = base_context({
			target: { ...base_context().target, engine_id: "claude" },
		});
		const result = await prepare(
			make_test_layer({
				context,
				earliest: [message_entry(1, "user", "Original objective.")],
				engines: [engine("claude"), engine("codex")],
				latest: [message_entry(18, "assistant", "Most recent verified work.")],
				launches,
			}),
		);

		expect(result._tag).toBe("portable");
		if (result._tag !== "portable") return;
		expect(result.checkpoint.method).toBe("canonical_transcript_summary");
		/** A genuinely bounded handoff names the objective it could not retain. */
		expect(result.checkpoint.summary).toContain("Canonical transcript fallback.");
		expect(result.checkpoint.summary).toContain("Original objective.");
		expect(result.checkpoint.summary).toContain("Omission: 1 earlier transcript entry was");
		expect(result.checkpoint.tail).toContainEqual({
			role: "assistant",
			text: "Most recent verified work.",
		});
		expect(result.lineage).toEqual({ kind: "canonical" });
	});
});
