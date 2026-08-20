import { Context, Data, Effect, Layer, Redacted, Scope } from "effect";
import type { CapabilityDetail } from "@artisan/protocol";
import * as HttpClient from "effect/unstable/http/HttpClient";

import { HttpMcpDriver, inspect_http_mcp_endpoint, type HttpMcpEndpoint } from "./http-transport";
import { SecretStore, type SecretReference } from "./secret-store";
import { StdioMcpDriver, type StdioLaunch } from "./stdio-transport";

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

/**
 * Selects a concrete transport from the reviewed canonical record. Acquisition is
 * inert; callers must explicitly invoke Connect after approval is durable.
 */
export class CapabilityTransportRegistry extends Context.Service<
	CapabilityTransportRegistry,
	{
		readonly Connect: (
			detail: Pick<CapabilityDetail, "auth" | "transport">,
		) => Effect.Effect<McpClientSession, McpTransportError, Scope.Scope>;
	}
>()("Artisan/Marketplace/CapabilityTransportRegistry") {}

export interface CapabilityTransportConnector {
	readonly Connect: (
		detail: Pick<CapabilityDetail, "auth" | "transport">,
	) => Effect.Effect<McpClientSession, McpTransportError, Scope.Scope>;
}

/**
 * Builds the canonical transport selector without acquiring either connector.
 * Connector acquisition and the returned registry Layer are both inert; only
 * an explicit `Connect` effect can start a process or open a network socket.
 */
export function make_capability_transport_registry_layer(input: {
	readonly http: CapabilityTransportConnector;
	readonly stdio: CapabilityTransportConnector;
}) {
	return Layer.succeed(CapabilityTransportRegistry, {
		Connect: (detail) =>
			detail.transport.kind === "stdio"
				? input.stdio.Connect(detail)
				: input.http.Connect(detail),
	});
}

const DefaultStdioLimits = {
	invocation_timeout_ms: 30_000,
	max_message_bytes: 4 * 1024 * 1024,
	max_stderr_bytes: 1024 * 1024,
} as const;

const DefaultHttpLimits = {
	max_pagination_bytes: 16 * 1024 * 1024,
	max_pagination_items: 10_000,
	max_pagination_pages: 32,
	max_response_bytes: 4 * 1024 * 1024,
	timeout_ms: 30_000,
} as const;

const to_secret_reference = (reference: {
	readonly provider: string;
	readonly secret_id: string;
}) => `${reference.provider}:${reference.secret_id}` as SecretReference;

/**
 * Production selector for canonical reviewed capability details. Layer acquisition
 * only captures dependencies; process launch, secret resolution and network I/O
 * happen exclusively when Connect is evaluated in an approved scoped operation.
 */
export const CapabilityTransportRegistryLive = Layer.effect(
	CapabilityTransportRegistry,
	Effect.gen(function* () {
		const stdio = yield* StdioMcpDriver;
		const http = yield* HttpMcpDriver;
		const secrets = yield* SecretStore;
		const http_client = yield* HttpClient.HttpClient;
		return {
			Connect: (detail) =>
				Effect.gen(function* () {
					if (detail.transport.kind === "stdio") {
						if (detail.auth.kind !== "none")
							return yield* Effect.fail(
								new McpTransportError({ operation: "start", state: "closed" }),
							);
						const entries = yield* Effect.forEach(
							detail.transport.env ?? [],
							(binding) =>
								secrets.Get(to_secret_reference(binding.secret_ref)).pipe(
									Effect.map(
										(value) => [binding.name, Redacted.value(value)] as const,
									),
									Effect.mapError(
										() =>
											new McpTransportError({
												operation: "start",
												state: "closed",
											}),
									),
								),
						);
						const launch: StdioLaunch = {
							args: detail.transport.args,
							command: detail.transport.command,
							...(detail.transport.cwd === undefined
								? {}
								: { cwd: detail.transport.cwd }),
							...(entries.length === 0 ? {} : { env: Object.fromEntries(entries) }),
							invocation_timeout_ms:
								detail.transport.invocation_timeout_ms ??
								DefaultStdioLimits.invocation_timeout_ms,
							max_message_bytes:
								detail.transport.max_message_bytes ??
								DefaultStdioLimits.max_message_bytes,
							max_stderr_bytes:
								detail.transport.max_stderr_bytes ??
								DefaultStdioLimits.max_stderr_bytes,
							startup_timeout_ms: detail.transport.startup_timeout_ms,
						};
						return yield* stdio.Open(launch);
					}

					const endpoint: HttpMcpEndpoint = {
						url: detail.transport.url,
						timeout_ms: detail.transport.timeout_ms ?? DefaultHttpLimits.timeout_ms,
						max_response_bytes:
							detail.transport.max_response_bytes ??
							DefaultHttpLimits.max_response_bytes,
						max_pagination_bytes:
							detail.transport.max_pagination_bytes ??
							DefaultHttpLimits.max_pagination_bytes,
						max_pagination_items:
							detail.transport.max_pagination_items ??
							DefaultHttpLimits.max_pagination_items,
						max_pagination_pages:
							detail.transport.max_pagination_pages ??
							DefaultHttpLimits.max_pagination_pages,
					};
					if (!inspect_http_mcp_endpoint(endpoint).allowed)
						return yield* Effect.fail(
							new McpTransportError({ operation: "start", state: "closed" }),
						);
					const auth_header = yield* Effect.gen(function* () {
						if (detail.auth.kind === "none") return undefined;
						if (detail.auth.kind === "oauth" && detail.auth.token_ref === undefined)
							return yield* Effect.fail(
								new McpTransportError({ operation: "start", state: "closed" }),
							);
						const reference =
							detail.auth.kind === "oauth"
								? detail.auth.token_ref
								: detail.auth.secret_ref;
						if (reference === undefined)
							return yield* Effect.fail(
								new McpTransportError({ operation: "start", state: "closed" }),
							);
						const secret = yield* secrets.Get(to_secret_reference(reference)).pipe(
							Effect.mapError(
								() =>
									new McpTransportError({
										operation: "start",
										state: "closed",
									}),
							),
						);
						if (detail.auth.kind === "api_key")
							return { name: detail.auth.header_name, value: secret };
						return {
							name: "Authorization",
							value: Redacted.make(`Bearer ${Redacted.value(secret)}`),
						};
					});
					const session = yield* http.Connect(
						auth_header === undefined
							? { endpoint, http_client }
							: { auth_header, endpoint, http_client },
					);
					yield* Effect.addFinalizer(() => session.Close.pipe(Effect.ignore));
					return session;
				}),
		};
	}),
);

/** The default registry fails closed until an explicit stdio/HTTP binding is composed. */
export const EmptyCapabilityTransportRegistryLive = Layer.succeed(CapabilityTransportRegistry, {
	Connect: () => Effect.fail(new McpTransportError({ operation: "start", state: "closed" })),
});
