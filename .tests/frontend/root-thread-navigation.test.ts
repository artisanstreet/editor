import { describe, expect, it } from "vitest";
import { Option } from "effect";

import type { ThreadListItem } from "@artisan/protocol";
import {
	ApplyRootThreadListUpdate,
	FormatRecentThreadTime,
	NoSettledThreads,
	PinnedThreads,
	ProjectScopedThreadGroups,
	ResolveThreadRoute,
	SettledThreads,
	SettleThread,
	SettleThreadStamp,
	ThreadRailDisplayEntries,
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

	it("keeps an active thread pinned even if a stale local stamp exists", () => {
		const working = {
			...MakeThread("working", "2026-07-25T12:00:00.000Z"),
			live_status: "Streaming",
		};
		const unread = MakeThread("unread", "2026-07-25T11:00:00.000Z");
		const threads = [working, unread];
		const unread_ids = new Set([unread.thread_id]);

		expect(PinnedThreads(threads, unread_ids).map((thread) => thread.thread_id)).toEqual([
			"working",
			"unread",
		]);

		const settled = SettleThread(NoSettledThreads, working);
		expect(settled).toBe(NoSettledThreads);
		expect(
			PinnedThreads(threads, unread_ids, settled).map((thread) => thread.thread_id),
		).toEqual(["working", "unread"]);
		/** A live run never lands in the settled list because the stamp is only local UI state. */
		expect(
			SettledThreads(threads, unread_ids, settled).map((thread) => thread.thread_id),
		).toEqual([]);

		/** New activity is a new reason to look, so the dismissal expires with it. */
		const spoke_again = [{ ...working, last_activity_at: "2026-07-25T13:00:00.000Z" }, unread];
		expect(
			PinnedThreads(spoke_again, unread_ids, settled).map((thread) => thread.thread_id),
		).toEqual(["working", "unread"]);
	});

	it("settles a thread the reader opened until it speaks again after they left", () => {
		const attention = {
			...MakeThread("attention", "2026-07-25T12:00:00.000Z"),
			live_status: "Failed to complete",
		};
		const unread_ids = new Set<string>();

		expect(PinnedThreads([attention], unread_ids).map((thread) => thread.thread_id)).toEqual([
			"attention",
		]);

		/** Reading it is what the pinned group was asking for, so it drops to the list. */
		const read = SettleThreadStamp(
			NoSettledThreads,
			attention.thread_id,
			attention.last_activity_at,
		);
		expect(PinnedThreads([attention], unread_ids, read)).toEqual([]);
		expect(
			SettledThreads([attention], unread_ids, read).map((thread) => thread.thread_id),
		).toEqual(["attention"]);

		/**
		 * The open thread re-settles against every activity it reports, so the
		 * same stamp must not mint a new map and rebuild both rail groups.
		 */
		expect(SettleThreadStamp(read, attention.thread_id, attention.last_activity_at)).toBe(read);

		/** A follow-up needs no special case: the run it triggers pins the thread again. */
		const replied = [{ ...attention, last_activity_at: "2026-07-25T13:00:00.000Z" }];
		expect(PinnedThreads(replied, unread_ids, read).map((thread) => thread.thread_id)).toEqual([
			"attention",
		]);

		/** Watching that activity arrive counts as reading it too. */
		const kept_up = SettleThreadStamp(read, attention.thread_id, "2026-07-25T13:00:00.000Z");
		expect(PinnedThreads(replied, unread_ids, kept_up)).toEqual([]);
	});

	it("bounds rail stress copies without changing navigable source identities", () => {
		const threads = [
			MakeThread("thread-one", "2026-07-25T12:00:00.000Z"),
			MakeThread("thread-two", "2026-07-25T11:00:00.000Z"),
		];

		for (const multiplier of [Number.NaN, Number.NEGATIVE_INFINITY, -3, 0, 0.9]) {
			expect(ThreadRailDisplayEntries(threads, multiplier)).toHaveLength(2);
		}
		expect(ThreadRailDisplayEntries(threads, 99)).toHaveLength(40);

		const entries = ThreadRailDisplayEntries(threads, 3);
		expect(entries.map((entry) => entry.thread.thread_id)).toEqual([
			"thread-one",
			"thread-one",
			"thread-one",
			"thread-two",
			"thread-two",
			"thread-two",
		]);
		expect(new Set(entries.map((entry) => entry.render_id)).size).toBe(entries.length);
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
