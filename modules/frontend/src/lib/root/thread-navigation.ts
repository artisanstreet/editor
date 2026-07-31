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

/** Builds a canonical URL directly from the authoritative thread-list projection. */
export const ThreadRoutePathFor = (thread: Pick<ThreadListItem, "primary_project" | "thread_id">) =>
	ThreadRoutePath(thread.primary_project?.project_id, thread.thread_id);

/** Rejects a route workspace that disagrees with the thread's authoritative project. */
export const ThreadRouteHasWorkspace = (
	thread: Pick<ThreadListItem, "primary_project">,
	route_workspace_id: string,
) => ThreadWorkspaceRouteId(thread.primary_project?.project_id) === route_workspace_id;

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
