import { Effect, Layer } from "effect";
import { describe, expect, it } from "@effect/vitest";

import {
	build_machine_switch_url,
	RecallHomeHost,
	RememberHomeHost,
} from "../../modules/frontend/src/lib/identity/machine-switch";
import {
	BootstrapBrowserPairing,
	BrowserNavigation,
	BrowserPairingExchange,
} from "../../modules/frontend/src/lib/runtime/pairing";

const RunPairing = (url: string, protocol: string, requests: Array<{ readonly code: string }>) =>
	BootstrapBrowserPairing.pipe(
		Effect.provide(
			Layer.merge(
				Layer.succeed(
					BrowserNavigation,
					BrowserNavigation.of({
						Location: Effect.succeed({
							hash: new URL(url).hash,
							pathname: "/",
							protocol,
							search: new URL(url).search,
						}),
						ReplaceUrl: () => Effect.void,
					}),
				),
				Layer.succeed(
					BrowserPairingExchange,
					BrowserPairingExchange.of({
						Pair: (request) =>
							Effect.sync(() => {
								requests.push(request);
								return true;
							}),
					}),
				),
			),
		),
	);

describe("machine switch", () => {
	it("builds the desktop handoff-style switch URL with pair and forge fragments", () => {
		const url = build_machine_switch_url(
			"artisan:",
			"artisan://app",
			"http://127.0.0.1:35785/",
			"ABCD-EFGH",
			"machine-switch-7",
		);

		expect(url).toBe(
			"artisan://app/?artisan-handoff=machine-switch-7#pair=ABCD-EFGH&forge=http%3A%2F%2F127.0.0.1%3A35785%2F",
		);
	});

	it("navigates to the peer origin itself on http(s) pages", () => {
		expect(
			build_machine_switch_url(
				"http:",
				"http://127.0.0.1:4848",
				"http://127.0.0.1:35785/",
				"ABCD-EFGH",
				"machine-switch-7",
			),
		).toBe("http://127.0.0.1:35785/#pair=ABCD-EFGH");
		expect(
			build_machine_switch_url(
				"https:",
				"https://localhost:5173",
				"http://127.0.0.1:35785",
				"A B",
				"n",
			),
		).toBe("http://127.0.0.1:35785/#pair=A%20B");
	});

	it.effect("produces URLs the real pairing bootstrap accepts, both variants", () =>
		Effect.gen(function* () {
			const desktop_requests: Array<{ readonly code: string }> = [];
			yield* RunPairing(
				build_machine_switch_url(
					"artisan:",
					"artisan://app",
					"http://127.0.0.1:35785/",
					"one time/code",
					"machine-switch-7",
				),
				"artisan:",
				desktop_requests,
			);
			expect(desktop_requests).toEqual([{ code: "one time/code" }]);

			const browser_requests: Array<{ readonly code: string }> = [];
			yield* RunPairing(
				build_machine_switch_url(
					"http:",
					"http://127.0.0.1:4848",
					"http://127.0.0.1:35785/",
					"one time/code",
					"machine-switch-7",
				),
				"http:",
				browser_requests,
			);
			expect(browser_requests).toEqual([{ code: "one time/code" }]);
		}),
	);

	it.effect("degrades home-host memory to absence without browser storage", () =>
		Effect.gen(function* () {
			yield* RememberHomeHost({ detail: "DESKTOP-1", label: "This computer" });
			expect(yield* RecallHomeHost).toBeUndefined();
		}),
	);
});
