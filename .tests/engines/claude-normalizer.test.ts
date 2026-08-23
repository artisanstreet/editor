import { describe, expect, it } from "vitest";

import { artisan_error_codes } from "@artisan/catalog";
import {
	classify_claude_semantic_failure,
	normalize_claude_event,
	read_claude_tool_uses,
	type ClaudeNormalizationInput,
} from "@artisan/engines";

const input = (
	payload: unknown,
	frame_sequence = 1,
	overrides: Partial<ClaudeNormalizationInput> = {},
) =>
	({
		artisan_run_id: "run",
		frame_sequence,
		native_thread_id: "claude-session",
		payload,
		raw_frame_base64: "eA==",
		turn_id: "turn",
		...overrides,
	}) satisfies ClaudeNormalizationInput;

describe("Claude normalization", () => {
	it("maps CLI task lifecycle states to one native subagent identity", () => {
		const normalize_task = (payload: unknown, frame_sequence: number) =>
			normalize_claude_event(
				input(payload, frame_sequence, { native_subagent_task: true }),
			)[0];

		expect(
			normalize_task(
				{
					type: "system",
					subtype: "task_started",
					task_id: "task-1",
					description: "Inspect code",
					subagent_type: "Explore",
				},
				1,
			),
		).toMatchObject({
			_tag: "subagent",
			agent_native_thread_id: "task-1",
			native_thread_id: "claude-session",
			parent_native_thread_id: "claude-session",
			agent_path: "Explore",
			state: "running",
		});
		expect(
			normalize_task(
				{
					type: "system",
					subtype: "task_updated",
					task_id: "task-1",
					patch: { status: "paused", description: "Awaiting result" },
				},
				2,
			),
		).toMatchObject({ _tag: "subagent", state: "waiting", activity: "Awaiting result" });
		expect(
			normalize_task(
				{
					type: "system",
					subtype: "task_notification",
					task_id: "task-1",
					status: "stopped",
					summary: "Cancelled",
				},
				3,
			),
		).toMatchObject({ _tag: "subagent", state: "interrupted", activity: "Cancelled" });
	});

	it("does not project an unclassified CLI background task as a subagent", () => {
		for (const payload of [
			{
				type: "system",
				subtype: "task_started",
				task_id: "background-shell",
				tool_use_id: "bash-tool",
				task_type: "shell",
				description: "Run the build",
			},
			{
				type: "system",
				subtype: "task_notification",
				task_id: "background-shell",
				status: "completed",
				summary: "Build complete",
			},
		]) {
			expect(normalize_claude_event(input(payload))).toEqual([]);
		}
	});

	it("aggregates public text, hides thinking, and maps Bash", () => {
		const events = normalize_claude_event(
			input({
				type: "assistant",
				message: {
					content: [
						{ type: "text", text: "a" },
						{ type: "thinking", thinking: "secret" },
						{ type: "text", text: "b" },
						{ type: "tool_use", id: "b", name: "Bash", input: { command: "pwd" } },
					],
				},
			}),
		);

		expect(events).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "agent_message_completed",
					item_id: "claude:run:message",
					message: "ab",
					/** Prose sharing a message with tool calls is narration, not the reply. */
					phase: "commentary",
				}),
				expect.objectContaining({ _tag: "terminal_activity", command: "pwd" }),
			]),
		);
		expect(events.filter((event) => event._tag === "native_action")).toHaveLength(0);
	});

	it("keeps the native message id and run-stable delta identity without inventing phases", () => {
		expect(
			normalize_claude_event(
				input({
					type: "assistant",
					message: { content: [{ type: "text", text: "reply" }], id: "msg_01" },
				}),
			),
		).toEqual([
			expect.objectContaining({
				_tag: "agent_message_completed",
				item_id: "msg_01",
				phase: "unspecified",
			}),
		]);
		expect(
			normalize_claude_event(
				input({
					type: "stream_event",
					event: {
						type: "content_block_delta",
						delta: { type: "text_delta", text: "par" },
					},
				}),
			),
		).toEqual([
			expect.objectContaining({
				_tag: "agent_message_delta",
				delta: "par",
				item_id: "claude:run:message",
				phase: "unspecified",
			}),
		]);
	});

	it("reports result usage as cumulative per-run totals", () => {
		expect(
			normalize_claude_event(
				input({
					type: "result",
					subtype: "success",
					usage: { input_tokens: 4, output_tokens: 5 },
				}),
			),
		).toEqual([
			expect.objectContaining({
				_tag: "usage",
				basis: "cumulative",
				input_tokens: 4,
				output_tokens: 5,
			}),
		]);
	});

	it("maps result cache reads without folding them into the context gauge", () => {
		const [usage] = normalize_claude_event(
			input({
				type: "result",
				subtype: "success",
				usage: {
					cache_creation_input_tokens: 8_689,
					cache_read_input_tokens: 21_360,
					input_tokens: 10,
					output_tokens: 290,
				},
			}),
		);

		expect(usage).toMatchObject({
			_tag: "usage",
			basis: "cumulative",
			cached_input_tokens: 21_360,
			input_tokens: 10,
			output_tokens: 290,
		});
		expect(usage).not.toHaveProperty("context_tokens");
	});

	it("measures the context window from an assistant frame's per-response usage", () => {
		const events = normalize_claude_event(
			input({
				type: "assistant",
				message: {
					content: [{ type: "text", text: "reply" }],
					id: "msg_01",
					usage: {
						cache_creation_input_tokens: 8_689,
						cache_read_input_tokens: 21_360,
						input_tokens: 10,
						output_tokens: 290,
					},
				},
			}),
		);

		expect(events).toEqual([
			expect.objectContaining({ _tag: "agent_message_completed", item_id: "msg_01" }),
			expect.objectContaining({
				_tag: "usage",
				basis: "cumulative",
				context_tokens: 30_059,
				observation_id: "run:claude:1:usage",
			}),
		]);
	});

	/**
	 * The current CLI spells the boundary's metadata
	 * `compact_metadata`; a schema demanding `compactMetadata` silently dropped
	 * every compaction as "Unknown Claude event type: system". The boundary is
	 * the one event that says the window's history was replaced, so it must
	 * always land as a canonical compaction observation.
	 */
	it("captures a CLI compact boundary as a compaction observation", () => {
		const events = normalize_claude_event(
			input({
				type: "system",
				subtype: "compact_boundary",
				uuid: "boundary-1",
				session_id: "session",
				compact_metadata: { trigger: "auto", pre_tokens: 342_765 },
			}),
		);

		expect(events).toEqual([
			expect.objectContaining({
				_tag: "compaction",
				compaction_id: "boundary-1",
				state: "completed",
			}),
		]);
	});

	it("resets the context gauge from the boundary's post-compaction measurement", () => {
		const events = normalize_claude_event(
			input({
				type: "system",
				subtype: "compact_boundary",
				uuid: "boundary-2",
				session_id: "session",
				compact_metadata: { trigger: "auto", pre_tokens: 342_765, post_tokens: 38_120 },
			}),
		);

		expect(events).toEqual([
			expect.objectContaining({ _tag: "compaction", compaction_id: "boundary-2" }),
			expect.objectContaining({
				_tag: "usage",
				basis: "cumulative",
				context_tokens: 38_120,
			}),
		]);
	});

	/**
	 * Claude Code encrypts its reasoning. A live 2.1.220 stream opens a thinking
	 * block, sends every `thinking_delta` with `thinking: ""`, and settles it
	 * with a signature and no text — so there is no summary to forward, and a
	 * delta that forwards the emptiness opens a reasoning row with nothing in it
	 * and nothing ever to put there.
	 */
	it("forwards no reasoning for a thinking delta that carries none", () => {
		expect(
			normalize_claude_event(
				input({
					type: "stream_event",
					event: {
						type: "content_block_delta",
						index: 0,
						delta: { type: "thinking_delta", thinking: "", estimated_tokens: 50 },
					},
				}),
			),
		).toEqual([]);
	});

	it("still forwards a thinking delta that does carry text", () => {
		const events = normalize_claude_event(
			input({
				type: "stream_event",
				event: {
					type: "content_block_delta",
					index: 0,
					delta: { type: "thinking_delta", thinking: "Weighing two readings" },
				},
			}),
		);

		expect(events).toEqual([
			expect.objectContaining({
				_tag: "reasoning_summary_delta",
				delta: "Weighing two readings",
			}),
		]);
	});

	/**
	 * The exact envelope Claude Code 2.1.220 writes: snake outside, camel within.
	 * Declaring one convention throughout read every count as absent, which left
	 * the gauge showing the pre-compaction reading — a million tokens, on a
	 * window that had just been emptied — until some later turn happened to
	 * report usage.
	 */
	it("reads a live boundary's camelCase measurements", () => {
		const events = normalize_claude_event(
			input({
				type: "system",
				subtype: "compact_boundary",
				uuid: "boundary-live",
				session_id: "session",
				compactMetadata: {
					trigger: "auto",
					preTokens: 1_000_564,
					postTokens: 12_345,
					durationMs: 133_291,
				},
			}),
		);

		expect(events).toEqual([
			expect.objectContaining({
				_tag: "compaction",
				compaction_id: "boundary-live",
				duration_ms: 133_291,
				state: "completed",
			}),
			expect.objectContaining({
				_tag: "usage",
				basis: "cumulative",
				context_tokens: 12_345,
			}),
		]);
	});

	/** Recognition must not hinge on metadata internals the protocol may reshape. */
	it("captures a compact boundary whose metadata is missing or differently spelled", () => {
		const bare = normalize_claude_event(
			input({ type: "system", subtype: "compact_boundary", uuid: "boundary-3" }),
		);
		const camel = normalize_claude_event(
			input({
				type: "system",
				subtype: "compact_boundary",
				uuid: "boundary-4",
				compactMetadata: { trigger: "manual", post_tokens: 12 },
			}),
		);

		expect(bare).toEqual([expect.objectContaining({ _tag: "compaction" })]);
		expect(camel).toEqual([
			expect.objectContaining({ _tag: "compaction" }),
			expect.objectContaining({ _tag: "usage", context_tokens: 12 }),
		]);
	});

	/**
	 * A fixed sentence saying the input lives in raw provenance described the
	 * adapter's bookkeeping where the work should be, and rendered every row of a
	 * kind as the same line. The one argument that identifies the call goes there
	 * instead, and a call that discloses nothing identifying carries no detail at
	 * all so the row keeps its normalized label.
	 */
	it("details a tool row with the argument that identifies the call", () => {
		const tool_details = (name: string, tool_input: Record<string, unknown>) => {
			const observation = normalize_claude_event(
				input({
					type: "assistant",
					message: {
						content: [{ type: "tool_use", id: "call", name, input: tool_input }],
					},
				}),
			).find((event) => event._tag === "tool");

			return observation?._tag === "tool" ? observation.detail : undefined;
		};

		expect(tool_details("Task", { description: "Audit the scroll code" })).toBe(
			"Audit the scroll code",
		);
		expect(tool_details("TodoWrite", { todos: [] })).toBeUndefined();
		expect(tool_details("mcp__server__tool", { pattern: "   " })).toBeUndefined();
	});

	/**
	 * Grep and Glob are searches of the workspace, not anonymous tool work. The
	 * search shape is what lets a chain of them count itself as files searched,
	 * and the workspace scope is what keeps them from reading as web searches.
	 */
	it("normalizes Grep and Glob to workspace-scoped searches", () => {
		const search = (name: string, tool_input: Record<string, unknown>) =>
			normalize_claude_event(
				input({
					type: "assistant",
					message: {
						content: [{ type: "tool_use", id: "call", name, input: tool_input }],
					},
				}),
			).find((event) => event._tag === "search");

		expect(search("Grep", { pattern: "conversation_reply", path: "modules" })).toEqual(
			expect.objectContaining({
				query: "conversation_reply",
				scope: "workspace",
				search_id: "call",
				state: "started",
			}),
		);
		expect(search("Glob", { path: "modules/frontend" })).toEqual(
			expect.objectContaining({ query: "modules/frontend", scope: "workspace" }),
		);
	});

	/**
	 * Reading a file is file work. Routed to the file semantics, the row carries
	 * the path it opened and a chain of reads counts itself as files read. Only
	 * the result frame emits it: the projection keys a read row off the
	 * observation that reported it, so emitting on the request too would leave
	 * two rows for one read.
	 */
	it("normalizes a file read to file semantics, once, on its result", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{
						type: "tool_use",
						id: "read",
						name: "Read",
						input: { file_path: "modules/frontend/src/lib/conversation/trace.ts" },
					},
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));

		expect(normalize_claude_event(input(assistant))).toEqual([]);
		expect(
			normalize_claude_event({
				...input(
					{
						type: "user",
						message: {
							content: [{ type: "tool_result", tool_use_id: "read", content: "ok" }],
						},
					},
					2,
				),
				tool_uses,
			}),
		).toEqual([
			expect.objectContaining({
				_tag: "file",
				action: "read",
				path: "modules/frontend/src/lib/conversation/trace.ts",
			}),
		]);
	});

	/** A failed read still needs the shape that can carry a failed state. */
	it("keeps a failed read as a tool failure naming the file", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{
						type: "tool_use",
						id: "read",
						name: "Read",
						input: { file_path: "missing.ts" },
					},
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));

		expect(
			normalize_claude_event({
				...input(
					{
						type: "user",
						message: {
							content: [
								{
									type: "tool_result",
									tool_use_id: "read",
									content: "no such file",
									is_error: true,
								},
							],
						},
					},
					2,
				),
				tool_uses,
			}),
		).toEqual([
			expect.objectContaining({ _tag: "tool", action: "failed", detail: "missing.ts" }),
		]);
	});

	/**
	 * The Windows harness runs commands through a `PowerShell` tool. Matching
	 * only Bash sent those to the generic tool shape, so a chain of commands
	 * read "Used 2 tools" with the command text as an anonymous detail.
	 */
	it("normalizes PowerShell commands as terminal activity like Bash", () => {
		const observations = normalize_claude_event(
			input({
				type: "assistant",
				message: {
					content: [
						{
							type: "tool_use",
							id: "pwsh",
							name: "PowerShell",
							input: {
								command: "Remove-Item modules/library/src/lib/mock-library.ts",
							},
						},
					],
				},
			}),
		);

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "terminal_activity",
				activity_id: "pwsh",
				command: "Remove-Item modules/library/src/lib/mock-library.ts",
			}),
		]);
	});

	it("keeps Bash, search, and named-tool result semantics from their tool uses", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{ type: "tool_use", id: "bash", name: "Bash", input: { command: "pwd" } },
					{ type: "tool_use", id: "search", name: "WebSearch", input: { query: "docs" } },
					{ type: "tool_use", id: "named", name: "mcp__server__tool", input: {} },
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));
		const started = normalize_claude_event(input(assistant));
		const completed = normalize_claude_event({
			...input(
				{
					type: "user",
					message: {
						content: [
							{ type: "tool_result", tool_use_id: "bash", content: "ok" },
							{ type: "tool_result", tool_use_id: "search", content: "ok" },
							{ type: "tool_result", tool_use_id: "named", content: "ok" },
						],
					},
				},
				2,
			),
			tool_uses,
		});

		expect(started).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "terminal_activity",
					activity_id: "bash",
					command: "pwd",
				}),
				expect.objectContaining({ _tag: "search", search_id: "search", query: "docs" }),
				expect.objectContaining({
					_tag: "tool",
					tool_id: "named",
					tool_name: "mcp__server__tool",
				}),
			]),
		);
		expect(completed).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "terminal_activity",
					activity_id: "bash",
					command: "pwd",
					output: "ok",
					state: "completed",
				}),
				expect.objectContaining({
					_tag: "search",
					search_id: "search",
					query: "docs",
					state: "completed",
				}),
				expect.objectContaining({
					_tag: "tool",
					tool_id: "named",
					tool_name: "mcp__server__tool",
					action: "completed",
				}),
			]),
		);
		expect(completed).not.toContainEqual(expect.objectContaining({ tool_name: "claude-tool" }));
		expect(completed.map((event) => event.observation_id)).toEqual([
			"run:claude:2:bash",
			"run:claude:2:search",
			"run:claude:2:named",
		]);
	});

	it("settles a correlated named tool as failed without replacing its name or detail", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [{ type: "tool_use", id: "named", name: "mcp__server__tool", input: {} }],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));
		const [started] = normalize_claude_event(input(assistant));
		const [failed] = normalize_claude_event({
			...input(
				{
					type: "user",
					message: {
						content: [
							{
								type: "tool_result",
								tool_use_id: "named",
								is_error: true,
								content: "no",
							},
						],
					},
				},
				2,
			),
			tool_uses,
		});

		expect(failed).toMatchObject({
			_tag: "tool",
			tool_id: "named",
			tool_name: "mcp__server__tool",
			action: "failed",
		});
		if (started?._tag !== "tool" || failed?._tag !== "tool")
			throw new Error("Named tool must remain a tool observation across its lifecycle");
		expect(failed?.detail).toBe(started?.detail);
	});

	it("treats assistant errors and every non-success result as semantic failure", () => {
		expect(classify_claude_semantic_failure({ type: "assistant", error: "rate_limit" })).toBe(
			true,
		);
		for (const status of ["rejected", "exceeded"]) {
			expect(
				classify_claude_semantic_failure({
					type: "rate_limit_event",
					rate_limit_info: { status },
				}),
			).toBe(true);
		}
		expect(
			classify_claude_semantic_failure({
				type: "rate_limit_event",
				rate_limit_info: { status: "allowed_warning" },
			}),
		).toBe(false);
		expect(
			classify_claude_semantic_failure({ type: "result", subtype: "error_max_turns" }),
		).toBe(true);
		expect(classify_claude_semantic_failure({ type: "result", subtype: "success" })).toBe(
			false,
		);
	});

	it("transfers a typed assistant error into Artisan custody even alongside prose", () => {
		const events = normalize_claude_event(
			input({
				type: "assistant",
				error: "authentication_failed",
				message: {
					content: [{ type: "text", text: "Failed to authenticate: OAuth expired" }],
				},
			}),
		);

		expect(events).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ _tag: "agent_message_completed" }),
				expect.objectContaining({
					_tag: "native_action",
					detail: "Claude assistant error: authentication_failed",
					error_ref: {
						artisan_code: artisan_error_codes.provider_auth_failed,
						provider_code: "authentication_failed",
					},
				}),
			]),
		);
	});

	it("maps a provider code minted after this adapter to the unknown Artisan code", () => {
		expect(
			normalize_claude_event(
				input({
					type: "assistant",
					error: "brand_new_failure",
					message: { content: [] },
				}),
			),
		).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				error_ref: expect.objectContaining({
					artisan_code: artisan_error_codes.unknown,
					provider_code: "brand_new_failure",
				}),
			}),
		]);
	});

	it("keeps the failed result classification alongside the usage it rides with", () => {
		const events = normalize_claude_event(
			input({
				type: "result",
				subtype: "error_max_turns",
				is_error: true,
				usage: { input_tokens: 1, output_tokens: 2 },
			}),
		);

		expect(events).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ _tag: "usage" }),
				expect.objectContaining({
					_tag: "native_action",
					error_ref: expect.objectContaining({
						artisan_code: artisan_error_codes.run_turn_limit,
						provider_code: "error_max_turns",
					}),
				}),
			]),
		);
	});

	it("classifies terminal rate limits with exact provider evidence and leaves warnings quiet", () => {
		const resets_at_seconds = 1_786_075_592;

		expect(
			normalize_claude_event(
				input({
					type: "rate_limit_event",
					rate_limit_info: {
						rateLimitType: "five_hour",
						resetsAt: resets_at_seconds,
						status: "rejected",
					},
				}),
			),
		).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				error_ref: expect.objectContaining({
					artisan_code: artisan_error_codes.usage_limit_reached,
					limit_id: "five_hour",
					limit_scope: "unknown",
					resets_at: new Date(resets_at_seconds * 1_000).toISOString(),
				}),
			}),
		]);

		expect(
			normalize_claude_event(
				input({
					type: "rate_limit_event",
					rate_limit_info: { rateLimitType: "seven_day", status: "exceeded" },
				}),
			),
		).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				error_ref: expect.objectContaining({
					artisan_code: artisan_error_codes.usage_limit_reached,
					limit_id: "seven_day",
					limit_scope: "unknown",
				}),
			}),
		]);

		const warning = normalize_claude_event(
			input({
				type: "rate_limit_event",
				rate_limit_info: { status: "allowed_warning" },
			}),
		)[0];
		expect(warning).toMatchObject({ _tag: "native_action" });
		expect(
			warning !== undefined && "error_ref" in warning ? warning.error_ref : undefined,
		).toBeUndefined();

		const invalid_reset = normalize_claude_event(
			input({
				type: "rate_limit_event",
				rate_limit_info: { resetsAt: 1e100, status: "rejected" },
			}),
		)[0];
		expect(invalid_reset).toMatchObject({
			_tag: "native_action",
			error_ref: { artisan_code: artisan_error_codes.usage_limit_reached },
		});
		expect(
			invalid_reset !== undefined && "error_ref" in invalid_reset
				? invalid_reset.error_ref
				: undefined,
		).not.toHaveProperty("resets_at");
	});

	it("retains API retry progress as a native action instead of a canonical retry", () => {
		expect(normalize_claude_event(input({ type: "system", subtype: "api_retry" }))).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				detail: "Claude API retry progress",
			}),
		]);
	});

	/**
	 * The payload shape here is the one the CLI emits in `stream-json`: the
	 * applied-edit report rides the user frame as `tool_use_result`, not the
	 * `toolUseResult` spelling the on-disk transcript uses.
	 */
	it("counts a created file from the write the engine reported applying", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{
						type: "tool_use",
						id: "tool-1",
						name: "Write",
						input: { file_path: "C:\\repo\\README.md" },
					},
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));
		const events = normalize_claude_event({
			...input(
				{
					type: "user",
					message: {
						content: [
							{
								type: "tool_result",
								tool_use_id: "tool-1",
								content: "File created successfully",
							},
						],
					},
					tool_use_result: {
						type: "create",
						filePath: "C:\\repo\\README.md",
						content: "a\nb\nc",
						structuredPatch: [],
						originalFile: null,
					},
				},
				2,
			),
			tool_uses,
		});

		expect(events).toEqual([
			expect.objectContaining({
				_tag: "file",
				action: "created",
				lines_added: 3,
				lines_deleted: 0,
				path: "C:\\repo\\README.md",
			}),
		]);
	});

	it("counts an edited file from its structured patch, not from the block it replaced", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{
						type: "tool_use",
						id: "tool-2",
						name: "Edit",
						input: { file_path: "C:\\repo\\app.ts" },
					},
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));
		const events = normalize_claude_event({
			...input(
				{
					type: "user",
					message: {
						content: [
							{ type: "tool_result", tool_use_id: "tool-2", content: "Updated" },
						],
					},
					tool_use_result: {
						type: "update",
						filePath: "C:\\repo\\app.ts",
						content: "irrelevant",
						structuredPatch: [
							{ lines: [" kept", "-gone", "+added", " kept"] },
							{ lines: ["+one more"] },
						],
						originalFile: "kept\ngone\nkept",
					},
				},
				2,
			),
			tool_uses,
		});

		expect(events).toEqual([
			expect.objectContaining({
				_tag: "file",
				action: "modified",
				lines_added: 2,
				lines_deleted: 1,
			}),
		]);
	});

	it("applies one outer edit report only to its uniquely matching parallel file result", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{
						type: "tool_use",
						id: "tool-a",
						name: "Edit",
						input: { file_path: "C:\\repo\\a.ts" },
					},
					{
						type: "tool_use",
						id: "tool-b",
						name: "Edit",
						input: { file_path: "C:\\repo\\b.ts" },
					},
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));
		const events = normalize_claude_event({
			...input(
				{
					type: "user",
					message: {
						content: [
							{ type: "tool_result", tool_use_id: "tool-a", content: "Updated" },
							{ type: "tool_result", tool_use_id: "tool-b", content: "Updated" },
						],
					},
					tool_use_result: {
						type: "update",
						filePath: "C:\\repo\\b.ts",
						structuredPatch: [{ lines: ["-old", "+new"] }],
					},
				},
				2,
			),
			tool_uses,
		});

		expect(events).toEqual([
			expect.objectContaining({
				_tag: "file",
				path: "C:\\repo\\a.ts",
			}),
			expect.objectContaining({
				_tag: "file",
				lines_added: 1,
				lines_deleted: 1,
				path: "C:\\repo\\b.ts",
			}),
		]);
		expect(events[0]).not.toHaveProperty("lines_added");
		expect(events[0]).not.toHaveProperty("lines_deleted");
	});

	it("waits for the applied report before admitting a file change, then falls back to its path", () => {
		const assistant = {
			type: "assistant",
			message: {
				content: [
					{
						type: "tool_use",
						id: "tool-3",
						name: "Write",
						input: { file_path: "C:\\repo\\new.ts" },
					},
				],
			},
		};
		const tool_uses = new Map(read_claude_tool_uses(assistant).map((tool) => [tool.id, tool]));
		const announced = normalize_claude_event(input(assistant));
		const [completed] = normalize_claude_event({
			...input(
				{
					type: "user",
					message: {
						content: [
							{
								type: "tool_result",
								tool_use_id: "tool-3",
								content: "File created successfully",
							},
						],
					},
					tool_use_result: { type: "text" },
				},
				2,
			),
			tool_uses,
		});

		expect(announced).toEqual([]);
		expect(completed).toMatchObject({
			_tag: "file",
			path: "C:\\repo\\new.ts",
			observation_id: "run:claude:2:tool-3",
		});
		expect(completed).not.toHaveProperty("lines_added");
		expect(completed).not.toHaveProperty("lines_deleted");
	});

	it("leaves a tool result that reports no edit as a tool observation", () => {
		expect(
			normalize_claude_event(
				input({
					type: "user",
					message: {
						content: [{ type: "tool_result", tool_use_id: "tool-4", content: "ok" }],
					},
					tool_use_result: { type: "text" },
				}),
			),
		).toEqual([expect.objectContaining({ _tag: "tool", tool_id: "tool-4" })]);
	});
});
