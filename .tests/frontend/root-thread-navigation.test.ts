import { describe, expect, it } from "vitest";
import { Option } from "effect";

import type { ThreadListItem } from "@artisan/protocol";
import {
	ApplyRootThreadListUpdate,
	FormatRecentThreadTime,
	ProjectScopedThreadGroups,
	ResolveThreadRoute,
	ThreadRouteId,
	ThreadRouteHasWorkspace,
	ThreadRouteOwnsTarget,
	ThreadRoutePath,
} from "../../modules/frontend/src/lib/root/thread-navigation";

const MakeThread = (
	thread_id: string,
	last_activity_at: string,
	project_id?: string,
): ThreadListItem => ({
	activity_version: 1,
	affinity_version: 1,
	created_at: "2026-07-25T10:00:00.000Z",
	current_goal: undefined,
	last_activity_at,
	live_status: "idle",
	metadata_version: 1,
	pinned: false,
	primary_project:
		project_id === undefined
			? undefined
			: {
					display_name: project_id,
					project_id,
					root_path: `C:\\projects\\${project_id}`,
				},
	project_affinity_scores: [],
	project_locked: false,
	rename_suggestion: undefined,
	rehome_suggestion: undefined,
	linked_projects: [],
	thread_id,
	title: thread_id,
	title_locked: false,
	title_source: "initial",
	updated_at: last_activity_at,
});

describe("root thread navigation", () => {
	it("uses encoded workspace and bare Snowflake segments in canonical thread routes", () => {
		const legacy = MakeThread("thread_13913946054463488", "2026-07-27T00:00:00.000Z");
		const current = MakeThread("13913946054463489", "2026-07-27T00:00:01.000Z");
		const workspace_id = "project / one";

		expect(ThreadRouteId(legacy.thread_id)).toBe("13913946054463488");
		expect(ThreadRoutePath(workspace_id, legacy.thread_id)).toBe(
			"/t/project%20%2F%20one/13913946054463488",
		);
		expect(ThreadRoutePath(workspace_id, current.thread_id)).toBe(
			"/t/project%20%2F%20one/13913946054463489",
		);
		expect(ThreadRoutePath(undefined, current.thread_id)).toBe("/t/_/13913946054463489");
		expect(Option.getOrThrow(ResolveThreadRoute([legacy], "13913946054463488"))).toBe(legacy);
		expect(Option.getOrThrow(ResolveThreadRoute([current], current.thread_id))).toBe(current);
	});

	it("rejects a route workspace that disagrees with the authoritative thread project", () => {
		const thread = MakeThread("thread_1", "2026-07-27T00:00:00.000Z", "project_1");

		expect(ThreadRouteHasWorkspace(thread, "project_1")).toBe(true);
		expect(ThreadRouteHasWorkspace(thread, "project_2")).toBe(false);
	});

	it("keeps the authoritative stream sorted and replaces an upsert by id", () => {
		const older = MakeThread("thread-older", "2026-07-25T10:00:00.000Z");
		const newer = MakeThread("thread-newer", "2026-07-25T11:00:00.000Z");
		const updated = MakeThread("thread-older", "2026-07-25T12:00:00.000Z");

		const snapshot = ApplyRootThreadListUpdate([], {
			journal_sequence: 1,
			threads: [older, newer],
			type: "snapshot",
		});
		expect(snapshot.map((thread) => thread.thread_id)).toEqual([
			"thread-newer",
			"thread-older",
		]);

		const result = ApplyRootThreadListUpdate(snapshot, {
			journal_sequence: 2,
			thread: updated,
			type: "upsert",
		});
		expect(result.map((thread) => thread.thread_id)).toEqual(["thread-older", "thread-newer"]);
	});

	it("groups recent threads once by their primary project and keeps unassigned threads last", () => {
		const groups = ProjectScopedThreadGroups([
			MakeThread("unassigned-new", "2026-07-25T14:00:00.000Z"),
			MakeThread("project-one-old", "2026-07-25T10:00:00.000Z", "project-one"),
			MakeThread("project-two-new", "2026-07-25T13:00:00.000Z", "project-two"),
			MakeThread("unassigned-old", "2026-07-25T11:00:00.000Z"),
			MakeThread("project-one-new", "2026-07-25T12:00:00.000Z", "project-one"),
		]);

		expect(groups.map((group) => group.type)).toEqual(["project", "project", "unassigned"]);
		expect(groups.map((group) => group.threads.map((thread) => thread.thread_id))).toEqual([
			["project-two-new"],
			["project-one-new", "project-one-old"],
			["unassigned-new", "unassigned-old"],
		]);
		expect(
			groups
				.filter((group) => group.type === "project")
				.map((group) => group.project.project_id),
		).toEqual(["project-two", "project-one"]);
	});

	it("does not duplicate a thread into linked project groups", () => {
		const thread = {
			...MakeThread("thread-primary", "2026-07-25T12:00:00.000Z", "project-primary"),
			linked_projects: [
				{
					display_name: "project-linked",
					project_id: "project-linked",
					root_path: "C:\\projects\\project-linked",
				},
			],
		};

		const groups = ProjectScopedThreadGroups([thread]);

		expect(groups).toHaveLength(1);
		expect(groups[0]).toMatchObject({
			project: { project_id: "project-primary" },
			threads: [{ thread_id: "thread-primary" }],
			type: "project",
		});
	});

	it("omits the unassigned group when every thread has a primary project", () => {
		const groups = ProjectScopedThreadGroups([
			MakeThread("thread-one", "2026-07-25T10:00:00.000Z", "project-one"),
		]);

		expect(groups.map((group) => group.type)).toEqual(["project"]);
	});

	it("uses compact relative timestamps", () => {
		const now = Date.parse("2026-07-25T12:00:00.000Z");
		expect(FormatRecentThreadTime("2026-07-25T11:58:00.000Z", now)).toBe("2 min ago");
		expect(FormatRecentThreadTime("2026-07-24T12:00:00.000Z", now)).toBe("Yesterday");
	});
});

describe("thread route ownership", () => {
	const conversation_owner = {
		route_id: "/t/[workspace]/[thread]",
		thread_route_id: "111",
	};

	it("owns the target when surface and thread both match", () => {
		expect(
			ThreadRouteOwnsTarget(conversation_owner, {
				route_id: "/t/[workspace]/[thread]",
				thread_param: "111",
			}),
		).toBe(true);
	});

	it("rejects another thread on the same surface", () => {
		expect(
			ThreadRouteOwnsTarget(conversation_owner, {
				route_id: "/t/[workspace]/[thread]",
				thread_param: "222",
			}),
		).toBe(false);
	});

	it("rejects the other surface of the same thread", () => {
		expect(
			ThreadRouteOwnsTarget(conversation_owner, {
				route_id: "/e/[workspace]/[thread]",
				thread_param: "111",
			}),
		).toBe(false);
	});

	it("rejects a navigation heading to a route without a thread", () => {
		expect(
			ThreadRouteOwnsTarget(conversation_owner, {
				route_id: "/settings",
				thread_param: undefined,
			}),
		).toBe(false);
	});
});
