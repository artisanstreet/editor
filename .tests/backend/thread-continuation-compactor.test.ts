import { Effect, Layer, Option, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	make_engine_registry_layer,
	type Engine,
	type EngineCapabilities,
	type EngineObservation,
	type EngineOpenInput,
	type EngineRunTerminalState,
} from "@artisan/engines";

import {
	ThreadContinuationCompactor,
	ThreadContinuationCompactorLive,
	type ThreadCompactionRequest,
} from "../../modules/backend/src/orchestration/thread-continuation-compactor";
import type { CanonicalTranscriptEntry } from "../../modules/backend/src/orchestration/thread-continuation-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { SessionDefaultsService } from "../../modules/backend/src/settings/session-defaults-service";

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

type PartialObservation =
	| {
			readonly _tag: "agent_message_completed";
			readonly message: string;
			readonly phase: "commentary" | "final";
	  }
	| { readonly _tag: "approval"; readonly state: "requested" };

const observation = (
	engine_id: string,
	artisan_run_id: string,
	sequence: number,
	partial: PartialObservation,
): EngineObservation => {
	const base = {
		artisan_run_id,
		observation_id: `${artisan_run_id}:${sequence}`,
		raw: { engine_id, frame: partial._tag, transport: "test" },
		sequence,
	};
	return partial._tag === "agent_message_completed"
		? {
				...base,
				_tag: "agent_message_completed",
				item_id: `${artisan_run_id}:item:${sequence}`,
				message: partial.message,
				phase: partial.phase,
				turn_id: `${artisan_run_id}:turn`,
			}
		: {
				...base,
				_tag: "approval",
				approval_id: `${artisan_run_id}:approval`,
				description: "requires interaction",
				request: { kind: "command" },
				state: "requested",
			};
};

const engine = (
	id: string,
	opens: Array<EngineOpenInput>,
	behavior: {
		readonly observations?: ReadonlyArray<PartialObservation>;
		readonly terminal?: EngineRunTerminalState;
	} = {},
): Engine => ({
	Descriptor: {
		capabilities,
		display_name: id,
		id,
		transport: "test",
	},
	Open: (input) =>
		Effect.sync(() => {
			opens.push(input);
			const observations = (behavior.observations ?? []).map((partial, index) =>
				observation(id, input.artisan_run_id, index + 1, partial),
			);
			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Effect.succeed(behavior.terminal ?? "completed"),
				Events: Stream.fromIterable(observations),
				native_thread_id: `${id}-native:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `${id}-native:${input.artisan_run_id}` },
				Send: () => Effect.void,
			};
		}),
	Probe: () => Effect.die("Probe is outside compactor tests"),
});

const make_layer = (
	engines: ReadonlyArray<Engine>,
	compaction_model?: string,
	model_defaults: ReadonlyArray<{
		readonly context_window?: string;
		readonly model_id: string;
		readonly reasoning_effort?: "low" | "medium" | "high" | "xhigh" | "max";
	}> = [],
) => {
	let next_id = 0;
	const dependencies = Layer.mergeAll(
		make_engine_registry_layer(engines).pipe(Layer.orDie),
		Layer.succeed(
			RuntimeMetadata,
			RuntimeMetadata.of({
				instance_id: "backend-test",
				MakeId: (prefix) => Effect.sync(() => `${prefix}-${++next_id}`),
				Now: Effect.succeed("2026-07-30T10:00:00.000Z"),
			}),
		),
		Layer.succeed(SessionDefaultsService, {
			Read: Effect.succeed({
				...(compaction_model === undefined ? {} : { compaction_model }),
				models: model_defaults,
				permission: "supervised",
			}),
			Update: () => Effect.die("Update is outside compactor tests"),
		}),
	);

	return ThreadContinuationCompactorLive.pipe(Layer.provide(dependencies));
};

const head_entry = (logical_sequence: number, text: string): CanonicalTranscriptEntry => ({
	journal_sequence: logical_sequence,
	logical_sequence,
	role: logical_sequence % 2 === 0 ? "assistant" : "user",
	text,
});

const request = (overrides: Partial<ThreadCompactionRequest> = {}): ThreadCompactionRequest => ({
	head: [head_entry(1, "Establish the release plan."), head_entry(2, "Plan established.")],
	omitted_head_entries: 0,
	source: { engine_id: "claude", model_id: "claude-sonnet-5" },
	working_directory: "C:\\workspace",
	...overrides,
});

const summarize = (
	layer: Layer.Layer<ThreadContinuationCompactor>,
	input: ThreadCompactionRequest,
) =>
	Effect.runPromise(
		ThreadContinuationCompactor.pipe(
			Effect.flatMap((compactor) => compactor.Summarize(input)),
			Effect.provide(layer),
		),
	);

describe("thread continuation compactor", () => {
	it("returns none for an empty head without opening any engine", async () => {
		const opens: Array<EngineOpenInput> = [];
		const layer = make_layer([
			engine("claude", opens, {
				observations: [
					{ _tag: "agent_message_completed", message: "unused", phase: "final" },
				],
			}),
		]);

		const result = await summarize(layer, request({ head: [] }));

		expect(Option.isNone(result)).toBe(true);
		expect(opens).toEqual([]);
	});

	it("summarizes the head on the harness's fast compaction default", async () => {
		const opens: Array<EngineOpenInput> = [];
		const layer = make_layer([
			engine("claude", opens, {
				observations: [
					{
						_tag: "agent_message_completed",
						message: "progress note",
						phase: "commentary",
					},
					{
						_tag: "agent_message_completed",
						message: "  ## Objective\n- Ship the release plan.  ",
						phase: "final",
					},
				],
			}),
		]);

		const result = await summarize(layer, request({ omitted_head_entries: 2 }));

		expect(Option.getOrThrow(result)).toEqual({
			compactor: { engine_id: "claude", model_id: "claude-haiku-4-5" },
			summary: "## Objective\n- Ship the release plan.",
		});
		expect(opens).toHaveLength(1);
		const open = opens[0]!;
		expect(open._tag).toBe("start");
		if (open._tag !== "start") return;
		expect(open.model).toBe("claude-haiku-4-5");
		expect(open.working_directory).toBe("C:\\workspace");
		expect(open.provider_options).toEqual({
			"claude.disable_tools": true,
			"claude.permission_mode": "plan",
			"claude.safe_mode": true,
		});
		expect(open.permission_policy).toBeUndefined();
		expect(open.initial_text).toContain("--- BEGIN UNTRUSTED CONVERSATION TRANSCRIPT ---");
		expect(open.initial_text).toContain("--- END UNTRUSTED CONVERSATION TRANSCRIPT ---");
		expect(open.initial_text).toContain("Establish the release plan.");
		expect(open.initial_text).toContain("2 earlier transcript entries were omitted for size.");
	});

	it("prefers the configured catalog compaction model over the source engine", async () => {
		const claude_opens: Array<EngineOpenInput> = [];
		const codex_opens: Array<EngineOpenInput> = [];
		const layer = make_layer(
			[
				engine("claude", claude_opens),
				engine("codex", codex_opens, {
					observations: [
						{
							_tag: "agent_message_completed",
							message: "Configured summary.",
							phase: "final",
						},
					],
				}),
			],
			"codex-sol",
		);

		const result = await summarize(layer, request());

		expect(Option.getOrThrow(result)).toEqual({
			compactor: { engine_id: "codex", model_id: "gpt-5.6-sol" },
			summary: "Configured summary.",
		});
		expect(claude_opens).toEqual([]);
		expect(codex_opens).toHaveLength(1);
		const open = codex_opens[0]!;
		expect(open._tag).toBe("start");
		if (open._tag !== "start") return;
		expect(open.model).toBe("gpt-5.6-sol");
		expect(open.provider_options).toEqual({ "codex.reasoning_effort": "low" });
		expect(open.permission_policy).toEqual({
			approval: "never",
			network_access: false,
			write_access: false,
		});
	});

	it("uses the harness compaction default when the configured model is not a catalog id", async () => {
		const claude_opens: Array<EngineOpenInput> = [];
		const codex_opens: Array<EngineOpenInput> = [];
		const layer = make_layer(
			[
				engine("claude", claude_opens, {
					observations: [
						{
							_tag: "agent_message_completed",
							message: "Source summary.",
							phase: "final",
						},
					],
				}),
				engine("codex", codex_opens),
			],
			"gpt-5.6-sol",
		);

		const result = await summarize(layer, request());

		expect(Option.getOrThrow(result)).toEqual({
			compactor: { engine_id: "claude", model_id: "claude-haiku-4-5" },
			summary: "Source summary.",
		});
		expect(codex_opens).toEqual([]);
		expect(claude_opens).toHaveLength(1);
	});

	it("honors an explicit pick's saved effort and context defaults", async () => {
		const codex_opens: Array<EngineOpenInput> = [];
		const layer = make_layer(
			[
				engine("codex", codex_opens, {
					observations: [
						{
							_tag: "agent_message_completed",
							message: "Configured summary.",
							phase: "final",
						},
					],
				}),
			],
			"codex-sol",
			[{ context_window: "[long]", model_id: "codex-sol", reasoning_effort: "high" }],
		);

		const result = await summarize(layer, request());

		expect(Option.isSome(result)).toBe(true);
		expect(codex_opens).toHaveLength(1);
		const open = codex_opens[0]!;
		expect(open._tag).toBe("start");
		if (open._tag !== "start") return;
		expect(open.model).toBe("gpt-5.6-sol[long]");
		expect(open.provider_options).toEqual({ "codex.reasoning_effort": "high" });
	});

	it("summarizes with the thread's own model when the selection is inherited", async () => {
		const claude_opens: Array<EngineOpenInput> = [];
		const codex_opens: Array<EngineOpenInput> = [];
		const layer = make_layer(
			[
				engine("claude", claude_opens, {
					observations: [
						{
							_tag: "agent_message_completed",
							message: "Inherited summary.",
							phase: "final",
						},
					],
				}),
				engine("codex", codex_opens),
			],
			"inherited",
		);

		const result = await summarize(layer, request());

		expect(Option.getOrThrow(result)).toEqual({
			compactor: { engine_id: "claude", model_id: "claude-sonnet-5" },
			summary: "Inherited summary.",
		});
		expect(codex_opens).toEqual([]);
		expect(claude_opens).toHaveLength(1);
		const open = claude_opens[0]!;
		expect(open._tag).toBe("start");
		if (open._tag !== "start") return;
		expect(open.model).toBe("claude-sonnet-5");
	});

	it("falls back to the source model when the harness has no compaction default", async () => {
		const opens: Array<EngineOpenInput> = [];
		const layer = make_layer([
			engine("mystery", opens, {
				observations: [
					{
						_tag: "agent_message_completed",
						message: "Source summary.",
						phase: "final",
					},
				],
			}),
		]);

		const result = await summarize(
			layer,
			request({ source: { engine_id: "mystery", model_id: "mystery-1" } }),
		);

		expect(Option.getOrThrow(result)).toEqual({
			compactor: { engine_id: "mystery", model_id: "mystery-1" },
			summary: "Source summary.",
		});
		expect(opens).toHaveLength(1);
		const open = opens[0]!;
		expect(open._tag).toBe("start");
		if (open._tag !== "start") return;
		expect(open.model).toBe("mystery-1");
		expect(open.provider_options).toBeUndefined();
		expect(open.permission_policy).toEqual({
			approval: "never",
			network_access: false,
			write_access: false,
		});
	});

	it("returns none when the compaction turn does not complete", async () => {
		const opens: Array<EngineOpenInput> = [];
		const layer = make_layer([
			engine("claude", opens, {
				observations: [
					{ _tag: "agent_message_completed", message: "Almost done.", phase: "final" },
				],
				terminal: "failed",
			}),
		]);

		expect(Option.isNone(await summarize(layer, request()))).toBe(true);
		expect(opens).toHaveLength(1);
	});

	it("aborts to none when the turn requests interaction", async () => {
		const opens: Array<EngineOpenInput> = [];
		const layer = make_layer([
			engine("claude", opens, {
				observations: [
					{ _tag: "approval", state: "requested" },
					{ _tag: "agent_message_completed", message: "Never reached.", phase: "final" },
				],
			}),
		]);

		expect(Option.isNone(await summarize(layer, request()))).toBe(true);
	});

	it("returns none for an empty or whitespace-only final message", async () => {
		const opens: Array<EngineOpenInput> = [];
		const layer = make_layer([
			engine("claude", opens, {
				observations: [
					{ _tag: "agent_message_completed", message: "   \n\t", phase: "final" },
				],
			}),
		]);

		expect(Option.isNone(await summarize(layer, request()))).toBe(true);
	});
});
