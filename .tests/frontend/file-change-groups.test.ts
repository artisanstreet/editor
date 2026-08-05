import { describe, expect, it } from "vitest";
import type { ConversationItem } from "@artisan/protocol";

import {
	canonical_file_change_path,
	group_file_changes,
} from "../../modules/frontend/src/lib/conversation/file-change-groups";

type FileChange = Extract<ConversationItem, { type: "file_change" }>;

const file_change = (
	id: string,
	path: string,
	operation: FileChange["operation"],
	diff: FileChange["diff"],
): FileChange => ({
	change_set_id: "change-set-1",
	created_at: "2026-08-03T10:00:00.000Z",
	diff,
	id,
	lifecycle: "completed",
	operation,
	ordinal: Number(id.slice(-1)),
	path,
	references: [],
	revision: 0,
	source_refs: [],
	turn_id: "turn-1",
	type: "file_change",
	updated_at: "2026-08-03T10:00:00.000Z",
});

describe("file-change presentation grouping", () => {
	it("canonicalizes Windows paths case-insensitively and normalizes separators", () => {
		expect(
			canonical_file_change_path(
				"C:\\Users\\Sander\\Desktop\\a-test\\src\\routes\\+page.svelte",
			),
		).toBe("c:/users/sander/desktop/a-test/src/routes/+page.svelte");
		expect(
			canonical_file_change_path("c:/users/sander/desktop/a-test/src/routes/+page.svelte"),
		).toBe("c:/users/sander/desktop/a-test/src/routes/+page.svelte");
		expect(canonical_file_change_path("/Users/Sander/File.ts")).toBe("/Users/Sander/File.ts");
	});

	it("preserves first-seen order, merges known counts, and keeps created through modifications", () => {
		const grouped = group_file_changes([
			file_change("file-1", "C:\\Project\\src\\+page.svelte", "created", {
				additions: 2,
				deletions: 0,
				kind: "known",
			}),
			file_change("file-2", "c:/project/src/+PAGE.SVELTE", "modified", {
				additions: 3,
				deletions: 1,
				kind: "known",
			}),
			file_change("file-3", "/project/README.md", "modified", {
				additions: 1,
				deletions: 2,
				kind: "known",
			}),
		]);

		expect(grouped).toHaveLength(2);
		expect(grouped[0]).toMatchObject({
			id: "file-1",
			operation: "created",
			diff: { additions: 5, deletions: 1, kind: "known" },
		});
		expect(grouped[1]).toMatchObject({ id: "file-3", path: "/project/README.md" });
	});

	it("does not fabricate totals when any duplicate has unavailable counts", () => {
		const grouped = group_file_changes([
			file_change("file-1", "C:\\Project\\src\\file.ts", "modified", {
				additions: 2,
				deletions: 1,
				kind: "known",
			}),
			file_change("file-2", "c:/project/src/file.ts", "modified", { kind: "unavailable" }),
		]);

		expect(grouped[0]?.diff).toEqual({ kind: "unavailable" });
	});

	it("uses the latest meaningful operation when the group does not end in a modification", () => {
		const grouped = group_file_changes([
			file_change("file-1", "src/file.ts", "modified", {
				additions: 1,
				deletions: 0,
				kind: "known",
			}),
			file_change("file-2", "src/file.ts", "deleted", {
				additions: 0,
				deletions: 4,
				kind: "known",
			}),
		]);

		expect(grouped[0]?.operation).toBe("deleted");
	});
});
