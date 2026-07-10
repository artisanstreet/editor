import { Schema } from "effect";

/** Pins the observed Codex CLI transport contract used by this adapter. @since 0.1.0 */
export const CodexTransportMetadata = {
	cli_version: "0.142.5",
	initialize_method: "initialize",
	protocol_version: "v1",
	transport: "stdio-jsonl",
} as const;

/** Describes the JSON-RPC request identifier used by Codex app-server. @since 0.1.0 */
export const CodexRequestId = Schema.Union([Schema.Int, Schema.String]);

/** Describes client identity supplied to the Codex initialize handshake. @since 0.1.0 */
export const CodexClientInfo = Schema.Struct({
	name: Schema.NonEmptyString,
	title: Schema.optional(Schema.String),
	version: Schema.NonEmptyString,
});

/** Describes the supported Codex initialize capability declarations. @since 0.1.0 */
export const CodexInitializeCapabilities = Schema.Struct({
	experimentalApi: Schema.optional(Schema.Boolean),
	mcpServerOpenaiFormElicitation: Schema.optional(Schema.Boolean),
	optOutNotificationMethods: Schema.optional(Schema.NullOr(Schema.Array(Schema.String))),
	requestAttestation: Schema.optional(Schema.Boolean),
});

/** Describes the version-pinned JSON-RPC initialize request. @since 0.1.0 */
export const CodexInitializeRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal(CodexTransportMetadata.initialize_method),
	params: Schema.Struct({
		capabilities: Schema.optional(CodexInitializeCapabilities),
		clientInfo: CodexClientInfo,
	}),
});

/** Describes the result emitted by a successful Codex initialize request. @since 0.1.0 */
export const CodexInitializeResult = Schema.Struct({
	codexHome: Schema.NonEmptyString,
	platformFamily: Schema.NonEmptyString,
	platformOs: Schema.NonEmptyString,
	userAgent: Schema.NonEmptyString,
});

/** Describes the version-pinned JSON-RPC initialize response. @since 0.1.0 */
export const CodexInitializeResponse = Schema.Struct({
	id: CodexRequestId,
	result: CodexInitializeResult,
});

/** Represents a validated Codex initialize request. @since 0.1.0 */
export type CodexInitializeRequest = Schema.Schema.Type<typeof CodexInitializeRequest>;

/** Represents a validated Codex initialize response. @since 0.1.0 */
export type CodexInitializeResponse = Schema.Schema.Type<typeof CodexInitializeResponse>;

/** Decodes unknown JSON into a validated Codex initialize response. @since 0.1.0 */
export const DecodeCodexInitializeResponse = Schema.decodeUnknownEffect(CodexInitializeResponse);

/**
 * Builds the non-billable JSON-RPC initialize request for a Codex app-server.
 *
 * @since 0.1.0
 * @param client_name - Stable client name reported to the app-server.
 * @param client_version - Client version reported to the app-server.
 * @returns A request compatible with the observed 0.142.5 transport.
 */
export function make_codex_initialize_request(
	client_name: string,
	client_version: string,
): CodexInitializeRequest {
	return {
		id: 1,
		method: CodexTransportMetadata.initialize_method,
		params: {
			capabilities: {
				experimentalApi: false,
				requestAttestation: false,
			},
			clientInfo: {
				name: client_name,
				version: client_version,
			},
		},
	};
}
