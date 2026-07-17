import { Schema } from "effect";

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

/** Binds one terminal stream request to its asserted durable ownership context. */
export const MessagePortTerminalStreamContext = Schema.Struct({
	terminal_id: Identifier,
	thread_id: Identifier,
	workspace_id: Identifier,
});

export type MessagePortTerminalStreamContext = typeof MessagePortTerminalStreamContext.Type;

const MessagePortStreamBindFrameBase = Schema.Struct({
	channel_id: Identifier,
	channel_sequence: Schema.Literal(0),
	kind: Schema.Literal("stream.bind"),
	stream_id: Identifier,
	stream_ticket: Identifier,
	terminal_context: Schema.optional(MessagePortTerminalStreamContext),
});

/** Requests one terminal or asset byte stream after protocol negotiation. */
export const MessagePortStreamBindFrame = MessagePortStreamBindFrameBase.check(
	Schema.makeFilter<typeof MessagePortStreamBindFrameBase.Type>((frame) => {
		const terminal_prefix = "terminal:";
		const is_terminal = frame.stream_id.startsWith(terminal_prefix);

		if (!is_terminal) {
			return frame.terminal_context === undefined
				? undefined
				: "Terminal stream context is only valid for terminal streams";
		}

		if (frame.terminal_context === undefined) {
			return "Terminal streams require an ownership context";
		}

		return frame.terminal_context.terminal_id === frame.stream_id.slice(terminal_prefix.length)
			? undefined
			: "Terminal stream context must match the requested terminal id";
	}),
);

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

const TerminalOutputGapDetailBase = Schema.Struct({
	from_sequence: StreamSequence.check(Schema.isGreaterThan(0)),
	reason: Schema.Literal("viewer_overflow"),
	to_sequence: StreamSequence.check(Schema.isGreaterThan(0)),
});

/** Preserves the exact backend output range evicted for one terminal viewer. */
export const TerminalOutputGapDetail = TerminalOutputGapDetailBase.check(
	Schema.makeFilter<typeof TerminalOutputGapDetailBase.Type>((detail) =>
		detail.from_sequence <= detail.to_sequence
			? undefined
			: "Terminal output gap range must be ordered",
	),
);

export type TerminalOutputGapDetail = typeof TerminalOutputGapDetail.Type;

const MessagePortStreamEndFrameBase = Schema.Struct({
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
	terminal_output_gap: Schema.optional(TerminalOutputGapDetail),
});

/** Closes one logical stream explicitly after completion, cancellation, or a gap. */
export const MessagePortStreamEndFrame = MessagePortStreamEndFrameBase.check(
	Schema.makeFilter<typeof MessagePortStreamEndFrameBase.Type>((frame) => {
		if (frame.terminal_output_gap === undefined) {
			return undefined;
		}

		if (frame.reason !== "gap") {
			return "Terminal output gap detail requires a gap stream end reason";
		}

		return frame.stream_id.startsWith("terminal:")
			? undefined
			: "Terminal output gap detail is only valid for terminal streams";
	}),
);

export type MessagePortStreamEndFrame = typeof MessagePortStreamEndFrame.Type;

/** Unions every logical frame allowed on the high-volume stream port. */
export const MessagePortStreamFrame = Schema.Union([
	MessagePortStreamBindFrame,
	MessagePortStreamReadyFrame,
	MessagePortStreamChunkFrame,
	MessagePortStreamEndFrame,
]);

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
]);

export type TransportFrame = typeof TransportFrame.Type;

/** Strictly decodes one unknown structured-clone transport value. */
export const DecodeTransportFrame = Schema.decodeUnknownEffect(TransportFrame, {
	onExcessProperty: "error",
});

/** Strictly decodes one logical high-volume stream frame. */
export const DecodeMessagePortStreamFrame = Schema.decodeUnknownEffect(MessagePortStreamFrame, {
	onExcessProperty: "error",
});

/** Strictly decodes one terminal or asset stream binding request. */
export const DecodeMessagePortStreamBindFrame = Schema.decodeUnknownEffect(
	MessagePortStreamBindFrame,
	{ onExcessProperty: "error" },
);
