import type { ConversationItem } from "@artisan/protocol";

type FileChange = Extract<ConversationItem, { type: "file_change" }>;

const is_windows_path = (path: string): boolean =>
	/^[A-Za-z]:[\\/]/.test(path) || path.includes("\\");

const is_absolute_path = (path: string): boolean => /^(?:[A-Za-z]:[\\/]|[\\/])/.test(path);

const without_trailing_separators = (path: string): string => {
	if (path === "/" || /^[A-Za-z]:[\\/]?$/.test(path)) return path;
	return path.replace(/[\\/]+$/, "");
};

const normalized_path = (path: string): string =>
	without_trailing_separators(path).replaceAll("\\", "/");

const contained_relative_path = (path: string): string | undefined => {
	const segments: Array<string> = [];

	for (const segment of path.split("/")) {
		if (segment === "" || segment === ".") continue;
		if (segment === "..") {
			if (segments.length === 0) return undefined;
			segments.pop();
			continue;
		}
		segments.push(segment);
	}

	return segments.length === 0 ? undefined : segments.join("/");
};

/**
 * Presents one changed file from its project folder instead of from the host
 * filesystem root. The project path identifies the Forge host's platform, so
 * paired browsers retain that host's native separator too.
 */
export const display_file_change_path = (path: string, project_root_path?: string): string => {
	if (project_root_path === undefined) return path;

	const windows_path = is_windows_path(project_root_path);
	const separator = windows_path ? "\\" : "/";
	const project_root = normalized_path(project_root_path);
	const file_path = normalized_path(path);
	const comparable_root = windows_path ? project_root.toLowerCase() : project_root;
	const comparable_file = windows_path ? file_path.toLowerCase() : file_path;
	const project_name = project_root.split("/").filter(Boolean).at(-1);

	if (project_name === undefined) return path;
	if (!is_absolute_path(path)) {
		const relative_path = contained_relative_path(file_path);
		if (relative_path === undefined) return path;
		return `${project_name}${separator}${relative_path.replaceAll("/", separator)}`;
	}

	const relative_path_from_root =
		comparable_file === comparable_root
			? ""
			: comparable_file.startsWith(`${comparable_root}/`)
				? file_path.slice(project_root.length + 1)
				: undefined;

	if (relative_path_from_root === undefined) return path;
	if (relative_path_from_root === "") return project_name;

	const relative_path = contained_relative_path(relative_path_from_root);
	return relative_path === undefined
		? path
		: `${project_name}${separator}${relative_path.replaceAll("/", separator)}`;
};

/** Produces an all-or-nothing diff total for the visible file rows. */
export const aggregate_file_change_diff = (
	files: ReadonlyArray<FileChange>,
): FileChange["diff"] => {
	if (files.length === 0 || !files.every((file) => file.diff.kind === "known")) {
		return { kind: "unavailable" };
	}

	return {
		additions: files.reduce(
			(total, file) => total + (file.diff.kind === "known" ? file.diff.additions : 0),
			0,
		),
		deletions: files.reduce(
			(total, file) => total + (file.diff.kind === "known" ? file.diff.deletions : 0),
			0,
		),
		kind: "known",
	};
};

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

	const diff = aggregate_file_change_diff(entries);

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
