import type { ProseWidth } from "../appearance-config";

/**
 * What the shell can afford to show beside the transcript.
 *
 * The thread inspector and the proximity rail want the same pixels. The rail
 * has nowhere else to go — it *is* the transcript's left margin — so when the
 * window cannot seat both, the inspector is what goes. This module is the one
 * place that arithmetic lives, so the layout and the rail cannot drift apart.
 */

/** The reading column's resolved width, mirroring the `--prose-width` tokens. */
export const ProseColumnPixels: Record<ProseWidth, number> = {
	balanced: 768,
	loose: 896,
	tight: 672,
};

/**
 * The narrowest band that seats a legible thread row — the `@min-[9rem]` gate
 * the rail's own markup opens on.
 */
export const ThreadRailBandPixels = 144;

/** Clear space the band leaves before the column, mirroring `--prose-rail-gap`. */
export const ThreadRailGapPixels = 16;

/** Breathing room right of the column, mirroring `--prose-gutter`. */
export const ProseGutterPixels = 32;

/**
 * The sidebar rail, the main gutter, the gap between the two surfaces, and the
 * primary card's own padding — everything spent before the box the column and
 * the band both measure from.
 */
const ShellChromePixels = 56 + 8 + 8 + 8;

/**
 * The inspector column, mirroring `clamp(16rem, 25vw, 350px)`. It is read from
 * the viewport rather than measured because this decides whether the column is
 * rendered at all, and a measurement of an unrendered column is zero.
 */
export const InspectorColumnPixels = (viewport_width: number) =>
	Math.min(Math.max(256, viewport_width * 0.25), 350);

/**
 * Whether the transcript keeps a margin wide enough for the proximity rail once
 * the inspector has taken its column. The reading column sits right of centre,
 * so the whole surplus beside it is the band — the right side only ever holds
 * the gutter.
 */
export const ThreadInspectorFitsBesideRail = (viewport_width: number, prose_width: ProseWidth) => {
	const transcript_width =
		viewport_width - ShellChromePixels - InspectorColumnPixels(viewport_width);
	const band_width =
		transcript_width - ProseColumnPixels[prose_width] - ProseGutterPixels - ThreadRailGapPixels;
	return band_width >= ThreadRailBandPixels;
};
