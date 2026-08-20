import { AttentionTitleMarkerFor, forge_repair_title_marker } from "@artisan/protocol";

import { dev_title_marker } from "./dev-instance";

/**
 * Only the joiner-terminated form is ours to remove. A route title that
 * happens to begin with plain parenthesised digits — a thread named
 * `(3) fix the build` — carries no U+2060 and is left untouched.
 */
const attention_marker_prefix = /^\(\d{1,4}\)\u2060 /u;

const dev_prefix = `${dev_title_marker} `;

/**
 * Rewrites a document title to carry exactly one attention marker, or none.
 *
 * The marker sits after the development marker when one is present, never
 * before it: `DevMarkedTitle` recognises its own marker only at the very
 * start of the title, and a count planted in front of it would make the two
 * observers rewrite each other's work forever instead of converging.
 */
export const AttentionMarkedTitle = (
	title: string,
	count: number | undefined,
	/**
	 * Rides the same rewrite rather than a second writer, because both markers
	 * live in one string and two observers racing over it would each keep
	 * erasing the other's work.
	 */
	requests_forge_repair = false,
): string => {
	const marked_for_development = title.startsWith(dev_prefix);
	const prefix = marked_for_development ? dev_prefix : "";
	const bare = title
		.slice(prefix.length)
		.replace(attention_marker_prefix, "")
		.replaceAll(forge_repair_title_marker, "");
	const suffix = requests_forge_repair ? forge_repair_title_marker : "";

	return count === undefined
		? `${prefix}${bare}${suffix}`
		: `${prefix}${AttentionTitleMarkerFor(count)} ${bare}${suffix}`;
};
