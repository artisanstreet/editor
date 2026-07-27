import { Effect, Layer, ManagedRuntime } from "effect";
import { describe, expect, it } from "vitest";

import {
	ArtisanClient,
	make_artisan_client_layer,
	MessagePortConnector,
	MessagePortConnectorError,
	TransportRuntimeLive,
} from "@artisan/transport/client";

describe("Artisan client connection retry", () => {
	it("exhausts five exponential attempts and waits for an explicit retry", async () => {
		let attempts = 0;
		const connector = Layer.succeed(MessagePortConnector, {
			Connect: Effect.sync(() => {
				attempts += 1;
			}).pipe(
				Effect.andThen(
					Effect.fail(
						new MessagePortConnectorError({
							cause: new Error("Forge unavailable"),
						}),
					),
				),
			),
		});
		const runtime = ManagedRuntime.make(
			make_artisan_client_layer({ reconnect_delay_ms: 1 }).pipe(
				Layer.provide(connector),
				Layer.provide(TransportRuntimeLive),
			),
		);

		try {
			const client = await runtime.runPromise(ArtisanClient);
			await runtime.runPromise(
				Effect.gen(function* () {
					while ((yield* client.ConnectionState).phase !== "exhausted") {
						yield* Effect.sleep("1 millis");
					}
				}),
			);
			expect(attempts).toBe(5);

			await runtime.runPromise(client.RetryConnection);
			await runtime.runPromise(
				Effect.gen(function* () {
					while ((yield* client.ConnectionState).phase !== "exhausted") {
						yield* Effect.sleep("1 millis");
					}
				}),
			);
			expect(attempts).toBe(10);
			expect((await runtime.runPromise(client.ConnectionState)).phase).toBe("exhausted");
		} finally {
			await runtime.dispose();
		}
	});
});
