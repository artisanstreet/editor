/** `C:\Users\sander`, `/home/sander`, `/Users/sander` — the conventional home roots. */
const HOME_ROOT = /^(?:[A-Za-z]:)?\/(?:Users|home)\/[^/]+(?=\/|$)/iu;

/** Windows' two spellings for a mounted WSL distribution. */
const WSL_UNC_ROOT = /^\/\/wsl(?:\$|\.localhost)\/([^/]+)(\/.*)?$/iu;

const CompactPath = (root_path: string, display_name?: string): string | undefined => {
	const shortened = root_path.replace(HOME_ROOT, "~");
	const segments = shortened.split("/");

	if (display_name !== undefined && segments.length > 1 && segments.at(-1) === display_name) {
		segments.pop();
	}

	const trimmed = segments.join("/");
	return trimmed === "" ? undefined : trimmed === "~" ? "~/" : trimmed;
};

/**
 * Shortens a project's root path for display: the home directory collapses to
 * `~`, separators normalize to `/`, and a trailing segment that merely repeats
 * the project's own name is dropped — the card already shows it in bold above.
 *
 * `C:\Users\sander\Desktop\artisan-editor` → `~/Desktop`
 *
 * @param root_path - The absolute path the backend reports for the project.
 * @param display_name - The project's name, shown separately from the path.
 * @returns A compact path, or `undefined` when the input carries no path.
 */
export const ShortProjectPath = (root_path: string, display_name?: string): string | undefined => {
	const normalized = root_path.replaceAll("\\", "/").replace(/\/+$/, "");
	const wsl = WSL_UNC_ROOT.exec(normalized);
	if (wsl === null) return CompactPath(normalized, display_name);

	const distro = wsl[1]!;
	const compact = CompactPath(wsl[2] ?? "/", display_name);
	return compact === undefined ? `${distro} (WSL)` : `${compact} · ${distro} (WSL)`;
};
