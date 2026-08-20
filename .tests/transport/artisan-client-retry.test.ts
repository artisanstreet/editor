import { MessageChannel, type MessagePort } from "node:worker_threads";

import { Effect, Layer, ManagedRuntime } from "effect";
import { describe, expect, it } from "vitest";

import type { MessagePortConnection } from "@artisan/transport";
import {
	ArtisanClient,
	make_artisan_client_layer,
	MessagePortConnector,
	MessagePortConnectorError,
	TransportRuntimeLive,
} from "@artisan/transport/client";
import { adapt_node_message_port } from "@artisan/transport/node";

import { make_transport_test_harness } from "./message-channel-harness";

describe("Artisan client connection retry", () => {
	it("does not pre-authorize a future outage when retry is signalled while ready", async () => {
		const harness = await make_transport_test_harness();
		let attempts = 0;
		let accept_connections = true;
		let active_ports: ReadonlyArray<MessagePort> = [];
		const connector = Layer.succeed(MessagePortConnector, {
			Connect: Effect.gen(function* () {
				attempts += 1;

				if (!accept_connections) {
					return yield* Effect.fail(
						new MessagePortConnectorError({
							cause: new Error("Forge unavailable"),
						}),
					);
				}

				const control = new MessageChannel();
				const stream = new MessageChannel();
				active_ports = [control.port1, control.port2, stream.port1, stream.port2];
				const adapt = (port: MessagePort) =>
					adapt_node_message_port(port).pipe(
						Effect.mapError((cause) => new MessagePortConnectorError({ cause })),
					);
				const client_ports: MessagePortConnection = {
					control_port: yield* adapt(control.port1),
					stream_port: yield* adapt(stream.port1),
				};
				const server_ports: MessagePortConnection = {
					control_port: yield* adapt(control.port2),
					stream_port: yield* adapt(stream.port2),
				};

				yield* harness.server.Serve(server_ports).pipe(Effect.exit, Effect.forkScoped);
				yield* Effect.addFinalizer(() =>
					Effect.sync(() => {
						for (const port of active_ports) port.close();
					}),
				);

				return client_ports;
			}),
		});
		const runtime = ManagedRuntime.make(
			make_artisan_client_layer({ reconnect_attempts: 1, reconnect_delay_ms: 1 }).pipe(
				Layer.provide(connector),
				Layer.provide(TransportRuntimeLive),
			),
		);

		try {
			const client = await runtime.runPromise(ArtisanClient);
			await runtime.runPromise(
				Effect.gen(function* () {
					while ((yield* client.ConnectionState).phase !== "ready") {
						yield* Effect.sleep("1 millis");
					}

					yield* client.RetryConnection;
					accept_connections = false;
					for (const port of active_ports) port.close();

					while ((yield* client.ConnectionState).phase !== "exhausted") {
						yield* Effect.sleep("1 millis");
					}
				}).pipe(Effect.timeout("2 seconds")),
			);

			expect(attempts).toBe(2);
			await runtime.runPromise(Effect.sleep("50 millis"));
			expect(attempts).toBe(2);
		} finally {
			await runtime.dispose();
			await harness.dispose();
		}
	});

	it("latches one recovery signal received before the first retry epoch exhausts", async () => {
		let attempts = 0;
		const connector = Layer.succeed(MessagePortConnector, {
			Connect: Effect.gen(function* () {
				attempts += 1;
				yield* Effect.sleep("20 millis");
				return yield* Effect.fail(
					new MessagePortConnectorError({
						cause: new Error("Forge unavailable"),
					}),
				);
			}),
		});
		const runtime = ManagedRuntime.make(
			make_artisan_client_layer({ reconnect_attempts: 2, reconnect_delay_ms: 1 }).pipe(
				Layer.provide(connector),
				Layer.provide(TransportRuntimeLive),
			),
		);

		try {
			const client = await runtime.runPromise(ArtisanClient);
			await runtime.runPromise(
				Effect.gen(function* () {
					while (attempts < 1) {
						yield* Effect.sleep("1 millis");
					}
					yield* client.RetryConnection;
					while (attempts < 4) {
						yield* Effect.sleep("1 millis");
					}
					while ((yield* client.ConnectionState).phase !== "exhausted") {
						yield* Effect.sleep("1 millis");
					}
				}).pipe(Effect.timeout("2 seconds")),
			);

			expect(attempts).toBe(4);
			await runtime.runPromise(Effect.sleep("50 millis"));
			expect(attempts).toBe(4);
		} finally {
			await runtime.dispose();
		}
	});

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
