/**
 * The document-title channel between a renderer and the desktop shell.
 *
 * The shell deliberately exposes no preload and no IPC surface, so the one
 * renderer-owned value the main process can already observe — the document
 * title, via Electron's `page-title-updated` event — carries the count of
 * threads needing reader attention. The same marker doubles as the ordinary
 * web convention in a paired browser tab: `(2) Thread › Artisan Editor`.
 *
 * The digits are followed by U+2060 WORD JOINER, an invisible character no
 * route-owned title ever contains, so a thread legitimately named `(3) fix
 * the build` can never masquerade as a marker: the shell trusts exactly the
 * joiner-terminated form and nothing else.
 */

/** Markers carry at most four digits; a count beyond that is already "many". */
const attention_marker = /\((\d{1,4})\)\u2060/u;

/** The marker a renderer embeds in its document title for `count` threads. */
export const AttentionTitleMarkerFor = (count: number): string =>
	`(${Math.max(0, Math.trunc(count))})\u2060`;

/**
 * The count a title's marker carries, or `undefined` when the title carries
 * no marker. Position-independent on purpose: the renderer may compose the
 * marker with other title prefixes, and the joiner discriminator alone is
 * what makes the match trustworthy.
 */
export const AttentionCountFromTitle = (title: string): number | undefined => {
	const digits = attention_marker.exec(title)?.[1];

	return digits === undefined ? undefined : Number.parseInt(digits, 10);
};
