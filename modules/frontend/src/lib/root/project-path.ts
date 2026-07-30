/** `C:\Users\sander`, `/home/sander`, `/Users/sander` — the conventional home roots. */
const HOME_ROOT = /^(?:[A-Za-z]:)?[\\/](?:Users|home)[\\/][^\\/]+(?=[\\/]|$)/;

/**
 * Shortens a project's root path for display: the home directory collapses to
 * `~`, separators normalize to `/`, and a trailing segment that merely repeats
 * the project's own name is dropped — the card already shows it in bold above.
 *
 * `C:\Users\sander\Desktop\artisan-editor` → `~/Desktop`
 *
 * @param root_path - The absolute path the backend reports for the project.
 * @param display_name - The project's name, shown separately from the path.
 * @returns A compact path, or `undefined` when nothing meaningful is left.
 */
export const ShortProjectPath = (root_path: string, display_name?: string): string | undefined => {
	const shortened = root_path.replace(HOME_ROOT, "~").replaceAll("\\", "/").replace(/\/+$/, "");
	const segments = shortened.split("/");

	if (display_name !== undefined && segments.length > 1 && segments.at(-1) === display_name) {
		segments.pop();
	}

	const trimmed = segments.join("/");
	return trimmed === "" || trimmed === "~" ? undefined : trimmed;
};
