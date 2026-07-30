import {
	SvglGitHubLogo,
	SvglGitLabLogo,
	SvglGitLogo,
	SvglMicrosoftAzureLogo,
} from "@selemondev/svgl-svelte";
import type { Component } from "svelte";
import type { RepositoryHost } from "@artisan/protocol";

/** Presents one repository host's mark. @since 0.8.0 */
export interface RepositoryMark {
	readonly icon: Component;
	/** Marks a single-color logo that must invert with the theme. */
	readonly monochrome: boolean;
}

/**
 * A host with no logo of its own falls back to the plain Git mark rather than
 * a wrong one — the same mark a local-only repository gets.
 */
const plain_git: RepositoryMark = { icon: SvglGitLogo, monochrome: false };

const repository_marks: Readonly<Record<RepositoryHost, RepositoryMark>> = {
	azure: { icon: SvglMicrosoftAzureLogo, monochrome: false },
	bitbucket: plain_git,
	codeberg: plain_git,
	gitea: plain_git,
	github: { icon: SvglGitHubLogo, monochrome: true },
	gitlab: { icon: SvglGitLabLogo, monochrome: false },
	other: plain_git,
	sourcehut: plain_git,
	unknown: plain_git,
};

/** Resolves the mark for a host, defaulting to the plain Git glyph. */
export const RepositoryMarkFor = (host: RepositoryHost | undefined): RepositoryMark =>
	host === undefined ? plain_git : repository_marks[host];

/** Names the Tailwind classes that size a host mark and keep it theme-correct. */
export const RepositoryMarkClass = (mark: RepositoryMark, size = "size-4") =>
	mark.monochrome ? `${size} shrink-0 dark:invert` : `${size} shrink-0`;

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
