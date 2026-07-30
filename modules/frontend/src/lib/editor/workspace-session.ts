import type { WorkspaceFileDiscoveryEntry, WorkspaceFileReadQueryResult } from "@artisan/protocol";

import type { EditorSaveOutcome, EditorWorkspaceFile } from "./service";

/**
 * Translation between the workspace protocol and the editor's own vocabulary.
 *
 * The editor addresses files by an opaque id and an opaque revision; the
 * workspace addresses them by path and content identity. Keeping the mapping
 * here — rather than inside the service or a component — is what lets the
 * service stay transport-free and lets these rules be tested without a client.
 */

/** A workspace path is unique within its workspace, so it is the file's identity. */
export const EditorFileFromRead = (result: WorkspaceFileReadQueryResult): EditorWorkspaceFile => ({
	content: result.content,
	id: result.path,
	/** The adapter re-derives the grammar from the path; the read carries no language. */
	language: "plaintext",
	path: result.path,
	revision: result.identity.content_hash,
	workspace_id: result.workspace_id,
});

/**
 * A save is only meaningful against the revision the editor last saw, so the
 * caller must hand back the identity it read. A hash that no longer matches is
 * a conflict, not an error: the agent may have written the file mid-edit.
 */
export const EditorSaveOutcomeFor = (input: {
	readonly expected_revision: string;
	readonly file: EditorWorkspaceFile;
	readonly observed_revision: string;
}): EditorSaveOutcome =>
	input.observed_revision !== input.expected_revision
		? {
				_tag: "Conflict",
				current_revision: input.observed_revision,
				file: input.file,
			}
		: { _tag: "Saved", file: input.file };

/** One entry in a directory listing, as the sidebar tree renders it. */
export interface WorkspaceTreeEntry {
	readonly kind: "directory" | "file";
	readonly name: string;
	readonly path: string;
}

/** The root of the tree is keyed by the empty string; every other key is a directory path. */
export const workspace_tree_root = "";

/** Directories first, then case-insensitive name order — the usual reading order. */
const by_reading_order = (left: WorkspaceTreeEntry, right: WorkspaceTreeEntry) =>
	left.kind === right.kind
		? left.name.localeCompare(right.name, undefined, { sensitivity: "base" })
		: left.kind === "directory"
			? -1
			: 1;

/**
 * Groups one discovery page by its parent directory.
 *
 * The tree loads a level at a time, so a page is always the children of one
 * directory rather than a whole subtree. Grouping by parent — instead of
 * building a nested tree — is what lets a later page for a deeper directory
 * merge in without rebuilding anything above it.
 */
export const WorkspaceEntriesByParent = (
	entries: ReadonlyArray<WorkspaceFileDiscoveryEntry>,
): ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>> => {
	const grouped = new Map<string, Array<WorkspaceTreeEntry>>();

	for (const entry of entries) {
		const segments = entry.path.split("/");
		const name = segments.at(-1) ?? entry.path;
		const parent =
			segments.length === 1 ? workspace_tree_root : segments.slice(0, -1).join("/");
		const siblings = grouped.get(parent) ?? [];
		siblings.push({ kind: entry.kind, name, path: entry.path });
		grouped.set(parent, siblings);
	}

	return new Map(
		[...grouped].map(([parent, siblings]) => [parent, siblings.toSorted(by_reading_order)]),
	);
};

/**
 * Folds a freshly loaded directory listing into what the tree already holds.
 *
 * Only the loaded directory and its descendants are taken. A prefixed
 * discovery page also contains the directory itself — it matches its own
 * prefix — which lands under the *parent's* key; adopting that would replace
 * the whole level above with a single entry and wipe every sibling off screen.
 *
 * A reload of the same directory replaces its children rather than appending,
 * so a renamed or deleted file does not linger.
 */
export const MergeWorkspaceEntries = (
	current: ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>>,
	incoming: ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>>,
	loaded_parent: string,
): ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>> => {
	const merged = new Map(current);
	const owns = (parent: string) =>
		loaded_parent === workspace_tree_root ||
		parent === loaded_parent ||
		parent.startsWith(`${loaded_parent}/`);

	/** An empty directory still counts as loaded, so record it before merging. */
	merged.set(loaded_parent, incoming.get(loaded_parent) ?? []);
	for (const [parent, siblings] of incoming)
		if (parent !== loaded_parent && owns(parent)) merged.set(parent, siblings);

	return merged;
};
