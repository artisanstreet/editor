import { readFileSync } from "node:fs";

import { Effect, Exit, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	BootstrapBrowserPairing,
	BrowserNavigation,
	BrowserPairingExchange,
} from "../../modules/frontend/src/lib/runtime/pairing";

const location = (hash: string) => ({
	hash,
	pathname: "/threads/thread_1",
	search: "?view=full",
});

const RunPairing = (
	hash: string,
	paired: boolean,
	requests: Array<{ readonly code: string }>,
	replacements: string[],
) =>
	BootstrapBrowserPairing.pipe(
		Effect.provide(
			Layer.merge(
				Layer.succeed(
					BrowserNavigation,
					BrowserNavigation.of({
						Location: Effect.succeed(location(hash)),
						ReplaceUrl: (url) =>
							Effect.sync(() => {
								replacements.push(url);
							}),
					}),
				),
				Layer.succeed(
					BrowserPairingExchange,
					BrowserPairingExchange.of({
						Pair: (request) =>
							Effect.sync(() => {
								requests.push(request);
								return paired;
							}),
					}),
				),
			),
		),
	);

describe("browser pairing bootstrap", () => {
	it("proxies the same-origin pairing exchange in browser development", () => {
		const config = readFileSync(
			new URL("../../modules/frontend/vite.config.ts", import.meta.url),
			"utf8",
		);
		/**
		 * The whole `/api` surface proxies to the development Forge, so the
		 * pairing exchange (`/api/pair`) stays same-origin on the HMR page
		 * exactly like it does on the built bundle.
		 */
		expect(config).toContain('"/api": {');
		expect(config).toContain("target: ForgeDevelopmentOrigin");
	});

	it("exchanges an exact fragment capability and removes it after successful pairing", async () => {
		const requests: Array<{ readonly code: string }> = [];
		const replacements: string[] = [];

		await Effect.runPromise(
			RunPairing("#pair=one%20time%2Fcode", true, requests, replacements),
		);

		expect(requests).toEqual([{ code: "one time/code" }]);
		expect(replacements).toEqual(["/threads/thread_1?view=full"]);
	});

	it("does not exchange or replace when there is no fragment", async () => {
		const requests: Array<{ readonly code: string }> = [];
		const replacements: string[] = [];

		await Effect.runPromise(RunPairing("", true, requests, replacements));

		expect(requests).toEqual([]);
		expect(replacements).toEqual([]);
	});

	it("ignores malformed fragments", async () => {
		const requests: Array<{ readonly code: string }> = [];

		await Effect.runPromise(RunPairing("#pair=secret&next=untrusted", true, requests, []));

		expect(requests).toEqual([]);
	});

	it("reports a failed exchange without exposing the capability", async () => {
		const requests: Array<{ readonly code: string }> = [];
		const replacements: string[] = [];
		const result = await Effect.runPromiseExit(
			RunPairing("#pair=secret", false, requests, replacements),
		);

		expect(Exit.isFailure(result)).toBe(true);
		expect(JSON.stringify(result)).not.toContain("secret");
		expect(replacements).toEqual([]);
	});
});
