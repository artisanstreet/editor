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
