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
	it("retains completion-only compaction notifications without fabricating an identity", async () => {
		const [observation] = await Effect.runPromise(
			normalise("thread/compacted", { threadId: "thread-1", turnId: "turn-1" }),
		);

		expect(observation).toMatchObject({ _tag: "compaction", state: "completed" });
		expect(observation).not.toHaveProperty("compaction_id");
	});

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

	it("keeps approval response identity opaque while projecting human action details", async () => {
		const [command, file_change] = await Effect.runPromise(
			Effect.all([
				normalise(
					"item/commandExecution/requestApproval",
					{
						additionalPermissions: { network: { enabled: true } },
						command: "pnpm test",
						cwd: "C:\\workspace",
						itemId: "call_command",
						reason: "Run the test suite",
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 6, id: "approval-command" },
				),
				normalise(
					"item/fileChange/requestApproval",
					{
						grantRoot: "C:\\workspace",
						itemId: "call_files",
						reason: "Apply the generated fixes",
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 7, id: "approval-files" },
				),
			]),
		);

		expect(command).toMatchObject([
			{
				approval_id: "approval-command",
				description: "Run the test suite",
				request: {
					command: "pnpm test",
					cwd: "C:\\workspace",
					kind: "command",
					reason: "Run the test suite",
				},
				state: "requested",
			},
		]);
		expect(file_change).toMatchObject([
			{
				approval_id: "approval-files",
				description: "Apply the generated fixes",
				request: {
					kind: "file_change",
					reason: "Apply the generated fixes",
				},
				state: "requested",
			},
		]);
		expect(command[0]?.raw.frame).toMatchObject({
			additionalPermissions: { network: { enabled: true } },
			itemId: "call_command",
		});
		expect(file_change[0]?.raw.frame).toMatchObject({
			grantRoot: "C:\\workspace",
			itemId: "call_files",
		});
		expect(JSON.stringify([...command, ...file_change])).not.toContain(
			"requestApproval for call_",
		);
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
			{ _tag: "compaction", compaction_id: "compact-1", state: "started" },
			{ _tag: "compaction", compaction_id: "compact-1", state: "completed" },
		]);
		expect(flattened[1]).not.toHaveProperty("delta");
	});

	it("keeps each assistant item's delta and completion identity stable within one turn", async () => {
		const observations = await Effect.runPromise(
			Effect.all([
				normalise(
					"item/agentMessage/delta",
					{
						delta: "First ",
						itemId: "assistant-item-1",
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 10 },
				),
				normalise(
					"item/agentMessage/delta",
					{
						delta: "Second ",
						itemId: "assistant-item-2",
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 11 },
				),
				normalise(
					"item/completed",
					{
						completedAtMs: 12,
						item: {
							id: "assistant-item-1",
							memoryCitation: null,
							phase: "commentary",
							text: "First message",
							type: "agentMessage",
						},
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 12 },
				),
				normalise(
					"item/completed",
					{
						completedAtMs: 13,
						item: {
							id: "assistant-item-2",
							memoryCitation: null,
							phase: null,
							text: "Second message",
							type: "agentMessage",
						},
						threadId: "thread-1",
						turnId: "turn-1",
					},
					{ frame_sequence: 13 },
				),
			]),
		);

		expect(observations.flat()).toMatchObject([
			{ _tag: "agent_message_delta", item_id: "assistant-item-1", phase: "unspecified" },
			{ _tag: "agent_message_delta", item_id: "assistant-item-2", phase: "unspecified" },
			{ _tag: "agent_message_completed", item_id: "assistant-item-1", phase: "commentary" },
			{ _tag: "agent_message_completed", item_id: "assistant-item-2", phase: "unspecified" },
		]);

		const [unknown_phase] = await Effect.runPromise(
			normalise("item/completed", {
				item: {
					id: "assistant-item-3",
					memoryCitation: null,
					phase: "future-provider-phase",
					text: "Third message",
					type: "agentMessage",
				},
				threadId: "thread-1",
				turnId: "turn-1",
			}),
		);
		expect(unknown_phase).toMatchObject({
			_tag: "agent_message_completed",
			phase: "unspecified",
		});
	});

	it("preserves the provider retry lifecycle rather than flattening errors into diagnostics", async () => {
		const observations = await Effect.runPromise(
			Effect.all([
				normalise("error", {
					error: { message: "Temporarily unavailable" },
					threadId: "thread-1",
					turnId: "turn-1",
					willRetry: true,
				}),
				normalise("error", {
					error: { message: "Invalid request" },
					threadId: "thread-1",
					turnId: "turn-1",
					willRetry: false,
				}),
			]),
		);

		expect(observations.flat()).toMatchObject([
			{
				_tag: "retry",
				attempt_state: "retrying",
				message: "Temporarily unavailable",
				will_retry: true,
			},
			{
				_tag: "retry",
				attempt_state: "terminal",
				message: "Invalid request",
				will_retry: false,
			},
		]);
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
