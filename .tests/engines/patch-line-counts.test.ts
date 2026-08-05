import { describe, expect, it } from "vitest";

import {
	CountPatchHunkLines,
	CountUnifiedDiffLines,
	CountWrittenLines,
} from "../../modules/engines/src/patch/unified-diff";

describe("applied patch line counts", () => {
	it("counts body lines and ignores the headers that share their markers", () => {
		const diff = [
			"--- a/README.md",
			"+++ b/README.md",
			"@@ -1,3 +1,5 @@",
			" unchanged context",
			"-removed line",
			"+first added line",
			"+second added line",
			"+third added line",
		].join("\n");

		expect(CountUnifiedDiffLines(diff)).toEqual({ lines_added: 3, lines_deleted: 1 });
	});

	it("counts a whole new file as additions with nothing removed", () => {
		const diff = [
			"--- /dev/null",
			"+++ b/README.md",
			"@@ -0,0 +1,5 @@",
			"+# Artisan test",
			"+",
			"+1. First prompt",
			"+2. Second prompt",
			"+3. Third prompt",
		].join("\n");

		expect(CountUnifiedDiffLines(diff)).toEqual({ lines_added: 5, lines_deleted: 0 });
	});

	it("does not count the no-newline marker as a removal", () => {
		const diff = ["@@ -1 +1 @@", "-old", "\\ No newline at end of file", "+new"].join("\n");

		expect(CountUnifiedDiffLines(diff)).toEqual({ lines_added: 1, lines_deleted: 1 });
	});

	it("reports nothing for a patch that changes nothing", () => {
		expect(CountUnifiedDiffLines("")).toEqual({ lines_added: 0, lines_deleted: 0 });
	});

	it("counts a body line that reads like a file header", () => {
		const diff = ["--- a/notes.md", "+++ b/notes.md", "@@ -1,2 +1,2 @@", "----", "+***"].join(
			"\n",
		);

		expect(CountUnifiedDiffLines(diff)).toEqual({ lines_added: 1, lines_deleted: 1 });
	});

	it("counts every hunk of a structured patch", () => {
		expect(
			CountPatchHunkLines([
				{ lines: [" context", "-gone", "+here", "+also here"] },
				{ lines: ["-removed", " context"] },
			]),
		).toEqual({ lines_added: 2, lines_deleted: 2 });
	});

	it("counts a created file as additions with nothing replaced", () => {
		expect(CountWrittenLines("a\nb\nc")).toEqual({ lines_added: 3, lines_deleted: 0 });
	});

	it("counts an overwrite against the text it replaced", () => {
		expect(CountWrittenLines("a\nb\nc", "one\ntwo")).toEqual({
			lines_added: 3,
			lines_deleted: 2,
		});
	});
});
