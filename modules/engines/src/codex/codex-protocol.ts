import { Data, Schema } from "effect";

/** Pins the observed Codex CLI transport contract used by this adapter. @since 0.1.0 */
export const CodexTransportMetadata = {
	cli_version: "0.142.5",
	initialize_method: "initialize",
	protocol_version: "v1",
	transport: "stdio-jsonl",
} as const;

/** Describes the JSON-RPC request identifier used by Codex app-server. @since 0.1.0 */
export const CodexRequestId = Schema.Union([Schema.Int, Schema.String]);

/** Represents a JSON-RPC request identifier used by Codex app-server. @since 0.3.0 */
export type CodexRequestId = Schema.Schema.Type<typeof CodexRequestId>;

/** Describes the error object carried by a JSON-RPC error response. @since 0.3.0 */
export const CodexJsonRpcError = Schema.Struct({
	code: Schema.Int,
	data: Schema.optional(Schema.Unknown),
	message: Schema.String,
});

/** Represents a typed JSON-RPC error response payload. @since 0.3.0 */
export type CodexJsonRpcError = Schema.Schema.Type<typeof CodexJsonRpcError>;

/** Describes a successful Codex JSON-RPC envelope without requiring jsonrpc. @since 0.3.0 */
export const CodexJsonRpcSuccessEnvelope = Schema.Struct({
	id: CodexRequestId,
	jsonrpc: Schema.optional(Schema.Literal("2.0")),
	result: Schema.Unknown,
});

/** Represents a successful Codex JSON-RPC envelope. @since 0.3.0 */
export type CodexJsonRpcSuccessEnvelope = Schema.Schema.Type<typeof CodexJsonRpcSuccessEnvelope>;

/** Describes an error Codex JSON-RPC envelope without requiring jsonrpc. @since 0.3.0 */
export const CodexJsonRpcErrorEnvelope = Schema.Struct({
	error: CodexJsonRpcError,
	id: CodexRequestId,
	jsonrpc: Schema.optional(Schema.Literal("2.0")),
});

/** Represents an error Codex JSON-RPC envelope. @since 0.3.0 */
export type CodexJsonRpcErrorEnvelope = Schema.Schema.Type<typeof CodexJsonRpcErrorEnvelope>;

/** Represents either valid response envelope accepted by the Codex transport. @since 0.3.0 */
export type CodexJsonRpcResponseEnvelope = CodexJsonRpcErrorEnvelope | CodexJsonRpcSuccessEnvelope;

/** Describes a server-initiated JSON-RPC request with a correlated identifier. @since 0.3.0 */
export const CodexJsonRpcServerRequestEnvelope = Schema.Struct({
	id: CodexRequestId,
	jsonrpc: Schema.optional(Schema.Literal("2.0")),
	method: Schema.NonEmptyString,
	params: Schema.optional(Schema.Unknown),
});

/** Represents a validated server-initiated JSON-RPC request. @since 0.3.0 */
export type CodexJsonRpcServerRequestEnvelope = Schema.Schema.Type<
	typeof CodexJsonRpcServerRequestEnvelope
>;

/** Describes a server JSON-RPC notification without a request identifier. @since 0.3.0 */
export const CodexJsonRpcNotificationEnvelope = Schema.Struct({
	jsonrpc: Schema.optional(Schema.Literal("2.0")),
	method: Schema.NonEmptyString,
	params: Schema.optional(Schema.Unknown),
});

/** Represents a validated server JSON-RPC notification. @since 0.3.0 */
export type CodexJsonRpcNotificationEnvelope = Schema.Schema.Type<
	typeof CodexJsonRpcNotificationEnvelope
>;

/** Describes every inbound JSON-RPC envelope routed by the app-server session. @since 0.3.0 */
export const CodexInboundEnvelope = Schema.Union([
	CodexJsonRpcSuccessEnvelope,
	CodexJsonRpcErrorEnvelope,
	CodexJsonRpcServerRequestEnvelope,
	CodexJsonRpcNotificationEnvelope,
]);

/** Represents every validated inbound JSON-RPC envelope. @since 0.3.0 */
export type CodexInboundEnvelope = Schema.Schema.Type<typeof CodexInboundEnvelope>;

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

/** Describes the minimal account/read request; future server parameters stay raw. @since 0.3.0 */
export const CodexAccountReadRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal("account/read"),
	params: Schema.Unknown,
});

/** Describes a minimal thread/start request while preserving the server-owned parameter shape. @since 0.3.0 */
export const CodexThreadStartRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal("thread/start"),
	params: Schema.Unknown,
});

/** Describes a minimal thread/resume request while preserving the server-owned parameter shape. @since 0.3.0 */
export const CodexThreadResumeRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal("thread/resume"),
	params: Schema.Unknown,
});

/** Describes a minimal turn/start request while preserving the server-owned parameter shape. @since 0.3.0 */
export const CodexTurnStartRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal("turn/start"),
	params: Schema.Unknown,
});

/** Describes a minimal turn/steer request while preserving the server-owned parameter shape. @since 0.3.0 */
export const CodexTurnSteerRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal("turn/steer"),
	params: Schema.Unknown,
});

/** Describes a minimal turn/interrupt request while preserving the server-owned parameter shape. @since 0.3.0 */
export const CodexTurnInterruptRequest = Schema.Struct({
	id: CodexRequestId,
	method: Schema.Literal("turn/interrupt"),
	params: Schema.Unknown,
});

/** Describes the stable approval decisions available to the next engine slice. @since 0.3.0 */
export const CodexApprovalDecision = Schema.Literals([
	"abort",
	"approved",
	"approved_for_session",
	"denied",
	"timed_out",
]);

/** Describes the simple approval response primitive accepted by server requests. @since 0.3.0 */
export const CodexApprovalResponse = Schema.Struct({
	decision: CodexApprovalDecision,
});

/** Describes a response to a server-initiated request for user input. @since 0.3.0 */
export const CodexQuestionResponse = Schema.Struct({
	answers: Schema.Record(
		Schema.String,
		Schema.Struct({
			answers: Schema.Array(Schema.String),
		}),
	),
});

/** Represents a validated Codex initialize request. @since 0.1.0 */
export type CodexInitializeRequest = Schema.Schema.Type<typeof CodexInitializeRequest>;

/** Represents a validated Codex initialize response. @since 0.1.0 */
export type CodexInitializeResponse = Schema.Schema.Type<typeof CodexInitializeResponse>;

/** Represents a validated Codex account/read request. @since 0.3.0 */
export type CodexAccountReadRequest = Schema.Schema.Type<typeof CodexAccountReadRequest>;

/** Represents a stable approval decision accepted by Codex 0.142.5. @since 0.3.0 */
export type CodexApprovalDecision = Schema.Schema.Type<typeof CodexApprovalDecision>;

/** Represents a response to a server-initiated approval request. @since 0.3.0 */
export type CodexApprovalResponse = Schema.Schema.Type<typeof CodexApprovalResponse>;

/** Represents a response to a server-initiated user-input request. @since 0.3.0 */
export type CodexQuestionResponse = Schema.Schema.Type<typeof CodexQuestionResponse>;

/** Reports an app-server error response correlated to a client request. @since 0.3.0 */
export class CodexAppServerResponseError extends Data.TaggedError("CodexAppServerResponseError")<{
	readonly error: CodexJsonRpcError;
	readonly id: CodexRequestId;
	readonly method: string;
}> {}

/** Reports a client request that exceeded its configured response deadline. @since 0.3.0 */
export class CodexAppServerRequestTimeoutError extends Data.TaggedError(
	"CodexAppServerRequestTimeoutError",
)<{
	readonly id: CodexRequestId;
	readonly method: string;
	readonly timeout_ms: number;
}> {}

/** Reports a session closing before a pending request received its response. @since 0.3.0 */
export class CodexAppServerClosedError extends Data.TaggedError("CodexAppServerClosedError")<{
	readonly reason: "closed" | "exited" | "notification_overflow" | "reader_failed";
}> {}

/** Reports terminal lossless-notification ingress exhaustion. @since 0.3.0 */
export class CodexAppServerNotificationOverflowError extends Data.TaggedError(
	"CodexAppServerNotificationOverflowError",
)<{
	readonly capacity: number;
}> {}

/** Reports an outbound value that cannot be represented faithfully as JSON. @since 0.3.0 */
export class CodexAppServerSerializationError extends Data.TaggedError(
	"CodexAppServerSerializationError",
)<{
	readonly message: string;
	readonly operation: "notify" | "request" | "respond";
}> {}

/** Reports a malformed response envelope or handshake result from the app-server. @since 0.3.0 */
export class CodexAppServerProtocolError extends Data.TaggedError("CodexAppServerProtocolError")<{
	readonly message: string;
}> {}

/** Reports invalid bounded transport settings before a session opens. @since 0.3.0 */
export class CodexAppServerConfigurationError extends Data.TaggedError(
	"CodexAppServerConfigurationError",
)<{
	readonly option: string;
	readonly value: number;
}> {}

/** Decodes unknown JSON into a validated Codex initialize response. @since 0.1.0 */
export const DecodeCodexInitializeResponse = Schema.decodeUnknownEffect(CodexInitializeResponse);

/** Strictly decodes one inbound Codex JSON-RPC envelope. @since 0.3.0 */
export const DecodeCodexInboundEnvelope = Schema.decodeUnknownEffect(CodexInboundEnvelope, {
	onExcessProperty: "error",
});

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

/**
 * Builds the request envelope used to read the Codex account state.
 *
 * @since 0.3.0
 * @param id - Unique request identifier assigned by the transport.
 * @returns The minimal account/read request accepted by Codex 0.142.5.
 */
export function make_codex_account_read_request(id: CodexRequestId): CodexAccountReadRequest {
	return { id, method: "account/read", params: {} };
}

/**
 * Builds a response envelope for a server-initiated approval or question request.
 *
 * @since 0.3.0
 * @param id - Server request identifier being answered.
 * @param result - Provider-specific response payload preserved without normalization.
 * @returns A JSON-RPC success response that does not require a jsonrpc field.
 */
export function make_codex_server_response(
	id: CodexRequestId,
	result: unknown,
): CodexJsonRpcSuccessEnvelope {
	return { id, result };
}

/**
 * Builds a response to a server-initiated approval request.
 *
 * @since 0.3.0
 * @param id - Server request identifier being answered.
 * @param decision - Stable user decision selected for the request.
 * @returns A JSON-RPC response carrying the selected approval decision.
 */
export function make_codex_approval_response(
	id: CodexRequestId,
	decision: CodexApprovalDecision,
): CodexJsonRpcSuccessEnvelope {
	return make_codex_server_response(id, { decision });
}

/**
 * Builds a response to a server-initiated user-input request.
 *
 * @since 0.3.0
 * @param id - Server request identifier being answered.
 * @param answers - Answers grouped by the provider's request question identifier.
 * @returns A JSON-RPC response carrying the selected user-input answers.
 */
export function make_codex_question_response(
	id: CodexRequestId,
	answers: CodexQuestionResponse["answers"],
): CodexJsonRpcSuccessEnvelope {
	return make_codex_server_response(id, { answers });
}
