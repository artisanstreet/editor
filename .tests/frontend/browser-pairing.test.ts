import { readFileSync } from "node:fs";

import { Effect, Exit, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	BootstrapBrowserPairing,
	BrowserNavigation,
	BrowserPairingExchange,
	MakeBrowserNavigationLive,
} from "../../modules/frontend/src/lib/runtime/pairing";

const location = (hash: string, protocol = "http:") => ({
	hash,
	pathname: "/threads/thread_1",
	protocol,
	search: "?view=full",
});

const RunPairing = (
	hash: string,
	paired: boolean,
	requests: Array<{ readonly code: string }>,
	replacements: string[],
	protocol = "http:",
) =>
	BootstrapBrowserPairing.pipe(
		Effect.provide(
			Layer.merge(
				Layer.succeed(
					BrowserNavigation,
					BrowserNavigation.of({
						Location: Effect.succeed(location(hash, protocol)),
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
	it("adapts an explicitly typed browser host without global casts", async () => {
		const replacements: string[] = [];
		const program = Effect.gen(function* () {
			const navigation = yield* BrowserNavigation;
			expect(yield* navigation.Location).toEqual(location("#pair=secret"));
			yield* navigation.ReplaceUrl("/threads/1");
		}).pipe(
			Effect.provide(
				MakeBrowserNavigationLive({
					history: {
						replaceState: (_data, _unused, url) => {
							replacements.push(url);
						},
					},
					location: location("#pair=secret"),
				}),
			),
		);

		await Effect.runPromise(program);
		expect(replacements).toEqual(["/threads/1"]);
	});

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

	it("self-pairs same-origin in development without any terminal round-trip", () => {
		const config = readFileSync(
			new URL("../../modules/frontend/vite.config.ts", import.meta.url),
			"utf8",
		);
		const layout = readFileSync(
			new URL("../../modules/frontend/src/routes/+layout.sv", import.meta.url),
			"utf8",
		);

		/** The dev server mints codes with the bearer secret; pages only receive them. */
		expect(config).toContain('server.middlewares.use("/api/dev/pair-code"');
		expect(config).toContain("/api/pair/request");
		expect(config).toContain("ARTISAN_DEV_AUTH_TOKEN");
		expect(layout).toContain("if (import.meta.env.DEV) {");
		expect(layout).toContain("yield* AttemptDevelopmentSelfPair;");
	});

	it("reports self-pairing as unavailable outside development", async () => {
		const original_fetch = globalThis.fetch;
		globalThis.fetch = (() =>
			Promise.resolve({ ok: false } as Response)) as typeof globalThis.fetch;
		try {
			const { AttemptDevelopmentSelfPair } =
				await import("../../modules/frontend/src/lib/runtime/pairing");
			expect(await Effect.runPromise(AttemptDevelopmentSelfPair)).toBe(false);
		} finally {
			globalThis.fetch = original_fetch;
		}
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

	it("accepts the editor's forge endpoint fragment only on a non-HTTP(S) page", async () => {
		const app_requests: Array<{ readonly code: string }> = [];
		const app_replacements: string[] = [];
		await Effect.runPromise(
			RunPairing(
				"#pair=editor%20code&forge=http%3A%2F%2F127.0.0.1%3A4949",
				true,
				app_requests,
				app_replacements,
				"artisan:",
			),
		);
		expect(app_requests).toEqual([{ code: "editor code" }]);
		expect(app_replacements).toEqual(["/threads/thread_1?view=full"]);

		/**
		 * A crafted link to a Forge-served HTTP page must not be able to point
		 * the client — and its host-scoped loopback cookies — at a sibling port.
		 */
		const web_requests: Array<{ readonly code: string }> = [];
		await Effect.runPromise(
			RunPairing("#pair=secret&forge=http%3A%2F%2F127.0.0.1%3A4949", true, web_requests, []),
		);
		expect(web_requests).toEqual([]);
	});

	it("rejects a non-loopback forge endpoint even on the app scheme", async () => {
		const requests: Array<{ readonly code: string }> = [];
		await Effect.runPromise(
			RunPairing(
				"#pair=secret&forge=http%3A%2F%2Fattacker.example%3A80",
				true,
				requests,
				[],
				"artisan:",
			),
		);
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
