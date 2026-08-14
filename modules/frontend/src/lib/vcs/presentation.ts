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
	 * Classes painting the chip face in the host's brand color. Brand-colored
	 * faces stay fixed across themes; GitLab deliberately keeps the
	 * recognizable orange over the deeper brand red, its 2.86:1 white-on-orange
	 * being the one sub-3:1 chip in the row. Near-black brands (GitHub,
	 * Sourcehut) instead oppose the theme — brand black by day, white by night
	 * — so the chip never sinks into matching chrome.
	 */
	readonly chip: string;
	/**
	 * Filter classes keeping the mark legible on that face. The brightness
	 * floor plus inversion is the only whitening that reaches per-path fills
	 * and gradients (GitLab's tri-tone tanuki, Azure's gradient), which a
	 * `fill` override cannot touch.
	 */
	readonly chip_mark: string;
}

/** Whitens any mark for a fixed brand-colored face. */
const white_mark = "brightness-0 invert";

/** Flips with a theme-opposing face: white mark by day, black by night. */
const opposing_mark = "brightness-0 invert dark:invert-0";

/**
 * A host with no logo of its own falls back to the plain Git mark rather than
 * a wrong one — the same mark a local-only repository gets. Its chip wears
 * git's own red, matching the installed svgl asset's fill.
 */
const plain_git: RepositoryMark = {
	icon: SvglGitLogo,
	monochrome: false,
	chip: "bg-[#DE4C36]",
	chip_mark: white_mark,
};

/**
 * Hosts svgl does not carry (Bitbucket, Codeberg, Sourcehut, Gitea) use marks
 * vendored from Simple Icons under `./marks`. Backgrounds are each host's
 * primary brand hex from the same curated set.
 */
const repository_marks: Readonly<Record<RepositoryHost, RepositoryMark>> = {
	azure: {
		icon: SvglMicrosoftAzureLogo,
		monochrome: false,
		chip: "bg-[#0078D4]",
		chip_mark: white_mark,
	},
	bitbucket: {
		icon: BitbucketMark,
		monochrome: false,
		chip: "bg-[#0052CC]",
		chip_mark: white_mark,
	},
	codeberg: {
		icon: CodebergMark,
		monochrome: false,
		chip: "bg-[#2185D0]",
		chip_mark: white_mark,
	},
	gitea: { icon: GiteaMark, monochrome: false, chip: "bg-[#609926]", chip_mark: white_mark },
	github: {
		icon: SvglGitHubLogo,
		monochrome: true,
		chip: "bg-[#181717] dark:bg-white",
		chip_mark: opposing_mark,
	},
	gitlab: {
		icon: SvglGitLabLogo,
		monochrome: false,
		chip: "bg-[#FC6D26]",
		chip_mark: white_mark,
	},
	other: plain_git,
	sourcehut: {
		icon: SourcehutMark,
		monochrome: false,
		chip: "bg-black dark:bg-white",
		chip_mark: opposing_mark,
	},
	unknown: plain_git,
};

/** Resolves the mark for a host, defaulting to the plain Git glyph. */
export const RepositoryMarkFor = (host: RepositoryHost | undefined): RepositoryMark =>
	host === undefined ? plain_git : repository_marks[host];

/** Names the Tailwind classes that size a host mark and keep it theme-correct. */
export const RepositoryMarkClass = (mark: RepositoryMark, size = "size-4") =>
	mark.monochrome ? `${size} shrink-0 dark:invert` : `${size} shrink-0`;

/** Names the classes that size a host mark and keep it legible on its chip. */
export const RepositoryChipMarkClass = (mark: RepositoryMark, size = "size-4") =>
	`${size} shrink-0 ${mark.chip_mark}`;
