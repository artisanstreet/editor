import { Schema } from "effect";

import {
	Identifier,
	IsoDateTime,
	NegotiatedProtocolVersion,
	SchemaVersion,
	StreamSequence,
} from "./common";

const StreamTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literals(["frontend", "backend"]),
	protocol_version: NegotiatedProtocolVersion,
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

/** Encodes portable text and binary stream chunk data for transport adapters. */
export const StreamPayload = Schema.Union([
	Schema.Struct({
		data: Schema.String,
		encoding: Schema.Literal("utf8"),
	}),
	Schema.Struct({
		data: Schema.Uint8ArrayFromBase64,
		encoding: Schema.Literal("base64"),
	}),
]);

export type StreamPayload = typeof StreamPayload.Type;

/** Reserves a stream-channel binding frame without defining transport behavior. */
export const StreamBindEnvelope = Schema.Struct({
	...StreamTraceMetadata,
	channel_id: Identifier,
	channel_sequence: StreamSequence,
	kind: Schema.Literal("stream.bind"),
	payload: Schema.Struct({
		stream_id: Identifier,
	}),
});

export type StreamBindEnvelope = typeof StreamBindEnvelope.Type;

/** Reserves a stream readiness frame without defining transport behavior. */
export const StreamReadyEnvelope = Schema.Struct({
	...StreamTraceMetadata,
	channel_id: Identifier,
	channel_sequence: StreamSequence,
	kind: Schema.Literal("stream.ready"),
	payload: Schema.Struct({
		stream_id: Identifier,
	}),
});

export type StreamReadyEnvelope = typeof StreamReadyEnvelope.Type;

/** Reserves an ordered ephemeral data chunk frame without defining transport behavior. */
export const StreamChunkEnvelope = Schema.Struct({
	...StreamTraceMetadata,
	channel_id: Identifier,
	channel_sequence: StreamSequence,
	kind: Schema.Literal("stream.chunk"),
	payload: StreamPayload,
	stream_id: Identifier,
});

export type StreamChunkEnvelope = typeof StreamChunkEnvelope.Type;

/** Reserves a terminal stream frame without defining transport behavior. */
export const StreamEndEnvelope = Schema.Struct({
	...StreamTraceMetadata,
	channel_id: Identifier,
	channel_sequence: StreamSequence,
	kind: Schema.Literal("stream.end"),
	payload: Schema.Struct({
		reason: Schema.optional(Identifier),
	}),
	stream_id: Identifier,
});

export type StreamEndEnvelope = typeof StreamEndEnvelope.Type;

/** Represents every reserved V1 stream-channel frame. */
export const StreamEnvelope = Schema.Union([
	StreamBindEnvelope,
	StreamReadyEnvelope,
	StreamChunkEnvelope,
	StreamEndEnvelope,
]);

export type StreamEnvelope = typeof StreamEnvelope.Type;
