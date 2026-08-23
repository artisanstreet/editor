import { describe, expect, it } from "vitest";
import { Option } from "effect";

import type { ThreadListItem } from "@artisan/protocol";
import {
	ApplyRootThreadListUpdate,
	FormatRecentThreadTime,
	PinnedThreads,
	ProjectScopedThreadGroups,
	ResolveThreadRoute,
	SettledThreads,
	ThreadNeedsAttention,
	ThreadSettled,
	ThreadRailTimeGroups,
	ThreadUnread,
	ThreadRouteId,
	ThreadRouteHasWorkspace,
	ThreadRouteOwnsTarget,
	ThreadRoutePath,
	WorkingThreads,
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
	last_message_at: last_activity_at,
	reader_activity_at: last_activity_at,
	reader_acknowledged_activity_at: undefined,
	live_status: "Idle",
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

	it("preserves the given position when non-message activity updates a thread", () => {
		const first = MakeThread("thread-first", "2026-07-25T12:00:00.000Z");
		const second = MakeThread("thread-second", "2026-07-25T11:00:00.000Z");
		const background_update = {
			...second,
			last_activity_at: "2026-07-25T13:00:00.000Z",
			updated_at: "2026-07-25T13:00:00.000Z",
		};

		const result = ApplyRootThreadListUpdate([first, second], {
			journal_sequence: 2,
			thread: background_update,
			type: "upsert",
		});

		expect(result.map((thread) => thread.thread_id)).toEqual(["thread-first", "thread-second"]);
		expect(result[1]).toBe(background_update);
	});

	it("orders snapshots by the last sent message rather than background activity", () => {
		const background_newer = {
			...MakeThread("thread-background", "2026-07-25T13:00:00.000Z"),
			last_message_at: "2026-07-25T10:00:00.000Z",
		};
		const message_newer = MakeThread("thread-message", "2026-07-25T12:00:00.000Z");

		const result = ApplyRootThreadListUpdate([], {
			journal_sequence: 1,
			threads: [background_newer, message_newer],
			type: "snapshot",
		});

		expect(result.map((thread) => thread.thread_id)).toEqual([
			"thread-message",
			"thread-background",
		]);
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

	it("sorts settled rows newest-first into the requested elapsed-time groups", () => {
		const now = Date.parse("2026-08-10T12:00:00.000Z");
		const groups = ThreadRailTimeGroups(
			[
				MakeThread("older", "2026-07-01T12:00:00.000Z"),
				MakeThread("seven-days-minus-ms", "2026-08-03T12:00:00.001Z"),
				MakeThread("seven-days", "2026-08-03T12:00:00.000Z"),
				MakeThread("three-days", "2026-08-07T12:00:00.000Z"),
				MakeThread("two-days", "2026-08-08T12:00:00.000Z"),
				MakeThread("one-day", "2026-08-09T12:00:00.000Z"),
				MakeThread("today-newer", "2026-08-10T11:00:00.000Z"),
				MakeThread("today-older", "2026-08-10T10:00:00.000Z"),
			],
			now,
		);

		expect(
			groups.map((group) => [group.label, group.threads.map((thread) => thread.thread_id)]),
		).toEqual([
			[undefined, ["today-newer", "today-older"]],
			["Yesterday", ["one-day"]],
			["Last 3 days", ["two-days"]],
			["Last 7 days", ["three-days", "seven-days-minus-ms"]],
			["Past month", ["seven-days", "older"]],
		]);
	});

	it("keeps non-resting threads in the pinned rail freshest first", () => {
		const threads = [
			{ ...MakeThread("resting", "2026-07-25T14:00:00.000Z"), live_status: "Idle" },
			{ ...MakeThread("working-old", "2026-07-25T12:00:00.000Z"), live_status: "Thinking" },
			{ ...MakeThread("complete", "2026-07-25T15:00:00.000Z"), live_status: "Complete" },
			{ ...MakeThread("working-new", "2026-07-25T13:00:00.000Z"), live_status: "Streaming" },
			{
				...MakeThread("waiting", "2026-07-25T16:00:00.000Z"),
				live_status: "Waiting for your reply",
			},
			{
				...MakeThread("attention", "2026-07-25T17:00:00.000Z"),
				live_status: "Needs attention",
			},
		];

		expect(WorkingThreads(threads).map((thread) => thread.thread_id)).toEqual([
			"attention",
			"waiting",
			"working-new",
			"working-old",
		]);
	});

	it("does not interpret assistant preview prose as lifecycle state", () => {
		for (const last_assistant_message of [
			"Complete",
			"Waiting for answer",
			"Failed to complete",
		]) {
			const thread = {
				...MakeThread(last_assistant_message, "2026-07-25T18:00:00.000Z"),
				last_assistant_message,
				live_status: "Working",
			};

			expect(WorkingThreads([thread])).toEqual([thread]);
		}
	});

	it("keeps an active thread pinned even when Forge records an acknowledgement", () => {
		const working = {
			...MakeThread("working", "2026-07-25T12:00:00.000Z"),
			live_status: "Streaming",
		};
		const idle = MakeThread("idle", "2026-07-25T11:00:00.000Z");
		const threads = [working, idle];
		expect(ThreadUnread(working)).toBe(true);
		expect(ThreadNeedsAttention(working)).toBe(false);
		expect(ThreadUnread(idle)).toBe(true);
		expect(ThreadNeedsAttention(idle)).toBe(false);

		expect(PinnedThreads(threads).map((thread) => thread.thread_id)).toEqual(["working"]);

		const acknowledged = {
			...working,
			reader_acknowledged_activity_at: working.reader_activity_at,
		};
		expect(PinnedThreads([acknowledged, idle]).map((thread) => thread.thread_id)).toEqual([
			"working",
		]);
		/** A live run never lands in the settled list despite its durable acknowledgement. */
		expect(SettledThreads([acknowledged, idle]).map((thread) => thread.thread_id)).toEqual([
			"idle",
		]);

		/** New activity is a new reason to look, so the dismissal expires with it. */
		const spoke_again = [
			{
				...acknowledged,
				last_activity_at: "2026-07-25T13:00:00.000Z",
				reader_activity_at: "2026-07-25T13:00:00.000Z",
			},
			idle,
		];
		expect(PinnedThreads(spoke_again).map((thread) => thread.thread_id)).toEqual(["working"]);
	});

	it("keeps unattended completion and failure pinned until Forge records them as read", () => {
		const complete = {
			...MakeThread("complete", "2026-07-25T12:00:00.000Z"),
			live_status: "Complete",
		};
		const failed = {
			...MakeThread("failed", "2026-07-25T13:00:00.000Z"),
			live_status: "Failed to complete",
		};

		expect(PinnedThreads([complete, failed]).map((thread) => thread.thread_id)).toEqual([
			"failed",
			"complete",
		]);
		expect(ThreadNeedsAttention(complete)).toBe(true);
		expect(ThreadNeedsAttention(failed)).toBe(true);

		const read = [complete, failed].map((thread) => ({
			...thread,
			reader_acknowledged_activity_at: thread.reader_activity_at,
		}));
		expect(read.every(ThreadSettled)).toBe(true);
		expect(PinnedThreads(read)).toEqual([]);
		expect(SettledThreads(read).map((thread) => thread.thread_id)).toEqual([
			"failed",
			"complete",
		]);

		/** A newer root-visible activity makes the thread unread again. */
		const replied = {
			...read[0]!,
			last_activity_at: "2026-07-25T14:00:00.000Z",
			reader_activity_at: "2026-07-25T14:00:00.000Z",
		};
		expect(PinnedThreads([replied])).toEqual([replied]);
	});

	it("does not move the root read cursor for hidden worker-only activity", () => {
		const root_activity_at = "2026-07-25T12:00:00.000Z";
		const thread = {
			...MakeThread("worker-finished", "2026-07-25T13:00:00.000Z"),
			live_status: "Working",
			reader_activity_at: root_activity_at,
		};
		const read = { ...thread, reader_acknowledged_activity_at: root_activity_at };

		/** Hidden worker recency cannot expire the root-visible acknowledgement. */
		expect(ThreadSettled({ ...read, live_status: "Complete" })).toBe(true);
		expect(SettledThreads([{ ...read, live_status: "Complete" }])).toEqual([
			{ ...read, live_status: "Complete" },
		]);

		/** The aggregate/root lifecycle does advance it and therefore needs a new read. */
		const root_completed = {
			...thread,
			live_status: "Complete",
			reader_activity_at: thread.last_activity_at,
		};
		expect(PinnedThreads([root_completed])).toEqual([root_completed]);
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
