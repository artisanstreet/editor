/** The lines one applied patch added and removed, counted from the patch itself. */
export interface PatchLineCounts {
	readonly lines_added: number;
	readonly lines_deleted: number;
}

const count_marked_lines = (lines: Iterable<string>): PatchLineCounts => {
	let lines_added = 0;
	let lines_deleted = 0;

	for (const line of lines) {
		if (line.startsWith("+")) lines_added += 1;
		else if (line.startsWith("-")) lines_deleted += 1;
	}

	return { lines_added, lines_deleted };
};

/**
 * Counts the lines a unified diff adds and removes.
 *
 * Attribution is why this exists rather than a working-tree diff: a patch belongs
 * to the run that applied it, so its counts stay correct when several engines
 * touch one workspace, and they need no repository to be read from.
 *
 * Only body lines count. A file header carries the same marker characters as
 * content, so it is recognised by the space that always follows its marker and
 * only outside a hunk — inside one the first character is the marker, which is
 * what lets a removed line that itself reads `---` count instead of vanishing.
 */
export const CountUnifiedDiffLines = (diff: string): PatchLineCounts => {
	const body: Array<string> = [];
	let in_hunk = false;

	for (const line of diff.split("\n")) {
		if (line.startsWith("@@")) {
			in_hunk = true;
			continue;
		}
		if (
			!in_hunk &&
			(line.startsWith("--- ") ||
				line.startsWith("+++ ") ||
				line.startsWith("diff ") ||
				line.startsWith("index "))
		) {
			continue;
		}
		body.push(line);
	}

	return count_marked_lines(body);
};

/**
 * Counts the lines of hunks already split into marked lines, as an engine that
 * reports a structured patch gives them. Every line here is body, so the first
 * character is always the marker and never a header.
 */
export const CountPatchHunkLines = (hunks: Iterable<{ readonly lines: ReadonlyArray<string> }>) =>
	count_marked_lines(
		(function* () {
			for (const hunk of hunks) yield* hunk.lines;
		})(),
	);

/**
 * Counts a whole-file write from the text on each side of it. A file that did
 * not exist removes nothing, which is why the previous content is optional
 * rather than assumed empty.
 */
export const CountWrittenLines = (written: string, replaced?: string): PatchLineCounts => ({
	lines_added: written.length === 0 ? 0 : written.split("\n").length,
	lines_deleted:
		replaced === undefined || replaced.length === 0 ? 0 : replaced.split("\n").length,
});

/**
 * Whether a payload is a unified diff at all, decided by a hunk header.
 *
 * An engine that reports whole-file content for a created file sends text with
 * no markers, and counting that as a diff yields a confident zero for a file
 * that plainly has lines. The header is the only marker a diff cannot omit —
 * content that happens to begin lines with `+` or `-`, such as a Markdown list,
 * is otherwise indistinguishable from a patch body.
 */
export const IsUnifiedDiff = (text: string): boolean => /^@@ /m.test(text);

/**
 * Counts one reported file change whose payload may be either a unified diff or
 * the file's own content, which is how engines differ for created files.
 *
 * Returns `undefined` when the payload is content but the operation modified an
 * existing file: the text says what the file now holds and nothing about what
 * it replaced, so any count would be invented. Uncounted renders as no numbers
 * at all, which is the honest reading — unlike a zero, which claims the edit
 * changed nothing.
 */
export const CountFileChangeLines = (
	operation: "created" | "deleted" | "modified",
	payload: string,
): PatchLineCounts | undefined => {
	if (IsUnifiedDiff(payload)) return CountUnifiedDiffLines(payload);
	if (operation === "created") return CountWrittenLines(payload);
	if (operation === "deleted") {
		const { lines_added } = CountWrittenLines(payload);
		return { lines_added: 0, lines_deleted: lines_added };
	}
	return undefined;
};
