import { describe, expect, it } from "vitest";

import {
	AdvanceClaudeTaskLineage,
	EmptyClaudeTaskLineage,
	ResolveClaudeChildTranscriptOwner,
} from "../../modules/engines/src/claude/task-lineage";
import {
	AdvanceClaudeChildTranscripts,
	EmptyClaudeChildTranscripts,
} from "../../modules/engines/src/claude/child-transcripts";

const root = "claude-session";
const lifecycle = (overrides: {
	task_id: string;
	tool_use_id?: string;
	subagent_type?: string;
}) => ({
	description: "Review the adapter",
	subtype: "task_started" as const,
	type: "system" as const,
	...overrides,
});

describe("Claude task lineage", () => {
	it("correlates a default Agent tool with its later task start", () => {
		const announced = AdvanceClaudeTaskLineage(EmptyClaudeTaskLineage(), {
			announced_tools: [{ id: "agent-tool", name: "Agent" }],
			lifecycle_has_subagent_hint: false,
			root_native_thread_id: root,
		});
		const started = AdvanceClaudeTaskLineage(announced, {
			announced_tools: [],
			lifecycle_has_subagent_hint: false,
			lifecycle_task_id: "agent-task",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "agent-task", tool_use_id: "agent-tool" }),
		});

		expect(started.subagent_task_ids).toContain("agent-task");
		expect(started.parent_native_thread_by_task_id.get("agent-task")).toBe(root);
	});

	it("repairs nested ownership when the child tool precedes its parent task start", () => {
		const nested_tool = AdvanceClaudeTaskLineage(EmptyClaudeTaskLineage(), {
			announced_tools: [{ id: "nested-tool", name: "Task" }],
			child_parent_tool_use_id: "parent-tool",
			lifecycle_has_subagent_hint: false,
			root_native_thread_id: root,
		});
		const nested_started = AdvanceClaudeTaskLineage(nested_tool, {
			announced_tools: [],
			lifecycle_has_subagent_hint: false,
			lifecycle_task_id: "nested-task",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "nested-task", tool_use_id: "nested-tool" }),
		});
		const parent_started = AdvanceClaudeTaskLineage(nested_started, {
			announced_tools: [],
			lifecycle_has_subagent_hint: false,
			lifecycle_task_id: "parent-task",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "parent-task", tool_use_id: "parent-tool" }),
		});

		expect(parent_started.subagent_task_ids).toContain("nested-task");
		expect(parent_started.parent_native_thread_by_task_id.get("nested-task")).toBe(
			"parent-task",
		);
	});

	it("does not promote a generic background task from child ownership alone", () => {
		const shell = AdvanceClaudeTaskLineage(EmptyClaudeTaskLineage(), {
			announced_tools: [{ id: "shell-tool", name: "Bash" }],
			lifecycle_has_subagent_hint: false,
			root_native_thread_id: root,
		});
		const child_frame = AdvanceClaudeTaskLineage(shell, {
			announced_tools: [],
			child_parent_tool_use_id: "shell-tool",
			lifecycle_has_subagent_hint: false,
			root_native_thread_id: root,
		});
		const started = AdvanceClaudeTaskLineage(child_frame, {
			announced_tools: [],
			lifecycle_has_subagent_hint: false,
			lifecycle_task_id: "shell-task",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "shell-task", tool_use_id: "shell-tool" }),
		});

		expect(started.subagent_task_ids).not.toContain("shell-task");
		expect(ResolveClaudeChildTranscriptOwner(started, "shell-tool")).toBeUndefined();
	});

	it("resolves nested child transcript ownership only after both task starts arrive", () => {
		const nested_tool = AdvanceClaudeTaskLineage(EmptyClaudeTaskLineage(), {
			announced_tools: [{ id: "nested-tool", name: "Task" }],
			child_parent_tool_use_id: "parent-tool",
			lifecycle_has_subagent_hint: false,
			root_native_thread_id: root,
		});
		const child_started = AdvanceClaudeTaskLineage(nested_tool, {
			announced_tools: [],
			lifecycle_has_subagent_hint: false,
			lifecycle_task_id: "child-task",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "child-task", tool_use_id: "nested-tool" }),
		});
		expect(ResolveClaudeChildTranscriptOwner(child_started, "nested-tool")).toBeUndefined();
		const parent_started = AdvanceClaudeTaskLineage(child_started, {
			announced_tools: [],
			lifecycle_has_subagent_hint: false,
			lifecycle_task_id: "parent-task",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "parent-task", tool_use_id: "parent-tool" }),
		});
		expect(ResolveClaudeChildTranscriptOwner(parent_started, "nested-tool")).toEqual({
			agent_native_thread_id: "child-task",
			parent_native_thread_id: "parent-task",
		});
	});

	it("accepts explicit provider agent metadata without a preceding tool frame", () => {
		const state = AdvanceClaudeTaskLineage(EmptyClaudeTaskLineage(), {
			announced_tools: [],
			lifecycle_has_subagent_hint: true,
			lifecycle_task_id: "remote-agent",
			root_native_thread_id: root,
			task_started: lifecycle({ task_id: "remote-agent", subagent_type: "Explore" }),
		});

		expect(state.subagent_task_ids).toContain("remote-agent");
	});

	it("retains every deferred child frame until its owner is known", () => {
		const lineage = EmptyClaudeTaskLineage();
		let state = EmptyClaudeChildTranscripts();

		for (let index = 0; index < 129; index += 1) {
			state = AdvanceClaudeChildTranscripts({
				base: {
					artisan_run_id: "run",
					protocol_version: "v1",
					raw_frame_base64: "e30=",
					transport: "stdio-jsonl",
					turn_id: "turn",
				},
				frame_sequence: index,
				lineage,
				message: { type: "assistant" },
				parent_tool_use_id: "not-yet-announced",
				state,
			}).state;
		}

		expect(state.deferred).toHaveLength(129);
	});
});
