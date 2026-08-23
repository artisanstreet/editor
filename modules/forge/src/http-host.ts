import { createReadStream } from "node:fs";
import { access, realpath, stat } from "node:fs/promises";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { extname, resolve, sep } from "node:path";
import { timingSafeEqual } from "node:crypto";

import { Data, Effect, Exit, FiberSet, Option, Schema, Scope } from "effect";

import type { ForgeControlAuthorityShape } from "./control-authority";
import type { ForgeConfig } from "./config";
import { ForgeHostAllowed } from "./host-policy";
import { ListForgeInstances } from "./instance-registry";

export class ForgeHttpFailure extends Data.TaggedError("ForgeHttpFailure")<{
	readonly cause: unknown;
	readonly operation: "close" | "listen";
}> {}

export interface ForgeHttpServer {
	readonly endpoint: URL;
	readonly Close: Effect.Effect<void, ForgeHttpFailure>;
	readonly node_server: Server;
}

export interface ForgeActivity {
	/** Number of model runs that an update would interrupt right now. */
	readonly ActiveWorkCount: Effect.Effect<number>;
}

const mime_types: Readonly<Record<string, string>> = {
	".css": "text/css; charset=utf-8",
	".html": "text/html; charset=utf-8",
	".js": "text/javascript; charset=utf-8",
	".json": "application/json; charset=utf-8",
	".svg": "image/svg+xml",
	".woff2": "font/woff2",
};

function respond_json(
	response: ServerResponse,
	status: number,
	body: unknown,
	headers: Record<string, string> = {},
) {
	response.writeHead(status, { "content-type": "application/json; charset=utf-8", ...headers });
	response.end(JSON.stringify(body));
}

const bearer_allowed = (request: IncomingMessage, config: ForgeConfig) => {
	if (config.auth_token === undefined) return false;
	const authorization = request.headers.authorization;
	if (authorization === undefined || !authorization.startsWith("Bearer ")) return false;
	const supplied = Buffer.from(authorization.slice("Bearer ".length));
	const expected = Buffer.from(config.auth_token);
	return supplied.length === expected.length && timingSafeEqual(supplied, expected);
};

/**
 * Resolves a configured application origin such as `artisan://app`. The
 * allow-list is per-instance configuration, so only compositions that opt into
 * an app scheme (the packaged Electron renderer) ever match; web pages on
 * foreign origins can never claim membership because browsers own the header.
 */
const allowed_app_origin = (request: IncomingMessage, config: ForgeConfig) => {
	const origin = request.headers.origin;
	if (origin === undefined) return undefined;
	return config.allowed_origins.includes(origin) ? origin : undefined;
};

const origin_allowed = (request: IncomingMessage, config: ForgeConfig) => {
	const origin = request.headers.origin;
	if (origin === undefined) return true;
	if (allowed_app_origin(request, config) !== undefined) return true;
	try {
		return new URL(origin).host === request.headers.host;
	} catch {
		return false;
	}
};

/**
 * Cross-origin headers are emitted only for a configured app origin and only
 * on the pre-session surfaces that origin legitimately needs: health, the
 * instance listing, and the one-time pairing exchange. Credentialed responses
 * echo the exact origin, never a wildcard.
 */
const app_cors_headers = (app_origin: string | undefined): Record<string, string> =>
	app_origin === undefined
		? {}
		: {
				"access-control-allow-credentials": "true",
				"access-control-allow-origin": app_origin,
				vary: "origin",
			};

const ReadJson = (request: IncomingMessage) =>
	Effect.callback<unknown, Error>((resume) => {
		const chunks: Buffer[] = [];
		let length = 0;
		const on_data = (chunk: Buffer | string) => {
			const buffer = Buffer.from(chunk);
			length += buffer.length;
			if (length > 4096) {
				cleanup();
				request.destroy();
				resume(Effect.fail(new Error("Forge control JSON exceeds 4 KiB")));
				return;
			}
			chunks.push(buffer);
		};
		const on_end = () => {
			cleanup();
			resume(
				Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					Buffer.concat(chunks).toString("utf8"),
				).pipe(
					Effect.mapError((cause) => new Error("Invalid Forge control JSON", { cause })),
				),
			);
		};
		const on_error = (cause: Error) => {
			cleanup();
			resume(Effect.fail(cause));
		};
		const cleanup = () => {
			request.off("data", on_data);
			request.off("end", on_end);
			request.off("error", on_error);
		};
		request.on("data", on_data);
		request.once("end", on_end);
		request.once("error", on_error);
		return Effect.sync(cleanup);
	});

const ResolveStaticFile = (root: string, pathname: string) =>
	Effect.gen(function* () {
		const decoded_pathname = yield* Effect.try({
			try: () => decodeURIComponent(pathname),
			catch: () => undefined,
		}).pipe(Effect.option);
		if (Option.isNone(decoded_pathname)) return { status: 400 as const };
		const requested = decoded_pathname.value;

		const root_path = yield* Effect.tryPromise(() => realpath(resolve(root))).pipe(
			Effect.option,
		);
		if (Option.isNone(root_path)) return { status: 404 as const };
		const real_root_path = root_path.value;
		const requested_candidate = resolve(
			real_root_path,
			`.${requested === "/" ? "/index.html" : requested}`,
		);

		if (
			requested_candidate !== real_root_path &&
			!requested_candidate.startsWith(`${real_root_path}${sep}`)
		) {
			return { status: 403 as const };
		}

		const candidate = yield* Effect.tryPromise(() =>
			Promise.all([
				access(requested_candidate),
				realpath(requested_candidate),
				stat(requested_candidate),
			]),
		).pipe(Effect.option);
		if (Option.isSome(candidate)) {
			const [, real_candidate, metadata] = candidate.value;
			if (
				real_candidate !== real_root_path &&
				!real_candidate.startsWith(`${real_root_path}${sep}`)
			) {
				return { status: 403 as const };
			}
			if (metadata.isFile())
				return { path: real_candidate, root: real_root_path, status: 200 as const };
		}

		const is_reserved_route =
			requested === "/api" ||
			requested.startsWith("/api/") ||
			requested === "/_app" ||
			requested.startsWith("/_app/");
		if (!is_reserved_route && extname(requested) === "") {
			const index_path = resolve(real_root_path, "index.html");
			const real_index = yield* Effect.tryPromise(() => realpath(index_path)).pipe(
				Effect.option,
			);
			if (Option.isSome(real_index)) {
				if (
					real_index.value !== real_root_path &&
					!real_index.value.startsWith(`${real_root_path}${sep}`)
				) {
					return { status: 403 as const };
				}
				return { path: real_index.value, root: real_root_path, status: 200 as const };
			}
		}
		return { status: 404 as const };
	});

interface EncodedVariant {
	readonly encoding: string;
	readonly path: string;
}

const encoded_variants: ReadonlyArray<{ readonly encoding: string; readonly suffix: string }> = [
	{ encoding: "br", suffix: ".br" },
	{ encoding: "gzip", suffix: ".gz" },
];

/**
 * Resolves the precompressed sibling that adapter-static already emits, so a
 * forwarded session does not pay for uncompressed bundle bytes. The variant is
 * realpath-checked against the root exactly like the origin file, because a
 * symlinked `.br` could otherwise escape the static directory.
 */
const ResolveEncodedVariant = (root: string, path: string, accept_encoding: string) =>
	Effect.gen(function* () {
		for (const variant of encoded_variants) {
			if (!accept_encoding.includes(variant.encoding)) continue;
			const real_variant = yield* Effect.tryPromise(() =>
				realpath(`${path}${variant.suffix}`),
			).pipe(Effect.option);
			if (Option.isNone(real_variant)) continue;
			if (real_variant.value !== root && !real_variant.value.startsWith(`${root}${sep}`)) {
				continue;
			}
			return Option.some<EncodedVariant>({
				encoding: variant.encoding,
				path: real_variant.value,
			});
		}
		return Option.none<EncodedVariant>();
	});

const ServeStatic = (
	root: string,
	pathname: string,
	method: "GET" | "HEAD",
	request: IncomingMessage,
	response: ServerResponse,
) =>
	Effect.gen(function* () {
		const resolved = yield* ResolveStaticFile(root, pathname);
		if (resolved.status !== 200) {
			response.writeHead(resolved.status);
			response.end();
			return;
		}
		const accept_encoding = request.headers["accept-encoding"];
		const variant = yield* ResolveEncodedVariant(
			resolved.root,
			resolved.path,
			Array.isArray(accept_encoding) ? accept_encoding.join(",") : (accept_encoding ?? ""),
		);
		const served_path = Option.isSome(variant) ? variant.value.path : resolved.path;
		const served_metadata = yield* Effect.tryPromise(() => stat(served_path)).pipe(
			Effect.option,
		);
		const etag = Option.isSome(served_metadata)
			? `"${served_metadata.value.size.toString(16)}-${served_metadata.value.mtimeMs.toString(16)}"`
			: undefined;
		const headers: Record<string, string> = {
			/**
			 * Hashed bundle output is content-addressed, so it is safe to pin.
			 * Everything else revalidates against the etag on each load.
			 */
			"cache-control": pathname.startsWith("/_app/immutable/")
				? "public, max-age=31536000, immutable"
				: "no-cache",
			"content-type": mime_types[extname(resolved.path)] ?? "application/octet-stream",
			vary: "accept-encoding",
		};
		if (Option.isSome(variant)) headers["content-encoding"] = variant.value.encoding;
		if (etag !== undefined) headers.etag = etag;

		if (etag !== undefined && request.headers["if-none-match"] === etag) {
			response.writeHead(304, headers);
			response.end();
			return;
		}

		response.writeHead(200, headers);
		if (method === "HEAD") {
			response.end();
			return;
		}
		yield* Effect.callback<void, Error>((resume) => {
			const stream = createReadStream(served_path);
			const on_error = (cause: Error) => resume(Effect.fail(cause));
			const on_finish = () => resume(Effect.void);
			stream.once("error", on_error);
			response.once("finish", on_finish);
			stream.pipe(response);
			return Effect.sync(() => {
				stream.off("error", on_error);
				response.off("finish", on_finish);
				stream.destroy();
			});
		});
	});

/**
 * Small host-owned HTTP boundary. It exists independently of Electron and is
 * deliberately limited to health/static hosting; the protocol transport is an
 * injected binding so it can share this listener without changing backend code.
 */
export function start_forge_http(
	config: ForgeConfig,
	authority: ForgeControlAuthorityShape,
	activity?: ForgeActivity,
): Effect.Effect<ForgeHttpServer, ForgeHttpFailure> {
	return Effect.gen(function* () {
		const request_scope = yield* Scope.make();
		const run_request = yield* FiberSet.makeRuntimePromise().pipe(
			Effect.provideService(Scope.Scope, request_scope),
		);
		const server = createServer((request, response) => {
			const url = new URL(request.url ?? "/", "http://artisan.invalid");
			const app_origin = allowed_app_origin(request, config);

			if (
				(url.pathname === "/health" ||
					url.pathname === "/healthz" ||
					url.pathname.startsWith("/api/")) &&
				!ForgeHostAllowed(request.headers.host, config)
			) {
				respond_json(response, 403, { error: "forbidden" });
				return;
			}
			if (url.pathname === "/health" || url.pathname === "/healthz") {
				/**
				 * The development marker is unauthenticated on purpose: the
				 * renderer reads it before pairing so it can label a development
				 * instance, and the listener is loopback-only. Development
				 * tooling states the marker explicitly rather than it being
				 * inferred from static hosting, so a development Forge that
				 * serves no SPA still identifies itself; nothing that identifies
				 * the data root or the machine belongs here.
				 */
				respond_json(
					response,
					200,
					{
						development: config.development === true,
						service: "artisan-forge",
						status: "ready",
						version: 1,
					},
					app_cors_headers(app_origin),
				);
				return;
			}
			if (
				request.method === "OPTIONS" &&
				url.pathname === "/api/pair" &&
				app_origin !== undefined
			) {
				/**
				 * The pairing exchange posts JSON, which browsers preflight from a
				 * cross-origin app renderer. Only the configured app origin gets a
				 * preflight grant; every other origin falls through to the 404
				 * default below. Requested headers are echoed because the typed
				 * HTTP client attaches tracing headers (`traceparent`) beside
				 * `content-type`; the origin allow-list is the actual boundary.
				 */
				const requested_headers = request.headers["access-control-request-headers"];
				response.writeHead(204, {
					"access-control-allow-headers":
						typeof requested_headers === "string" && requested_headers.length > 0
							? requested_headers
							: "content-type",
					"access-control-allow-methods": "POST",
					"access-control-max-age": "600",
					...app_cors_headers(app_origin),
				});
				response.end();
				return;
			}
			if (request.method === "GET" && url.pathname === "/api/instances") {
				void run_request(
					Effect.gen(function* () {
						if (!origin_allowed(request, config)) {
							respond_json(response, 403, { error: "forbidden" });
							return;
						}
						/**
						 * Deliberately reachable before pairing: the connection gate
						 * uses this listing to offer other local Forges when this one
						 * has no session yet. It carries no secrets — loopback
						 * endpoints only — and the origin check keeps it away from
						 * foreign websites.
						 */
						const instances =
							config.instance_registry_root === undefined
								? []
								: yield* ListForgeInstances(config.instance_registry_root);
						respond_json(
							response,
							200,
							{
								instances: instances.map((instance) => ({
									endpoint: instance.endpoint,
									self: instance.instance_id === config.instance_id,
								})),
							},
							{ "cache-control": "no-store", ...app_cors_headers(app_origin) },
						);
					}),
				);
				return;
			}
			if (request.method === "GET" && url.pathname === "/api/control/status") {
				void run_request(
					Effect.gen(function* () {
						if (!origin_allowed(request, config)) {
							respond_json(response, 403, { error: "forbidden" });
							return;
						}
						if (!bearer_allowed(request, config)) {
							respond_json(response, 401, { error: "unauthorized" });
							return;
						}
						const active_work_count =
							activity === undefined ? 0 : yield* activity.ActiveWorkCount;
						respond_json(
							response,
							200,
							{
								active_work_count,
								instance_id: config.instance_id,
								pid: process.pid,
								service: "artisan-forge",
								status: "ready",
								version: 1,
							},
							{ "cache-control": "no-store" },
						);
					}),
				);
				return;
			}
			if (request.method === "POST" && url.pathname === "/api/control/shutdown") {
				void run_request(
					Effect.gen(function* () {
						if (!origin_allowed(request, config)) {
							respond_json(response, 403, { error: "forbidden" });
							return;
						}
						if (!bearer_allowed(request, config)) {
							respond_json(response, 401, { error: "unauthorized" });
							return;
						}
						yield* authority.RequestShutdown;
						respond_json(response, 202, { status: "shutdown_requested" });
					}),
				);
				return;
			}
			if (request.method === "POST" && url.pathname === "/api/pair/request") {
				void run_request(
					Effect.gen(function* () {
						if (!origin_allowed(request, config)) {
							respond_json(response, 403, { error: "forbidden" });
							return;
						}
						if (!bearer_allowed(request, config)) {
							respond_json(response, 401, { error: "unauthorized" });
							return;
						}
						respond_json(
							response,
							200,
							{ code: yield* authority.RequestPair },
							{ "cache-control": "no-store" },
						);
					}),
				);
				return;
			}
			if (request.method === "POST" && url.pathname === "/api/pair") {
				void run_request(
					Effect.gen(function* () {
						if (!origin_allowed(request, config)) {
							respond_json(response, 403, { error: "forbidden" });
							return;
						}
						if (
							!request.headers["content-type"]
								?.toLowerCase()
								.startsWith("application/json")
						) {
							respond_json(response, 415, {
								error: "unsupported_media_type",
							});
							return;
						}
						const body = yield* ReadJson(request);
						const code =
							typeof (body as { readonly code?: unknown }).code === "string"
								? (body as { readonly code: string }).code
								: undefined;
						if (code === undefined || code.length > 256) {
							respond_json(response, 400, { error: "invalid_pairing_code" });
							return;
						}
						const session = yield* authority.ConsumePair(code);
						if (session._tag === "None") {
							respond_json(
								response,
								401,
								{ error: "invalid_pairing_code" },
								app_cors_headers(app_origin),
							);
							return;
						}
						/**
						 * A same-origin browser session keeps the strict cookie. The
						 * configured app origin (the Electron renderer on its custom
						 * scheme) is a cross-site caller in cookie terms, so its
						 * session must be `SameSite=None`; Chromium then requires
						 * `Secure`, which it accepts for this trustworthy loopback
						 * listener without TLS.
						 */
						const cookie_policy =
							app_origin === undefined ? "SameSite=Strict" : "SameSite=None; Secure";
						respond_json(
							response,
							200,
							{ status: "paired" },
							{
								"cache-control": "no-store",
								"set-cookie": `artisan_forge_session=${session.value}; HttpOnly; Max-Age=43200; ${cookie_policy}; Path=/`,
								...app_cors_headers(app_origin),
							},
						);
					}).pipe(
						Effect.catch(() =>
							Effect.sync(() =>
								respond_json(response, 400, {
									error: "invalid_pairing_request",
								}),
							),
						),
					),
				);
				return;
			}

			if (
				(request.method === "GET" || request.method === "HEAD") &&
				config.static_frontend_root !== undefined
			) {
				void run_request(
					ServeStatic(
						config.static_frontend_root,
						url.pathname,
						request.method,
						request,
						response,
					).pipe(
						Effect.catch(() =>
							Effect.sync(() => {
								if (!response.headersSent) response.writeHead(500);
								response.end();
							}),
						),
					),
				);
				return;
			}

			response.writeHead(404);
			response.end();
		});
		yield* Effect.callback<void, ForgeHttpFailure>((resume) => {
			const on_error = (cause: Error) =>
				resume(Effect.fail(new ForgeHttpFailure({ cause, operation: "listen" as const })));
			server.once("error", on_error);
			server.listen({ host: config.listen_host, port: config.listen_port }, () => {
				server.off("error", on_error);
				resume(Effect.void);
			});
			return Effect.sync(() => {
				server.off("error", on_error);
				server.close();
			});
		});
		const address = server.address();
		if (address === null || typeof address === "string") {
			return yield* Effect.fail(
				new ForgeHttpFailure({
					cause: new Error("Artisan Forge did not expose a TCP listener address"),
					operation: "listen",
				}),
			);
		}
		const listener_port = address.port;
		const Close = Effect.gen(function* () {
			yield* Scope.close(request_scope, Exit.void);
			yield* Effect.callback<void, ForgeHttpFailure>((resume) => {
				server.close((cause) =>
					resume(
						cause === undefined
							? Effect.void
							: Effect.fail(
									new ForgeHttpFailure({
										cause,
										operation: "close" as const,
									}),
								),
					),
				);
			});
		});
		return {
			endpoint: new URL(
				`http://${config.listen_host === "::1" ? "[::1]" : config.listen_host}:${listener_port}`,
			),
			node_server: server,
			Close,
		};
	});
}
