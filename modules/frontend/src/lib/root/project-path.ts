import {
	FormatPathSeparators,
	PathSeparatorCharacter,
	type PathSeparator,
} from "../appearance/display-format";

type PathDialect = "posix" | "windows";

interface ProjectPathContext {
	readonly dialect: PathDialect;
	readonly display_name: string | undefined;
	readonly suffix: string | undefined;
	anchor: string;
	segments: Array<string>;
}

type ProjectPathRule = (path: ProjectPathContext) => void;

const HOME_SEGMENTS = new Set(["home", "users"]);
const MAXIMUM_CONTEXT_SEGMENTS = 3;

const SeparatorFor = (dialect: PathDialect) => (dialect === "windows" ? "\\" : "/");

/** Collapses only conventional absolute home roots; a nested `home` directory is left alone. */
const CollapseHome: ProjectPathRule = (path) => {
	const first = path.segments[0]?.toLowerCase();
	if (path.anchor === "/") {
		if (!HOME_SEGMENTS.has(first ?? "") || path.segments.length < 2) return;
	} else if (!/^[A-Za-z]:$/u.test(path.anchor)) {
		return;
	} else if (first !== "users" || path.segments.length < 2) {
		return;
	}

	path.anchor = "~";
	path.segments.splice(0, 2);
};

/** The project name is already the primary row, so an identical final segment adds no context. */
const RemoveRepeatedProjectName: ProjectPathRule = (path) => {
	const final_segment = path.segments.at(-1);
	if (final_segment === undefined || path.display_name === undefined) return;
	const repeated =
		path.dialect === "windows"
			? final_segment.localeCompare(path.display_name, undefined, {
					sensitivity: "accent",
				}) === 0
			: final_segment === path.display_name;
	if (repeated) path.segments.pop();
};

/** Keeps broad and immediate context while eliding low-value middle directories in deep roots. */
const CompactDeepMiddle: ProjectPathRule = (path) => {
	if (path.segments.length <= MAXIMUM_CONTEXT_SEGMENTS) return;
	path.segments = [path.segments[0]!, "…", path.segments.at(-1)!];
};

const PROJECT_PATH_RULES: ReadonlyArray<ProjectPathRule> = [
	CollapseHome,
	RemoveRepeatedProjectName,
	CompactDeepMiddle,
];

const ParsePath = (
	root_path: string,
	display_name: string | undefined,
): ProjectPathContext | undefined => {
	const source = root_path.trim();
	if (source.length === 0) return undefined;
	const dialect: PathDialect =
		source.includes("\\") || /^[A-Za-z]:[\\/]/u.test(source) ? "windows" : "posix";
	const normalized = source.replaceAll("\\", "/").replace(/\/+$/u, "");

	const wsl = /^\/\/wsl(?:\$|\.localhost)\/([^/]+)(?:\/(.*))?$/iu.exec(normalized);
	if (wsl !== null) {
		return {
			anchor: "/",
			dialect: "windows",
			display_name,
			segments: (wsl[2] ?? "").split("/").filter(Boolean),
			suffix: `${wsl[1]!} (WSL)`,
		};
	}

	const unc = /^\/\/([^/]+)\/([^/]+)(?:\/(.*))?$/u.exec(normalized);
	if (unc !== null) {
		return {
			anchor: `\\\\${unc[1]!}\\${unc[2]!}`,
			dialect: "windows",
			display_name,
			segments: (unc[3] ?? "").split("/").filter(Boolean),
			suffix: undefined,
		};
	}

	const drive = /^([A-Za-z]:)(?:\/(.*))?$/u.exec(normalized);
	if (drive !== null) {
		return {
			anchor: drive[1]!,
			dialect: "windows",
			display_name,
			segments: (drive[2] ?? "").split("/").filter(Boolean),
			suffix: undefined,
		};
	}

	return {
		anchor: normalized.startsWith("/") ? "/" : "",
		dialect,
		display_name,
		segments: normalized.split("/").filter(Boolean),
		suffix: undefined,
	};
};

const RenderPath = (path: ProjectPathContext, preference?: PathSeparator): string => {
	const separator =
		preference === undefined ? SeparatorFor(path.dialect) : PathSeparatorCharacter(preference);
	let rendered: string;
	if (path.anchor === "~") {
		rendered =
			path.segments.length === 0
				? `~${separator}`
				: `~${separator}${path.segments.join(separator)}`;
	} else if (path.anchor === "/") {
		rendered = path.segments.length === 0 ? "/" : `/${path.segments.join(separator)}`;
	} else if (path.anchor.length > 0) {
		rendered =
			path.segments.length === 0
				? `${path.anchor}${separator}`
				: `${path.anchor}${separator}${path.segments.join(separator)}`;
	} else {
		rendered = path.segments.join(separator);
	}

	const formatted =
		preference === undefined ? rendered : FormatPathSeparators(rendered, preference);
	return path.suffix === undefined ? formatted : `${formatted} · ${path.suffix}`;
};

/**
 * Produces a compact, platform-native location for a project picker row.
 * Rules are deliberately ordered: establish a home anchor, remove the name the
 * row already shows, then shorten only genuinely deep remaining context.
 *
 * `C:\Users\sander\Desktop\artisan-editor` → `~\Desktop`
 */
export const ShortProjectPath = (
	root_path: string,
	display_name?: string,
	separator?: PathSeparator,
): string | undefined => {
	const path = ParsePath(root_path, display_name);
	if (path === undefined) return undefined;
	for (const rule of PROJECT_PATH_RULES) rule(path);
	const rendered = RenderPath(path, separator);
	return rendered.length === 0 ? undefined : rendered;
};
