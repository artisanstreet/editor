import { Cause, Effect, Exit, Layer, Redacted } from "effect";
import * as HttpClient from "effect/unstable/http/HttpClient";
import { describe, expect, it } from "vitest";

import {
	CapabilityTransportRegistry,
	CapabilityTransportRegistryLive,
	type McpClientSession,
} from "../../modules/backend/src/marketplace/capabilities/mcp-transport";
import {
	HttpMcpDriver,
	inspect_http_mcp_endpoint,
} from "../../modules/backend/src/marketplace/capabilities/http-transport";
import { SecretStore } from "../../modules/backend/src/marketplace/capabilities/secret-store";
import {
	StdioMcpDriver,
	type StdioLaunch,
} from "../../modules/backend/src/marketplace/capabilities/stdio-transport";

const session: McpClientSession = {
	CallTool: () => Effect.succeed({}),
	Close: Effect.void,
	Health: Effect.succeed("connected"),
	Initialize: Effect.succeed({ protocol_version: "1", server_name: "test" }),
	ListResources: Effect.succeed([]),
	ListTools: Effect.succeed([]),
};

const fake_http_client = {} as HttpClient.HttpClient;
const reference = (secret_id: string) => ({ provider: "keychain", secret_id });
type HttpConnectInput = {
	readonly auth_header?: { readonly name: string; readonly value: Redacted.Redacted<string> };
	readonly endpoint: Parameters<typeof inspect_http_mcp_endpoint>[0];
	readonly http_client: HttpClient.HttpClient;
};

const MakeLayer = (captures: {
	readonly http: Array<HttpConnectInput>;
	readonly secrets: Array<string>;
	readonly stdio: Array<StdioLaunch>;
}) =>
	CapabilityTransportRegistryLive.pipe(
		Layer.provide(
			Layer.succeed(StdioMcpDriver, {
				Open: (launch) =>
					Effect.sync(() => captures.stdio.push(launch)).pipe(Effect.as(session)),
			}),
		),
		Layer.provide(
			Layer.succeed(HttpMcpDriver, {
				Connect: (input) =>
					Effect.sync(() => captures.http.push(input)).pipe(Effect.as(session)),
			}),
		),
		Layer.provide(
			Layer.succeed(SecretStore, {
				Get: (secret_reference) =>
					Effect.sync(() => {
						captures.secrets.push(secret_reference);
						return Redacted.make(`value-for-${secret_reference}`);
					}),
			}),
		),
		Layer.provide(Layer.succeed(HttpClient.HttpClient, fake_http_client)),
	);

describe("Marketplace production capability transport registry", () => {
	it("is inert on acquisition and selects only the exact reviewed transport", async () => {
		const captures: {
			http: Array<HttpConnectInput>;
			secrets: Array<string>;
			stdio: Array<StdioLaunch>;
		} = { http: [], secrets: [], stdio: [] };
		const layer = MakeLayer(captures);
		await Effect.runPromise(Effect.void.pipe(Effect.provide(layer)));
		expect(captures).toEqual({ http: [], secrets: [], stdio: [] });
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const registry = yield* CapabilityTransportRegistry;
					yield* registry.Connect({
						auth: { kind: "none" },
						transport: {
							args: [],
							command: "server",
							kind: "stdio",
							startup_timeout_ms: 123,
						},
					});
					yield* registry.Connect({
						auth: { kind: "none" },
						transport: { kind: "streamable_http", url: "https://mcp.example.test" },
					});
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(captures.stdio).toHaveLength(1);
		expect(captures.stdio[0]).toMatchObject({
			invocation_timeout_ms: 30_000,
			max_message_bytes: 4 * 1024 * 1024,
			max_pending_requests: 64,
			max_stderr_bytes: 1024 * 1024,
			startup_timeout_ms: 123,
		});
		expect(captures.http).toHaveLength(1);
	});

	it("resolves stdio env references immediately before launch and honors reviewed bounds", async () => {
		const captures: {
			http: Array<HttpConnectInput>;
			secrets: Array<string>;
			stdio: Array<StdioLaunch>;
		} = { http: [], secrets: [], stdio: [] };
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* (yield* CapabilityTransportRegistry).Connect({
						auth: { kind: "none" },
						transport: {
							args: ["--safe"],
							command: "server",
							env: [{ name: "MCP_TOKEN", secret_ref: reference("stdio") }],
							invocation_timeout_ms: 41,
							kind: "stdio",
							max_message_bytes: 42,
							max_pending_requests: 43,
							max_stderr_bytes: 44,
							startup_timeout_ms: 45,
						},
					});
				}).pipe(Effect.provide(MakeLayer(captures))),
			),
		);
		expect(captures.secrets).toEqual(["keychain:stdio"]);
		expect(captures.stdio[0]).toMatchObject({
			env: { MCP_TOKEN: "value-for-keychain:stdio" },
			invocation_timeout_ms: 41,
			max_message_bytes: 42,
			max_pending_requests: 43,
			max_stderr_bytes: 44,
		});
	});

	it("builds bearer, explicit API-key and OAuth headers while missing OAuth refs fail closed", async () => {
		const captures: {
			http: Array<HttpConnectInput>;
			secrets: Array<string>;
			stdio: Array<StdioLaunch>;
		} = { http: [], secrets: [], stdio: [] };
		const layer = MakeLayer(captures);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const registry = yield* CapabilityTransportRegistry;
					const transport = {
						kind: "streamable_http" as const,
						url: "https://mcp.example.test",
					};
					yield* registry.Connect({
						auth: { kind: "bearer", secret_ref: reference("bearer") },
						transport,
					});
					yield* registry.Connect({
						auth: {
							header_name: "X-Api-Key",
							kind: "api_key",
							secret_ref: reference("api"),
						},
						transport,
					});
					yield* registry.Connect({
						auth: {
							authorization_url: "https://auth.example.test/authorize",
							kind: "oauth",
							provider: "example",
							scopes: [],
							token_ref: reference("oauth"),
							token_status: "authorized",
						},
						transport,
					});
					return yield* Effect.exit(
						registry.Connect({
							auth: {
								authorization_url: "https://auth.example.test/authorize",
								kind: "oauth",
								provider: "example",
								scopes: [],
								token_status: "not_started",
							},
							transport,
						}),
					);
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(
			captures.http.map((input) => [
				input.auth_header?.name,
				input.auth_header && Redacted.value(input.auth_header.value),
			]),
		).toEqual([
			["Authorization", "Bearer value-for-keychain:bearer"],
			["X-Api-Key", "value-for-keychain:api"],
			["Authorization", "Bearer value-for-keychain:oauth"],
		]);
		expect(captures.http).toHaveLength(3);
	});

	it("reports broad local binding policy without attempting access", () => {
		expect(
			inspect_http_mcp_endpoint({
				max_response_bytes: 1,
				timeout_ms: 1,
				url: "http://0.0.0.0:3000/mcp",
			}),
		).toEqual({ allowed: false, broad_local_binding_warning: true });
	});

	it("never includes resolved secret material in failures", async () => {
		const secret = "highly-sensitive-value";
		const layer = CapabilityTransportRegistryLive.pipe(
			Layer.provide(Layer.succeed(StdioMcpDriver, { Open: () => Effect.succeed(session) })),
			Layer.provide(Layer.succeed(HttpMcpDriver, { Connect: () => Effect.succeed(session) })),
			Layer.provide(
				Layer.succeed(SecretStore, { Get: () => Effect.succeed(Redacted.make(secret)) }),
			),
			Layer.provide(Layer.succeed(HttpClient.HttpClient, fake_http_client)),
		);
		const exit = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					return yield* Effect.exit(
						(yield* CapabilityTransportRegistry).Connect({
							auth: {
								authorization_url: "https://auth.example.test/authorize",
								kind: "oauth",
								provider: "example",
								scopes: [],
								token_status: "not_started",
							},
							transport: { kind: "streamable_http", url: "https://mcp.example.test" },
						}),
					);
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(String(Exit.isFailure(exit) ? Cause.pretty(exit.cause) : exit)).not.toContain(
			secret,
		);
	});
});
