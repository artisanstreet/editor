/**
 * An installed release always serves the `default` profile, so any other name
 * means this renderer is talking to a development Forge pointed at a separate
 * data root. That single unauthenticated `/health` fact drives every piece of
 * development-instance visibility: the shell badge and the document-title
 * marker. Production installs report `default` and stay unmarked.
 */
export const release_forge_profile = "default";

export const dev_title_marker = "[Dev]";

/** Extracts the profile from a `/health` body when it names a non-release Forge. */
export const DevInstanceProfile = (health: unknown): string | undefined => {
	if (typeof health !== "object" || health === null || !("profile" in health)) return undefined;
	const profile = (health as { readonly profile?: unknown }).profile;
	if (typeof profile !== "string" || profile.length === 0) return undefined;

	return profile === release_forge_profile ? undefined : profile;
};

/** Idempotently prefixes a document title so route-owned titles keep the marker. */
export const DevMarkedTitle = (title: string): string =>
	title.startsWith(dev_title_marker) ? title : `${dev_title_marker} ${title}`.trimEnd();
