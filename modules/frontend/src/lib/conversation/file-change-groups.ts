import type { ConversationItem } from "@artisan/protocol";

type FileChange = Extract<ConversationItem, { type: "file_change" }>;

const is_windows_path = (path: string): boolean =>
	/^[A-Za-z]:[\\/]/.test(path) || path.includes("\\");

/** Produces the presentation identity used to collapse repeated file changes. */
export const canonical_file_change_path = (path: string): string => {
	if (!is_windows_path(path)) return path;

	return path
		.replaceAll("\\", "/")
		.replace(/\/{2,}/g, "/")
		.toLowerCase();
};

const merge_file_changes = (entries: ReadonlyArray<FileChange>): FileChange => {
	const first = entries[0];
	const latest = entries.at(-1);
	if (first === undefined || latest === undefined) {
		throw new Error("Cannot merge an empty file-change group");
	}

	const diff = entries.every((entry) => entry.diff.kind === "known")
		? {
				additions: entries.reduce(
					(total, entry) =>
						total + (entry.diff.kind === "known" ? entry.diff.additions : 0),
					0,
				),
				deletions: entries.reduce(
					(total, entry) =>
						total + (entry.diff.kind === "known" ? entry.diff.deletions : 0),
					0,
				),
				kind: "known" as const,
			}
		: { kind: "unavailable" as const };

	const operation =
		latest.operation === "modified" && entries.some((entry) => entry.operation === "created")
			? "created"
			: latest.operation;

	return { ...latest, diff, id: first.id, operation };
};

/** Groups duplicate file-change rows while preserving their first-seen order. */
export const group_file_changes = (files: ReadonlyArray<FileChange>): ReadonlyArray<FileChange> => {
	const groups = new Map<string, Array<FileChange>>();

	for (const file of files) {
		const key = canonical_file_change_path(file.path);
		const entries = groups.get(key);
		if (entries === undefined) groups.set(key, [file]);
		else entries.push(file);
	}

	return [...groups.values()].map(merge_file_changes);
};
