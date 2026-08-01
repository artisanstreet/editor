import { describe, expect, it } from "vitest";

import { classify_claude_semantic_failure, normalize_claude_event } from "@artisan/engines";

const input = (payload: unknown) => ({
	artisan_run_id: "run",
	frame_sequence: 1,
	payload,
	raw_frame_base64: "eA==",
	turn_id: "turn",
});

describe("Claude normalization", () => {
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
					phase: "unspecified",
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

	it("maps file/search/tool families and correlates tool results", () => {
		expect(
			normalize_claude_event(
				input({
					type: "assistant",
					message: {
						content: [
							{
								type: "tool_use",
								id: "e",
								name: "Edit",
								input: { file_path: "a.ts" },
							},
							{
								type: "tool_use",
								id: "w",
								name: "WebSearch",
								input: { query: "docs" },
							},
							{ type: "tool_use", id: "m", name: "mcp__server__tool", input: {} },
						],
					},
				}),
			),
		).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ _tag: "file", path: "a.ts" }),
				expect.objectContaining({ _tag: "search", query: "docs" }),
				expect.objectContaining({ _tag: "tool", tool_name: "mcp__server__tool" }),
			]),
		);
		expect(
			normalize_claude_event(
				input({
					type: "user",
					message: {
						content: [{ type: "tool_result", tool_use_id: "e", content: "ok" }],
					},
				}),
			),
		).toEqual([expect.objectContaining({ _tag: "tool", tool_id: "e", action: "completed" })]);
	});

	it("treats assistant errors and every non-success result as semantic failure", () => {
		expect(classify_claude_semantic_failure({ type: "assistant", error: "rate_limit" })).toBe(
			true,
		);
		expect(
			classify_claude_semantic_failure({ type: "result", subtype: "error_max_turns" }),
		).toBe(true);
		expect(classify_claude_semantic_failure({ type: "result", subtype: "success" })).toBe(
			false,
		);
	});

	it("retains API retry progress as a native action instead of a canonical retry", () => {
		expect(normalize_claude_event(input({ type: "system", subtype: "api_retry" }))).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				detail: "Claude API retry progress",
			}),
		]);
	});
});
