import { Cause, Deferred, Effect, Queue, Schema } from "effect";
import { RpcSerialization } from "effect/unstable/rpc";

import type { MessagePortAdapterOptions, MessagePortError, MessagePortLike } from "../message-port";
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
	readonly send: (data: Uint8Array) => void;
}

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

/** Adapts one channel on a shared socket into the established bounded MessagePort contract. */
export const make_websocket_message_port = (
	endpoint: WebSocketEndpoint,
	channel: WebSocketChannel,
	options: MessagePortAdapterOptions = {},
) => {
	const incoming_capacity = options.incoming_capacity ?? 256;

	return Effect.gen(function* () {
		if (!Number.isSafeInteger(incoming_capacity) || incoming_capacity <= 0) {
			return yield* Effect.fail(
				new MessagePortFailure({
					cause: new Error("incoming_capacity must be a positive safe integer"),
					code: "configuration",
					dropped_messages: 0,
				}),
			);
		}

		const raw_frames = yield* Effect.acquireRelease(
			Queue.dropping<unknown, MessagePortError>(incoming_capacity),
			Queue.shutdown,
		);
		const incoming = yield* Effect.acquireRelease(
			Queue.dropping<unknown, MessagePortError>(incoming_capacity),
			Queue.shutdown,
		);
		const closed = yield* Deferred.make<{
			readonly code: "closed" | "message_error" | "overflow";
			readonly dropped_messages: number;
		}>();
		const Fail = (code: "message_error" | "overflow", cause: unknown) =>
			new MessagePortFailure({ cause, code, dropped_messages: code === "overflow" ? 1 : 0 });
		const Finish = (code: "closed" | "message_error" | "overflow", cause?: unknown) =>
			Effect.sync(() => {
				Deferred.doneUnsafe(
					closed,
					Effect.succeed({
						code,
						dropped_messages: code === "overflow" ? 1 : 0,
					}),
				);
				if (cause !== undefined) {
					Queue.failCauseUnsafe(
						incoming,
						Cause.fail(Fail(code as "message_error" | "overflow", cause)),
					);
				}
			});

		const remove_message = endpoint.add_message_listener((raw) => {
			if (!Queue.offerUnsafe(raw_frames, raw)) {
				Queue.failCauseUnsafe(
					raw_frames,
					Cause.fail(Fail("overflow", new Error("WebSocket frame buffer overflowed"))),
				);
			}
		});
		const remove_error = endpoint.add_error_listener((cause) => {
			Queue.failCauseUnsafe(raw_frames, Cause.fail(Fail("message_error", cause)));
		});
		const remove_close = endpoint.add_close_listener(() => {
			Queue.failCauseUnsafe(
				raw_frames,
				Cause.fail(Fail("message_error", new Error("WebSocket closed"))),
			);
		});
		yield* Effect.addFinalizer(() =>
			Effect.sync(() => {
				remove_message();
				remove_error();
				remove_close();
			}).pipe(
				Effect.andThen(Finish("closed")),
				Effect.tap(() => Effect.sync(endpoint.close)),
			),
		);

		yield* Effect.forever(
			Queue.take(raw_frames).pipe(
				Effect.flatMap(DecodeWebSocketEnvelope),
				Effect.flatMap((envelope) =>
					envelope.channel === channel
						? Queue.offer(incoming, envelope.payload)
						: Effect.void,
				),
			),
		).pipe(
			Effect.catchCause((cause) => Finish("message_error", Cause.squash(cause))),
			Effect.forkScoped,
		);

		const Close = Finish("closed").pipe(Effect.tap(() => Effect.sync(endpoint.close)));
		const Send = (message: unknown) =>
			EncodeWebSocketTransportFrame(channel, message).pipe(
				Effect.flatMap((encoded) =>
					Effect.try({
						try: () => endpoint.send(encoded),
						catch: (cause) =>
							new MessagePortFailure({ cause, code: "send", dropped_messages: 0 }),
					}),
				),
				Effect.mapError((cause) =>
					cause instanceof MessagePortFailure
						? cause
						: new MessagePortFailure({ cause, code: "send", dropped_messages: 0 }),
				),
			);

		return {
			Close,
			Closed: Deferred.await(closed),
			Receive: Queue.take(incoming),
			Send,
		} satisfies MessagePortLike;
	});
};

/** Opens the two existing logical transport ports over one shared WebSocket endpoint. */
export const MakeWebSocketConnection = (
	endpoint: WebSocketEndpoint,
	options: MessagePortAdapterOptions = {},
): Effect.Effect<MessagePortConnection, MessagePortError, import("effect").Scope.Scope> =>
	Effect.gen(function* () {
		const control_port: MessagePortLike = yield* make_websocket_message_port(
			endpoint,
			"control",
			options,
		);
		const stream_port: MessagePortLike = yield* make_websocket_message_port(
			endpoint,
			"stream",
			options,
		);

		return { control_port, stream_port };
	});
