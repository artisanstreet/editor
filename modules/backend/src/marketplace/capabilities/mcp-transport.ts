import { Context, Data, Effect, Layer, Scope } from "effect";

export interface McpTool {
	readonly name: string;
	readonly description?: string;
	readonly input_schema: Readonly<Record<string, unknown>>;
}
export interface McpResource {
	readonly uri: string;
	readonly name: string;
	readonly description?: string;
}
export interface McpInitialize {
	readonly protocol_version: string;
	readonly server_name: string;
	readonly server_version?: string;
	readonly instructions?: string;
}
export interface McpToolCall {
	readonly name: string;
	readonly arguments: Readonly<Record<string, unknown>>;
}
export type McpHealth = "connected" | "crashed" | "closed" | "unhealthy";

/** A redaction-safe failure from an MCP transport. */
export class McpTransportError extends Data.TaggedError("McpTransportError")<{
	readonly operation:
		| "call_tool"
		| "close"
		| "health"
		| "initialize"
		| "list_resources"
		| "list_tools"
		| "start";
	readonly state: McpHealth;
}> {}

export interface McpClientSession {
	readonly Initialize: Effect.Effect<McpInitialize, McpTransportError>;
	readonly ListTools: Effect.Effect<ReadonlyArray<McpTool>, McpTransportError>;
	readonly ListResources: Effect.Effect<ReadonlyArray<McpResource>, McpTransportError>;
	readonly CallTool: (call: McpToolCall) => Effect.Effect<unknown, McpTransportError>;
	readonly Health: Effect.Effect<McpHealth, McpTransportError>;
	readonly Close: Effect.Effect<void, McpTransportError>;
}

/** Explicit connection boundary. It is intentionally inert during Layer acquisition. */
export class McpTransport extends Context.Service<
	McpTransport,
	{
		readonly Connect: () => Effect.Effect<McpClientSession, McpTransportError, Scope.Scope>;
	}
>()("Artisan/Marketplace/McpTransport") {}
export const EmptyMcpTransportLive = Layer.succeed(McpTransport, {
	Connect: () => Effect.fail(new McpTransportError({ operation: "start", state: "closed" })),
});
