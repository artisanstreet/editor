import { Option } from "effect";

/** Selects absolute HTTP(S) destinations that Forge may resolve as rich links. */
export const rich_link_metadata_url = (href: string | undefined): Option.Option<string> => {
	if (href === undefined) return Option.none();

	try {
		const url = new URL(href);
		return url.protocol === "https:" || url.protocol === "http:"
			? Option.some(url.href)
			: Option.none();
	} catch {
		return Option.none();
	}
};
