import { Cause, Deferred, Effect, Queue, Schema } from "effect";
import { RpcSerialization } from "effect/unstable/rpc";

import type { MessagePortClose, MessagePortError, MessagePortLike } from "../message-port";
import { MessagePortError as MessagePortFailure } from "../message-port";
import type { MessagePortConnection } from "../connector";
import { DecodeTransportFrame } from "../wire";

/** Keeps the existing reliable control and high-volume stream protocols distinct. */
export const WebSocketChannel = Schema.Literals(["control", "stream"]);
export type WebSocketChannel = typeof WebSocketChannel.Type;

const WebSocketEnvelope = Schema.Struct({
	channel: WebSocketChannel,
	payload: Schema.Unknown,
});

/** Minimal event surface common to browser WebSocket and Node WebSocket hosts. */
export interface WebSocketEndpoint {
	readonly add_close_listener: (listener: () => void) => () => void;
	readonly add_error_listener: (listener: (cause: unknown) => void) => () => void;
	readonly add_message_listener: (listener: (data: unknown) => void) => () => void;
	readonly close: () => void;
	readonly send: (data: Uint8Array) => void | Promise<void>;
}

/**
 * Browser message events can outrun MessagePack decoding by orders of
 * magnitude during a journal resume. Bound both the native ingress handoff and
 * each decoded logical lane so retained ArrayBuffers have a hard ceiling.
 */
export const websocket_frame_queue_capacity = 512;

const bytes = (input: unknown) => {
	if (input instanceof Uint8Array) {
		return input;
	}
	if (input instanceof ArrayBuffer) {
		return new Uint8Array(input);
	}
	if (ArrayBuffer.isView(input)) {
		return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
	}

	throw new Error("WebSocket transport accepts binary MessagePack frames only");
};

/** Parses and schema-validates an entire multiplexed WebSocket envelope. */
export const DecodeWebSocketEnvelope = (input: unknown) =>
	Effect.try({
		try: () => {
			const messages = RpcSerialization.msgPack.makeUnsafe().decode(bytes(input));

			if (messages.length !== 1) {
				throw new Error("Expected exactly one MessagePack WebSocket envelope");
			}

			return messages[0];
		},
		catch: (cause) => cause,
	}).pipe(
		Effect.flatMap(
			Schema.decodeUnknownEffect(WebSocketEnvelope, { onExcessProperty: "error" }),
		),
		Effect.flatMap((envelope) =>
			DecodeTransportFrame(envelope.payload).pipe(
				Effect.map((payload) => ({ channel: envelope.channel, payload })),
			),
		),
	);

/** Validates that an envelope belongs to the receiving logical channel. */
export const DecodeWebSocketTransportFrame = (input: unknown, channel: WebSocketChannel) =>
	DecodeWebSocketEnvelope(input).pipe(
		Effect.flatMap((envelope) =>
			envelope.channel === channel
				? Effect.succeed(envelope.payload)
				: Effect.fail(new Error(`WebSocket channel mismatch: ${envelope.channel}`)),
		),
	);

/** Serializes only a validated transport frame, preserving stream bytes without base64 at callers. */
export const EncodeWebSocketTransportFrame = (channel: WebSocketChannel, input: unknown) =>
	DecodeTransportFrame(input).pipe(
		Effect.flatMap((payload) =>
			Effect.sync(() => RpcSerialization.msgPack.makeUnsafe().encode({ channel, payload })),
		),
		Effect.flatMap((encoded) =>
			encoded instanceof Uint8Array
				? Effect.succeed(encoded)
				: Effect.fail(new Error("MessagePack encoder did not return a binary frame")),
		),
	);

/** Adapts one channel on a shared socket into the established MessagePort contract. */
const MakeWebSocketMultiplexer = (
	endpoint: WebSocketEndpoint,
	accepted_channel?: WebSocketChannel,
) => {
	return Effect.gen(function* () {
		const raw_frames = yield* Effect.acquireRelease(
			Queue.bounded<unknown, MessagePortError>(websocket_frame_queue_capacity),
			Queue.shutdown,
		);
		const control_incoming = yield* Effect.acquireRelease(
			Queue.bounded<unknown, MessagePortError>(websocket_frame_queue_capacity),
			Queue.shutdown,
		);
		const stream_incoming = yield* Effect.acquireRelease(
			Queue.bounded<unknown, MessagePortError>(websocket_frame_queue_capacity),
			Queue.shutdown,
		);
		const closed = yield* Deferred.make<MessagePortClose>();
		let close_state: MessagePortClose | undefined;
		let socket_closed = false;
		const Fail = (code: "message_error", cause: unknown, dropped_messages = 0) =>
			new MessagePortFailure({ cause, code, dropped_messages });
		const close_socket = () => {
			if (!socket_closed) {
				socket_closed = true;
				try {
					endpoint.close();
				} catch {
					/** Socket close is best-effort after local transport state is closed. */
				}
			}
		};
		const finish = (
			state: MessagePortClose,
			failure?: MessagePortError,
			close_endpoint = false,
		) => {
			if (close_state) {
				return;
			}

			close_state = state;
			Deferred.doneUnsafe(closed, Effect.succeed(state));
			const queue_failure =
				failure ??
				new MessagePortFailure({
					cause: new Error("WebSocket closed"),
					code: "closed",
					dropped_messages: state.dropped_messages,
				});
			const cause = Cause.fail(queue_failure);
			Queue.failCauseUnsafe(raw_frames, cause);
			Queue.failCauseUnsafe(control_incoming, cause);
			Queue.failCauseUnsafe(stream_incoming, cause);

			if (close_endpoint) {
				close_socket();
			}
		};

		const remove_message = endpoint.add_message_listener((raw) => {
			if (close_state || Queue.offerUnsafe(raw_frames, raw)) return;
			const dropped_messages = 1;
			const failure = Fail(
				"message_error",
				new Error("WebSocket ingress exceeded its bounded frame queue"),
				dropped_messages,
			);
			finish({ code: "message_error", dropped_messages }, failure, true);
		});
		const remove_error = endpoint.add_error_listener((cause) => {
			const failure = Fail("message_error", cause);
			finish({ code: "message_error", dropped_messages: 0 }, failure, true);
		});
		const remove_close = endpoint.add_close_listener(() => {
			finish({ code: "closed", dropped_messages: 0 });
		});
		yield* Effect.addFinalizer(() =>
			Effect.sync(() => {
				remove_message();
				remove_error();
				remove_close();
				finish({ code: "closed", dropped_messages: 0 }, undefined, true);
			}),
		);

		yield* Effect.forever(
			Queue.take(raw_frames).pipe(
				Effect.flatMap(DecodeWebSocketEnvelope),
				Effect.flatMap((envelope) =>
					Effect.suspend(() => {
						if (
							accepted_channel !== undefined &&
							envelope.channel !== accepted_channel
						) {
							return Effect.void;
						}
						const incoming =
							envelope.channel === "control" ? control_incoming : stream_incoming;
						return Queue.offer(incoming, envelope.payload).pipe(Effect.asVoid);
					}),
				),
			),
		).pipe(
			Effect.catchCause((cause) =>
				Cause.hasInterruptsOnly(cause)
					? Effect.void
					: Effect.sync(() => {
							const failure = Fail("message_error", Cause.squash(cause));
							finish({ code: "message_error", dropped_messages: 0 }, failure, true);
						}),
			),
			Effect.forkScoped,
		);

		const Close = Effect.sync(() =>
			finish({ code: "closed", dropped_messages: 0 }, undefined, true),
		);
		const MakePort = (
			channel: WebSocketChannel,
			incoming: Queue.Queue<unknown, MessagePortError>,
		): MessagePortLike => ({
			Close,
			Closed: Deferred.await(closed),
			Receive: Queue.take(incoming),
			Send: (message: unknown) =>
				EncodeWebSocketTransportFrame(channel, message).pipe(
					Effect.flatMap((encoded) =>
						Effect.tryPromise({
							try: async () => {
								if (close_state) {
									throw new MessagePortFailure({
										cause: new Error("cannot send after WebSocket close"),
										code: "closed",
										dropped_messages: close_state.dropped_messages,
									});
								}
								await endpoint.send(encoded);
							},
							catch: (cause) =>
								cause instanceof MessagePortFailure
									? cause
									: new MessagePortFailure({
											cause,
											code: "send",
											dropped_messages: 0,
										}),
						}),
					),
					Effect.mapError((cause) =>
						cause instanceof MessagePortFailure
							? cause
							: new MessagePortFailure({ cause, code: "send", dropped_messages: 0 }),
					),
				),
		});

		return {
			control_port: MakePort("control", control_incoming),
			stream_port: MakePort("stream", stream_incoming),
		} satisfies MessagePortConnection;
	});
};

/**
 * Adapts one channel on a socket into the established MessagePort contract.
 * A socket requiring both channels must use MakeWebSocketConnection so it has one owner.
 */
export const make_websocket_message_port = (
	endpoint: WebSocketEndpoint,
	channel: WebSocketChannel,
) =>
	MakeWebSocketMultiplexer(endpoint, channel).pipe(
		Effect.map((connection) =>
			channel === "control" ? connection.control_port : connection.stream_port,
		),
	);

/** Opens the two existing logical transport ports over one shared WebSocket endpoint. */
export const MakeWebSocketConnection = (
	endpoint: WebSocketEndpoint,
): Effect.Effect<MessagePortConnection, MessagePortError, import("effect").Scope.Scope> =>
	MakeWebSocketMultiplexer(endpoint);
