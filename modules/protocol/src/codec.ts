import { Schema } from "effect";

import { CommandEnvelope, InboundControlEnvelope, OutboundControlEnvelope } from "./control";
import { StreamEnvelope } from "./stream";

/** Decodes an unknown input as a complete inbound control-channel frame. */
export const DecodeInboundControlEnvelope = Schema.decodeUnknownEffect(InboundControlEnvelope, {
	onExcessProperty: "error",
});

/** Decodes an unknown input as the legacy command-only control frame. */
export const DecodeCommandEnvelope = Schema.decodeUnknownEffect(CommandEnvelope, {
	onExcessProperty: "error",
});

/** Decodes an unknown backend frame for transport and compatibility tests. */
export const DecodeOutboundControlEnvelope = Schema.decodeUnknownEffect(OutboundControlEnvelope, {
	onExcessProperty: "error",
});

/** Encodes a complete outbound control-channel frame for a transport adapter. */
export const EncodeOutboundControlEnvelope = Schema.encodeUnknownEffect(OutboundControlEnvelope);

/** Encodes the legacy outbound control frame union for existing backend callers. */
export const EncodeOutboundEnvelope = EncodeOutboundControlEnvelope;

/** Decodes an unknown frame carried on the ephemeral stream channel. */
export const DecodeStreamEnvelope = Schema.decodeUnknownEffect(StreamEnvelope, {
	onExcessProperty: "error",
});

/** Encodes one ephemeral stream-channel frame for a transport adapter. */
export const EncodeStreamEnvelope = Schema.encodeUnknownEffect(StreamEnvelope);
