import { describe, expect, it } from "@effect/vitest";
import { MessageChannel, type MessagePort } from "node:worker_threads";

import { Effect, Fiber, Layer, Ref } from "effect";
import { TestClock } from "effect/testing";

import {
	ArtisanClient,
	make_artisan_client_layer,
	MessagePortConnector,
	MessagePortConnectorError,
	TransportRuntimeLive,
} from "@artisan/transport/client";
import type { MessagePortConnection } from "@artisan/transport";
import { adapt_node_message_port } from "@artisan/transport/node";
import { type MessagePortTransportServer } from "@artisan/transport/server";

import { make_transport_test_harness } from "./message-channel-harness";

describe("Artisan client establishment deadline", () => {
	it.effect("exhausts stalled pre-ready attempts and starts a fresh epoch on retry", () =>
		Effect.gen(function* () {
			const attempts = yield* Ref.make(0);
			const interrupted_attempts = yield* Ref.make(0);
			const connector = Layer.succeed(MessagePortConnector, {
				Connect: Effect.gen(function* () {
					yield* Ref.update(attempts, (current) => current + 1);
					yield* Effect.addFinalizer(() =>
						Ref.update(interrupted_attempts, (current) => current + 1),
					);
					return yield* Effect.never;
				}),
			});
			const client_layer = make_artisan_client_layer({
				reconnect_attempts: 2,
				reconnect_delay_ms: 0,
			}).pipe(Layer.provide(connector), Layer.provide(TransportRuntimeLive));

			yield* Effect.scoped(
				Effect.gen(function* () {
					const client = yield* ArtisanClient;

					yield* TestClock.adjust("30 seconds");
					expect(yield* client.ConnectionState).toMatchObject({
						attempts: 2,
						phase: "exhausted",
					});
					expect(yield* Ref.get(attempts)).toBe(2);
					expect(yield* Ref.get(interrupted_attempts)).toBe(2);

					yield* client.RetryConnection;
					yield* TestClock.adjust("30 seconds");
					expect(yield* client.ConnectionState).toMatchObject({
						attempts: 2,
						phase: "exhausted",
					});
					expect(yield* Ref.get(attempts)).toBe(4);
					expect(yield* Ref.get(interrupted_attempts)).toBe(4);
				}),
			).pipe(Effect.provide(client_layer));
		}).pipe(Effect.provide(TestClock.layer())),
	);

	it.effect("keeps a ready session alive past the establishment deadline", () =>
		Effect.gen(function* () {
			const harness = yield* Effect.tryPromise(() => make_transport_test_harness());
			yield* Effect.addFinalizer(() => Effect.promise(() => harness.dispose()));
			const fixture = yield* MakeReadyThenStalledConnector(harness.server);
			const client_layer = make_artisan_client_layer({
				reconnect_attempts: 2,
				reconnect_delay_ms: 0,
			}).pipe(Layer.provide(fixture.layer), Layer.provide(TransportRuntimeLive));

			yield* Effect.scoped(
				Effect.gen(function* () {
					const client = yield* ArtisanClient;
					yield* WaitForConnectionPhase(client, "ready");
					yield* TestClock.adjust("30 seconds");
					expect(yield* client.ConnectionState).toEqual({ phase: "ready" });
				}),
			).pipe(Effect.provide(client_layer));
		}).pipe(Effect.provide(TestClock.layer())),
	);

	it.effect("parks a send when its ready MessagePort session falls into stalled reconnects", () =>
		Effect.gen(function* () {
			const harness = yield* Effect.tryPromise(() => make_transport_test_harness());
			yield* Effect.addFinalizer(() => Effect.promise(() => harness.dispose()));
			const fixture = yield* MakeReadyThenStalledConnector(harness.server);
			const client_layer = make_artisan_client_layer({
				reconnect_attempts: 2,
				reconnect_delay_ms: 0,
			}).pipe(Layer.provide(fixture.layer), Layer.provide(TransportRuntimeLive));

			yield* Effect.scoped(
				Effect.gen(function* () {
					const client = yield* ArtisanClient;
					yield* WaitForConnectionPhase(client, "ready");
					yield* TestClock.adjust("30 seconds");
					expect(yield* client.ConnectionState).toEqual({ phase: "ready" });

					yield* fixture.FailNextControlSend;
					const request = yield* client.ListProjects.pipe(Effect.exit, Effect.forkScoped);
					yield* WaitForConnectionPhase(client, "reconnecting");
					yield* TestClock.adjust("30 seconds");

					expect(yield* client.ConnectionState).toMatchObject({
						attempts: 2,
						phase: "exhausted",
					});
					const request_exit = yield* Fiber.join(request);
					expect(request_exit._tag).toBe("Failure");
				}),
			).pipe(Effect.provide(client_layer));
		}).pipe(Effect.provide(TestClock.layer())),
	);
});

const WaitForConnectionPhase = (
	client: typeof ArtisanClient.Service,
	phase: "ready" | "reconnecting",
) =>
	Effect.gen(function* () {
		while ((yield* client.ConnectionState).phase !== phase) {
			yield* Effect.yieldNow;
		}
	});

const MakeReadyThenStalledConnector = (server: typeof MessagePortTransportServer.Service) =>
	Effect.gen(function* () {
		const connects = yield* Ref.make(0);
		let active_ports: ReadonlyArray<MessagePort> = [];
		let fail_next_control_send = false;
		const layer = Layer.succeed(MessagePortConnector, {
			Connect: Effect.gen(function* () {
				const ordinal = yield* Ref.updateAndGet(connects, (current) => current + 1);
				if (ordinal > 1) return yield* Effect.never;

				const control = new MessageChannel();
				const stream = new MessageChannel();
				active_ports = [control.port1, control.port2, stream.port1, stream.port2];
				const adapt = (port: MessagePort) =>
					adapt_node_message_port(port).pipe(
						Effect.mapError((cause) => new MessagePortConnectorError({ cause })),
					);
				const raw_client_control = yield* adapt(control.port1);
				const client_ports: MessagePortConnection = {
					control_port: {
						...raw_client_control,
						Send: (message, transfer) =>
							Effect.gen(function* () {
								if (fail_next_control_send) {
									fail_next_control_send = false;
									for (const port of active_ports) port.close();
								}
								yield* raw_client_control.Send(message, transfer);
							}),
					},
					stream_port: yield* adapt(stream.port1),
				};
				const server_ports: MessagePortConnection = {
					control_port: yield* adapt(control.port2),
					stream_port: yield* adapt(stream.port2),
				};

				yield* server.Serve(server_ports).pipe(Effect.exit, Effect.forkScoped);
				yield* Effect.addFinalizer(() =>
					Effect.sync(() => {
						for (const port of active_ports) port.close();
					}),
				);
				return client_ports;
			}),
		});

		return {
			FailNextControlSend: Effect.sync(() => {
				fail_next_control_send = true;
			}),
			layer,
		};
	});
