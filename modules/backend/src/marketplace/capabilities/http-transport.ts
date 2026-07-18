import { Context, Effect, Layer, Redacted, Ref, Schema, Stream } from "effect";
import * as HttpClient from "effect/unstable/http/HttpClient";
import * as FetchHttpClient from "effect/unstable/http/FetchHttpClient";
import * as HttpClientRequest from "effect/unstable/http/HttpClientRequest";
import type * as HttpClientResponse from "effect/unstable/http/HttpClientResponse";

import {
	McpTransport,
	McpTransportError,
	type McpClientSession,
	type McpHealth,
	type McpInitialize,
	type McpResource,
	type McpTool,
} from "./mcp-transport";
import { SecretStore, type SecretReference } from "./secret-store";

export interface HttpMcpEndpoint {
	readonly url: string;
	readonly auth?:
		| { readonly kind: "none" }
		| {
				readonly header_name: string;
				readonly kind: "secret_header";
				readonly secret_reference: SecretReference;
		  };
	readonly timeout_ms: number;
	readonly max_response_bytes: number;
	readonly max_pagination_bytes?: number;
	readonly max_pagination_items?: number;
	readonly max_pagination_pages?: number;
}

/** Renderer-safe endpoint trust metadata; callers must show a warning before approving broad local bindings. */
export interface HttpMcpEndpointPolicy {
	readonly allowed: boolean;
	readonly broad_local_binding_warning: boolean;
}

const JsonRpcError = Schema.Struct({
	code: Schema.Int,
	data: Schema.optional(Schema.Unknown),
	message: Schema.String,
});
const JsonRpcResponse = Schema.Struct({
	jsonrpc: Schema.Literal("2.0"),
	id: Schema.Int,
	result: Schema.optional(Schema.Unknown),
	error: Schema.optional(JsonRpcError),
});
const McpInitializeResult = Schema.Struct({
	protocolVersion: Schema.String,
	capabilities: Schema.Record(Schema.String, Schema.Unknown),
	serverInfo: Schema.Struct({
		name: Schema.String,
		version: Schema.optional(Schema.String),
	}),
	instructions: Schema.optional(Schema.String),
});
const McpToolResult = Schema.Struct({
	nextCursor: Schema.optional(Schema.String),
	tools: Schema.Array(
		Schema.Struct({
			_meta: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
			annotations: Schema.optional(Schema.Unknown),
			name: Schema.String,
			description: Schema.optional(Schema.String),
			execution: Schema.optional(Schema.Unknown),
			icons: Schema.optional(Schema.Array(Schema.Unknown)),
			inputSchema: Schema.Record(Schema.String, Schema.Unknown),
			outputSchema: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
			title: Schema.optional(Schema.String),
		}),
	),
});
const McpResourceResult = Schema.Struct({
	nextCursor: Schema.optional(Schema.String),
	resources: Schema.Array(
		Schema.Struct({
			_meta: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
			annotations: Schema.optional(Schema.Unknown),
			uri: Schema.String,
			name: Schema.String,
			description: Schema.optional(Schema.String),
			icons: Schema.optional(Schema.Array(Schema.Unknown)),
			mimeType: Schema.optional(Schema.String),
			size: Schema.optional(Schema.Number),
			title: Schema.optional(Schema.String),
		}),
	),
});

type McpOperation = McpTransportError["operation"];

const endpoint_policy = (url: string): HttpMcpEndpointPolicy => {
	try {
		const parsed = new URL(url);
		const broad_local_binding_warning =
			parsed.hostname === "0.0.0.0" || parsed.hostname === "[::]";
		const local_http =
			parsed.hostname === "localhost" ||
			parsed.hostname === "127.0.0.1" ||
			parsed.hostname === "[::1]";
		return {
			allowed:
				parsed.username === "" &&
				parsed.password === "" &&
				parsed.hash === "" &&
				(parsed.protocol === "https:" || (parsed.protocol === "http:" && local_http)),
			broad_local_binding_warning,
		};
	} catch {
		return { allowed: false, broad_local_binding_warning: false };
	}
};

/** Returns typed policy metadata without initiating a network connection. */
export const inspect_http_mcp_endpoint = (endpoint: HttpMcpEndpoint): HttpMcpEndpointPolicy =>
	endpoint_policy(endpoint.url);

const transport_error = (operation: McpOperation, state: McpHealth) =>
	new McpTransportError({ operation, state });

const is_valid_endpoint = (endpoint: HttpMcpEndpoint) =>
	inspect_http_mcp_endpoint(endpoint).allowed &&
	(endpoint.auth?.kind !== "secret_header" ||
		/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(endpoint.auth.header_name)) &&
	Number.isSafeInteger(endpoint.timeout_ms) &&
	endpoint.timeout_ms > 0 &&
	Number.isSafeInteger(endpoint.max_response_bytes) &&
	endpoint.max_response_bytes > 0 &&
	(endpoint.max_pagination_bytes === undefined ||
		(Number.isSafeInteger(endpoint.max_pagination_bytes) &&
			endpoint.max_pagination_bytes > 0)) &&
	(endpoint.max_pagination_items === undefined ||
		(Number.isSafeInteger(endpoint.max_pagination_items) &&
			endpoint.max_pagination_items > 0)) &&
	(endpoint.max_pagination_pages === undefined ||
		(Number.isSafeInteger(endpoint.max_pagination_pages) && endpoint.max_pagination_pages > 0));

const content_type_is_json = (content_type: string | undefined) =>
	content_type?.toLowerCase().split(";", 1)[0] === "application/json";

const ReadBoundedJson = (
	response: HttpClientResponse.HttpClientResponse,
	max_response_bytes: number,
	operation: McpOperation,
) =>
	Stream.runFoldEffect(
		response.stream,
		() => ({ bytes: 0, chunk_count: 0, chunks: [] as Array<Uint8Array> }),
		(accumulator, chunk) => {
			const bytes = accumulator.bytes + chunk.byteLength;
			const chunk_count = accumulator.chunk_count + 1;
			if (bytes > max_response_bytes || chunk_count > max_response_bytes + 1)
				return Effect.fail(transport_error(operation, "unhealthy"));
			if (chunk.byteLength > 0) accumulator.chunks.push(chunk);
			return Effect.succeed({ ...accumulator, bytes, chunk_count });
		},
	).pipe(
		Effect.mapError(() => transport_error(operation, "unhealthy")),
		Effect.flatMap(({ bytes, chunks }) =>
			Effect.try({
				try: () => {
					const payload = new Uint8Array(bytes);
					let offset = 0;
					for (const chunk of chunks) {
						payload.set(chunk, offset);
						offset += chunk.byteLength;
					}
					return {
						bytes,
						payload: JSON.parse(new TextDecoder().decode(payload)) as unknown,
					};
				},
				catch: () => transport_error(operation, "unhealthy"),
			}),
		),
	);

/** Adapts pinned Effect unstable HttpClient APIs and Streamable HTTP JSON-RPC behind a stable Marketplace seam. */
export class HttpMcpDriver extends Context.Service<
	HttpMcpDriver,
	{
		readonly Connect: (input: {
			readonly endpoint: HttpMcpEndpoint;
			readonly auth_header?: {
				readonly name: string;
				readonly value: Redacted.Redacted<string>;
			};
			readonly http_client: HttpClient.HttpClient;
		}) => Effect.Effect<McpClientSession, McpTransportError>;
	}
>()("Artisan/Marketplace/HttpMcpDriver") {}

/** Concrete Streamable HTTP JSON-RPC adapter built only on Effect's pinned unstable client. */
export const EffectHttpMcpDriverLive = Layer.succeed(HttpMcpDriver, {
	Connect: ({ endpoint, auth_header, http_client }) =>
		Effect.gen(function* () {
			if (!is_valid_endpoint(endpoint))
				return yield* Effect.fail(transport_error("start", "closed"));
			const health = yield* Ref.make<McpHealth>("connected");
			const close_claimed = yield* Ref.make(false);
			let next_id = 1;
			let session_id: string | undefined;

			const SetUnhealthy = (operation: McpOperation) =>
				Ref.set(health, "unhealthy").pipe(
					Effect.andThen(Effect.fail(transport_error(operation, "unhealthy"))),
				);
			const AddHeaders = (request: HttpClientRequest.HttpClientRequest) => {
				let next = HttpClientRequest.setHeader(request, "Accept", "application/json");
				if (auth_header)
					next = HttpClientRequest.setHeader(
						next,
						auth_header.name,
						Redacted.value(auth_header.value),
					);
				if (session_id)
					next = HttpClientRequest.setHeader(next, "Mcp-Session-Id", session_id);
				return next;
			};
			const Execute = (
				operation: McpOperation,
				request: HttpClientRequest.HttpClientRequest,
			) =>
				http_client.execute(AddHeaders(request)).pipe(
					Effect.provideService(FetchHttpClient.RequestInit, { redirect: "manual" }),
					Effect.timeout(endpoint.timeout_ms),
					Effect.mapError(() => transport_error(operation, "unhealthy")),
					Effect.tap((response) => {
						/** A changed response request proves the supplied client followed a redirect; this driver never accepts hops. */
						if (new URL(response.request.url).href !== new URL(endpoint.url).href)
							return Effect.fail(transport_error(operation, "unhealthy"));
						const response_session = response.headers["mcp-session-id"];
						return typeof response_session === "string"
							? Effect.sync(() => {
									session_id = response_session;
								})
							: Effect.void;
					}),
				);
			const Request = (
				operation: McpOperation,
				method: string,
				params: Readonly<Record<string, unknown>> = {},
			) =>
				Effect.gen(function* () {
					if ((yield* Ref.get(health)) !== "connected")
						return yield* Effect.fail(
							transport_error(operation, yield* Ref.get(health)),
						);
					const id = next_id++;
					const request = yield* HttpClientRequest.bodyJson(
						HttpClientRequest.post(endpoint.url),
						{
							jsonrpc: "2.0",
							id,
							method,
							params,
						},
					).pipe(Effect.mapError(() => transport_error(operation, "unhealthy")));
					const response = yield* Execute(operation, request).pipe(
						Effect.catch(() => SetUnhealthy(operation)),
					);
					if (
						response.status < 200 ||
						response.status >= 300 ||
						!content_type_is_json(response.headers["content-type"])
					)
						return yield* SetUnhealthy(operation);
					const { bytes: response_bytes, payload } = yield* ReadBoundedJson(
						response,
						endpoint.max_response_bytes,
						operation,
					).pipe(Effect.catch(() => SetUnhealthy(operation)));
					const decoded = yield* Schema.decodeUnknownEffect(JsonRpcResponse, {
						onExcessProperty: "error",
					})(payload).pipe(Effect.catch(() => SetUnhealthy(operation)));
					if (
						decoded.id !== id ||
						decoded.id <= 0 ||
						(decoded.result === undefined) === (decoded.error === undefined)
					)
						return yield* SetUnhealthy(operation);
					if (decoded.error !== undefined) return yield* SetUnhealthy(operation);
					return { response_bytes, result: decoded.result };
				});
			const NotifyInitialized = Effect.gen(function* () {
				const response = yield* Execute(
					"initialize",
					HttpClientRequest.post(endpoint.url).pipe(
						HttpClientRequest.bodyJsonUnsafe({
							jsonrpc: "2.0",
							method: "notifications/initialized",
						}),
					),
				).pipe(Effect.catch(() => SetUnhealthy("initialize")));
				if (response.status < 200 || response.status >= 300)
					return yield* SetUnhealthy("initialize");
			});
			const Initialize = Request("initialize", "initialize", {
				protocolVersion: "2025-03-26",
				capabilities: {},
				clientInfo: { name: "artisan-editor", version: "0" },
			}).pipe(
				Effect.flatMap(({ result }) =>
					Schema.decodeUnknownEffect(McpInitializeResult, { onExcessProperty: "error" })(
						result,
					).pipe(Effect.catch(() => SetUnhealthy("initialize"))),
				),
				Effect.tap(() => NotifyInitialized),
				Effect.map(
					(result): McpInitialize => ({
						protocol_version: result.protocolVersion,
						server_name: result.serverInfo.name,
						...(result.serverInfo.version === undefined
							? {}
							: { server_version: result.serverInfo.version }),
						...(result.instructions === undefined
							? {}
							: { instructions: result.instructions }),
					}),
				),
			);
			const ListPaginated = <A, Page>(
				operation: "list_resources" | "list_tools",
				method: "resources/list" | "tools/list",
				DecodePage: (input: unknown) => Effect.Effect<Page, McpTransportError>,
				select_items: (page: Page) => ReadonlyArray<A>,
				select_cursor: (page: Page) => string | undefined,
			) =>
				Effect.gen(function* () {
					const max_pages = endpoint.max_pagination_pages ?? 32;
					const max_items = endpoint.max_pagination_items ?? 10_000;
					const max_bytes =
						endpoint.max_pagination_bytes ??
						Math.min(Number.MAX_SAFE_INTEGER, endpoint.max_response_bytes * max_pages);
					const cursors = new Set<string>();
					const items: Array<A> = [];
					let cursor: string | undefined;
					let pages = 0;
					let bytes = 0;
					do {
						pages += 1;
						if (pages > max_pages) return yield* SetUnhealthy(operation);
						const page_response = yield* Request(
							operation,
							method,
							cursor === undefined ? {} : { cursor },
						);
						bytes += page_response.response_bytes;
						if (!Number.isSafeInteger(bytes) || bytes > max_bytes)
							return yield* SetUnhealthy(operation);
						const page = yield* DecodePage(page_response.result);
						const page_items = select_items(page);
						if (page_items.length > max_items - items.length)
							return yield* SetUnhealthy(operation);
						for (const item of page_items) items.push(item);
						cursor = select_cursor(page);
						if (cursor !== undefined && (!cursors.add(cursor) || cursor.length === 0))
							return yield* SetUnhealthy(operation);
					} while (cursor !== undefined);
					return items;
				});
			const Close = Ref.modify(close_claimed, (claimed) => [!claimed, true] as const).pipe(
				Effect.flatMap((should_close) =>
					Effect.gen(function* () {
						if (should_close && session_id !== undefined) {
							const response = yield* Execute(
								"close",
								HttpClientRequest.delete(endpoint.url),
							).pipe(
								Effect.catch(() =>
									Effect.fail(transport_error("close", "unhealthy")),
								),
							);
							if (response.status < 200 || response.status >= 300)
								return yield* Effect.fail(transport_error("close", "unhealthy"));
						}
					}),
				),
				Effect.ensuring(Ref.set(health, "closed")),
			);
			return {
				Initialize,
				ListTools: ListPaginated(
					"list_tools",
					"tools/list",
					(input) =>
						Schema.decodeUnknownEffect(McpToolResult, { onExcessProperty: "error" })(
							input,
						).pipe(Effect.catch(() => SetUnhealthy("list_tools"))),
					(page) => page.tools,
					(page) => page.nextCursor,
				).pipe(
					Effect.map(
						(tools): ReadonlyArray<McpTool> =>
							tools.map((tool) => ({
								name: tool.name,
								...(tool.description === undefined
									? {}
									: { description: tool.description }),
								input_schema: tool.inputSchema,
							})),
					),
				),
				ListResources: ListPaginated(
					"list_resources",
					"resources/list",
					(input) =>
						Schema.decodeUnknownEffect(McpResourceResult, {
							onExcessProperty: "error",
						})(input).pipe(Effect.catch(() => SetUnhealthy("list_resources"))),
					(page) => page.resources,
					(page) => page.nextCursor,
				).pipe(
					Effect.map(
						(resources): ReadonlyArray<McpResource> =>
							resources.map((resource) => ({
								uri: resource.uri,
								name: resource.name,
								...(resource.description === undefined
									? {}
									: { description: resource.description }),
							})),
					),
				),
				CallTool: (call) =>
					Request("call_tool", "tools/call", {
						name: call.name,
						arguments: call.arguments,
					}).pipe(Effect.map(({ result }) => result)),
				Health: Ref.get(health),
				Close,
			};
		}),
});

export const make_http_mcp_transport_layer = (endpoint: HttpMcpEndpoint) =>
	Layer.effect(
		McpTransport,
		Effect.gen(function* () {
			const driver = yield* HttpMcpDriver;
			const secrets = yield* SecretStore;
			const http_client = yield* HttpClient.HttpClient;
			const connected = yield* Ref.make(false);
			return {
				Connect: () =>
					Ref.modify(connected, (current) => [!current, true] as const).pipe(
						Effect.flatMap((claimed) => {
							if (!claimed || !is_valid_endpoint(endpoint))
								return Effect.fail(
									transport_error("start", claimed ? "closed" : "connected"),
								);
							return Effect.gen(function* () {
								const secret =
									endpoint.auth?.kind === "secret_header"
										? yield* secrets
												.Get(endpoint.auth.secret_reference)
												.pipe(
													Effect.mapError(() =>
														transport_error("start", "closed"),
													),
												)
										: undefined;
								const auth_header =
									secret === undefined || endpoint.auth?.kind !== "secret_header"
										? undefined
										: { name: endpoint.auth.header_name, value: secret };
								return yield* driver
									.Connect(
										auth_header === undefined
											? { endpoint, http_client }
											: { auth_header, endpoint, http_client },
									)
									.pipe(
										Effect.tap((session) =>
											Effect.addFinalizer(() =>
												session.Close.pipe(
													Effect.ignore,
													Effect.andThen(Ref.set(connected, false)),
												),
											),
										),
										Effect.onError(() => Ref.set(connected, false)),
									);
							});
						}),
					),
			};
		}),
	);
