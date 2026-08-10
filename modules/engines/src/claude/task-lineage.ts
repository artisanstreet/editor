import type { ClaudeTaskStarted } from "./task-lifecycle";

/** The tool identity needed to correlate a provider task without normalizing its transcript. */
export interface ClaudeTaskToolUse {
	readonly id: string;
	readonly name: string;
}

/** Run-owned native task/tool topology for one Claude CLI session. */
export interface ClaudeTaskLineageState {
	/** The task spawned by a native Agent/Task tool invocation. */
	readonly spawned_task_by_tool_use_id: ReadonlyMap<string, string>;
	/** The session/task transcript which emitted a native tool invocation. */
	readonly owner_native_thread_by_tool_use_id: ReadonlyMap<string, string>;
	/** Resolved canonical parent for each task, including a root session parent. */
	readonly parent_native_thread_by_task_id: ReadonlyMap<string, string>;
	/** Child tool ids observed before their parent task start arrives. */
	readonly pending_tool_use_ids_by_parent_tool_use_id: ReadonlyMap<string, ReadonlyArray<string>>;
	/** Tool invocations proven to own an isolated Agent/Task transcript. */
	readonly subagent_tool_use_ids: ReadonlySet<string>;
	/** Generic provider task ids proven to represent agents rather than background jobs. */
	readonly subagent_task_ids: ReadonlySet<string>;
}

/** Creates empty topology for one CLI stream. @since 0.8.0 */
export function EmptyClaudeTaskLineage(): ClaudeTaskLineageState {
	return {
		owner_native_thread_by_tool_use_id: new Map(),
		parent_native_thread_by_task_id: new Map(),
		pending_tool_use_ids_by_parent_tool_use_id: new Map(),
		spawned_task_by_tool_use_id: new Map(),
		subagent_task_ids: new Set(),
		subagent_tool_use_ids: new Set(),
	};
}

export interface ClaudeTaskLineageFrame {
	readonly announced_tools: ReadonlyArray<ClaudeTaskToolUse>;
	/** Non-null only for a child transcript frame. */
	readonly child_parent_tool_use_id?: string;
	readonly lifecycle_has_subagent_hint: boolean;
	readonly lifecycle_task_id?: string;
	readonly root_native_thread_id: string;
	readonly task_started?: ClaudeTaskStarted;
}

/** Proven native ownership for one multiplexed child transcript stream. */
export interface ClaudeChildTranscriptOwner {
	readonly agent_native_thread_id: string;
	readonly parent_native_thread_id: string;
}

function is_subagent_tool(tool: ClaudeTaskToolUse) {
	return tool.name === "Agent" || tool.name === "Task";
}

/**
 * Resolves a child transcript only after its spawning Agent/Task invocation
 * and its native task parent are both known. Generic background child frames
 * deliberately remain unowned and must not be rendered as agent work.
 */
export function ResolveClaudeChildTranscriptOwner(
	state: ClaudeTaskLineageState,
	parent_tool_use_id: string,
): ClaudeChildTranscriptOwner | undefined {
	if (!state.subagent_tool_use_ids.has(parent_tool_use_id)) return undefined;
	const agent_native_thread_id = state.spawned_task_by_tool_use_id.get(parent_tool_use_id);
	if (agent_native_thread_id === undefined) return undefined;
	const parent_native_thread_id =
		state.parent_native_thread_by_task_id.get(agent_native_thread_id);
	return parent_native_thread_id === undefined
		? undefined
		: { agent_native_thread_id, parent_native_thread_id };
}

/**
 * Advances native task topology from one CLI frame without performing I/O.
 * Start, classification, then ownership are intentionally ordered so both
 * task-first and tool-first streams converge on the same graph.
 *
 * @since 0.8.0
 */
export function AdvanceClaudeTaskLineage(
	state: ClaudeTaskLineageState,
	frame: ClaudeTaskLineageFrame,
): ClaudeTaskLineageState {
	const spawned_tasks = new Map(state.spawned_task_by_tool_use_id);
	const owners = new Map(state.owner_native_thread_by_tool_use_id);
	const parents = new Map(state.parent_native_thread_by_task_id);
	const pending = new Map(state.pending_tool_use_ids_by_parent_tool_use_id);

	const started_tool_use_id = frame.task_started?.tool_use_id;
	const started_task_id = frame.task_started?.task_id;
	if (started_tool_use_id !== undefined && started_task_id !== undefined) {
		spawned_tasks.set(started_tool_use_id, started_task_id);
		const parent_native_thread_id =
			owners.get(started_tool_use_id) ??
			(frame.lifecycle_has_subagent_hint ? frame.root_native_thread_id : undefined);
		if (parent_native_thread_id !== undefined)
			parents.set(started_task_id, parent_native_thread_id);

		for (const child_tool_use_id of pending.get(started_tool_use_id) ?? []) {
			owners.set(child_tool_use_id, started_task_id);
			const child_task_id = spawned_tasks.get(child_tool_use_id);
			if (child_task_id !== undefined && child_task_id !== started_task_id)
				parents.set(child_task_id, started_task_id);
		}
		pending.delete(started_tool_use_id);
	}

	const subagent_tool_use_ids = new Set(state.subagent_tool_use_ids);
	const subagent_task_ids = new Set(state.subagent_task_ids);
	const announced_subagent_tools = frame.announced_tools.filter(is_subagent_tool);
	if (announced_subagent_tools.length > 0 || frame.lifecycle_task_id !== undefined) {
		for (const tool of announced_subagent_tools) subagent_tool_use_ids.add(tool.id);
		if (frame.lifecycle_has_subagent_hint && frame.task_started?.tool_use_id !== undefined)
			subagent_tool_use_ids.add(frame.task_started.tool_use_id);
		for (const tool_use_id of subagent_tool_use_ids) {
			const task_id = spawned_tasks.get(tool_use_id);
			if (task_id !== undefined) subagent_task_ids.add(task_id);
		}
		if (frame.lifecycle_task_id !== undefined && frame.lifecycle_has_subagent_hint)
			subagent_task_ids.add(frame.lifecycle_task_id);
	}

	if (frame.announced_tools.length > 0) {
		if (frame.child_parent_tool_use_id === undefined) {
			for (const tool of frame.announced_tools) {
				owners.set(tool.id, frame.root_native_thread_id);
				const spawned_task_id = spawned_tasks.get(tool.id);
				if (spawned_task_id !== undefined)
					parents.set(spawned_task_id, frame.root_native_thread_id);
			}
		} else {
			const parent_task_id = spawned_tasks.get(frame.child_parent_tool_use_id);
			if (parent_task_id === undefined) {
				pending.set(frame.child_parent_tool_use_id, [
					...(pending.get(frame.child_parent_tool_use_id) ?? []),
					...frame.announced_tools.map((tool) => tool.id),
				]);
			} else {
				for (const tool of frame.announced_tools) {
					owners.set(tool.id, parent_task_id);
					const child_task_id = spawned_tasks.get(tool.id);
					if (child_task_id !== undefined && child_task_id !== parent_task_id)
						parents.set(child_task_id, parent_task_id);
				}
			}
		}
	}

	return {
		owner_native_thread_by_tool_use_id: owners,
		parent_native_thread_by_task_id: parents,
		pending_tool_use_ids_by_parent_tool_use_id: pending,
		spawned_task_by_tool_use_id: spawned_tasks,
		subagent_task_ids,
		subagent_tool_use_ids,
	};
}
