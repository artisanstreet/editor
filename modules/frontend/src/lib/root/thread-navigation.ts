import type { ProjectRef, ThreadListItem } from "@artisan/protocol";
import type { ThreadListUpdate } from "@artisan/transport/client";
import { Option } from "effect";

const legacy_thread_prefix = "thread_";
const detached_workspace_route_id = "_";

/** Removes the historical domain prefix from the public thread route segment. */
export const ThreadRouteId = (thread_id: string) => {
	const route_id = thread_id.startsWith(legacy_thread_prefix)
		? thread_id.slice(legacy_thread_prefix.length)
		: thread_id;

	return route_id.length > 0 ? route_id : thread_id;
};

/** Maps a missing project to the reserved URL segment used by detached historical threads. */
export const ThreadWorkspaceRouteId = (workspace_id: string | undefined) =>
	workspace_id === undefined ? detached_workspace_route_id : workspace_id;

/** Restores the domain workspace identity represented by one thread-route segment. */
export const ThreadWorkspaceId = (route_workspace_id: string) =>
	route_workspace_id === detached_workspace_route_id ? undefined : route_workspace_id;

/** Builds the canonical workspace-scoped URL for current and historical thread identities. */
export const ThreadRoutePath = (workspace_id: string | undefined, thread_id: string) =>
	`/t/${encodeURIComponent(ThreadWorkspaceRouteId(workspace_id))}/${encodeURIComponent(
		ThreadRouteId(thread_id),
	)}`;

/**
 * The workspace's own route: a project entered, with no thread chosen in it yet.
 *
 * The same `/t/` space as a conversation on purpose — you are in the project
 * either way, and the thread segment is what says which conversation. Its
 * absence is the next one.
 */
export const WorkspaceRoutePath = (workspace_id: string | undefined) =>
	`/t/${encodeURIComponent(ThreadWorkspaceRouteId(workspace_id))}`;

/** Builds a canonical URL directly from the authoritative thread-list projection. */
export const ThreadRoutePathFor = (thread: Pick<ThreadListItem, "primary_project" | "thread_id">) =>
	ThreadRoutePath(thread.primary_project?.project_id, thread.thread_id);

/** Rejects a route workspace that disagrees with the thread's authoritative project. */
export const ThreadRouteHasWorkspace = (
	thread: Pick<ThreadListItem, "primary_project">,
	route_workspace_id: string,
) => ThreadWorkspaceRouteId(thread.primary_project?.project_id) === route_workspace_id;

/**
 * Whether a route instance still owns the navigation target — the rendered
 * page when idle, the navigation in flight otherwise. The async renderer keeps
 * an outgoing route scope alive until its replacement finishes rendering, and
 * background activity keeps running that scope's subscriptions; a navigation
 * issued from it would yank the user back to the thread — or the surface — it
 * belongs to and cancel the navigation they actually asked for. Only the
 * instance whose surface (route id) and thread the target points at may steer
 * the URL.
 */
export const ThreadRouteOwnsTarget = (
	owner: {
		readonly route_id: string | null;
		readonly thread_route_id: string;
	},
	target: {
		readonly route_id: string | null | undefined;
		readonly thread_param: string | undefined;
	},
): boolean => target.route_id === owner.route_id && target.thread_param === owner.thread_route_id;

/**
 * Resolves canonical bare route IDs while retaining access to historical
 * `thread_` records. Exact current identities win if both forms ever coexist.
 */
export const ResolveThreadRoute = (
	threads: ReadonlyArray<ThreadListItem>,
	route_id: string,
): Option.Option<ThreadListItem> =>
	Option.fromUndefinedOr(
		threads.find((thread) => thread.thread_id === route_id) ??
			threads.find((thread) => ThreadRouteId(thread.thread_id) === route_id),
	);

/** Applies the authoritative thread-list stream without introducing UI-only identity. */
export const ApplyRootThreadListUpdate = (
	threads: ReadonlyArray<ThreadListItem>,
	update: ThreadListUpdate,
): ReadonlyArray<ThreadListItem> => {
	if (update.type === "snapshot") return SortRecentThreads(update.threads);
	if (update.type === "remove")
		return threads.filter((thread) => thread.thread_id !== update.thread_id);

	return SortRecentThreads([
		...threads.filter((thread) => thread.thread_id !== update.thread.thread_id),
		update.thread,
	]);
};

/** Sorts durable projections by their backend-owned activity timestamp. */
export const SortRecentThreads = (threads: ReadonlyArray<ThreadListItem>) =>
	[...threads].sort((left, right) => right.last_activity_at.localeCompare(left.last_activity_at));

/** Describes one sidebar project section and its durably scoped recent threads. */
export type ProjectScopedThreadGroup =
	| {
			readonly project: ProjectRef;
			readonly threads: ReadonlyArray<ThreadListItem>;
			readonly type: "project";
	  }
	| {
			readonly threads: ReadonlyArray<ThreadListItem>;
			readonly type: "unassigned";
	  };

/** Groups each thread exactly once by its primary project for sidebar presentation. */
export const ProjectScopedThreadGroups = (
	threads: ReadonlyArray<ThreadListItem>,
): ReadonlyArray<ProjectScopedThreadGroup> => {
	const project_groups = new Map<
		string,
		{
			project: ProjectRef;
			threads: Array<ThreadListItem>;
		}
	>();
	const unassigned_threads: Array<ThreadListItem> = [];

	for (const thread of SortRecentThreads(threads)) {
		const project = thread.primary_project;
		if (project === undefined) {
			unassigned_threads.push(thread);
			continue;
		}

		const group = project_groups.get(project.project_id);
		if (group === undefined) {
			project_groups.set(project.project_id, { project, threads: [thread] });
			continue;
		}

		group.threads.push(thread);
	}

	return [
		...[...project_groups.values()].map((group) => ({
			project: group.project,
			threads: group.threads,
			type: "project" as const,
		})),
		...(unassigned_threads.length === 0
			? []
			: [{ threads: unassigned_threads, type: "unassigned" as const }]),
	];
};

/** Formats an ISO activity timestamp for the compact recent-thread table. */
export const FormatRecentThreadTime = (value: string, now_ms: number) => {
	const elapsed_seconds = Math.max(0, Math.floor((now_ms - Date.parse(value)) / 1_000));
	if (elapsed_seconds < 60) return "Just now";
	if (elapsed_seconds < 3_600) return `${Math.floor(elapsed_seconds / 60)} min ago`;
	if (elapsed_seconds < 86_400) return `${Math.floor(elapsed_seconds / 3_600)} hr ago`;
	if (elapsed_seconds < 172_800) return "Yesterday";
	return `${Math.floor(elapsed_seconds / 86_400)} days ago`;
};

/** Built-in settled statuses that no longer need the always-visible rail group. */
export const RestingThreadStatuses: ReadonlySet<string> = new Set(["Complete", "Idle"]);

/** Waiting and attention states remain pinned because they still require the user. */
export const ThreadIsWorking = (thread: ThreadListItem) =>
	!RestingThreadStatuses.has(thread.live_status);

/**
 * The status a run reports after failing — it means the run errored, not that
 * the thread is waiting on you.
 *
 * Two strings, because it is sticky: `live_status` is only rewritten by the
 * next refinement trigger on that thread, so rows that failed under the old
 * wording keep it until something runs there again.
 */
export const FailedStatus = "Failed to complete";
const LegacyFailedStatus = "Needs attention";

export const IsFailedStatus = (status: string) =>
	status === FailedStatus || status === LegacyFailedStatus;

export const ThreadFailed = (thread: ThreadListItem) => IsFailedStatus(thread.live_status);

/** The status a run reports after finishing cleanly. */
export const CompleteStatus = "Complete";

export const ThreadCompleted = (thread: ThreadListItem) => thread.live_status === CompleteStatus;

/** A running or waiting Forge-owned run cannot be dismissed by local rail state. */
export const ThreadHasActiveWork = (thread: ThreadListItem) =>
	ThreadIsWorking(thread) && !ThreadFailed(thread);

/**
 * The non-resting threads, freshest first. Work, waits, and attention states
 * should not need summoning, so the rail lifts them into its pinned group.
 */
export const WorkingThreads = (threads: ReadonlyArray<ThreadListItem>) =>
	SortRecentThreads(threads).filter(ThreadIsWorking);

export type ThreadRailTimeGroup = {
	readonly id: "today" | "yesterday" | "last_3_days" | "last_7_days" | "past_month";
	readonly label: string | undefined;
	readonly threads: ReadonlyArray<ThreadListItem>;
};

const MillisecondsPerDay = 24 * 60 * 60 * 1_000;

/**
 * Partitions the settled rail chronologically, preserving the authoritative
 * newest-first order inside every section. Past month is the final historical
 * section: records older than thirty days stay reachable there rather than
 * being silently omitted from navigation or gaining an extra visual label.
 */
export const ThreadRailTimeGroups = (
	threads: ReadonlyArray<ThreadListItem>,
	now_ms: number,
): ReadonlyArray<ThreadRailTimeGroup> => {
	const groups: Array<{
		id: ThreadRailTimeGroup["id"];
		label: string | undefined;
		threads: Array<ThreadListItem>;
	}> = [
		{ id: "today", label: undefined, threads: [] },
		{ id: "yesterday", label: "Yesterday", threads: [] },
		{ id: "last_3_days", label: "Last 3 days", threads: [] },
		{ id: "last_7_days", label: "Last 7 days", threads: [] },
		{ id: "past_month", label: "Past month", threads: [] },
	];

	for (const thread of SortRecentThreads(threads)) {
		const elapsed_ms = now_ms - Date.parse(thread.last_activity_at);
		const group_index =
			Number.isFinite(elapsed_ms) && elapsed_ms < MillisecondsPerDay
				? 0
				: elapsed_ms < 2 * MillisecondsPerDay
					? 1
					: elapsed_ms < 3 * MillisecondsPerDay
						? 2
						: elapsed_ms < 7 * MillisecondsPerDay
							? 3
							: 4;

		groups[group_index]?.threads.push(thread);
	}

	return groups.filter((group) => group.threads.length > 0);
};

/**
 * Unread threads first, each group still freshest-first.
 *
 * Partitioned rather than weighted into the sort key: a thread that settles
 * while the list is on screen should cross the whole list to the top, and a
 * comparator that mixed unread-ness with recency would only nudge it. Same
 * shape the model picker uses to float favourites above their engine.
 */
export const SortUnreadFirst = (
	threads: ReadonlyArray<ThreadListItem>,
	unread: ReadonlySet<string>,
) => {
	const ordered = SortRecentThreads(threads);
	const pending = ordered.filter((thread) => unread.has(thread.thread_id));
	return pending.length === 0
		? ordered
		: [...pending, ...ordered.filter((thread) => !unread.has(thread.thread_id))];
};

/** Legacy projections predate the root/worker activity boundary. */
export const ThreadReaderActivityAt = (thread: ThreadListItem): string =>
	thread.reader_activity_at ?? thread.last_activity_at;

/**
 * Forge owns acknowledgement. It is stamped against root-visible activity so
 * a later root response repins the thread, while hidden worker activity does
 * not invalidate a read the reader already made.
 */
export const ThreadSettled = (thread: ThreadListItem) =>
	thread.reader_acknowledged_activity_at === ThreadReaderActivityAt(thread);

/** Durable attention remains unread until Forge records the reader acknowledgement. */
export const ThreadUnread = (thread: ThreadListItem) => !ThreadSettled(thread);

/** Only an inactive unread thread reports an outcome that needs reader attention. */
export const ThreadNeedsAttention = (thread: ThreadListItem) =>
	!ThreadHasActiveWork(thread) &&
	ThreadUnread(thread) &&
	(ThreadCompleted(thread) || ThreadFailed(thread));

/**
 * What the pinned group holds: anything running, plus anything that finished
 * and has not been opened since — minus whatever the reader has settled.
 *
 * A thread does not drop to the list the moment its run ends — that is exactly
 * when the reader has a reason to look at it, and a row that leaves the pinned
 * group on completion is a row that vanishes from where the eye was resting.
 * It stays until it has been read, and anything it says afterwards puts it
 * back by outrunning the stamp that settled it.
 */
export const PinnedThreads = (threads: ReadonlyArray<ThreadListItem>) =>
	SortRecentThreads(threads).filter(
		(thread) =>
			/** A durable acknowledgement never outranks Forge's live run authority. */
			ThreadHasActiveWork(thread) || ThreadNeedsAttention(thread),
	);

/** Everything the pinned group is not already showing. */
export const SettledThreads = (threads: ReadonlyArray<ThreadListItem>) =>
	SortRecentThreads(threads).filter(
		(thread) => !ThreadHasActiveWork(thread) && !ThreadNeedsAttention(thread),
	);

/**
 * The status a thread reports while it is stopped on a question.
 *
 * Matched as a value rather than inferred, so the rail lights up the moment the
 * projection emits it: `ThreadMetadataRefiner` derives `live_status` from run
 * triggers only — started, completed, failed — and has no trigger for a run
 * that paused to ask. Until one exists this is unreachable, and the shape of
 * the fix is a trigger there, not a guess here.
 */
export const AwaitingAnswerStatus = "Waiting for answer";

export const ThreadIsAwaitingAnswer = (thread: ThreadListItem) =>
	thread.live_status === AwaitingAnswerStatus;
