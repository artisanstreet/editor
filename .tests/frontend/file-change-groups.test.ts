import { describe, expect, it } from "vitest";
import type { ConversationItem } from "@artisan/protocol";

import {
	aggregate_file_change_diff,
	canonical_file_change_path,
	display_file_change_path,
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
	it("starts Windows file labels at the project folder with native separators", () => {
		expect(
			display_file_change_path(
				"C:\\Users\\sander\\Desktop\\svelte-effect-runtime\\modules\\runtime.ts",
				"C:\\Users\\sander\\Desktop\\svelte-effect-runtime",
			),
		).toBe("svelte-effect-runtime\\modules\\runtime.ts");
		expect(
			display_file_change_path(
				"c:/users/SANDER/desktop/SVELTE-EFFECT-RUNTIME/package.json",
				"C:\\Users\\sander\\Desktop\\svelte-effect-runtime\\",
			),
		).toBe("svelte-effect-runtime\\package.json");
	});

	it("starts macOS and Linux file labels at the project folder with slash separators", () => {
		expect(
			display_file_change_path(
				"/Users/sander/code/artisan-editor/modules/frontend/package.json",
				"/Users/sander/code/artisan-editor",
			),
		).toBe("artisan-editor/modules/frontend/package.json");
		expect(
			display_file_change_path(
				"/home/sander/code/artisan-editor/README.md",
				"/home/sander/code/artisan-editor/",
			),
		).toBe("artisan-editor/README.md");
	});

	it("honours the selected separator instead of the source platform", () => {
		expect(
			display_file_change_path(
				"C:\\projects\\artisan\\src\\file.ts",
				"C:\\projects\\artisan",
				"forward-slash",
			),
		).toBe("artisan/src/file.ts");
		expect(
			display_file_change_path(
				"/projects/artisan/src/file.ts",
				"/projects/artisan",
				"backslash",
			),
		).toBe("artisan\\src\\file.ts");
	});

	it("prefixes project-relative paths and keeps outside or unknown paths truthful", () => {
		expect(
			display_file_change_path(
				"C:\\projects\\artisan-tools\\readme.md",
				"C:\\projects\\artisan",
			),
		).toBe("C:\\projects\\artisan-tools\\readme.md");
		expect(display_file_change_path("src/file.ts", "C:\\projects\\artisan")).toBe(
			"artisan\\src\\file.ts",
		);
		expect(display_file_change_path("./src/file.ts", "/projects/artisan")).toBe(
			"artisan/src/file.ts",
		);
		expect(display_file_change_path("../other/file.ts", "/projects/artisan")).toBe(
			"../other/file.ts",
		);
		expect(display_file_change_path("src/../readme.md", "/projects/artisan")).toBe(
			"artisan/readme.md",
		);
		expect(display_file_change_path("src/../../outside.ts", "/projects/artisan")).toBe(
			"src/../../outside.ts",
		);
		expect(display_file_change_path("src\\..\\..\\outside.ts", "C:\\projects\\artisan")).toBe(
			"src\\..\\..\\outside.ts",
		);
		expect(
			display_file_change_path(
				"C:\\projects\\artisan\\src\\..\\..\\outside.ts",
				"C:\\projects\\artisan",
			),
		).toBe("C:\\projects\\artisan\\src\\..\\..\\outside.ts");
		expect(
			display_file_change_path("/projects/artisan/src/../../outside.ts", "/projects/artisan"),
		).toBe("/projects/artisan/src/../../outside.ts");
		expect(display_file_change_path("/projects/artisan/readme.md")).toBe(
			"/projects/artisan/readme.md",
		);
	});

	it("uses the project folder itself when the paths are equal", () => {
		expect(display_file_change_path("C:\\projects\\artisan", "c:\\projects\\artisan\\")).toBe(
			"artisan",
		);
		expect(display_file_change_path("/projects/artisan", "/projects/artisan")).toBe("artisan");
	});

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

	it("keeps known duplicate totals as a lower bound when one edit lacks counts", () => {
		const grouped = group_file_changes([
			file_change("file-1", "C:\\Project\\src\\file.ts", "modified", {
				additions: 2,
				deletions: 1,
				kind: "known",
			}),
			file_change("file-2", "c:/project/src/file.ts", "modified", { kind: "unavailable" }),
		]);

		expect(grouped[0]?.diff).toEqual({
			additions: 2,
			deletions: 1,
			kind: "partial",
			unavailable_files: 1,
		});
	});

	it("aggregates grouped file counts only when every visible file has known data", () => {
		const grouped = group_file_changes([
			file_change("file-1", "src/file.ts", "modified", {
				additions: 2,
				deletions: 1,
				kind: "known",
			}),
			file_change("file-2", "src/file.ts", "modified", {
				additions: 3,
				deletions: 4,
				kind: "known",
			}),
			file_change("file-3", "src/readme.md", "modified", {
				additions: 5,
				deletions: 0,
				kind: "known",
			}),
		]);

		expect(aggregate_file_change_diff(grouped)).toEqual({
			additions: 10,
			deletions: 5,
			kind: "known",
		});
	});

	it("keeps known aggregate counts as a lower bound when some files lack diff data", () => {
		expect(aggregate_file_change_diff([])).toEqual({ kind: "unavailable" });
		expect(
			aggregate_file_change_diff([
				file_change("file-1", "src/file.ts", "modified", {
					additions: 2,
					deletions: 1,
					kind: "known",
				}),
				file_change("file-2", "src/readme.md", "modified", { kind: "unavailable" }),
			]),
		).toEqual({
			additions: 2,
			deletions: 1,
			kind: "partial",
			unavailable_files: 1,
		});
	});

	it("keeps the aggregate unavailable when no file has line counts", () => {
		expect(
			aggregate_file_change_diff([
				file_change("file-1", "src/file.ts", "modified", { kind: "unavailable" }),
				file_change("file-2", "src/readme.md", "modified", { kind: "unavailable" }),
			]),
		).toEqual({ kind: "unavailable" });
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
