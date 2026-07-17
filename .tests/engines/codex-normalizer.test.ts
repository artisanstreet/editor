import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { normalise_codex_notification } from "@artisan/engines";
import {
	accept_codex_turn_start,
	is_codex_resume_usage_baseline,
	make_codex_resumed_usage_state,
	observe_codex_turn_started,
} from "../../modules/engines/src/codex/internal/codex-resumed-usage";

function make_turn(id: string, status = "inProgress") {
	return {
		completedAt: status === "inProgress" ? null : 2,
		durationMs: status === "inProgress" ? null : 1_000,
		error: null,
		id,
		items: [],
		itemsView: "full",
		startedAt: 1,
		status,
	};
}

function token_usage(input_tokens: number, output_tokens: number) {
	return {
		cachedInputTokens: 0,
		inputTokens: input_tokens,
		outputTokens: output_tokens,
		reasoningOutputTokens: 0,
		totalTokens: input_tokens + output_tokens,
	};
}

function normalise(
	method: string,
	payload: unknown,
	options: {
		readonly expected_usage_turn_id?: string;
		readonly frame_sequence?: number;
		readonly id?: string | number;
		readonly usage_is_resume_baseline?: boolean;
	} = {},
) {
	return normalise_codex_notification({
		artisan_run_id: "normalizer-run",
		...(options.expected_usage_turn_id === undefined
			? {}
			: { expected_usage_turn_id: options.expected_usage_turn_id }),
		frame_sequence: options.frame_sequence ?? 1,
		...(options.id === undefined ? {} : { id: options.id }),
		method,
		payload,
		protocol_version: "v1",
		raw_frame_base64: "e30=",
		transport: "stdio-jsonl",
		...(options.usage_is_resume_baseline ? { usage_is_resume_baseline: true } : {}),
	});
}

describe("Codex normalizer", () => {
	it("accepts official 0.142.5 shapes while retaining byte-faithful provenance", async () => {
		const observations = await Effect.runPromise(
			Effect.all([
				normalise("turn/started", {
					threadId: "thread-1",
					turn: make_turn("turn-1"),
				}),
				normalise("turn/plan/updated", {
					explanation: null,
					plan: [{ status: "inProgress", step: "Inspect" }],
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("thread/tokenUsage/updated", {
					threadId: "thread-1",
					tokenUsage: {
						last: {
							cachedInputTokens: 1,
							inputTokens: 4,
							outputTokens: 2,
							reasoningOutputTokens: 1,
							totalTokens: 7,
						},
						modelContextWindow: 200_000,
						total: {
							cachedInputTokens: 1,
							inputTokens: 4,
							outputTokens: 2,
							reasoningOutputTokens: 1,
							totalTokens: 7,
						},
					},
					turnId: "turn-1",
				}),
			]),
		);
		const flattened = observations.flat();

		expect(flattened.map((observation) => observation._tag)).toEqual([
			"turn_state",
			"plan",
			"usage",
		]);
		expect(flattened[0]?.raw).toMatchObject({
			frame_sequence: 1,
			raw_frame_base64: "e30=",
		});
		expect(flattened[1]).toMatchObject({
			entries: [{ id: "turn-1:plan:0", status: "in_progress", text: "Inspect" }],
		});
		expect(flattened[2]).toMatchObject({
			input_tokens: 4,
			output_tokens: 2,
			sample_scope: "turn_total",
			turn_id: "turn-1",
		});
	});

	it("uses current-turn counters when a resumed thread includes earlier usage", async () => {
		const observations = await Effect.runPromise(
			normalise("thread/tokenUsage/updated", {
				threadId: "resumed-native-thread",
				tokenUsage: {
					last: token_usage(7, 3),
					modelContextWindow: 200_000,
					total: token_usage(10_007, 5_003),
				},
				turnId: "new-artisan-turn",
			}),
		);

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "usage",
				input_tokens: 7,
				output_tokens: 3,
				sample_scope: "turn_total",
				turn_id: "new-artisan-turn",
			}),
		]);
	});

	it("does not open resumed usage until a requested turn is observed started", () => {
		const resumed = make_codex_resumed_usage_state(true);
		const before_turn_start = observe_codex_turn_started(resumed, "historical-turn");
		const requested_turn = accept_codex_turn_start(before_turn_start, "current-turn");
		const historical_turn = observe_codex_turn_started(requested_turn, "historical-turn");
		const current_turn = observe_codex_turn_started(historical_turn, "current-turn");

		expect(is_codex_resume_usage_baseline(resumed)).toBe(true);
		expect(is_codex_resume_usage_baseline(before_turn_start)).toBe(true);
		expect(is_codex_resume_usage_baseline(requested_turn)).toBe(true);
		expect(is_codex_resume_usage_baseline(historical_turn)).toBe(true);
		expect(is_codex_resume_usage_baseline(current_turn)).toBe(false);
	});

	it("keeps resumed baseline usage raw until an observed new turn starts", async () => {
		const baseline = await Effect.runPromise(
			normalise(
				"thread/tokenUsage/updated",
				{
					threadId: "resumed-native-thread",
					tokenUsage: {
						last: token_usage(10_007, 5_003),
						modelContextWindow: 200_000,
						total: token_usage(10_007, 5_003),
					},
					turnId: "prior-native-turn",
				},
				{ frame_sequence: 1, usage_is_resume_baseline: true },
			),
		);
		const started = await Effect.runPromise(
			normalise(
				"turn/started",
				{ threadId: "resumed-native-thread", turn: make_turn("new-artisan-turn") },
				{ frame_sequence: 2 },
			),
		);
		const delayed_historical_usage = await Effect.runPromise(
			normalise(
				"thread/tokenUsage/updated",
				{
					threadId: "resumed-native-thread",
					tokenUsage: {
						last: token_usage(10_007, 5_003),
						modelContextWindow: 200_000,
						total: token_usage(10_007, 5_003),
					},
					turnId: "prior-native-turn",
				},
				{ expected_usage_turn_id: "new-artisan-turn", frame_sequence: 3 },
			),
		);
		const usage = await Effect.runPromise(
			normalise(
				"thread/tokenUsage/updated",
				{
					threadId: "resumed-native-thread",
					tokenUsage: {
						last: token_usage(7, 3),
						modelContextWindow: 200_000,
						total: token_usage(10_014, 5_006),
					},
					turnId: "new-artisan-turn",
				},
				{ expected_usage_turn_id: "new-artisan-turn", frame_sequence: 4 },
			),
		);

		expect(baseline).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				raw: expect.objectContaining({
					frame_sequence: 1,
					native_method: "thread/tokenUsage/updated",
				}),
			}),
		]);
		expect(
			[...baseline, ...started, ...delayed_historical_usage].some(
				(observation) => observation._tag === "usage",
			),
		).toBe(false);
		expect(delayed_historical_usage).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				raw: expect.objectContaining({ frame_sequence: 3 }),
			}),
		]);
		expect(usage).toEqual([
			expect.objectContaining({
				_tag: "usage",
				input_tokens: 7,
				output_tokens: 3,
				sample_scope: "turn_total",
				turn_id: "new-artisan-turn",
			}),
		]);
	});

	it("rejects fractional usage counters instead of producing a durable sample", async () => {
		const observations = await Effect.runPromise(
			normalise("thread/tokenUsage/updated", {
				threadId: "thread-1",
				tokenUsage: {
					last: token_usage(1.5, 2),
					total: token_usage(1.5, 2),
				},
				turnId: "turn-1",
			}),
		);

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				detail: "Malformed known Codex payload",
			}),
		]);
	});

	it("expands multi-question and multi-file frames into uniquely identified observations", async () => {
		const [questions, files] = await Effect.runPromise(
			Effect.all([
				normalise(
					"item/tool/requestUserInput",
					{
						autoResolutionMs: null,
						itemId: "question-tool",
						questions: [
							{
								header: "One",
								id: "question-1",
								isOther: false,
								isSecret: false,
								options: null,
								question: "First?",
							},
							{
								header: "Two",
								id: "question-2",
								isOther: true,
								isSecret: false,
								options: [],
								question: "Second?",
							},
						],
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 4, id: "question-request" },
				),
				normalise(
					"item/completed",
					{
						completedAtMs: 12,
						item: {
							changes: [
								{
									diff: "+new",
									kind: { type: "add" },
									path: "src/new.ts",
								},
								{
									diff: "-old",
									kind: { type: "delete" },
									path: "src/old.ts",
								},
							],
							id: "files-1",
							status: "completed",
							type: "fileChange",
						},
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 5 },
				),
			]),
		);
		const expanded = [...questions, ...files];

		expect(questions).toMatchObject([
			{ question_id: "question-1", state: "requested" },
			{ question_id: "question-2", state: "requested" },
		]);
		expect(files).toMatchObject([
			{ action: "created", path: "src/new.ts" },
			{ action: "deleted", path: "src/old.ts" },
		]);
		expect(new Set(expanded.map((observation) => observation.observation_id)).size).toBe(4);
	});

	it("surfaces safe reasoning summaries and stateful search plus compaction activity", async () => {
		const observations = await Effect.runPromise(
			Effect.all([
				normalise("item/reasoning/summaryTextDelta", {
					delta: "Checking the adapter",
					itemId: "reasoning-1",
					summaryIndex: 0,
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/reasoning/textDelta", {
					contentIndex: 0,
					delta: "private reasoning",
					itemId: "reasoning-1",
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/started", {
					item: {
						action: null,
						id: "search-1",
						query: "Codex app-server docs",
						type: "webSearch",
					},
					startedAtMs: 1,
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/completed", {
					completedAtMs: 2,
					item: {
						action: null,
						id: "search-1",
						query: "Codex app-server docs",
						type: "webSearch",
					},
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/started", {
					item: { id: "compact-1", type: "contextCompaction" },
					startedAtMs: 3,
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/completed", {
					completedAtMs: 4,
					item: { id: "compact-1", type: "contextCompaction" },
					threadId: "thread-1",
					turnId: "turn-1",
				}),
			]),
		);
		const flattened = observations.flat();

		expect(flattened).toMatchObject([
			{
				_tag: "reasoning_summary_delta",
				delta: "Checking the adapter",
				summary_index: 0,
			},
			{
				_tag: "native_action",
				detail: "Private reasoning text retained only in raw provenance",
			},
			{ _tag: "search", state: "started" },
			{ _tag: "search", state: "completed" },
			{ _tag: "compaction", state: "started" },
			{ _tag: "compaction", state: "completed" },
		]);
		expect(flattened[1]).not.toHaveProperty("delta");
	});

	it("preserves unknown and malformed known frames without inventing canonical facts", async () => {
		const observations = await Effect.runPromise(
			Effect.all([
				normalise("item/fileChange/outputDelta", {
					delta: "patch",
					itemId: "file-item-not-a-path",
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("future/method", { whatever: true }),
				normalise("turn/started", { threadId: "thread-1", turn: { id: 4 } }),
			]),
		);
		const flattened = observations.flat();

		expect(flattened.map((observation) => observation._tag)).toEqual([
			"native_action",
			"native_action",
			"native_action",
		]);
		expect(flattened[0]).toMatchObject({
			action: "item/fileChange/outputDelta",
			detail: "Received deprecated file-change output delta",
		});
		expect(flattened[2]).toMatchObject({
			action: "turn/started",
			detail: "Malformed known Codex payload",
		});
	});
});
