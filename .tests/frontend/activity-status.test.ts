import { describe, expect, it } from "vitest";
import { Option } from "effect";

import type { SurfaceItem } from "@artisan/protocol";
import {
	HasActiveWorkspaceWork,
	MakeActivityStatusView,
	PlayfulActivityLabels,
} from "../../modules/frontend/src/lib/live-workspace/activity-status";
import type { LiveWorkspaceSnapshot } from "../../modules/frontend/src/lib/live-workspace/store";

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	errors: {},
	global_guidance: Option.none(),
	capabilities: Option.none(),
	capability_detail: Option.none(),
	capability_oauth: Option.none(),
	git_diff: Option.none(),
	git_workspace: Option.none(),
	model_behaviour: Option.none(),
	orchestration_graph: Option.none(),
	orchestration_groups: Option.none(),
	phase: "ready",
	preview_asset_metadata: Option.none(),
	preview_inspection_result: Option.none(),
	preview_inspection_session: Option.none(),
	preview_target: Option.none(),
	preview_targets: [],
	routine_detail: Option.none(),
	routines: Option.none(),
	selected_group_id: Option.none(),
	selected_thread_id: Option.some("thread-1"),
	session: Option.none(),
	surface_items: Option.none(),
	surface_usage: Option.none(),
	terminals: [],
	terminal_output: {},
	thread_work: Option.none(),
	tool_approvals: Option.none(),
	tool_invocations: Option.none(),
	tool_registry: Option.none(),
	transcript: Option.none(),
	threads: [],
	workspace_change_diff: Option.none(),
	workspace_changes: Option.none(),
	workspace_conflicts: Option.none(),
	workspace_file: Option.none(),
	workspace_file_page: Option.none(),
};

const RunningSnapshot = (
	surface_items: ReadonlyArray<SurfaceItem> = [],
): LiveWorkspaceSnapshot => ({
	...EmptySnapshot,
	surface_items: Option.some({ items: surface_items, journal_sequence: 1 }),
	thread_work: Option.some({
		agent_id: "agent-1",
		display_name: "Artisan",
		engine_id: "codex",
		role: "primary",
		run_id: "run-1",
		status: "running",
	}),
});

const ProcessSurface: SurfaceItem = {
	attribution: { run_id: "run-1", thread_id: "thread-1" },
	category: "process",
	kind: "shell_command",
	occurred_at: "2026-07-21T20:00:00.000Z",
	summary: { label: "Running tests" },
	surface_id: "surface-1",
};

describe("working activity status", () => {
	it("rotates original playful labels only for otherwise-generic active work", () => {
		expect(MakeActivityStatusView(RunningSnapshot(), 2, false)).toMatchObject({
			active: true,
			label: PlayfulActivityLabels[2],
			mode: "working",
		});
	});

	it("prefers safe observable work over a generic phrase", () => {
		expect(MakeActivityStatusView(RunningSnapshot([ProcessSurface]), 0, false).label).toBe(
			"Running tests",
		);
	});

	it("does not render opaque provider reasoning text", () => {
		const opaque: SurfaceItem = {
			attribution: { run_id: "run-1" },
			category: "native_action",
			kind: "opaque_engine_work",
			occurred_at: "2026-07-21T20:00:01.000Z",
			summary: { detail: "hidden reasoning", label: "Secret chain of thought" },
			surface_id: "surface-2",
		};
		expect(MakeActivityStatusView(RunningSnapshot([opaque]), 0, false).label).toBe(
			PlayfulActivityLabels[0],
		);
	});

	it("uses a stable plain label under reduced motion and restores idle", () => {
		expect(MakeActivityStatusView(RunningSnapshot(), 4, true).label).toBe("Working");
		expect(MakeActivityStatusView(EmptySnapshot, 0, false)).toEqual({
			active: false,
			label: "Idle",
			mode: "idle",
		});
	});

	it("derives one native working signal from selected work, orchestration, or thread metadata", () => {
		expect(HasActiveWorkspaceWork(EmptySnapshot)).toBe(false);
		expect(HasActiveWorkspaceWork(RunningSnapshot())).toBe(true);
		expect(
			HasActiveWorkspaceWork({
				...EmptySnapshot,
				orchestration_groups: Option.some({
					groups: [
						{
							coordinator_agent_id: "agent-1",
							created_at: "2026-07-21T20:00:00.000Z",
							group_id: "group-1",
							max_concurrency: 1,
							state: "joining",
							thread_id: "thread-1",
							updated_at: "2026-07-21T20:00:01.000Z",
							version: 1,
						},
					],
					journal_sequence: 1,
				}),
			}),
		).toBe(true);
		expect(
			HasActiveWorkspaceWork({
				...EmptySnapshot,
				threads: [
					{
						activity_version: 1,
						affinity_version: 1,
						created_at: "2026-07-21T20:00:00.000Z",
						last_activity_at: "2026-07-21T20:00:01.000Z",
						linked_projects: [],
						live_status: "Working",
						metadata_version: 1,
						pinned: false,
						project_affinity_scores: [],
						project_locked: false,
						thread_id: "thread-2",
						title: "Background work",
						title_locked: false,
						title_source: "automatic",
						updated_at: "2026-07-21T20:00:01.000Z",
					},
				],
			}),
		).toBe(true);
	});
});
