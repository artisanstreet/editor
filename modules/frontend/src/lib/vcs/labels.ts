/**
 * Reduces a repository web URL to the repository's own name. The owner is
 * dropped: the row is already scoped to one project, so the name alone is what
 * distinguishes it, and it survives truncation in a narrow panel.
 *
 * @param web_url - An https page derived from a remote.
 * @returns The repository name, or the host when the URL carries no path.
 */
export const RepositoryLinkLabel = (web_url: string): string => {
	try {
		const parsed = new URL(web_url);
		const segments = parsed.pathname.split("/").filter((segment) => segment !== "");

		return segments.at(-1) ?? parsed.hostname;
	} catch {
		return web_url;
	}
};

/**
 * Reduces a repository web URL to `owner/repo`. Unlike {@link RepositoryLinkLabel}
 * this keeps the owner: a workspace header names the repository absolutely, not
 * within a card already scoped to one project. Nested-group hosts keep only the
 * innermost group, which is the part that still distinguishes the repository.
 *
 * @param web_url - An https page derived from a remote.
 * @returns `owner/repo`, the bare name, or the host when the URL carries no path.
 */
export const RepositoryQualifiedLabel = (web_url: string): string => {
	try {
		const parsed = new URL(web_url);
		const segments = parsed.pathname.split("/").filter((segment) => segment !== "");
		const name = segments.at(-1);
		if (name === undefined) return parsed.hostname;
		const owner = segments.at(-2);

		return owner === undefined ? name : `${owner}/${name}`;
	} catch {
		return web_url;
	}
};

/**
 * Names a repository link without its transport scheme, preserving the host so
 * a destination button says exactly where it will open.
 *
 * @param web_url - An https page derived from a remote.
 * @returns `host/owner/repo`, or the raw value when it is not a URL.
 */
export const RepositoryDestinationLabel = (web_url: string): string => {
	try {
		const parsed = new URL(web_url);
		const pathname = parsed.pathname.replace(/\/$/u, "");

		return `${parsed.hostname}${pathname}`;
	} catch {
		return web_url;
	}
};
