import { describe, expect, it } from "vitest";
import { Effect, Layer, SubscriptionRef } from "effect";
import { TestClock } from "effect/testing";
import { ArtisanClient } from "@artisan/transport/client";

import { AttentionReconnectLive } from "../../modules/frontend/src/lib/runtime/attention-reconnect";
import { BrowserReaderAttention } from "../../modules/frontend/src/lib/browser/reader-attention";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

/** Replays an attention script against the layer and counts retry releases. */
const Observe = (
	initially_watching: boolean,
	attention_script: ReadonlyArray<boolean>,
): Promise<number> => {
	let retries = 0;

	return Effect.runPromise(
		Effect.gen(function* () {
			const attention = yield* SubscriptionRef.make(initially_watching);
			const client_layer = Layer.succeed(ArtisanClient, {
				...FixtureArtisanClientService,
				RetryConnection: Effect.sync(() => {
					retries += 1;
				}),
			});
			const attention_layer = Layer.succeed(BrowserReaderAttention, {
				Changes: SubscriptionRef.changes(attention),
				Current: SubscriptionRef.get(attention),
			});
			const reconnect_layer = AttentionReconnectLive.pipe(
				Layer.provide(Layer.merge(client_layer, attention_layer)),
			);

			yield* Effect.scoped(
				Effect.gen(function* () {
					yield* Layer.build(reconnect_layer);
					yield* TestClock.adjust(1);

					for (const watching of attention_script) {
						yield* SubscriptionRef.set(attention, watching);
						yield* TestClock.adjust(1);
					}
				}),
			);

			return retries;
		}).pipe(Effect.provide(TestClock.layer())),
	);
};

describe("browser attention reconnect", () => {
	it("stays quiet while the window keeps the user's attention", async () => {
		expect(await Observe(true, [true, true])).toBe(0);
	});

	it("does not treat the initial attention state as a return", async () => {
		expect(await Observe(true, [])).toBe(0);
		expect(await Observe(false, [])).toBe(0);
	});

	it("releases one retry when attention returns to the window", async () => {
		expect(await Observe(true, [false, true])).toBe(1);
	});

	it("releases one retry for each separate return", async () => {
		expect(await Observe(false, [true, false, false, true])).toBe(2);
	});

	it("ignores repeated watching reports within one return", async () => {
		expect(await Observe(false, [true, true, true])).toBe(1);
	});
});
