import {
	SvglGitHubLogo,
	SvglGitLabLogo,
	SvglGitLogo,
	SvglMicrosoftAzureLogo,
} from "@selemondev/svgl-svelte";
import type { Component } from "svelte";
import type { RepositoryHost } from "@artisan/protocol";
import BitbucketMark from "./marks/bitbucket.svelte";
import CodebergMark from "./marks/codeberg.svelte";
import GiteaMark from "./marks/gitea.svelte";
import SourcehutMark from "./marks/sourcehut.svelte";

/** Presents one repository host's mark. @since 0.8.0 */
export interface RepositoryMark {
	readonly icon: Component;
	/** Marks a single-color logo that must invert with the theme. */
	readonly monochrome: boolean;
	/**
	 * The host's brand color, painting the chip behind an always-white mark.
	 * GitLab deliberately keeps the recognizable orange over the deeper brand
	 * red; its 2.86:1 white-on-orange is the one sub-3:1 chip in the row.
	 */
	readonly background: string;
}

/**
 * A host with no logo of its own falls back to the plain Git mark rather than
 * a wrong one — the same mark a local-only repository gets. Its chip wears
 * git's own red, matching the installed svgl asset's fill.
 */
const plain_git: RepositoryMark = {
	icon: SvglGitLogo,
	monochrome: false,
	background: "#DE4C36",
};

/**
 * Hosts svgl does not carry (Bitbucket, Codeberg, Sourcehut, Gitea) use marks
 * vendored from Simple Icons under `./marks`. Backgrounds are each host's
 * primary brand hex from the same curated set.
 */
const repository_marks: Readonly<Record<RepositoryHost, RepositoryMark>> = {
	azure: { icon: SvglMicrosoftAzureLogo, monochrome: false, background: "#0078D4" },
	bitbucket: { icon: BitbucketMark, monochrome: false, background: "#0052CC" },
	codeberg: { icon: CodebergMark, monochrome: false, background: "#2185D0" },
	gitea: { icon: GiteaMark, monochrome: false, background: "#609926" },
	github: { icon: SvglGitHubLogo, monochrome: true, background: "#181717" },
	gitlab: { icon: SvglGitLabLogo, monochrome: false, background: "#FC6D26" },
	other: plain_git,
	sourcehut: { icon: SourcehutMark, monochrome: false, background: "#000000" },
	unknown: plain_git,
};

/** Resolves the mark for a host, defaulting to the plain Git glyph. */
export const RepositoryMarkFor = (host: RepositoryHost | undefined): RepositoryMark =>
	host === undefined ? plain_git : repository_marks[host];

/** Names the Tailwind classes that size a host mark and keep it theme-correct. */
export const RepositoryMarkClass = (mark: RepositoryMark, size = "size-4") =>
	mark.monochrome ? `${size} shrink-0 dark:invert` : `${size} shrink-0`;

/**
 * Names the classes that render a host mark white on its brand chip in both
 * themes. The brightness floor plus inversion is the only whitening that
 * reaches per-path fills and gradients (GitLab's tri-tone tanuki, Azure's
 * gradient), which a `fill` override cannot touch.
 */
export const RepositoryChipMarkClass = (size = "size-4") => `${size} shrink-0 brightness-0 invert`;
