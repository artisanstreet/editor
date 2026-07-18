import { describe, expect, it } from "vitest";
import { Cause, Effect, Exit, Layer, Redacted } from "effect";
import * as HttpClient from "effect/unstable/http/HttpClient";
import * as FetchHttpClient from "effect/unstable/http/FetchHttpClient";
import * as HttpClientRequest from "effect/unstable/http/HttpClientRequest";
import * as HttpClientResponse from "effect/unstable/http/HttpClientResponse";

import {
	EffectHttpMcpDriverLive,
	EmptySecretStoreLive,
	HttpMcpDriver,
	McpTransport,
	SecretStore,
	make_http_mcp_transport_layer,
} from "@artisan/backend";

function failure(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function response(
	request: Parameters<HttpClient.HttpClient["execute"]>[0],
	body: string,
	options: { readonly headers?: Record<string, string>; readonly status?: number } = {},
) {
	return HttpClientResponse.fromWeb(
		request,
		new Response(body, {
			status: options.status ?? 200,
			headers: { "content-type": "application/json", ...options.headers },
		}),
	);
}

function chunked_response(
	request: Parameters<HttpClient.HttpClient["execute"]>[0],
	chunks: ReadonlyArray<Uint8Array>,
) {
	return HttpClientResponse.fromWeb(
		request,
		new Response(
			new ReadableStream<Uint8Array>({
				start(controller) {
					for (const chunk of chunks) controller.enqueue(chunk);
					controller.close();
				},
			}),
			{ headers: { "content-type": "application/json" } },
		),
	);
}

function fake_client(
	handler: (
		request: Parameters<HttpClient.HttpClient["execute"]>[0],
	) => Effect.Effect<ReturnType<typeof response>>,
) {
	return { execute: handler } as unknown as HttpClient.HttpClient;
}

const endpoint = { url: "https://mcp.example.test/mcp", timeout_ms: 25, max_response_bytes: 128 };
const initialize_result =
	'{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake"}}}';

describe("Marketplace Streamable HTTP MCP transport", () => {
	it("is inert before explicit Connect and resolves a bearer only for the connection", async () => {
		const requests: Array<Parameters<HttpClient.HttpClient["execute"]>[0]> = [];
		const client = fake_client((request) =>
			Effect.sync(() => {
				requests.push(request);
				return response(request, initialize_result, {
					headers: { "mcp-session-id": "session-1" },
				});
			}),
		);
		const layer = make_http_mcp_transport_layer({
			...endpoint,
			bearer_secret_reference: "token" as never,
		}).pipe(
			Layer.provide(EffectHttpMcpDriverLive),
			Layer.provide(Layer.succeed(HttpClient.HttpClient, client)),
			Layer.provide(
				Layer.succeed(SecretStore, { Get: () => Effect.succeed(Redacted.make("secret")) }),
			),
		);
		await Effect.runPromise(Effect.void.pipe(Effect.provide(layer)));
		expect(requests).toEqual([]);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					yield* session.Initialize;
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(requests[0]?.headers.authorization).toBe("Bearer secret");
		expect(requests[1]?.headers["mcp-session-id"]).toBe("session-1");
	});

	it("rejects bad HTTP, JSON-RPC, content, oversized, timeout, and redirect responses", async () => {
		let redirect_requests = 0;
		for (const handler of [
			(request: Parameters<HttpClient.HttpClient["execute"]>[0]) =>
				Effect.succeed(response(request, "no", { status: 500 })),
			(request: Parameters<HttpClient.HttpClient["execute"]>[0]) =>
				Effect.succeed(response(request, "not json")),
			(request: Parameters<HttpClient.HttpClient["execute"]>[0]) =>
				Effect.succeed(
					response(
						request,
						'{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"no","data":{"reason":"denied"}}}',
					),
				),
			(request: Parameters<HttpClient.HttpClient["execute"]>[0]) =>
				Effect.succeed(response(request, `"${"x".repeat(200)}"`)),
			(request: Parameters<HttpClient.HttpClient["execute"]>[0]) =>
				Effect.sync(() => {
					redirect_requests += 1;
					return response(request, "", {
						status: 302,
						headers: { location: "https://other.example.test" },
					});
				}),
			() => Effect.never,
		] as const) {
			const exit = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const driver = yield* HttpMcpDriver;
						const session = yield* driver.Connect({
							endpoint,
							http_client: fake_client(handler),
						});
						return yield* Effect.exit(session.Initialize);
					}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
				),
			);
			expect(failure(exit)).toMatchObject({
				_tag: "McpTransportError",
				operation: "initialize",
				state: "unhealthy",
			});
		}
		expect(redirect_requests).toBe(1);
		const invalid_origin = await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				return yield* Effect.exit(
					driver.Connect({
						endpoint: { ...endpoint, url: "http://example.test/mcp" },
						http_client: fake_client(() => Effect.never),
					}),
				);
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(failure(invalid_origin)).toMatchObject({
			_tag: "McpTransportError",
			operation: "start",
			state: "closed",
		});
		for (const url of [
			"https://user:pass@mcp.example.test/mcp",
			"https://mcp.example.test/mcp#fragment",
		]) {
			const invalid_url = await Effect.runPromise(
				Effect.gen(function* () {
					const driver = yield* HttpMcpDriver;
					return yield* Effect.exit(
						driver.Connect({
							endpoint: { ...endpoint, url },
							http_client: fake_client(() => Effect.never),
						}),
					);
				}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
			);
			expect(failure(invalid_url)).toMatchObject({ operation: "start", state: "closed" });
		}
		const ipv6 = await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				return yield* driver.Connect({
					endpoint: { ...endpoint, url: "http://[::1]:3210/mcp" },
					http_client: fake_client(() => Effect.never),
				});
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(yield_health(ipv6)).resolves.toBe("connected");

		const followed_redirect = await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				const session = yield* driver.Connect({
					endpoint,
					http_client: fake_client((request) => {
						const redirected = HttpClientRequest.setUrl(
							request,
							"https://mcp.example.test/other",
						);
						return Effect.succeed(response(redirected, initialize_result));
					}),
				});
				return yield* Effect.exit(session.Initialize);
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(failure(followed_redirect)).toMatchObject({
			operation: "initialize",
			state: "unhealthy",
		});
	});

	it("sends initialized, propagates the session, and explicitly disconnects without killing anything", async () => {
		const methods: string[] = [];
		const client = fake_client((request) =>
			Effect.sync(() => {
				methods.push(request.method);
				return response(request, request.method === "DELETE" ? "" : initialize_result, {
					headers: { "mcp-session-id": "session-1" },
				});
			}),
		);
		await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				const session = yield* driver.Connect({ endpoint, http_client: client });
				yield* session.Initialize;
				yield* session.Close;
				yield* session.Close;
				expect(yield* session.Health).toBe("closed");
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(methods).toEqual(["POST", "POST", "DELETE"]);

		const scoped_methods: string[] = [];
		const scoped_client = fake_client((request) =>
			Effect.sync(() => {
				scoped_methods.push(request.method);
				return response(request, request.method === "DELETE" ? "" : initialize_result, {
					headers: { "mcp-session-id": "session-2" },
				});
			}),
		);
		const layer = make_http_mcp_transport_layer(endpoint).pipe(
			Layer.provide(EffectHttpMcpDriverLive),
			Layer.provide(Layer.succeed(HttpClient.HttpClient, scoped_client)),
			Layer.provide(EmptySecretStoreLive),
		);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					yield* session.Initialize;
					yield* session.Close;
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(scoped_methods).toEqual(["POST", "POST", "DELETE"]);

		const implicit_methods: string[] = [];
		const implicit_client = fake_client((request) =>
			Effect.sync(() => {
				implicit_methods.push(request.method);
				return response(request, request.method === "DELETE" ? "" : initialize_result, {
					headers: { "mcp-session-id": "session-3" },
				});
			}),
		);
		const implicit_layer = make_http_mcp_transport_layer(endpoint).pipe(
			Layer.provide(EffectHttpMcpDriverLive),
			Layer.provide(Layer.succeed(HttpClient.HttpClient, implicit_client)),
			Layer.provide(EmptySecretStoreLive),
		);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					yield* session.Initialize;
				}).pipe(Effect.provide(implicit_layer)),
			),
		);
		expect(implicit_methods).toEqual(["POST", "POST", "DELETE"]);
	});

	it("accepts compatible capabilities and cursors while bounding adversarial fragmentation", async () => {
		const encoder = new TextEncoder();
		const bytes = encoder.encode(initialize_result);
		let calls = 0;
		const fragmented = fake_client((request) =>
			Effect.sync(() => {
				calls += 1;
				return calls === 1
					? chunked_response(
							request,
							[...bytes].map((byte) => Uint8Array.of(byte)),
						)
					: response(request, "");
			}),
		);
		const initialize = await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				const session = yield* driver.Connect({
					endpoint: { ...endpoint, max_response_bytes: bytes.byteLength },
					http_client: fragmented,
				});
				return yield* session.Initialize;
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(initialize.server_name).toBe("fake");

		const empty_flood = fake_client((request) =>
			Effect.succeed(
				chunked_response(
					request,
					Array.from({ length: endpoint.max_response_bytes + 2 }, () => new Uint8Array()),
				),
			),
		);
		const flood_exit = await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				const session = yield* driver.Connect({ endpoint, http_client: empty_flood });
				return yield* Effect.exit(session.Initialize);
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(failure(flood_exit)).toMatchObject({ operation: "initialize", state: "unhealthy" });

		let compatible_calls = 0;
		const compatible = fake_client((request) =>
			Effect.sync(() => {
				compatible_calls += 1;
				const payloads = [
					initialize_result,
					"",
					'{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"one","title":"One","inputSchema":{},"outputSchema":{},"annotations":{},"icons":[],"execution":{},"_meta":{}}],"nextCursor":"tools-next"}}',
					'{"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"two","inputSchema":{}}]}}',
					'{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"file:///one","name":"one","title":"One","mimeType":"text/plain","size":1,"annotations":{},"icons":[],"_meta":{}}],"nextCursor":"resources-next"}}',
					'{"jsonrpc":"2.0","id":5,"result":{"resources":[{"uri":"file:///two","name":"two"}]}}',
				];
				return response(request, payloads[compatible_calls - 1] ?? "");
			}),
		);
		const discovered = await Effect.runPromise(
			Effect.gen(function* () {
				const driver = yield* HttpMcpDriver;
				const session = yield* driver.Connect({
					endpoint: { ...endpoint, max_response_bytes: 512 },
					http_client: compatible,
				});
				yield* session.Initialize;
				const tools = yield* session.ListTools;
				const resources = yield* session.ListResources;
				return { resources, tools };
			}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
		);
		expect(discovered.tools.map(({ name }) => name)).toEqual(["one", "two"]);
		expect(discovered.resources.map(({ name }) => name)).toEqual(["one", "two"]);
	});

	it("forces pinned Fetch to manual redirect semantics before adding authorization", async () => {
		const fetches: Array<{
			readonly authorization: string | null;
			readonly redirect: string;
			readonly url: string;
		}> = [];
		const fetch: typeof globalThis.fetch = async (input, init) => {
			const headers = new Headers(init?.headers);
			fetches.push({
				authorization: headers.get("authorization"),
				redirect: init?.redirect ?? "follow",
				url: String(input),
			});
			return new Response("", {
				status: 302,
				headers: { location: "https://other.example.test/mcp" },
			});
		};
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const client = yield* HttpClient.HttpClient;
				const driver = yield* HttpMcpDriver;
				const session = yield* driver.Connect({
					endpoint,
					bearer_token: Redacted.make("secret"),
					http_client: client,
				});
				return yield* Effect.exit(session.Initialize);
			}).pipe(
				Effect.provide(EffectHttpMcpDriverLive),
				Effect.provide(FetchHttpClient.layer),
				Effect.provideService(FetchHttpClient.Fetch, fetch),
			),
		);
		expect(failure(exit)).toMatchObject({ operation: "initialize", state: "unhealthy" });
		expect(fetches).toEqual([
			{ authorization: "Bearer secret", redirect: "manual", url: endpoint.url },
		]);
	});

	it("rejects item, page, aggregate-byte, repeated-cursor, and empty-cursor pagination overflow", async () => {
		const scenarios: ReadonlyArray<{
			readonly limits: Partial<typeof endpoint> & {
				readonly max_pagination_bytes?: number;
				readonly max_pagination_items?: number;
				readonly max_pagination_pages?: number;
			};
			readonly pages: ReadonlyArray<string>;
		}> = [
			{
				limits: { max_pagination_items: 1 },
				pages: [
					'{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"one","inputSchema":{}},{"name":"two","inputSchema":{}}]}}',
				],
			},
			{
				limits: { max_pagination_pages: 1 },
				pages: ['{"jsonrpc":"2.0","id":2,"result":{"tools":[],"nextCursor":"next"}}'],
			},
			{
				limits: { max_pagination_bytes: 1 },
				pages: ['{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}'],
			},
			{
				limits: {},
				pages: [
					'{"jsonrpc":"2.0","id":2,"result":{"tools":[],"nextCursor":"same"}}',
					'{"jsonrpc":"2.0","id":3,"result":{"tools":[],"nextCursor":"same"}}',
				],
			},
			{
				limits: {},
				pages: ['{"jsonrpc":"2.0","id":2,"result":{"tools":[],"nextCursor":""}}'],
			},
		];
		for (const scenario of scenarios) {
			let calls = 0;
			const client = fake_client((request) =>
				Effect.sync(() => {
					calls += 1;
					if (calls === 1) return response(request, initialize_result);
					if (calls === 2) return response(request, "");
					return response(request, scenario.pages[calls - 3] ?? "");
				}),
			);
			const exit = await Effect.runPromise(
				Effect.gen(function* () {
					const driver = yield* HttpMcpDriver;
					const session = yield* driver.Connect({
						endpoint: { ...endpoint, max_response_bytes: 1_024, ...scenario.limits },
						http_client: client,
					});
					yield* session.Initialize;
					return yield* Effect.exit(session.ListTools);
				}).pipe(Effect.provide(EffectHttpMcpDriverLive)),
			);
			expect(failure(exit)).toMatchObject({ operation: "list_tools", state: "unhealthy" });
		}
	});
});

const yield_health = (session: { readonly Health: Effect.Effect<string, unknown> }) =>
	Effect.runPromise(session.Health);
