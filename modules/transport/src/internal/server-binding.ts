import { Effect, Option, Queue, Ref, Stream } from "effect";

import {
	DecodeInboundControlEnvelope,
	type InboundControlEnvelope,
	type OutboundControlEnvelope,
} from "@artisan/protocol";
import { ProtocolServer } from "@artisan/backend";

import type { MessagePortConnection } from "../connector";
import type { MessagePortLike } from "../message-port";
import type {
	MessagePortTransportServerError,
	MessagePortTransportServerOptions,
	MessagePortTransportServerShape,
} from "../server-contract";
import { BinaryStreamSource } from "../stream-source";
import { TransportRuntime } from "../transport-runtime";
import {
	DecodeTransportFrame,
	SupportedTransportVersions,
	type TransportCloseFrame,
	type TransportControlFrame,
	type TransportErrorFrame,
	type TransportHelloFrame,
	type TransportStreamFrame,
} from "../wire";
import { map_server_port_error, server_error } from "./server-common";
import { make_server_stream_channel } from "./server-stream-channel";

/**
 * Maximum durable events handed to a peer without their ACKs advancing.
 * Projection frames remain independent; this window specifically prevents a
 * journal resume from dumping an arbitrarily large retained history into a
 * browser faster than its renderer can decode it.
 */
export const reliable_event_window_capacity = 32;

/** Builds the server binding implementation from protocol and stream services. */
export const make_server_binding = (options: Required<MessagePortTransportServerOptions>) =>
	Effect.gen(function* () {
		const protocol_server = yield* ProtocolServer;
		const stream_source = yield* BinaryStreamSource;
		const runtime = yield* TransportRuntime;

		const serve = (ports: MessagePortConnection) =>
			Effect.scoped(
				Effect.gen(function* () {
					const close_ports = Effect.all(
						[ports.control_port.Close, ports.stream_port.Close],
						{ concurrency: "unbounded", discard: true },
					);

					yield* Effect.addFinalizer(() => close_ports);

					const raw_hellos = yield* Effect.all(
						[ports.control_port.Receive, ports.stream_port.Receive],
						{ concurrency: "unbounded" },
					).pipe(Effect.mapError(map_server_port_error));
					const decoded_hellos = yield* Effect.forEach(raw_hellos, (raw) =>
						DecodeTransportFrame(raw).pipe(
							Effect.mapError((cause) => server_error("bootstrap", cause)),
						),
					);
					const control_hello = decoded_hellos[0];
					const stream_hello = decoded_hellos[1];

					if (
						control_hello?.kind !== "transport.hello" ||
						control_hello.channel !== "control" ||
						stream_hello?.kind !== "transport.hello" ||
						stream_hello.channel !== "stream" ||
						control_hello.attempt_id !== stream_hello.attempt_id ||
						control_hello.session_id !== stream_hello.session_id
					) {
						return yield* Effect.fail(
							server_error(
								"bootstrap",
								new Error(
									"control and stream bootstrap frames do not form one pair",
								),
							),
						);
					}

					const connection_id = yield* runtime.MakeId("transport_connection");
					const ready_for = (hello: TransportHelloFrame) => ({
						...hello,
						connection_id,
						kind: "transport.ready" as const,
					});

					yield* Effect.all(
						[
							ports.control_port.Send(ready_for(control_hello)),
							ports.stream_port.Send(ready_for(stream_hello)),
						],
						{ concurrency: "unbounded", discard: true },
					).pipe(Effect.mapError(map_server_port_error));

					const connection = yield* protocol_server.Open;
					const stream_ticket = yield* Ref.make(Option.none<string>());
					const seen_control_ids = new Set<string>();
					const pending_event_sequences = yield* Ref.make<ReadonlyArray<number>>([]);
					const event_window_changed = yield* Effect.acquireRelease(
						Queue.sliding<void>(1),
						Queue.shutdown,
					);
					const AcknowledgeEventWindow = (journal_sequence: number) =>
						Ref.update(pending_event_sequences, (pending) =>
							pending.filter((sequence) => sequence > journal_sequence),
						).pipe(
							Effect.andThen(Queue.offer(event_window_changed, undefined)),
							Effect.asVoid,
						);
					const ReserveEventWindow = (journal_sequence: number) =>
						Effect.gen(function* () {
							while (
								(yield* Ref.get(pending_event_sequences)).length >=
								reliable_event_window_capacity
							) {
								yield* Queue.take(event_window_changed);
							}
							yield* Ref.update(pending_event_sequences, (pending) => [
								...pending,
								journal_sequence,
							]);
						});
					const stream_channel = yield* make_server_stream_channel(
						connection_id,
						ports.stream_port,
						stream_source,
						stream_ticket,
						options.max_active_streams,
					);

					yield* Effect.addFinalizer(() =>
						Effect.all([connection.Close, close_ports], {
							concurrency: "unbounded",
							discard: true,
						}),
					);

					const send_transport_error = (
						port: MessagePortLike,
						channel: TransportErrorFrame["channel"],
						code: string,
						message: string,
					) =>
						port
							.Send({
								channel,
								code,
								connection_id,
								kind: "transport.error",
								message,
								retryable: false,
								transport_version: SupportedTransportVersions[0],
							} satisfies TransportErrorFrame)
							.pipe(Effect.ignore);

					const validate_connection_id = <
						Frame extends { readonly connection_id: string },
					>(
						frame: Frame,
					) => {
						if (frame.connection_id !== connection_id) {
							return Effect.fail(
								server_error(
									"stale_connection",
									new Error("frame belongs to a stale transport connection"),
								),
							);
						}

						return Effect.succeed(frame);
					};
					const decode_control_frame = (
						raw: unknown,
					): Effect.Effect<
						TransportCloseFrame | TransportControlFrame,
						MessagePortTransportServerError
					> =>
						DecodeTransportFrame(raw).pipe(
							Effect.mapError((cause) => server_error("malformed", cause)),
							Effect.flatMap((frame) => {
								if (
									frame.kind !== "transport.close" &&
									frame.kind !== "transport.control"
								) {
									return Effect.fail(
										server_error(
											"malformed",
											new Error(
												"expected transport.control or transport.close frame",
											),
										),
									);
								}

								return validate_connection_id(frame);
							}),
						);
					const decode_stream_frame = (
						raw: unknown,
					): Effect.Effect<TransportStreamFrame, MessagePortTransportServerError> =>
						DecodeTransportFrame(raw).pipe(
							Effect.mapError((cause) => server_error("malformed", cause)),
							Effect.flatMap((frame) => {
								if (frame.kind !== "transport.stream") {
									return Effect.fail(
										server_error(
											"malformed",
											new Error("expected transport.stream frame"),
										),
									);
								}

								return validate_connection_id(frame);
							}),
						);

					const control_inbound = Effect.forever(
						ports.control_port.Receive.pipe(
							Effect.mapError(map_server_port_error),
							Effect.flatMap(decode_control_frame),
							Effect.flatMap((frame) =>
								frame.kind === "transport.close"
									? connection.Close
									: DecodeInboundControlEnvelope(frame.payload).pipe(
											Effect.mapError((cause) =>
												server_error("malformed", cause),
											),
											Effect.flatMap((envelope: InboundControlEnvelope) => {
												if (seen_control_ids.has(envelope.message_id)) {
													return Effect.fail(
														server_error(
															"correlation_conflict",
															new Error(
																"message id was reused on one reliable connection",
															),
														),
													);
												}

												seen_control_ids.add(envelope.message_id);

												return connection
													.Receive(envelope)
													.pipe(
														envelope.kind === "ack"
															? Effect.andThen(
																	AcknowledgeEventWindow(
																		envelope.payload
																			.journal_sequence,
																	),
																)
															: (effect) => effect,
													);
											}),
										),
							),
						),
					);

					const control_outbound = connection.Outbound.pipe(
						Stream.runForEach((envelope: OutboundControlEnvelope) =>
							Effect.gen(function* () {
								if (envelope.kind === "event") {
									yield* ReserveEventWindow(envelope.journal_sequence);
								}
								if (envelope.kind === "welcome") {
									yield* Ref.set(
										stream_ticket,
										Option.some(envelope.payload.stream_ticket),
									);
								}

								yield* ports.control_port
									.Send({
										connection_id,
										kind: "transport.control",
										payload: envelope,
										transport_version: SupportedTransportVersions[0],
									})
									.pipe(Effect.mapError(map_server_port_error));
							}),
						),
					);

					const stream_inbound = Effect.forever(
						ports.stream_port.Receive.pipe(
							Effect.mapError(map_server_port_error),
							Effect.flatMap(decode_stream_frame),
							Effect.flatMap((frame) => stream_channel.Handle(frame.frame)),
						),
					);

					const guarded_control = control_inbound.pipe(
						Effect.tapError((error) =>
							send_transport_error(
								ports.control_port,
								"control",
								`transport.${error.code}`,
								"The control port rejected an invalid or stale frame.",
							),
						),
					);
					const guarded_stream = stream_inbound.pipe(
						Effect.tapError((error) =>
							send_transport_error(
								ports.stream_port,
								"stream",
								`transport.${error.code}`,
								"The stream port rejected an invalid or stale frame.",
							),
						),
					);

					return yield* Effect.raceAllFirst([
						guarded_control,
						control_outbound,
						guarded_stream,
						connection.Closed,
					]);
				}),
			);

		return { Serve: serve } satisfies MessagePortTransportServerShape;
	});
