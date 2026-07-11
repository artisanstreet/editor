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
				expect.objectContaining({ _tag: "agent_message_completed", message: "ab" }),
				expect.objectContaining({ _tag: "terminal_activity", command: "pwd" }),
			]),
		);
		expect(events.filter((event) => event._tag === "native_action")).toHaveLength(0);
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
});
