import { Schema } from "effect";

import { CommandEnvelope, OutboundEnvelope } from "./messages";

export const DecodeCommandEnvelope = Schema.decodeUnknownEffect(CommandEnvelope);

export const EncodeOutboundEnvelope = Schema.encodeUnknownEffect(OutboundEnvelope);
