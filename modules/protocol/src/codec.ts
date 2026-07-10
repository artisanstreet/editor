import { Schema } from "effect";

import { CommandEnvelope, InboundControlEnvelope, OutboundControlEnvelope } from "./control";

/** Decodes an unknown input as a complete inbound control-channel frame. */
export const DecodeInboundControlEnvelope = Schema.decodeUnknownEffect(InboundControlEnvelope, {
	onExcessProperty: "error",
});

/** Decodes an unknown input as the legacy command-only control frame. */
export const DecodeCommandEnvelope = Schema.decodeUnknownEffect(CommandEnvelope, {
	onExcessProperty: "error",
});

/** Encodes a complete outbound control-channel frame for a transport adapter. */
export const EncodeOutboundControlEnvelope = Schema.encodeUnknownEffect(OutboundControlEnvelope);

/** Encodes the legacy outbound control frame union for existing backend callers. */
export const EncodeOutboundEnvelope = EncodeOutboundControlEnvelope;
