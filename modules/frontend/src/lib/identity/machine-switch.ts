import { Effect } from "effect";

import { desktop_handoff_navigation_parameter } from "@artisan/protocol";

import { RunBrowserDom } from "../browser/dom";

/**
 * Remembers the machine a document belonged to before it switched away, so
 * the Machine select on the destination Forge can offer the way back. Session
 * storage survives the switch navigation (same tab, same partition) and dies
 * with the tab, matching the lifetime of the adopted endpoint itself.
 */
const home_host_key = "artisan.home-host";

export type HomeHostMemory = {
	readonly detail?: string;
	readonly label: string;
};

/** Denied storage degrades to the return row simply being absent. */
export const RememberHomeHost = (memory: HomeHostMemory) =>
	RunBrowserDom(() => {
		globalThis.sessionStorage?.setItem(home_host_key, JSON.stringify(memory));
	}).pipe(Effect.ignore);

export const RecallHomeHost: Effect.Effect<HomeHostMemory | undefined> = RunBrowserDom(() => {
	const raw = globalThis.sessionStorage?.getItem(home_host_key);
	if (raw === null || raw === undefined) return undefined;
	const parsed: unknown = JSON.parse(raw);
	if (typeof parsed !== "object" || parsed === null) return undefined;
	const label = (parsed as Record<string, unknown>)["label"];
	const detail = (parsed as Record<string, unknown>)["detail"];
	if (typeof label !== "string" || label.length === 0) return undefined;
	return typeof detail === "string" && detail.length > 0
		? ({ detail, label } satisfies HomeHostMemory)
		: ({ label } satisfies HomeHostMemory);
}).pipe(
	Effect.match({
		onFailure: () => undefined,
		onSuccess: (memory) => memory,
	}),
);

/**
 * Builds the navigation that moves this document onto another Forge.
 *
 * On the desktop app scheme the switch mirrors the shell's own handoff URL:
 * a fresh `artisan-handoff` marker forces a document navigation, and the
 * fragment carries the pair code plus the endpoint for adoption — exactly the
 * grammar `BootstrapBrowserPairing` already accepts. On http(s) pages endpoint
 * adoption is refused by design, so the switch instead navigates to the peer
 * Forge's own origin, where pairing is same-origin and needs no `forge=`.
 */
export function build_machine_switch_url(
	page_protocol: string,
	page_origin: string,
	endpoint: string,
	pair_code: string,
	nonce: string,
): string {
	if (page_protocol === "http:" || page_protocol === "https:") {
		const origin = endpoint.endsWith("/") ? endpoint.slice(0, -1) : endpoint;
		return `${origin}/#pair=${encodeURIComponent(pair_code)}`;
	}

	const fragment = `#pair=${encodeURIComponent(pair_code)}&forge=${encodeURIComponent(endpoint)}`;
	return `${page_origin}/?${desktop_handoff_navigation_parameter}=${nonce}${fragment}`;
}
