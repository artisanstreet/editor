import { Effect, Schema } from "effect";

import { Identifier, StreamSequence } from "@artisan/protocol";

/** Lists transport versions implemented by this MessagePort package. */
export const SupportedTransportVersions = [1] as const;

/** Validates the transport version carried by every MessagePort frame. */
export const TransportVersion = Schema.Literals(SupportedTransportVersions);

/** Starts one versioned control or stream-port bootstrap. */
export const TransportHelloFrame = Schema.Struct({
	attempt_id: Identifier,
	channel: Schema.Literals(["control", "stream"]),
	kind: Schema.Literal("transport.hello"),
	session_id: Identifier,
	transport_version: TransportVersion,
});

export type TransportHelloFrame = typeof TransportHelloFrame.Type;

/** Confirms one paired port and fences later traffic to its connection id. */
export const TransportReadyFrame = Schema.Struct({
	attempt_id: Identifier,
	channel: Schema.Literals(["control", "stream"]),
	connection_id: Identifier,
	kind: Schema.Literal("transport.ready"),
	session_id: Identifier,
	transport_version: TransportVersion,
});

export type TransportReadyFrame = typeof TransportReadyFrame.Type;

/** Carries one existing protocol envelope on the isolated control port. */
export const TransportControlFrame = Schema.Struct({
	connection_id: Identifier,
	kind: Schema.Literal("transport.control"),
	payload: Schema.Unknown,
	transport_version: TransportVersion,
});

export type TransportControlFrame = typeof TransportControlFrame.Type;

/** Reports a transport-level validation or lifecycle failure. */
export const TransportErrorFrame = Schema.Struct({
	attempt_id: Schema.optional(Identifier),
	channel: Schema.Literals(["control", "stream"]),
	code: Identifier,
	connection_id: Schema.optional(Identifier),
	kind: Schema.Literal("transport.error"),
	message: Schema.NonEmptyString,
	retryable: Schema.Boolean,
	transport_version: TransportVersion,
});

export type TransportErrorFrame = typeof TransportErrorFrame.Type;

/** Announces an intentional transport session close without packet acknowledgements. */
export const TransportCloseFrame = Schema.Struct({
	connection_id: Identifier,
	kind: Schema.Literal("transport.close"),
	reason: Identifier,
	transport_version: TransportVersion,
});

export type TransportCloseFrame = typeof TransportCloseFrame.Type;

/** Requests one terminal or asset byte stream after protocol negotiation. */
export const MessagePortStreamBindFrame = Schema.Struct({
	channel_id: Identifier,
	channel_sequence: Schema.Literal(0),
	kind: Schema.Literal("stream.bind"),
	stream_id: Identifier,
	stream_ticket: Identifier,
});

export type MessagePortStreamBindFrame = typeof MessagePortStreamBindFrame.Type;

/** Confirms one logical byte stream and begins its ordered sequence. */
export const MessagePortStreamReadyFrame = Schema.Struct({
	channel_id: Identifier,
	channel_sequence: Schema.Literal(0),
	kind: Schema.Literal("stream.ready"),
	stream_id: Identifier,
});

export type MessagePortStreamReadyFrame = typeof MessagePortStreamReadyFrame.Type;

/** Carries one structured-clone Uint8Array without base64 expansion. */
export const MessagePortStreamChunkFrame = Schema.Struct({
	channel_id: Identifier,
	channel_sequence: StreamSequence,
	data: Schema.Uint8Array,
	kind: Schema.Literal("stream.chunk"),
	stream_id: Identifier,
});

export type MessagePortStreamChunkFrame = typeof MessagePortStreamChunkFrame.Type;

/** Closes one logical stream explicitly after completion, cancellation, or a gap. */
export const MessagePortStreamEndFrame = Schema.Struct({
	channel_id: Identifier,
	channel_sequence: StreamSequence,
	kind: Schema.Literal("stream.end"),
	reason: Schema.Literals([
		"cancelled",
		"closed",
		"completed",
		"gap",
		"not_found",
		"overflow",
		"source_error",
	]),
	stream_id: Identifier,
});

export type MessagePortStreamEndFrame = typeof MessagePortStreamEndFrame.Type;

/** Unions every logical frame allowed on the high-volume stream port. */
export const MessagePortStreamFrame = Schema.Union([
	MessagePortStreamBindFrame,
	MessagePortStreamReadyFrame,
	MessagePortStreamChunkFrame,
	MessagePortStreamEndFrame,
]).pipe(Schema.toTaggedUnion("kind"));

export type MessagePortStreamFrame = typeof MessagePortStreamFrame.Type;

/** Carries one logical byte-stream frame on its connection-fenced port. */
export const TransportStreamFrame = Schema.Struct({
	connection_id: Identifier,
	frame: MessagePortStreamFrame,
	kind: Schema.Literal("transport.stream"),
	transport_version: TransportVersion,
});

export type TransportStreamFrame = typeof TransportStreamFrame.Type;

/** Unions every MessagePort transport frame for shell-neutral decoding. */
export const TransportFrame = Schema.Union([
	TransportHelloFrame,
	TransportReadyFrame,
	TransportControlFrame,
	TransportStreamFrame,
	TransportErrorFrame,
	TransportCloseFrame,
]).pipe(Schema.toTaggedUnion("kind"));

export type TransportFrame = typeof TransportFrame.Type;

const DecodeStreamFrameKind = Schema.decodeUnknownEffect(
	Schema.Struct({
		kind: Schema.Literals(["stream.bind", "stream.chunk", "stream.end", "stream.ready"]),
	}),
	{ onExcessProperty: "preserve" },
);

const DecodeTransportFrameKind = Schema.decodeUnknownEffect(
	Schema.Struct({
		kind: Schema.Literals([
			"transport.close",
			"transport.control",
			"transport.error",
			"transport.hello",
			"transport.ready",
			"transport.stream",
		]),
	}),
	{ onExcessProperty: "preserve" },
);

const TransportStreamEnvelope = Schema.Struct({
	connection_id: Identifier,
	frame: Schema.Unknown,
	kind: Schema.Literal("transport.stream"),
	transport_version: TransportVersion,
});

/** Strictly decodes one unknown structured-clone transport value. */
export const DecodeTransportFrame = (input: unknown) =>
	DecodeTransportFrameKind(input).pipe(
		Effect.flatMap((frame): Effect.Effect<TransportFrame, Schema.SchemaError> => {
			switch (frame.kind) {
				case "transport.close":
					return Schema.decodeUnknownEffect(TransportCloseFrame, {
						onExcessProperty: "error",
					})(input);
				case "transport.control":
					return Schema.decodeUnknownEffect(TransportControlFrame, {
						onExcessProperty: "error",
					})(input);
				case "transport.error":
					return Schema.decodeUnknownEffect(TransportErrorFrame, {
						onExcessProperty: "error",
					})(input);
				case "transport.hello":
					return Schema.decodeUnknownEffect(TransportHelloFrame, {
						onExcessProperty: "error",
					})(input);
				case "transport.ready":
					return Schema.decodeUnknownEffect(TransportReadyFrame, {
						onExcessProperty: "error",
					})(input);
				case "transport.stream":
					return Schema.decodeUnknownEffect(TransportStreamEnvelope, {
						onExcessProperty: "error",
					})(input).pipe(
						Effect.flatMap((envelope) =>
							DecodeMessagePortStreamFrame(envelope.frame).pipe(
								Effect.map((stream_frame) => ({
									...envelope,
									frame: stream_frame,
								})),
							),
						),
					);
			}
		}),
	);

/** Strictly decodes one logical high-volume stream frame. */
export const DecodeMessagePortStreamFrame = (input: unknown) =>
	DecodeStreamFrameKind(input).pipe(
		Effect.flatMap((frame): Effect.Effect<MessagePortStreamFrame, Schema.SchemaError> => {
			switch (frame.kind) {
				case "stream.bind":
					return Schema.decodeUnknownEffect(MessagePortStreamBindFrame, {
						onExcessProperty: "error",
					})(input);
				case "stream.chunk":
					return Schema.decodeUnknownEffect(MessagePortStreamChunkFrame, {
						onExcessProperty: "error",
					})(input);
				case "stream.end":
					return Schema.decodeUnknownEffect(MessagePortStreamEndFrame, {
						onExcessProperty: "error",
					})(input);
				case "stream.ready":
					return Schema.decodeUnknownEffect(MessagePortStreamReadyFrame, {
						onExcessProperty: "error",
					})(input);
			}
		}),
	);
