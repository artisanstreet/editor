import {
	Cause,
	Deferred,
	Effect,
	Exit,
	Fiber,
	Option,
	Queue,
	Ref,
	Schema,
	Scope,
	Stream,
} from "effect";

import type { MessagePortLike } from "../message-port";
import type { MessagePortTransportServerError } from "../server-contract";
import type { BinaryStreamSourceError, BinaryStreamSourceShape } from "../stream-source";
import {
	TerminalOutputGapDetail,
	type MessagePortStreamEndFrame,
	type MessagePortStreamFrame,
	type TransportStreamFrame,
} from "../wire";
import { map_server_port_error, server_error } from "./server-common";

interface ActiveBinaryStream {
	readonly fiber: Fiber.Fiber<void, never>;
	readonly stream_id: string;
}

/** Owns bounded logical stream lifecycles on one isolated MessagePort. */
export interface ServerStreamChannel {
	readonly Handle: (
		frame: MessagePortStreamFrame,
	) => Effect.Effect<void, MessagePortTransportServerError, Scope.Scope>;
}

function stream_end(
	channel_id: string,
	channel_sequence: number,
	stream_id: string,
	reason: MessagePortStreamEndFrame["reason"],
	terminal_output_gap?: TerminalOutputGapDetail,
): MessagePortStreamEndFrame {
	const decoded_gap = Schema.decodeUnknownOption(TerminalOutputGapDetail)(terminal_output_gap);
	const safe_terminal_output_gap =
		reason === "gap" && stream_id.startsWith("terminal:") && Option.isSome(decoded_gap)
			? decoded_gap.value
			: undefined;

	return {
		channel_id,
		channel_sequence,
		kind: "stream.end",
		reason,
		stream_id,
		...(safe_terminal_output_gap === undefined
			? {}
			: { terminal_output_gap: safe_terminal_output_gap }),
	};
}

function source_end_reason(error: BinaryStreamSourceError) {
	return error.code === "gap" ? "gap" : error.code === "not_found" ? "not_found" : "source_error";
}

/** Builds one session-scoped binary stream channel. */
export const make_server_stream_channel = (
	connection_id: string,
	stream_port: MessagePortLike,
	stream_source: BinaryStreamSourceShape,
	stream_ticket: Ref.Ref<Option.Option<string>>,
	max_active_streams: number,
	stream_outbound_capacity: number,
) =>
	Effect.gen(function* () {
		const active_streams = yield* Ref.make(new Map<string, ActiveBinaryStream>());

		const send = (frame: MessagePortStreamFrame) =>
			stream_port
				.Send({
					connection_id,
					frame,
					kind: "transport.stream",
					transport_version: 1,
				} satisfies TransportStreamFrame)
				.pipe(Effect.mapError(map_server_port_error));

		const run_stream = (
			channel_id: string,
			stream_id: string,
			output: Stream.Stream<Uint8Array, BinaryStreamSourceError>,
		) =>
			Effect.scoped(
				Effect.gen(function* () {
					const queue = yield* Effect.acquireRelease(
						Queue.dropping<MessagePortStreamFrame>(stream_outbound_capacity),
						Queue.shutdown,
					);
					const end_sent = yield* Deferred.make<void>();
					const ready_sent = yield* Deferred.make<void>();
					let sequence = 0;
					const writer = Effect.gen(function* () {
						while (true) {
							const frame = yield* Queue.take(queue);

							yield* send(frame);

							if (frame.kind === "stream.ready") {
								yield* Deferred.succeed(ready_sent, undefined);
							}

							if (frame.kind === "stream.end") {
								yield* Deferred.succeed(end_sent, undefined);

								return;
							}
						}
					});

					yield* writer.pipe(Effect.forkScoped);
					yield* Queue.offer(queue, {
						channel_id,
						channel_sequence: 0,
						kind: "stream.ready",
						stream_id,
					});
					yield* Deferred.await(ready_sent);

					const enqueue_end = (
						reason: MessagePortStreamEndFrame["reason"],
						terminal_output_gap?: TerminalOutputGapDetail,
					) =>
						Effect.gen(function* () {
							sequence += 1;
							const offered = yield* Queue.offer(
								queue,
								stream_end(
									channel_id,
									sequence,
									stream_id,
									reason,
									terminal_output_gap,
								),
							);

							if (offered) {
								return;
							}

							yield* Queue.takeAll(queue);
							yield* Queue.offer(
								queue,
								stream_end(channel_id, sequence, stream_id, "overflow"),
							);
						});

					const enqueue_chunk = (data: Uint8Array) =>
						Effect.gen(function* () {
							sequence += 1;
							const offered = yield* Queue.offer(queue, {
								channel_id,
								channel_sequence: sequence,
								data: data.slice(),
								kind: "stream.chunk",
								stream_id,
							});

							if (offered) {
								return;
							}

							yield* Queue.takeAll(queue);
							sequence += 1;
							yield* Queue.offer(
								queue,
								stream_end(channel_id, sequence, stream_id, "overflow"),
							);

							return yield* Effect.fail(
								server_error(
									"stream_overflow",
									new Error("logical stream outbound queue overflowed"),
								),
							);
						});

					const exit = yield* Effect.exit(output.pipe(Stream.runForEach(enqueue_chunk)));

					if (Exit.isSuccess(exit)) {
						yield* enqueue_end("completed");
					} else {
						const failure = Cause.findErrorOption(exit.cause);
						const overflowed =
							Option.isSome(failure) &&
							failure.value._tag === "MessagePortTransportServerError" &&
							failure.value.code === "stream_overflow";

						if (!overflowed) {
							const source_failure =
								Option.isSome(failure) &&
								failure.value._tag === "BinaryStreamSourceError"
									? failure.value
									: undefined;

							yield* enqueue_end(
								source_failure === undefined
									? "source_error"
									: source_end_reason(source_failure),
								source_failure?.terminal_output_gap,
							);
						}
					}

					yield* Deferred.await(end_sent);
				}),
			).pipe(
				Effect.ensuring(
					Ref.update(active_streams, (current) => {
						const next = new Map(current);

						next.delete(channel_id);

						return next;
					}),
				),
				Effect.catchCause(() => Effect.void),
			);

		const bind = (frame: Extract<MessagePortStreamFrame, { kind: "stream.bind" }>) =>
			Effect.gen(function* () {
				const ticket = yield* Ref.get(stream_ticket);
				const active = yield* Ref.get(active_streams);

				if (Option.isNone(ticket) || ticket.value !== frame.stream_ticket) {
					return yield* Effect.fail(
						server_error(
							"malformed",
							new Error("stream bind did not carry the negotiated ticket"),
						),
					);
				}

				if (active.has(frame.channel_id)) {
					return yield* Effect.fail(
						server_error(
							"correlation_conflict",
							new Error("stream channel id is already active"),
						),
					);
				}

				if (active.size >= max_active_streams) {
					yield* send(stream_end(frame.channel_id, 0, frame.stream_id, "overflow"));

					return;
				}

				const opened = yield* Effect.exit(
					stream_source.Open({
						stream_id: frame.stream_id,
						...(frame.terminal_context === undefined
							? {}
							: { terminal_context: frame.terminal_context }),
					}),
				);

				if (Exit.isFailure(opened)) {
					const failure = Cause.findErrorOption(opened.cause);
					const reason = Option.isSome(failure)
						? source_end_reason(failure.value)
						: "source_error";

					yield* send(
						stream_end(
							frame.channel_id,
							0,
							frame.stream_id,
							reason,
							Option.isSome(failure) ? failure.value.terminal_output_gap : undefined,
						),
					);

					return;
				}

				const start = yield* Deferred.make<void>();
				const fiber = yield* Deferred.await(start).pipe(
					Effect.andThen(run_stream(frame.channel_id, frame.stream_id, opened.value)),
					Effect.forkScoped,
				);

				yield* Ref.update(active_streams, (current) =>
					new Map(current).set(frame.channel_id, {
						fiber,
						stream_id: frame.stream_id,
					}),
				);
				yield* Deferred.succeed(start, undefined);
			});

		const close = (frame: Extract<MessagePortStreamFrame, { kind: "stream.end" }>) =>
			Effect.gen(function* () {
				const active = yield* Ref.get(active_streams);
				const target = active.get(frame.channel_id);

				if (!target) {
					return;
				}

				if (target.stream_id !== frame.stream_id || frame.channel_sequence !== 1) {
					return yield* Effect.fail(
						server_error(
							"malformed",
							new Error("stream close does not match an active channel"),
						),
					);
				}

				yield* Fiber.interrupt(target.fiber);
				yield* send(stream_end(frame.channel_id, 0, frame.stream_id, "cancelled"));
			});

		const handle = (frame: MessagePortStreamFrame) => {
			switch (frame.kind) {
				case "stream.bind":
					return bind(frame);
				case "stream.end":
					return close(frame);
				default:
					return Effect.fail(
						server_error(
							"malformed",
							new Error("client sent a backend-only stream frame"),
						),
					);
			}
		};

		return { Handle: handle } satisfies ServerStreamChannel;
	});
