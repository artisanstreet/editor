import { Schema } from "effect";

import { Identifier, IsoDateTime, NegotiatedProtocolVersion, SchemaVersion } from "../common";

export const FrontendTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literal("frontend"),
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

export const BackendTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literal("backend"),
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

export const NegotiatedFrontendTraceMetadata = {
	...FrontendTraceMetadata,
	protocol_version: NegotiatedProtocolVersion,
};

export const NegotiatedBackendTraceMetadata = {
	...BackendTraceMetadata,
	protocol_version: NegotiatedProtocolVersion,
};
