/** Builds the canonical thread-scoped editor URL, optionally landing on one file. */
export const EditorRoutePath = (workspace_id: string, thread_id: string, file?: string): string =>
	`/e/${encodeURIComponent(workspace_id)}/${encodeURIComponent(ThreadRouteId(thread_id))}${
		file === undefined ? "" : `?file=${encodeURIComponent(file)}`
	}`;

/** Resolves whether an authoritative thread can remain in the editor after a metadata change. */
export const EditorRouteTargetForThread = (
	thread: Pick<ThreadListItem, "primary_project" | "thread_id">,
	file?: string,
) =>
	thread.primary_project === undefined
		? {
				path: ThreadRoutePathFor(thread),
				type: "thread" as const,
			}
		: {
				path: EditorRoutePath(thread.primary_project.project_id, thread.thread_id, file),
				type: "editor" as const,
				workspace_id: thread.primary_project.project_id,
			};
import type { ThreadListItem } from "@artisan/protocol";
import { ThreadRouteId, ThreadRoutePathFor } from "../root/thread-navigation";
