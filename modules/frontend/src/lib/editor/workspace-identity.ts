/**
 * Which workspace the editor is looking at: the `?workspace=` the URL pins,
 * and nothing else.
 *
 * There is deliberately no fallback to "the first attached project" — the
 * editor is only reachable from a context that already names a workspace (an
 * open thread's project, the draft's chosen project, a deep link), so an URL
 * without one means no workspace is open, not "guess one".
 *
 * A Forge project is a directory the user attached, and the workspace
 * filesystem the protocol reads is that same directory — so the project id is
 * the workspace id. Keeping the assumption in one named function means the day
 * a project owns several workspaces, only this file changes.
 */
export const EditorWorkspaceId = (url: URL): string | undefined =>
	url.searchParams.get("workspace") ?? undefined;

/** Builds the editor URL for a workspace, optionally landing on one file. */
export const EditorRoutePath = (workspace_id: string, file?: string): string =>
	`/editor?workspace=${encodeURIComponent(workspace_id)}${
		file === undefined ? "" : `&file=${encodeURIComponent(file)}`
	}`;
