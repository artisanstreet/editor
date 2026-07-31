/**
 * Which workspace the editor route names, and nothing else. A Forge project is
 * currently the workspace filesystem, so the project id is the workspace id.
 */
export const EditorWorkspaceId = (route_workspace_id: string | undefined) =>
	route_workspace_id === undefined || route_workspace_id.length === 0
		? undefined
		: route_workspace_id;

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

/** Preserves pre-thread editor deep links while all product navigation uses `EditorRoutePath`. */
export const LegacyEditorRoutePath = (workspace_id: string, file?: string): string =>
	`/editor?workspace=${encodeURIComponent(workspace_id)}${
		file === undefined ? "" : `&file=${encodeURIComponent(file)}`
	}`;
import type { ThreadListItem } from "@artisan/protocol";
import { ThreadRouteId, ThreadRoutePathFor } from "../root/thread-navigation";
