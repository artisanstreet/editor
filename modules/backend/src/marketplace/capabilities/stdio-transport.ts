import { Context, Deferred, Effect, Layer, Ref, Schema, Scope, Stream } from "effect";
import { EngineJsonlFramer, EngineProcessFactory } from "@artisan/engines";

import {
	McpTransport,
	McpTransportError,
	type McpClientSession,
	type McpHealth,
	type McpInitialize,
	type McpResource,
	type McpTool,
} from "./mcp-transport";

export interface StdioLaunch {
	readonly args: ReadonlyArray<string>;
	readonly command: string;
	readonly cwd?: string;
	readonly env?: Readonly<Record<string, string>>;
	readonly startup_timeout_ms: number;
	readonly invocation_timeout_ms: number;
	readonly max_message_bytes: number;
	readonly max_stderr_bytes: number;
}

/** Driver boundary deliberately takes argv arrays; implementations must never invoke a shell. */
export class StdioMcpDriver extends Context.Service<
	StdioMcpDriver,
	{
		readonly Open: (
			launch: StdioLaunch,
		) => Effect.Effect<McpClientSession, McpTransportError, Scope.Scope>;
	}
>()("Artisan/Marketplace/StdioMcpDriver") {}

const JsonRpcError = Schema.Struct({ code: Schema.Int, message: Schema.String });
const JsonRpcResponse = Schema.Struct({
	jsonrpc: Schema.Literal("2.0"),
	id: Schema.Int,
	result: Schema.optional(Schema.Unknown),
	error: Schema.optional(JsonRpcError),
});
const JsonRpcNotification = Schema.Struct({
	jsonrpc: Schema.Literal("2.0"),
	method: Schema.String,
	params: Schema.optional(Schema.Unknown),
});
const JsonRpcServerRequest = Schema.Struct({
	jsonrpc: Schema.Literal("2.0"),
	id: Schema.Union([Schema.Int, Schema.String]),
	method: Schema.String,
	params: Schema.optional(Schema.Unknown),
});
const JsonRpcIncoming = Schema.Union([JsonRpcResponse, JsonRpcNotification, JsonRpcServerRequest]);
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
	tools: Schema.Array(
		Schema.Struct({
			name: Schema.String,
			title: Schema.optional(Schema.String),
			description: Schema.optional(Schema.String),
			inputSchema: Schema.Record(Schema.String, Schema.Unknown),
			outputSchema: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
			annotations: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
			icons: Schema.optional(Schema.Array(Schema.Unknown)),
		}),
	),
	nextCursor: Schema.optional(Schema.String),
});
const McpResourceResult = Schema.Struct({
	resources: Schema.Array(
		Schema.Struct({
			uri: Schema.String,
			name: Schema.String,
			title: Schema.optional(Schema.String),
			description: Schema.optional(Schema.String),
			mimeType: Schema.optional(Schema.String),
			size: Schema.optional(Schema.Number),
			annotations: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
			icons: Schema.optional(Schema.Array(Schema.Unknown)),
		}),
	),
	nextCursor: Schema.optional(Schema.String),
});
const McpAnnotations = Schema.Record(Schema.String, Schema.Unknown);
const McpContentMetadata = Schema.Record(Schema.String, Schema.Unknown);
const McpCallToolContent = Schema.Union([
	Schema.Struct({
		type: Schema.Literal("text"),
		text: Schema.String,
		annotations: Schema.optional(McpAnnotations),
		_meta: Schema.optional(McpContentMetadata),
	}),
	Schema.Struct({
		type: Schema.Literal("image"),
		data: Schema.String,
		mimeType: Schema.String,
		annotations: Schema.optional(McpAnnotations),
		_meta: Schema.optional(McpContentMetadata),
	}),
	Schema.Struct({
		type: Schema.Literal("audio"),
		data: Schema.String,
		mimeType: Schema.String,
		annotations: Schema.optional(McpAnnotations),
		_meta: Schema.optional(McpContentMetadata),
	}),
	Schema.Struct({
		type: Schema.Literal("resource_link"),
		uri: Schema.String,
		name: Schema.String,
		title: Schema.optional(Schema.String),
		description: Schema.optional(Schema.String),
		mimeType: Schema.optional(Schema.String),
		size: Schema.optional(Schema.Number),
		icons: Schema.optional(Schema.Array(Schema.Unknown)),
		annotations: Schema.optional(McpAnnotations),
		_meta: Schema.optional(McpContentMetadata),
	}),
	Schema.Struct({
		type: Schema.Literal("resource"),
		resource: Schema.Union([
			Schema.Struct({
				uri: Schema.String,
				mimeType: Schema.optional(Schema.String),
				text: Schema.String,
				_meta: Schema.optional(McpContentMetadata),
			}),
			Schema.Struct({
				uri: Schema.String,
				mimeType: Schema.optional(Schema.String),
				blob: Schema.String,
				_meta: Schema.optional(McpContentMetadata),
			}),
		]),
		annotations: Schema.optional(McpAnnotations),
		_meta: Schema.optional(McpContentMetadata),
	}),
]);
const McpToolCallResult = Schema.Struct({
	content: Schema.Array(McpCallToolContent),
	structuredContent: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
	isError: Schema.optional(Schema.Boolean),
	_meta: Schema.optional(McpContentMetadata),
});

type McpOperation = McpTransportError["operation"];
interface PendingRequest {
	readonly operation: McpOperation;
	readonly waiter: Deferred.Deferred<unknown, McpTransportError>;
}

interface RequestState {
	readonly close_started: boolean;
	readonly next_id: number;
	readonly pending: ReadonlyMap<number, PendingRequest>;
}

const encoder = new TextEncoder();

function transport_error(operation: McpOperation, state: McpHealth) {
	return new McpTransportError({ operation, state });
}

const WithTimeout = <A>(
	effect: Effect.Effect<A, McpTransportError>,
	duration: number,
	error: McpTransportError,
) =>
	effect.pipe(
		Effect.timeout(duration),
		Effect.mapError(() => error),
	);

function is_valid_launch(launch: StdioLaunch) {
	return (
		launch.command.length > 0 &&
		Number.isSafeInteger(launch.max_message_bytes) &&
		launch.max_message_bytes > 0 &&
		Number.isSafeInteger(launch.max_stderr_bytes) &&
		launch.max_stderr_bytes > 0 &&
		Number.isSafeInteger(launch.startup_timeout_ms) &&
		launch.startup_timeout_ms > 0 &&
		Number.isSafeInteger(launch.invocation_timeout_ms) &&
		launch.invocation_timeout_ms > 0
	);
}

/** Concrete argv-only NDJSON JSON-RPC driver backed by Artisan's owned process factory. */
export const EngineProcessStdioMcpDriverLive = Layer.effect(
	StdioMcpDriver,
	Effect.gen(function* () {
		const factory = yield* EngineProcessFactory;
		return {
			Open: (launch) =>
				Effect.gen(function* () {
					if (!is_valid_launch(launch))
						return yield* Effect.fail(transport_error("start", "closed"));
					const process = yield* factory
						.Spawn({
							args: launch.args,
							command: launch.command,
							...(launch.cwd === undefined ? {} : { cwd: launch.cwd }),
							...(launch.env === undefined ? {} : { env: launch.env }),
						})
						.pipe(Effect.mapError(() => transport_error("start", "crashed")));
					const state = yield* Ref.make<McpHealth>("connected");
					const initialization = yield* Ref.make<"idle" | "running" | "ready" | "failed">(
						"idle",
					);
					const initialized = yield* Deferred.make<McpInitialize, McpTransportError>();
					const requests = yield* Ref.make<RequestState>({
						close_started: false,
						next_id: 1,
						pending: new Map(),
					});
					const FailAndClearPending = (health: McpHealth) =>
						Effect.gen(function* () {
							const pending = yield* Ref.modify(requests, (current) => [
								[...current.pending.values()],
								{ ...current, pending: new Map() },
							]);
							yield* Effect.forEach(pending, ({ operation, waiter }) =>
								Deferred.fail(waiter, transport_error(operation, health)),
							);
						});
					const Terminate = (health: McpHealth) =>
						Effect.gen(function* () {
							const current = yield* Ref.get(state);
							if (
								current === "closed" ||
								current === "crashed" ||
								current === "unhealthy"
							)
								return;
							yield* Ref.set(state, health);
							const close_process = yield* Ref.modify(requests, (current) => [
								!current.close_started,
								{ ...current, close_started: true },
							]);
							if (close_process) {
								yield* process.Close.pipe(Effect.ignore);
							}
							yield* FailAndClearPending(health);
							yield* Deferred.fail(
								initialized,
								transport_error("initialize", health),
							);
						});
					const CloseProcess = Terminate("closed");
					const framer = new EngineJsonlFramer({
						max_frame_bytes: launch.max_message_bytes,
					});
					const RouteIncoming = (payload: unknown) =>
						Schema.decodeUnknownEffect(JsonRpcIncoming, { onExcessProperty: "error" })(
							payload,
						).pipe(
							Effect.mapError(() => transport_error("health", "unhealthy")),
							Effect.flatMap((message) =>
								Effect.gen(function* () {
									if (!("result" in message) && !("error" in message)) return;
									if (
										message.id <= 0 ||
										(message.result === undefined) ===
											(message.error === undefined)
									)
										return yield* Effect.fail(
											transport_error("health", "unhealthy"),
										);
									const request = yield* Ref.modify(requests, (current) => {
										const request = current.pending.get(message.id);
										if (request === undefined)
											return [undefined, current] as const;
										const pending = new Map(current.pending);
										pending.delete(message.id);
										return [request, { ...current, pending }] as const;
									});
									if (!request) return;
									return yield* message.error === undefined
										? Deferred.succeed(request.waiter, message.result)
										: Deferred.fail(
												request.waiter,
												transport_error(request.operation, "unhealthy"),
											);
								}),
							),
						);
					const PumpStdout = Stream.fromAsyncIterable(process.Stdout, () =>
						transport_error("health", "unhealthy"),
					).pipe(
						Stream.runForEach((chunk) =>
							Effect.forEach(framer.Push(chunk), RouteIncoming, {
								discard: true,
							}),
						),
						Effect.andThen(
							Effect.try({
								try: () => framer.Finish(),
								catch: () => transport_error("health", "unhealthy"),
							}),
						),
						Effect.matchEffect({
							onFailure: () => Terminate("unhealthy"),
							onSuccess: () => Terminate("crashed"),
						}),
					);
					const PumpStderr = Stream.fromAsyncIterable(process.Stderr, () =>
						transport_error("health", "crashed"),
					).pipe(
						Stream.runFoldEffect(
							() => 0,
							(bytes, chunk) => {
								const total = bytes + chunk.byteLength;
								return total <= launch.max_stderr_bytes
									? Effect.succeed(total)
									: Effect.fail(transport_error("health", "crashed"));
							},
						),
						Effect.catch(() => Terminate("crashed")),
						Effect.asVoid,
					);
					const WatchExit = process.Exit.pipe(
						Effect.ignore,
						Effect.andThen(Terminate("crashed")),
					);
					yield* Effect.forkScoped(PumpStdout);
					yield* Effect.forkScoped(PumpStderr);
					yield* Effect.forkScoped(WatchExit);
					/** Close first so blocked async iterators wake before their scoped fibers stop. */
					yield* Effect.addFinalizer(() => CloseProcess);
					const Request = (
						operation: McpOperation,
						method: string,
						params: Readonly<Record<string, unknown>>,
						timeout_ms: number,
					) =>
						Effect.gen(function* () {
							const health = yield* Ref.get(state);
							if (health !== "connected")
								return yield* Effect.fail(transport_error(operation, health));
							const waiter = yield* Deferred.make<unknown, McpTransportError>();
							const id = yield* Ref.modify(requests, (current) => {
								const id = current.next_id;
								const pending = new Map(current.pending);
								pending.set(id, { operation, waiter });
								return [id, { ...current, next_id: id + 1, pending }] as const;
							});
							const RemovePending = Ref.update(requests, (current) => {
								if (!current.pending.has(id)) return current;
								const pending = new Map(current.pending);
								pending.delete(id);
								return { ...current, pending };
							});
							yield* process
								.Write(
									encoder.encode(
										`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`,
									),
								)
								.pipe(
									Effect.mapError(() => transport_error(operation, "crashed")),
									Effect.onError(() => RemovePending),
								);
							return yield* WithTimeout(
								Deferred.await(waiter),
								timeout_ms,
								transport_error(operation, "unhealthy"),
							).pipe(Effect.ensuring(RemovePending));
						});
					const InitializeOnce = Request(
						"initialize",
						"initialize",
						{
							protocolVersion: "2025-03-26",
							capabilities: {},
							clientInfo: { name: "artisan-editor", version: "0" },
						},
						launch.startup_timeout_ms,
					).pipe(
						Effect.flatMap((result) =>
							Schema.decodeUnknownEffect(McpInitializeResult, {
								onExcessProperty: "error",
							})(result).pipe(
								Effect.mapError(() => transport_error("initialize", "unhealthy")),
							),
						),
						Effect.tap(() =>
							process
								.Write(
									encoder.encode(
										'{"jsonrpc":"2.0","method":"notifications/initialized"}\n',
									),
								)
								.pipe(
									Effect.mapError(() => transport_error("initialize", "crashed")),
								),
						),
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
					const Initialize = Ref.modify(initialization, (current) =>
						current === "idle"
							? ([true, "running"] as const)
							: ([false, current] as const),
					).pipe(
						Effect.flatMap((owner) =>
							owner
								? InitializeOnce.pipe(
										Effect.tap((value) =>
											Deferred.succeed(initialized, value).pipe(
												Effect.andThen(Ref.set(initialization, "ready")),
											),
										),
										Effect.catch((error) =>
											Ref.set(initialization, "failed").pipe(
												Effect.andThen(Deferred.fail(initialized, error)),
												Effect.andThen(
													Terminate(
														error.state === "crashed"
															? "crashed"
															: "unhealthy",
													),
												),
												Effect.andThen(Effect.fail(error)),
											),
										),
									)
								: Deferred.await(initialized),
						),
					);
					const AfterInitialize = <A>(
						operation: McpOperation,
						effect: Effect.Effect<A, McpTransportError>,
					) =>
						Ref.get(initialization).pipe(
							Effect.flatMap((status) =>
								status === "idle"
									? Effect.fail(transport_error(operation, "unhealthy"))
									: WithTimeout(
											Deferred.await(initialized).pipe(
												Effect.mapError((error) =>
													transport_error(operation, error.state),
												),
											),
											launch.startup_timeout_ms,
											transport_error(operation, "unhealthy"),
										),
							),
							Effect.andThen(effect),
						);
					const DecodeDiscovery = <A>(
						operation: "list_resources" | "list_tools",
						decoded: Effect.Effect<A, unknown>,
					) =>
						decoded.pipe(
							Effect.mapError(() => transport_error(operation, "unhealthy")),
							Effect.tapError(() => Terminate("unhealthy")),
						);
					const ListAllTools = Effect.gen(function* () {
						const tools: Array<McpTool> = [];
						const cursors = new Set<string>();
						let cursor: string | undefined;
						do {
							const page = yield* Request(
								"list_tools",
								"tools/list",
								cursor === undefined ? {} : { cursor },
								launch.invocation_timeout_ms,
							).pipe(
								Effect.flatMap((result) =>
									DecodeDiscovery(
										"list_tools",
										Schema.decodeUnknownEffect(McpToolResult, {
											onExcessProperty: "error",
										})(result),
									),
								),
							);
							for (const tool of page.tools)
								tools.push({
									name: tool.name,
									...(tool.description === undefined
										? {}
										: { description: tool.description }),
									input_schema: tool.inputSchema,
								});
							cursor = page.nextCursor;
							if (cursor !== undefined) {
								if (cursors.has(cursor) || cursors.size >= 1_024) {
									yield* Terminate("unhealthy");
									return yield* Effect.fail(
										transport_error("list_tools", "unhealthy"),
									);
								}
								cursors.add(cursor);
							}
						} while (cursor !== undefined);
						return tools;
					});
					const ListAllResources = Effect.gen(function* () {
						const resources: Array<McpResource> = [];
						const cursors = new Set<string>();
						let cursor: string | undefined;
						do {
							const page = yield* Request(
								"list_resources",
								"resources/list",
								cursor === undefined ? {} : { cursor },
								launch.invocation_timeout_ms,
							).pipe(
								Effect.flatMap((result) =>
									DecodeDiscovery(
										"list_resources",
										Schema.decodeUnknownEffect(McpResourceResult, {
											onExcessProperty: "error",
										})(result),
									),
								),
							);
							for (const resource of page.resources)
								resources.push({
									uri: resource.uri,
									name: resource.name,
									...(resource.description === undefined
										? {}
										: { description: resource.description }),
								});
							cursor = page.nextCursor;
							if (cursor !== undefined) {
								if (cursors.has(cursor) || cursors.size >= 1_024) {
									yield* Terminate("unhealthy");
									return yield* Effect.fail(
										transport_error("list_resources", "unhealthy"),
									);
								}
								cursors.add(cursor);
							}
						} while (cursor !== undefined);
						return resources;
					});
					return {
						Initialize,
						ListTools: AfterInitialize("list_tools", ListAllTools),
						ListResources: AfterInitialize("list_resources", ListAllResources),
						CallTool: (call) =>
							AfterInitialize(
								"call_tool",
								Request(
									"call_tool",
									"tools/call",
									{ name: call.name, arguments: call.arguments },
									launch.invocation_timeout_ms,
								).pipe(
									Effect.flatMap((result) =>
										Schema.decodeUnknownEffect(McpToolCallResult, {
											onExcessProperty: "error",
										})(result).pipe(
											Effect.mapError(() =>
												transport_error("call_tool", "unhealthy"),
											),
											Effect.tapError(() => Terminate("unhealthy")),
										),
									),
									Effect.flatMap((result) =>
										result.isError === true
											? Effect.fail(transport_error("call_tool", "connected"))
											: Effect.succeed(result),
									),
								),
							),
						Health: Ref.get(state),
						Close: CloseProcess,
					};
				}),
		};
	}),
);

export const make_stdio_mcp_transport_layer = (launch: StdioLaunch) =>
	Layer.effect(
		McpTransport,
		Effect.gen(function* () {
			const driver = yield* StdioMcpDriver;
			const active = yield* Ref.make<number | undefined>(undefined);
			let next_session = 1;
			const Release = (session_id: number) =>
				Ref.update(active, (current) => (current === session_id ? undefined : current));
			return {
				Connect: () =>
					Effect.gen(function* () {
						const session_id = next_session++;
						const claimed = yield* Ref.modify(active, (current) =>
							current === undefined
								? ([true, session_id] as const)
								: ([false, current] as const),
						);
						if (!claimed)
							return yield* Effect.fail(transport_error("start", "connected"));
						const session = yield* driver
							.Open(launch)
							.pipe(Effect.onError(() => Release(session_id)));
						yield* Effect.addFinalizer(() => Release(session_id));
						return {
							...session,
							Close: session.Close.pipe(Effect.ensuring(Release(session_id))),
						};
					}),
			};
		}),
	);
