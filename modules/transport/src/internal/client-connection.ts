import {
	Cause,
	Deferred,
	Effect,
	Fiber,
	Option,
	Ref,
	Schedule,
	Scope,
	Stream,
	SubscriptionRef,
} from "effect";

import {
	DecodeOutboundControlEnvelope,
	IsControlRpcSuccess,
	SupportedProtocolVersions,
	type AckEnvelope,
	type HeartbeatPongEnvelope,
	type HelloEnvelope,
	type InboundControlEnvelope,
	type OutboundControlEnvelope,
} from "@artisan/protocol";

import { ArtisanClientError } from "../client-contract";
import type { ArtisanConnectionState } from "../client-contract";
import { MessagePortConnector } from "../connector";
import { TransportRuntime } from "../transport-runtime";
import {
	DecodeTransportFrame,
	SupportedTransportVersions,
	type TransportCloseFrame,
	type TransportControlFrame,
	type TransportHelloFrame,
} from "../wire";
import {
	client_error,
	map_port_error,
	protocol_client_error,
	type ActiveClientSession,
	type AwaitActive,
	type FrontendTrace,
	type SendCurrent,
} from "./client-common";
import type { ClientRequestCoordinator } from "./client-request-coordinator";
import type { ClientStreamChannel } from "./client-stream-channel";
import type { ClientSubscriptionCoordinator } from "./client-subscription-coordinator";

interface ConnectionState {
	readonly active: Option.Option<ActiveClientSession>;
	readonly connection_signal: Deferred.Deferred<void>;
	readonly disposed: boolean;
}

/** Supplies the coordinators invoked by one negotiated connection. */
export interface ClientConnectionHandlers {
	readonly on_fatal: (error: ArtisanClientError) => Effect.Effect<void>;
	readonly publish_error: (error: ArtisanClientError) => Effect.Effect<void>;
	readonly requests: ClientRequestCoordinator;
	readonly streams: ClientStreamChannel;
	readonly subscriptions: ClientSubscriptionCoordinator;
}

/** Owns bootstrap, protocol negotiation, heartbeat, reconnect, and stale-frame fencing. */
export interface ClientConnectionLifecycle {
	readonly AwaitActive: AwaitActive;
	readonly Current: Effect.Effect<Option.Option<ActiveClientSession>>;
	readonly ConnectionChanges: Stream.Stream<ArtisanConnectionState>;
	readonly ConnectionState: Effect.Effect<ArtisanConnectionState>;
	readonly Dispose: Effect.Effect<void>;
	readonly MakeTrace: Effect.Effect<FrontendTrace>;
	readonly RetryConnection: Effect.Effect<void>;
	readonly SendCurrent: SendCurrent;
	readonly Start: (handlers: ClientConnectionHandlers) => Effect.Effect<void, never, Scope.Scope>;
}

/** Builds client connection state without starting its reconnect supervisor. */
export const make_client_connection_lifecycle = (
	reconnect_delay_ms: number,
	reconnect_attempts = 5,
) =>
	Effect.gen(function* () {
		const connector = yield* MessagePortConnector;
		const runtime = yield* TransportRuntime;
		const session_id = yield* runtime.MakeId("client_session");
		const initial_signal = yield* Deferred.make<void>();
		const disposed_signal = yield* Deferred.make<void>();
		const state = yield* Ref.make<ConnectionState>({
			active: Option.none(),
			connection_signal: initial_signal,
			disposed: false,
		});
		const connection_state = yield* SubscriptionRef.make<ArtisanConnectionState>({
			phase: "connecting",
		});
		const ever_connected = yield* Ref.make(false);
		const initial_retry_gate = yield* Deferred.make<void>();
		const retry_gate = yield* Ref.make(initial_retry_gate);

		const SetConnectionState = (next: ArtisanConnectionState) =>
			SubscriptionRef.set(connection_state, next);

		const retry_connection = Effect.gen(function* () {
			const current = yield* SubscriptionRef.get(connection_state);
			if (current.phase !== "exhausted") return;
			yield* Ref.get(retry_gate).pipe(
				Effect.flatMap((gate) => Deferred.succeed(gate, undefined)),
			);
		});

		const make_trace = Effect.gen(function* () {
			return {
				message_id: yield* runtime.MakeId("message"),
				origin: "frontend" as const,
				protocol_version: SupportedProtocolVersions[0],
				schema_version: 1 as const,
				sent_at: yield* runtime.Now,
			} satisfies FrontendTrace;
		});

		const send_control = (active: ActiveClientSession, envelope: InboundControlEnvelope) =>
			active.ports.control_port
				.Send({
					connection_id: active.connection_id,
					kind: "transport.control",
					payload: envelope,
					transport_version: SupportedTransportVersions[0],
				} satisfies TransportControlFrame)
				.pipe(Effect.mapError(map_port_error));
		const send_active = (active: ActiveClientSession, envelope: InboundControlEnvelope) =>
			send_control(active, envelope).pipe(
				Effect.catch(() =>
					Effect.all([active.ports.control_port.Close, active.ports.stream_port.Close], {
						concurrency: "unbounded",
						discard: true,
					}),
				),
			);

		const send_current = (envelope: InboundControlEnvelope) =>
			Ref.get(state).pipe(
				Effect.flatMap((current) =>
					Option.match(current.active, {
						onNone: () => Effect.void,
						onSome: (active) => send_active(active, envelope),
					}),
				),
			);

		const await_active = Effect.gen(function* () {
			while (true) {
				const current = yield* Ref.get(state);

				if (current.disposed) {
					return yield* Effect.fail(
						client_error(
							"disposed",
							"The Artisan client was disposed.",
							new Error("client disposed"),
						),
					);
				}

				if (Option.isSome(current.active)) {
					return current.active.value;
				}

				yield* Deferred.await(current.connection_signal);
			}
		});

		const dispose = Effect.gen(function* () {
			const current = yield* Ref.getAndUpdate(state, (value) => ({
				...value,
				active: Option.none(),
				disposed: true,
			}));

			if (current.disposed) {
				return;
			}

			yield* Deferred.succeed(current.connection_signal, undefined);
			yield* Deferred.succeed(disposed_signal, undefined);
			yield* Ref.get(retry_gate).pipe(
				Effect.flatMap((gate) => Deferred.succeed(gate, undefined)),
			);

			if (Option.isSome(current.active)) {
				yield* current.active.value.ports.control_port
					.Send({
						connection_id: current.active.value.connection_id,
						kind: "transport.close",
						reason: "client_dispose",
						transport_version: SupportedTransportVersions[0],
					} satisfies TransportCloseFrame)
					.pipe(Effect.ignore);
				yield* Effect.all(
					[
						current.active.value.ports.control_port.Close,
						current.active.value.ports.stream_port.Close,
					],
					{ concurrency: "unbounded", discard: true },
				);
			}
		});

		const decode_ready = (raw: unknown, hello: TransportHelloFrame) =>
			DecodeTransportFrame(raw).pipe(
				Effect.mapError((cause) =>
					client_error("connection", "Transport bootstrap failed.", cause, true),
				),
				Effect.flatMap((frame) => {
					if (
						frame.kind !== "transport.ready" ||
						frame.attempt_id !== hello.attempt_id ||
						frame.session_id !== hello.session_id ||
						frame.channel !== hello.channel
					) {
						return Effect.fail(
							client_error(
								"connection",
								"Transport readiness did not match its bootstrap hello.",
								new Error("mismatched transport ready frame"),
								true,
							),
						);
					}

					return Effect.succeed(frame);
				}),
			);

		const decode_control = (raw: unknown, connection_id: string) =>
			DecodeTransportFrame(raw).pipe(
				Effect.mapError((cause) =>
					client_error("malformed", "The control transport frame is malformed.", cause),
				),
				Effect.flatMap((frame) => {
					if (frame.kind === "transport.close") {
						return frame.connection_id === connection_id
							? Effect.fail(
									client_error(
										"connection",
										"The backend closed the transport session.",
										frame,
										true,
									),
								)
							: Effect.fail(
									client_error(
										"malformed",
										"The close frame belongs to a stale connection.",
										new Error("stale transport close"),
									),
								);
					}

					if (frame.kind === "transport.error") {
						if (frame.connection_id && frame.connection_id !== connection_id) {
							return Effect.fail(
								client_error(
									"malformed",
									"The transport error belongs to a stale connection.",
									new Error("stale transport error"),
								),
							);
						}

						return Effect.fail(
							client_error(
								"protocol",
								frame.message,
								frame,
								frame.retryable,
								frame.code,
							),
						);
					}

					if (frame.kind !== "transport.control") {
						return Effect.fail(
							client_error(
								"malformed",
								"Expected a control transport frame.",
								new Error("unexpected control port frame"),
							),
						);
					}

					if (frame.connection_id !== connection_id) {
						return Effect.fail(
							client_error(
								"malformed",
								"The control frame belongs to a stale connection.",
								new Error("stale transport connection id"),
							),
						);
					}

					return DecodeOutboundControlEnvelope(frame.payload).pipe(
						Effect.mapError((cause) =>
							client_error(
								"malformed",
								"The backend control envelope is malformed.",
								cause,
							),
						),
					);
				}),
			);

		const decode_stream = (raw: unknown, connection_id: string) =>
			DecodeTransportFrame(raw).pipe(
				Effect.mapError((cause) =>
					client_error("malformed", "The stream transport frame is malformed.", cause),
				),
				Effect.flatMap((frame) => {
					if (frame.kind === "transport.error") {
						return Effect.fail(
							client_error(
								"protocol",
								frame.message,
								frame,
								frame.retryable,
								frame.code,
							),
						);
					}

					if (frame.kind !== "transport.stream") {
						return Effect.fail(
							client_error(
								"malformed",
								"Expected a stream transport frame.",
								new Error("unexpected stream port frame"),
							),
						);
					}

					if (frame.connection_id !== connection_id) {
						return Effect.fail(
							client_error(
								"malformed",
								"The stream frame belongs to a stale connection.",
								new Error("stale stream connection id"),
							),
						);
					}

					return Effect.succeed(frame.frame);
				}),
			);

		const start = (handlers: ClientConnectionHandlers) => {
			const acknowledge = (active: ActiveClientSession, cursors: AckEnvelope["payload"]) =>
				Effect.gen(function* () {
					const trace = yield* make_trace;
					const ack: AckEnvelope = { ...trace, kind: "ack", payload: cursors };

					yield* send_control(active, ack).pipe(Effect.ignore);
				});

			const handle_protocol_error = (
				envelope: Extract<OutboundControlEnvelope, { kind: "protocol.error" }>,
			) =>
				Effect.gen(function* () {
					if (!envelope.correlation_id) {
						yield* handlers.publish_error(protocol_client_error(envelope.payload));

						return;
					}

					const request_handled = yield* handlers.requests.Reject(
						envelope.correlation_id,
						envelope.payload,
					);

					if (request_handled) {
						return;
					}

					const subscription_handled = yield* handlers.subscriptions.Reject(
						envelope.correlation_id,
						envelope.payload,
					);

					if (!subscription_handled) {
						yield* handlers.publish_error(protocol_client_error(envelope.payload));
					}
				});

			const handle_control = (
				active: ActiveClientSession,
				envelope: OutboundControlEnvelope,
			) => {
				if (IsControlRpcSuccess(envelope)) {
					return handlers.requests.Resolve(envelope);
				}

				switch (envelope.kind) {
					case "event":
						return handlers.subscriptions.ApplyEvent(envelope).pipe(
							Effect.flatMap((cursors) =>
								acknowledge(active, {
									event_cursors: cursors.event_cursors,
									journal_sequence: cursors.last_journal_sequence,
								}),
							),
							Effect.catch((error) =>
								error.code === "event_overflow"
									? handlers.on_fatal(error)
									: Effect.fail(error),
							),
						);
					case "heartbeat.ping":
						return Effect.gen(function* () {
							const trace = yield* make_trace;
							const pong: HeartbeatPongEnvelope = {
								...trace,
								correlation_id: envelope.message_id,
								kind: "heartbeat.pong",
								payload: { nonce: envelope.payload.nonce },
							};

							yield* send_control(active, pong).pipe(Effect.ignore);
						});
					case "protocol.error":
						return handle_protocol_error(envelope);
					case "subscription.started":
						return handlers.subscriptions.HandleStarted(envelope);
					case "thread.list.snapshot":
					case "thread.list.upsert":
					case "thread.list.remove":
					case "project.list.snapshot":
					case "project.list.updated":
					case "orchestration.graph.snapshot":
					case "orchestration.graph.patch":
					case "conversation.snapshot":
					case "conversation.patch":
					case "thread.transcript.snapshot":
					case "thread.transcript.append":
					case "orchestration.group.list.snapshot":
					case "orchestration.group.list.patch":
					case "thread.session.snapshot":
					case "surface.list.snapshot":
					case "surface.usage.aggregate.snapshot":
					case "workspace.conflict.list.snapshot":
						return handlers.subscriptions.HandleUpdate(envelope);
					case "replay.complete":
					case "subscription.stopped":
						return Effect.void;
					case "welcome":
						return Effect.fail(
							client_error(
								"malformed",
								"The backend sent a second welcome frame.",
								new Error("duplicate welcome"),
							),
						);
					default:
						return Effect.fail(
							client_error(
								"malformed",
								"The backend sent an unsupported control envelope.",
								new Error("unsupported outbound control envelope"),
							),
						);
				}
			};

			const run_session = Effect.scoped(
				Effect.gen(function* () {
					const ports = yield* connector.Connect.pipe(
						Effect.mapError((cause) =>
							client_error(
								"connection",
								"A fresh MessagePort pair could not be opened.",
								cause,
								true,
							),
						),
					);
					const attempt_id = yield* runtime.MakeId("connection_attempt");
					const control_hello: TransportHelloFrame = {
						attempt_id,
						channel: "control",
						kind: "transport.hello",
						session_id,
						transport_version: SupportedTransportVersions[0],
					};
					const stream_hello: TransportHelloFrame = {
						...control_hello,
						channel: "stream",
					};

					yield* Effect.all(
						[
							ports.control_port.Send(control_hello),
							ports.stream_port.Send(stream_hello),
						],
						{ concurrency: "unbounded", discard: true },
					).pipe(Effect.mapError(map_port_error));

					const raw_ready = yield* Effect.all(
						[ports.control_port.Receive, ports.stream_port.Receive],
						{ concurrency: "unbounded" },
					).pipe(Effect.mapError(map_port_error));
					const control_ready = yield* decode_ready(raw_ready[0], control_hello);
					const stream_ready = yield* decode_ready(raw_ready[1], stream_hello);

					if (control_ready.connection_id !== stream_ready.connection_id) {
						return yield* Effect.fail(
							client_error(
								"connection",
								"Control and stream ports were not paired to one connection.",
								new Error("transport connection id mismatch"),
								true,
							),
						);
					}

					const resume = yield* handlers.subscriptions.ResumeCursors;
					const trace = yield* make_trace;
					const hello: HelloEnvelope = {
						kind: "hello",
						message_id: trace.message_id,
						origin: "frontend",
						payload: {
							event_cursors: resume.event_cursors,
							last_journal_sequence: resume.last_journal_sequence,
							supported_protocol_versions: [...SupportedProtocolVersions],
						},
						schema_version: 1,
						sent_at: trace.sent_at,
					};

					yield* ports.control_port
						.Send({
							connection_id: control_ready.connection_id,
							kind: "transport.control",
							payload: hello,
							transport_version: SupportedTransportVersions[0],
						} satisfies TransportControlFrame)
						.pipe(Effect.mapError(map_port_error));

					const welcome = yield* ports.control_port.Receive.pipe(
						Effect.mapError(map_port_error),
						Effect.flatMap((raw) => decode_control(raw, control_ready.connection_id)),
					);

					if (welcome.kind !== "welcome" || welcome.correlation_id !== hello.message_id) {
						return yield* Effect.fail(
							client_error(
								"connection",
								"The backend did not complete protocol negotiation.",
								new Error("welcome correlation mismatch"),
								true,
							),
						);
					}

					const active: ActiveClientSession = {
						connection_id: control_ready.connection_id,
						ports,
						protocol_connection_id: welcome.payload.connection_id,
						stream_ticket: welcome.payload.stream_ticket,
					};
					yield* handlers.requests.ResetConnection;
					yield* handlers.subscriptions.Retry((envelope) =>
						send_active(active, envelope),
					);

					const control_loop = Effect.forever(
						ports.control_port.Receive.pipe(
							Effect.mapError(map_port_error),
							Effect.flatMap((raw) => decode_control(raw, active.connection_id)),
							Effect.flatMap((envelope) => handle_control(active, envelope)),
						),
					);
					const stream_loop = Effect.forever(
						ports.stream_port.Receive.pipe(
							Effect.mapError(map_port_error),
							Effect.flatMap((raw) => decode_stream(raw, active.connection_id)),
							Effect.flatMap((frame) =>
								frame.kind === "stream.bind"
									? Effect.fail(
											client_error(
												"malformed",
												"The backend sent a client-only stream bind frame.",
												new Error("unexpected stream.bind"),
											),
										)
									: handlers.streams.Handle(active, frame),
							),
						),
					);
					const session = Effect.raceAllFirst([
						control_loop,
						stream_loop,
						ports.control_port.Closed,
						ports.stream_port.Closed,
						Deferred.await(disposed_signal),
					]);
					const session_fiber = yield* Effect.forkScoped(session);
					yield* Effect.raceFirst(
						handlers.subscriptions.AwaitReady,
						Fiber.join(session_fiber).pipe(
							Effect.andThen(
								Effect.fail(
									client_error(
										"connection",
										"The transport session ended before subscriptions were established.",
										new Error("subscription reconnect readiness interrupted"),
										true,
									),
								),
							),
						),
					);
					const previous_signal = yield* Ref.modify(
						state,
						(current): readonly [Deferred.Deferred<void>, ConnectionState] => [
							current.connection_signal,
							{ ...current, active: Option.some(active) },
						],
					);
					yield* Deferred.succeed(previous_signal, undefined);
					yield* handlers.requests.Retry;
					yield* SetConnectionState({ phase: "ready" });
					yield* Ref.set(ever_connected, true);
					const session_exit = yield* Effect.exit(Fiber.join(session_fiber));
					const next_signal = yield* Deferred.make<void>();

					yield* Ref.update(state, (current) =>
						Option.isSome(current.active) &&
						current.active.value.connection_id === active.connection_id
							? {
									...current,
									active: Option.none(),
									connection_signal: next_signal,
								}
							: current,
					);
					yield* handlers.streams.Disconnect(active.connection_id);
					yield* handlers.subscriptions.ResetConnection;

					const failure =
						session_exit._tag === "Failure"
							? Cause.squash(session_exit.cause)
							: new Error("transport session closed");
					const connection_error =
						failure instanceof ArtisanClientError
							? failure
							: client_error(
									"connection",
									"The transport session disconnected.",
									failure,
									true,
								);

					yield* handlers.publish_error(connection_error);
					return yield* Effect.fail(connection_error);
				}),
			);

			const supervisor = Effect.gen(function* () {
				while (true) {
					const current = yield* Ref.get(state);

					if (current.disposed) {
						return;
					}

					const retry_schedule = Schedule.exponential(
						`${reconnect_delay_ms} millis`,
					).pipe(Schedule.jittered, Schedule.upTo({ times: reconnect_attempts - 1 }));
					const session = yield* Effect.exit(
						Effect.gen(function* () {
							const connected = yield* Ref.get(ever_connected);
							yield* SetConnectionState({
								phase: connected ? "reconnecting" : "connecting",
							});
							return yield* run_session;
						}).pipe(Effect.retry(retry_schedule)),
					);
					const after = yield* Ref.get(state);

					if (after.disposed) {
						return;
					}

					if (session._tag === "Success") {
						continue;
					}

					const failure = Cause.squash(session.cause);
					const error =
						failure instanceof ArtisanClientError
							? failure
							: client_error(
									"connection",
									"The transport disconnected and will reconnect.",
									failure,
									true,
								);

					yield* handlers.publish_error(error);
					yield* SetConnectionState({
						attempts: reconnect_attempts,
						error,
						phase: "exhausted",
					});
					const gate = yield* Ref.get(retry_gate);
					yield* Deferred.await(gate);
					const next_gate = yield* Deferred.make<void>();
					yield* Ref.set(retry_gate, next_gate);
				}
			});

			return supervisor.pipe(Effect.forkScoped, Effect.asVoid);
		};

		return {
			AwaitActive: await_active,
			ConnectionChanges: SubscriptionRef.changes(connection_state),
			ConnectionState: SubscriptionRef.get(connection_state),
			Current: Ref.get(state).pipe(Effect.map((current) => current.active)),
			Dispose: dispose,
			MakeTrace: make_trace,
			RetryConnection: retry_connection,
			SendCurrent: send_current,
			Start: start,
		} satisfies ClientConnectionLifecycle;
	});
