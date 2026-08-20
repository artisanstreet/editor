import { Deferred, Effect, Queue } from "effect";
import { describe, expect, it } from "vitest";

import type { InboundControlEnvelope } from "@artisan/protocol";
import { MessagePortConnector } from "@artisan/transport/client";

import type { MessagePortConnection } from "../../modules/transport/src/connector";
import {
	make_client_connection_lifecycle,
	type ClientConnectionHandlers,
} from "../../modules/transport/src/internal/client-connection";
import { make_client_diagnostics } from "../../modules/transport/src/internal/client-diagnostics";
import type { MessagePortError, MessagePortLike } from "../../modules/transport/src/message-port";
import { TransportRuntime } from "../../modules/transport/src/transport-runtime";

const connection_id = "bootstrap_connection";

const MakePort = (
	receive: Queue.Queue<unknown, MessagePortError>,
	send: (message: unknown) => void,
): MessagePortLike => ({
	Close: Effect.void,
	Closed: Effect.never,
	Receive: Queue.take(receive),
	Send: (message) => Effect.sync(() => send(message)),
});

describe("Artisan client WebSocket bootstrap", () => {
	it("starts the live control reader before subscription retry waits for its response", async () => {
		const outcome = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const control_incoming = yield* Queue.unbounded<unknown, MessagePortError>();
					const stream_incoming = yield* Queue.unbounded<unknown, MessagePortError>();
					const subscription_started = yield* Deferred.make<void>();
					let id = 0;
					const runtime = TransportRuntime.of({
						MakeId: (prefix) => Effect.sync(() => `${prefix}_${(id += 1)}`),
						Now: Effect.succeed("2026-08-15T09:00:00.000Z"),
					});
					const SendControl = (message: unknown) => {
						const frame = message as {
							attempt_id?: string;
							channel?: "control" | "stream";
							connection_id?: string;
							kind?: string;
							payload?: {
								kind?: string;
								message_id?: string;
								subscription_id?: string;
							};
							session_id?: string;
						};
						if (frame.kind === "transport.hello" && frame.channel === "control") {
							Queue.offerUnsafe(control_incoming, {
								...frame,
								connection_id,
								kind: "transport.ready",
							});
							return;
						}
						if (frame.kind !== "transport.control") return;
						if (frame.payload?.kind === "hello") {
							Queue.offerUnsafe(control_incoming, {
								connection_id,
								kind: "transport.control",
								payload: {
									correlation_id: frame.payload.message_id,
									kind: "welcome",
									message_id: "welcome_message",
									origin: "backend",
									payload: {
										connection_id: "protocol_connection",
										current_event_cursors: [],
										heartbeat_interval_ms: 15_000,
										heartbeat_timeout_ms: 45_000,
										journal_sequence: 0,
										stream_ticket: "stream_ticket",
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at: "2026-08-15T09:00:00.000Z",
								},
								transport_version: 1,
							});
							return;
						}
						if (frame.payload?.kind === "subscribe") {
							Queue.offerUnsafe(control_incoming, {
								connection_id,
								kind: "transport.control",
								payload: {
									correlation_id: frame.payload.message_id,
									kind: "subscription.started",
									message_id: "subscription_started_message",
									origin: "backend",
									payload: { stream_id: "subscription_stream" },
									protocol_version: 1,
									schema_version: 1,
									sent_at: "2026-08-15T09:00:00.000Z",
									subscription_id: frame.payload.subscription_id,
								},
								transport_version: 1,
							});
						}
					};
					const ports: MessagePortConnection = {
						control_port: MakePort(control_incoming, SendControl),
						stream_port: MakePort(stream_incoming, (message) => {
							const frame = message as {
								attempt_id?: string;
								channel?: string;
								kind?: string;
								session_id?: string;
							};
							if (frame.kind !== "transport.hello" || frame.channel !== "stream")
								return;
							Queue.offerUnsafe(stream_incoming, {
								...frame,
								connection_id,
								kind: "transport.ready",
							});
						}),
					};
					const diagnostics = yield* make_client_diagnostics(runtime);
					const lifecycle = yield* make_client_connection_lifecycle(
						0,
						1,
						diagnostics,
					).pipe(
						Effect.provideService(MessagePortConnector, {
							Connect: Effect.succeed(ports),
						}),
						Effect.provideService(TransportRuntime, runtime),
					);
					const handlers = {
						on_fatal: () => Effect.void,
						publish_error: () => Effect.void,
						requests: {
							Park: () => Effect.void,
							ResetConnection: Effect.void,
							Resume: Effect.void,
							Retry: Effect.void,
						},
						streams: { Disconnect: () => Effect.void, Handle: () => Effect.void },
						subscriptions: {
							ApplyEvent: () =>
								Effect.succeed({ event_cursors: {}, last_journal_sequence: 0 }),
							ApplyReplayComplete: () => Effect.void,
							AwaitReady: Deferred.await(subscription_started),
							DropResumeState: Effect.void,
							HandleStarted: () => Deferred.succeed(subscription_started, undefined),
							HandleUpdate: () => Effect.void,
							Reject: () => Effect.succeed(false),
							ResetConnection: Effect.void,
							ResumeCursors: Effect.succeed({
								event_cursors: {},
								last_journal_sequence: 0,
							}),
							Retry: (
								send: (envelope: InboundControlEnvelope) => Effect.Effect<void>,
							) =>
								send({
									kind: "subscribe",
									message_id: "subscribe_message",
									origin: "frontend",
									payload: { type: "thread.list" },
									protocol_version: 1,
									schema_version: 1,
									sent_at: "2026-08-15T09:00:00.000Z",
									subscription_id: "subscription",
								}).pipe(Effect.andThen(Deferred.await(subscription_started))),
						},
					} as unknown as ClientConnectionHandlers;
					yield* lifecycle.Start(handlers);
					return yield* Effect.gen(function* () {
						while ((yield* lifecycle.ConnectionState).phase !== "ready") {
							yield* Effect.sleep("1 millis");
						}
						return yield* lifecycle.ConnectionState;
					}).pipe(Effect.timeout("1 second"));
				}),
			),
		);

		expect(outcome).toEqual({ phase: "ready" });
	});
});
