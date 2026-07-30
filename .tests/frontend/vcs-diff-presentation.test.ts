import { describe, expect, it } from "vitest";

import type { RepositoryDiffSnapshot } from "@artisan/protocol";

import {
	ComparisonLabel,
	DiffCount,
	DiffFileCount,
	HasReportableWork,
} from "../../modules/frontend/src/lib/vcs/diff-presentation";

const no_counts = {
	binary_file_count: 0,
	file_count: 0,
	lines_added: 0,
	lines_deleted: 0,
};

const clean: RepositoryDiffSnapshot = {
	comparisons: [],
	staged: no_counts,
	state: "repository",
	stash_count: 0,
	truncated: false,
	unstaged: no_counts,
	untracked_file_count: 0,
	working: no_counts,
};

describe("reportable work", () => {
	it("reports nothing for a clean repository level with its baseline", () => {
		expect(HasReportableWork(clean)).toBe(false);
	});

	it("reports measured lines", () => {
		expect(
			HasReportableWork({
				...clean,
				working: { ...no_counts, file_count: 1, lines_added: 4 },
			}),
		).toBe(true);
	});

	/**
	 * The three states a line-count predicate misses. Each has a row in the
	 * detail, so gating on lines alone would hide work the tooltip describes.
	 */
	it("reports work that carries no line counts", () => {
		expect(
			HasReportableWork({
				...clean,
				working: { ...no_counts, binary_file_count: 1, file_count: 1 },
			}),
		).toBe(true);
		expect(HasReportableWork({ ...clean, untracked_file_count: 2 })).toBe(true);
		expect(HasReportableWork({ ...clean, stash_count: 1 })).toBe(true);
	});

	it("reports a truncated reading even when its counts are zero", () => {
		expect(HasReportableWork({ ...clean, truncated: true })).toBe(true);
	});

	it("reports a branch comparison", () => {
		expect(
			HasReportableWork({
				...clean,
				comparisons: [
					{ ahead: 2, behind: 0, counts: no_counts, kind: "upstream", ref: "origin/x" },
				],
			}),
		).toBe(true);
	});
});

describe("diff labels", () => {
	it("groups digits so a five-figure count stays readable", () => {
		expect(DiffCount(1_204)).toBe("1,204");
	});

	it("pluralizes a file tally", () => {
		expect(DiffFileCount(1)).toBe("1 file");
		expect(DiffFileCount(0)).toBe("0 files");
		expect(DiffFileCount(12)).toBe("12 files");
	});

	it("names what each comparison ref is to the branch", () => {
		expect(ComparisonLabel("upstream")).toBe("upstream");
		expect(ComparisonLabel("default_branch")).toBe("default branch");
	});
});
