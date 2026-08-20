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

/**
 * The marker a renderer embeds once it has given up reaching the Forge it was
 * paired with.
 *
 * The endpoint arrives exactly once, in the handoff fragment, so a Forge that
 * died and came back on a different port is unreachable to the renderer no
 * matter how long it retries — only the shell can ask `ae` for a new one. This
 * is the renderer asking it to.
 *
 * Two joiners rather than one: the sequence is invisible in a browser tab,
 * which shows its title, and no route-owned title contains even a single
 * joiner, so it cannot be forged by naming a thread. Written as escapes because
 * the literal characters are invisible in an editor.
 */
export const forge_repair_title_marker = "\u2060\u2060";

/** Whether a title is asking the shell to pair the renderer with a live Forge. */
export const TitleRequestsForgeRepair = (title: string): boolean =>
	title.includes(forge_repair_title_marker);
