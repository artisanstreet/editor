import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { normalise_codex_notification } from "@artisan/engines";

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

function normalise(
	method: string,
	payload: unknown,
	options: { readonly frame_sequence?: number; readonly id?: string | number } = {},
) {
	return normalise_codex_notification({
		artisan_run_id: "normalizer-run",
		frame_sequence: options.frame_sequence ?? 1,
		...(options.id === undefined ? {} : { id: options.id }),
		method,
		payload,
		protocol_version: "v1",
		raw_frame_base64: "e30=",
		transport: "stdio-jsonl",
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
		expect(flattened[2]).toMatchObject({ basis: "cumulative" });
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
