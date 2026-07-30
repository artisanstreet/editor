import type { WorkspaceFileDiscoveryEntry } from "@artisan/protocol";
import { describe, expect, it } from "vitest";

import {
	EditorFileFromRead,
	EditorSaveOutcomeFor,
	MergeWorkspaceEntries,
	WorkspaceEntriesByParent,
	workspace_tree_root,
} from "../../modules/frontend/src/lib/editor/workspace-session";
import { EditorWorkspaceId } from "../../modules/frontend/src/lib/editor/workspace-identity";

const entry = (
	path: string,
	kind: WorkspaceFileDiscoveryEntry["kind"] = "file",
): WorkspaceFileDiscoveryEntry => ({
	kind,
	modified_at: "2026-07-29T00:00:00.000Z",
	path,
	size: 0,
});

describe("editor workspace session", () => {
	it("adopts the workspace path as the file identity and the content hash as its revision", () => {
		const file = EditorFileFromRead({
			content: "export const a = 1;\n",
			identity: { algorithm: "sha256", byte_count: 20, content_hash: "a".repeat(64) },
			path: "src/a.ts",
			workspace_id: "workspace_1",
		});

		expect(file.id).toBe("src/a.ts");
		expect(file.path).toBe("src/a.ts");
		expect(file.revision).toBe("a".repeat(64));
		expect(file.workspace_id).toBe("workspace_1");
	});

	/**
	 * An agent writing the file mid-edit must surface as a conflict rather than
	 * a silent overwrite, which is the whole reason saves carry a revision.
	 */
	it("treats a moved revision as a conflict rather than a save", () => {
		const file = {
			content: "next",
			id: "src/a.ts",
			language: "typescript",
			path: "src/a.ts",
			revision: "a".repeat(64),
			workspace_id: "workspace_1",
		};

		expect(
			EditorSaveOutcomeFor({
				expected_revision: "a".repeat(64),
				file,
				observed_revision: "a".repeat(64),
			}),
		).toEqual({ _tag: "Saved", file });
		expect(
			EditorSaveOutcomeFor({
				expected_revision: "a".repeat(64),
				file,
				observed_revision: "b".repeat(64),
			}),
		).toEqual({ _tag: "Conflict", current_revision: "b".repeat(64), file });
	});

	it("groups one directory listing by its parent", () => {
		const grouped = WorkspaceEntriesByParent([entry("src", "directory"), entry("README.md")]);

		expect(grouped.get(workspace_tree_root)?.map((child) => child.name)).toEqual([
			"src",
			"README.md",
		]);
	});

	it("keys a nested listing by its own directory", () => {
		const grouped = WorkspaceEntriesByParent([
			entry("src/lib/service.ts"),
			entry("src/lib/theme.ts"),
		]);

		expect(grouped.get("src/lib")?.map((child) => child.name)).toEqual([
			"service.ts",
			"theme.ts",
		]);
		expect(grouped.has(workspace_tree_root)).toBe(false);
	});

	it("orders directories before files and ignores case", () => {
		const grouped = WorkspaceEntriesByParent([
			entry("zebra.ts"),
			entry("Alpha.ts"),
			entry("src", "directory"),
			entry("docs", "directory"),
		]);

		expect(grouped.get(workspace_tree_root)?.map((child) => child.name)).toEqual([
			"docs",
			"src",
			"Alpha.ts",
			"zebra.ts",
		]);
	});

	/** Expanding a folder must not disturb the levels already on screen. */
	it("merges a newly loaded directory into the existing listing", () => {
		const root = WorkspaceEntriesByParent([entry("src", "directory")]);
		const merged = MergeWorkspaceEntries(
			root,
			WorkspaceEntriesByParent([entry("src/service.ts")]),
			"src",
		);

		expect(merged.get(workspace_tree_root)?.map((child) => child.name)).toEqual(["src"]);
		expect(merged.get("src")?.map((child) => child.name)).toEqual(["service.ts"]);
	});

	it("records an empty directory as loaded so it stops asking", () => {
		const merged = MergeWorkspaceEntries(new Map(), new Map(), "src/empty");

		expect(merged.has("src/empty")).toBe(true);
		expect(merged.get("src/empty")).toEqual([]);
	});

	/** A reload replaces the listing so a deleted file does not linger. */
	it("replaces a reloaded directory rather than appending to it", () => {
		const first = WorkspaceEntriesByParent([entry("src/old.ts"), entry("src/keep.ts")]);
		const merged = MergeWorkspaceEntries(
			first,
			WorkspaceEntriesByParent([entry("src/keep.ts")]),
			"src",
		);

		expect(merged.get("src")?.map((child) => child.name)).toEqual(["keep.ts"]);
	});

	/**
	 * The URL is the only source of a workspace: without `?workspace=` there is
	 * no fallback to "some attached project", so a route outside any workspace
	 * never opens the editor into one by accident.
	 */
	it("resolves the workspace from the URL and nothing else", () => {
		expect(EditorWorkspaceId(new URL("https://forge/editor?workspace=other"))).toBe("other");
		expect(EditorWorkspaceId(new URL("https://forge/editor"))).toBeUndefined();
	});
});
