import type { RepositoryBranchComparison, RepositoryDiffSnapshot } from "@artisan/protocol";

/**
 * Presents one project's diff reading.
 *
 * Separate from `presentation.ts`, which pulls in host logo components: these
 * are pure functions over protocol data, and keeping them free of Svelte
 * imports is what lets them be tested directly.
 *
 * @since 0.8.0
 */

/** Groups digits so a five-figure line count stays readable at 12px. */
export const DiffCount = (value: number): string => value.toLocaleString("en");

/** Pluralizes a file tally, which appears in every row of the diff detail. */
export const DiffFileCount = (value: number): string =>
	`${DiffCount(value)} ${value === 1 ? "file" : "files"}`;

/** Names what a comparison's ref is to the checked-out branch. */
export const ComparisonLabel = (kind: RepositoryBranchComparison["kind"]): string =>
	kind === "upstream" ? "upstream" : "default branch";

/**
 * Decides whether a diff has anything to report.
 *
 * Every row the detail can render is covered here. Gating the lip on line
 * counts alone would hide a working tree whose only change is a replaced
 * binary or a pure rename — states `git status` calls dirty and this would
 * call clean — while a row exists to describe them.
 */
export const HasReportableWork = (diff: RepositoryDiffSnapshot): boolean =>
	diff.working.file_count > 0 ||
	diff.untracked_file_count > 0 ||
	diff.stash_count > 0 ||
	diff.comparisons.length > 0 ||
	diff.truncated;
