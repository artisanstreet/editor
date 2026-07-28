/**
 * The renderer normally talks to the Forge that served it, strictly
 * same-origin. The installed editor is the one exception: Electron loads this
 * bundle from the `artisan://app` scheme and hands the profile's loopback
 * Forge endpoint over in the pairing fragment. That endpoint is adopted here —
 * validated, session-scoped, never durable — and every Forge-facing URL is
 * built through it so the rest of the client stays origin-agnostic.
 */

const storage_key = "artisan.forge-endpoint";

/**
 * Adoption is restricted to non-HTTP(S) pages. A web page served by a Forge
 * must never be redirectable to a sibling loopback port through a crafted
 * fragment, because loopback cookies are host-scoped rather than port-scoped;
 * the app scheme cannot be loaded by a browser, so gating on the page protocol
 * removes the redirection surface entirely.
 */
const endpoint_bearing_page = (page_protocol: string) =>
	page_protocol !== "http:" && page_protocol !== "https:";

/** Accepts only an uncredentialed loopback HTTP origin with an explicit port. */
export const DecodeLoopbackForgeEndpoint = (candidate: unknown): string | undefined => {
	if (typeof candidate !== "string" || candidate.length === 0 || candidate.length > 256) {
		return undefined;
	}
	try {
		const url = new URL(candidate);
		const loopback =
			url.hostname === "127.0.0.1" || url.hostname === "[::1]" || url.hostname === "::1";
		if (
			url.protocol !== "http:" ||
			!loopback ||
			url.username !== "" ||
			url.password !== "" ||
			url.port === ""
		) {
			return undefined;
		}
		return url.origin;
	} catch {
		return undefined;
	}
};

const session_store = (): Pick<Storage, "getItem" | "removeItem" | "setItem"> | undefined => {
	try {
		return (globalThis as { readonly sessionStorage?: Storage }).sessionStorage;
	} catch {
		/** Storage access can throw in privacy modes; the endpoint just stays unset. */
		return undefined;
	}
};

/**
 * Remembers a validated endpoint for the lifetime of this window session, so
 * an in-window reload of the editor keeps talking to the same Forge after the
 * one-time fragment has been consumed. The endpoint is not a secret and no
 * credential is ever stored beside it.
 */
export const AdoptForgeEndpoint = (candidate: string, page_protocol: string): boolean => {
	if (!endpoint_bearing_page(page_protocol)) return false;
	const endpoint = DecodeLoopbackForgeEndpoint(candidate);
	if (endpoint === undefined) return false;
	session_store()?.setItem(storage_key, endpoint);
	return true;
};

/** The adopted loopback Forge origin, or undefined on ordinary same-origin pages. */
export const ResolveForgeEndpoint = (): string | undefined => {
	const stored = session_store()?.getItem(storage_key);
	return stored === null || stored === undefined
		? undefined
		: DecodeLoopbackForgeEndpoint(stored);
};

/** Builds a Forge HTTP URL: absolute against the adopted endpoint, else same-origin relative. */
export const ForgeHttpUrl = (path: string): string => {
	const endpoint = ResolveForgeEndpoint();
	return endpoint === undefined ? path : `${endpoint}${path}`;
};
