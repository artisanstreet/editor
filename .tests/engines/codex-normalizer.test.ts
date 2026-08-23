import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import {
	MakeEngineSubagentTranscriptObservation,
	normalise_codex_notification,
} from "@artisan/engines";

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
	it("normalizes provider-native subagent discovery without leaking it as an action", async () => {
		const [observation] = await Effect.runPromise(
			normalise("item/started", {
				item: {
					agentPath: "/root/reviewer",
					agentThreadId: "thread-child",
					id: "subagent-1",
					kind: "started",
					type: "subAgentActivity",
				},
				threadId: "thread-root",
				turnId: "turn-root",
			}),
		);

		expect(observation).toMatchObject({
			_tag: "subagent",
			agent_native_thread_id: "thread-child",
			agent_path: "/root/reviewer",
			activity: "started",
			parent_native_thread_id: "thread-root",
			state: "discovered",
			turn_id: "turn-root",
		});
	});

	it("normalizes current collaboration tool recipients as native subagents", async () => {
		const observations = await Effect.runPromise(
			normalise("item/started", {
				item: {
					agentsStates: {},
					id: "collab-1",
					receiverThreadIds: ["thread-child-a", "thread-child-b"],
					senderThreadId: "thread-root",
					status: "inProgress",
					tool: "spawn_agent",
					type: "collabAgentToolCall",
				},
				threadId: "thread-root",
				turnId: "turn-root",
			}),
		);

		expect(observations).toMatchObject([
			{
				_tag: "subagent",
				agent_native_thread_id: "thread-child-a",
				activity: "spawn_agent",
				parent_native_thread_id: "thread-root",
				state: "discovered",
			},
			{
				_tag: "subagent",
				agent_native_thread_id: "thread-child-b",
			},
		]);
	});

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
		expect(flattened[0]).toMatchObject({ native_thread_id: "thread-1" });
		expect(flattened[1]).toMatchObject({
			entries: [{ id: "turn-1:plan:0", status: "in_progress", text: "Inspect" }],
		});
		expect(flattened[2]).toMatchObject({
			basis: "cumulative",
			cached_input_tokens: 1,
			context_tokens: 7,
			context_window_tokens: 200_000,
			input_tokens: 4,
			output_tokens: 2,
		});
	});

	/**
	 * Every turn resends the conversation, so the running total counts the same
	 * prefix once per turn. Gauging the window from it reports a short thread as
	 * most of the way full; only the last request describes what is in there.
	 */
	it("gauges the window from the last request while billing counts stay cumulative", async () => {
		const [usage] = await Effect.runPromise(
			normalise("thread/tokenUsage/updated", {
				threadId: "thread-1",
				tokenUsage: {
					last: {
						cachedInputTokens: 29_000,
						inputTokens: 34_000,
						outputTokens: 900,
						totalTokens: 34_900,
					},
					modelContextWindow: 258_400,
					total: {
						cachedInputTokens: 129_536,
						inputTokens: 148_722,
						outputTokens: 1_013,
						totalTokens: 149_735,
					},
				},
				turnId: "turn-1",
			}),
		);

		expect(usage).toMatchObject({
			cached_input_tokens: 129_536,
			context_tokens: 34_900,
			context_window_tokens: 258_400,
			input_tokens: 148_722,
			output_tokens: 1_013,
		});
	});

	it("leaves the window unknown when no last request is reported", async () => {
		const [usage] = await Effect.runPromise(
			normalise("thread/tokenUsage/updated", {
				threadId: "thread-1",
				tokenUsage: {
					modelContextWindow: 258_400,
					total: { inputTokens: 148_722, outputTokens: 1_013, totalTokens: 149_735 },
				},
				turnId: "turn-1",
			}),
		);

		expect(usage).not.toHaveProperty("context_tokens");
		expect(usage).toMatchObject({ input_tokens: 148_722 });
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
			{ _tag: "search", state: "started" },
			{ _tag: "search", state: "completed" },
			{ _tag: "compaction", compaction_id: "compact-1", state: "started" },
			{ _tag: "compaction", compaction_id: "compact-1", state: "completed" },
		]);
		/** Private reasoning deltas stay out of both canonical and native-visible output. */
		expect(JSON.stringify(flattened)).not.toContain("private reasoning");
	});

	it("keeps public reasoning boundaries and authoritative completion separate from private content", async () => {
		const observations = await Effect.runPromise(
			Effect.all([
				normalise("item/reasoning/summaryPartAdded", {
					itemId: "reasoning-1",
					summaryIndex: 0,
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/reasoning/summaryTextDelta", {
					delta: "Inspecting the existing path",
					itemId: "reasoning-1",
					summaryIndex: 0,
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/reasoning/summaryPartAdded", {
					itemId: "reasoning-1",
					summaryIndex: 1,
					threadId: "thread-1",
					turnId: "turn-1",
				}),
				normalise("item/completed", {
					item: {
						content: ["private chain of thought"],
						id: "reasoning-1",
						summary: ["Inspecting the existing path", "Applying the narrow fix"],
						type: "reasoning",
					},
					threadId: "thread-1",
					turnId: "turn-1",
				}),
			]),
		);
		const flattened = observations.flat();

		expect(flattened).toMatchObject([
			{
				_tag: "reasoning_summary_delta",
				delta: "Inspecting the existing path",
				summary_index: 0,
			},
			{
				_tag: "reasoning_summary_delta",
				delta: "\n\n",
				summary_index: 1,
			},
			{
				_tag: "reasoning_summary_completed",
				item_id: "reasoning-1",
				text: "Inspecting the existing path\n\nApplying the narrow fix",
			},
		]);
		expect(flattened.filter(({ _tag }) => _tag === "reasoning_summary_completed")).toHaveLength(
			1,
		);
		const completed = flattened.find(({ _tag }) => _tag === "reasoning_summary_completed");
		expect(completed).not.toHaveProperty("content");
		expect(completed).not.toHaveProperty("delta");
		if (completed?._tag !== "reasoning_summary_completed") return;
		expect(
			MakeEngineSubagentTranscriptObservation({
				agent_native_thread_id: "child-thread",
				observation: completed,
				parent_native_thread_id: "parent-thread",
			}),
		).toMatchObject({
			content: {
				_tag: "reasoning_summary_completed",
				text: "Inspecting the existing path\n\nApplying the narrow fix",
			},
		});
	});

	it("rejects malformed authoritative reasoning summaries without projecting content", async () => {
		const [observation] = await Effect.runPromise(
			normalise("item/completed", {
				item: {
					id: "reasoning-1",
					summary: ["public", 2],
					type: "reasoning",
				},
				threadId: "thread-1",
				turnId: "turn-1",
			}),
		);

		expect(observation).toMatchObject({
			_tag: "native_action",
			detail: "Malformed known Codex payload",
		});
	});

	it("settles an empty public reasoning item without manufacturing visible text", async () => {
		const [observation] = await Effect.runPromise(
			normalise("item/completed", {
				item: { id: "reasoning-empty", summary: [], type: "reasoning" },
				threadId: "thread-1",
				turnId: "turn-1",
			}),
		);

		expect(observation).toMatchObject({
			_tag: "reasoning_summary_completed",
			item_id: "reasoning-empty",
		});
		expect(observation).not.toHaveProperty("text");
	});

	it("preserves a final message phase from item start before phase-less deltas arrive", async () => {
		const [observation] = await Effect.runPromise(
			normalise("item/started", {
				item: {
					id: "assistant-final",
					memoryCitation: null,
					phase: "final",
					text: "",
					type: "agentMessage",
				},
				threadId: "thread-1",
				turnId: "turn-1",
			}),
		);

		expect(observation).toMatchObject({
			_tag: "agent_message_delta",
			delta: "",
			item_id: "assistant-final",
			phase: "final",
			turn_id: "turn-1",
		});
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

	it("adds terminal usage-limit evidence without replacing the retry observation", async () => {
		const payload = {
			error: { message: "You have insufficient quota for this request" },
			threadId: "thread-1",
			turnId: "turn-1",
			willRetry: false,
		};
		const observations = await Effect.runPromise(
			normalise("error", payload, { frame_sequence: 28 }),
		);

		expect(observations).toMatchObject([
			{
				_tag: "retry",
				attempt_state: "terminal",
				message: payload.error.message,
				will_retry: false,
			},
			{
				_tag: "native_action",
				detail: payload.error.message,
				error_ref: {
					artisan_code: "AE-PROVIDER-201",
					limit_scope: "unknown",
				},
			},
		]);
		expect(observations.map(({ observation_id }) => observation_id)).toEqual([
			"normalizer-run:native:28",
			"normalizer-run:native:28:usage_limit",
		]);
		const usage_action = observations.find(({ _tag }) => _tag === "native_action");
		if (usage_action?._tag !== "native_action") throw new Error("Expected usage action");
		expect(usage_action.raw).toMatchObject({ frame: payload, frame_sequence: 28 });
		expect(usage_action.error_ref).not.toHaveProperty("affected_model_id");
		expect(usage_action.error_ref).not.toHaveProperty("limit_id");
		expect(usage_action.error_ref).not.toHaveProperty("limit_label");
		expect(usage_action.error_ref).not.toHaveProperty("resets_at");

		for (const message of ["Rate limit exceeded", "Your usage-limit was reached"]) {
			const classified = await Effect.runPromise(
				normalise("error", {
					error: { message },
					threadId: "thread-1",
					turnId: "turn-1",
					willRetry: false,
				}),
			);
			expect(classified, message).toHaveLength(2);
			expect(classified[1]).toMatchObject({
				_tag: "native_action",
				error_ref: { artisan_code: "AE-PROVIDER-201" },
			});
		}
	});

	it("does not promote retrying, overload, billing, or request errors into quota evidence", async () => {
		const cases = [
			{ message: "Rate limit exceeded", willRetry: true },
			{ message: "Provider is overloaded", willRetry: false },
			{ message: "Billing limit reached", willRetry: false },
			{ message: "Invalid request", willRetry: false },
		];

		for (const [index, error_case] of cases.entries()) {
			const observations = await Effect.runPromise(
				normalise(
					"error",
					{
						error: { message: error_case.message },
						threadId: "thread-1",
						turnId: "turn-1",
						willRetry: error_case.willRetry,
					},
					{ frame_sequence: 40 + index },
				),
			);

			expect(observations, error_case.message).toHaveLength(1);
			expect(observations[0]).toMatchObject({ _tag: "retry" });
		}
	});

	it("classifies terminal sign-in failures without replacing the retry observation", async () => {
		const payload = {
			error: {
				message:
					"Your access token could not be refreshed because your refresh token was already used. Please log out and sign in again.",
			},
			threadId: "thread-1",
			turnId: "turn-1",
			willRetry: false,
		};
		const observations = await Effect.runPromise(
			normalise("error", payload, { frame_sequence: 29 }),
		);

		expect(observations).toMatchObject([
			{
				_tag: "retry",
				attempt_state: "terminal",
				message: payload.error.message,
				will_retry: false,
			},
			{
				_tag: "native_action",
				detail: payload.error.message,
				error_ref: { artisan_code: "AE-CLIENT_STATE-102" },
			},
		]);
		expect(observations.map(({ observation_id }) => observation_id)).toEqual([
			"normalizer-run:native:29",
			"normalizer-run:native:29:auth",
		]);

		for (const message of [
			"401 Unauthorized",
			"Provided authentication token is expired. Please try signing in again.",
			"You are not signed in",
		]) {
			const classified = await Effect.runPromise(
				normalise("error", {
					error: { message },
					threadId: "thread-1",
					turnId: "turn-1",
					willRetry: false,
				}),
			);
			expect(classified, message).toHaveLength(2);
			expect(classified[1]).toMatchObject({
				_tag: "native_action",
				error_ref: { artisan_code: "AE-CLIENT_STATE-102" },
			});
		}

		const retrying = await Effect.runPromise(
			normalise("error", {
				error: { message: "401 Unauthorized" },
				threadId: "thread-1",
				turnId: "turn-1",
				willRetry: true,
			}),
		);
		expect(retrying).toHaveLength(1);
		expect(retrying[0]).toMatchObject({ _tag: "retry" });
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
