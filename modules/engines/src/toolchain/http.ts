import { Context, Data, Effect, Layer, Schema, Stream } from "effect";
import * as FetchHttpClient from "effect/unstable/http/FetchHttpClient";
import * as HttpClient from "effect/unstable/http/HttpClient";
import * as HttpClientRequest from "effect/unstable/http/HttpClientRequest";
import * as HttpClientResponse from "effect/unstable/http/HttpClientResponse";

const maximum_redirects = 5;

/**
 * Hosts a toolchain download may touch. Both vendor channels publish over
 * HTTPS from fixed origins, so anything outside this list is a configuration
 * error or a tampered release descriptor — never something to download.
 */
const allowed_hostnames = new Set([
	"api.github.com",
	"github.com",
	"objects.githubusercontent.com",
	"storage.googleapis.com",
]);

const is_allowed_hostname = (hostname: string) =>
	allowed_hostnames.has(hostname) || hostname.endsWith(".githubusercontent.com");

export const ToolchainHttpResult = Schema.Struct({
	bytes: Schema.Uint8Array,
	status: Schema.Int,
	url: Schema.String,
});
export type ToolchainHttpResult = typeof ToolchainHttpResult.Type;

export class ToolchainHttpFailure extends Data.TaggedError("ToolchainHttpFailure")<{
	readonly cause?: unknown;
	readonly code: "body_too_large" | "disallowed_url" | "http_status" | "network" | "redirect";
	readonly url: string;
}> {}

/** Bounded, allowlisted HTTPS reads for engine release metadata and binaries. */
export class ToolchainReleaseHttp extends Context.Service<
	ToolchainReleaseHttp,
	{
		readonly Get: (
			url: string,
			maximum_bytes: number,
		) => Effect.Effect<ToolchainHttpResult, ToolchainHttpFailure>;
		readonly GetStream: (
			url: string,
			maximum_bytes: number,
		) => Stream.Stream<Uint8Array, ToolchainHttpFailure>;
	}
>()("Artisan/Engines/ToolchainReleaseHttp") {}

const ParseAllowedUrl = (value: string) =>
	Effect.try({
		try: () => {
			const url = new URL(value);
			if (url.protocol !== "https:" || !is_allowed_hostname(url.hostname.toLowerCase()))
				throw new Error("Toolchain URL is outside the trusted release boundary");
			if (url.username !== "" || url.password !== "")
				throw new Error("Toolchain URLs must not contain credentials");
			return url;
		},
		catch: (cause) => new ToolchainHttpFailure({ cause, code: "disallowed_url", url: value }),
	});

/** Enforces the limit while bytes arrive, regardless of Content-Length truth. */
const BoundedResponseStream = (
	response: HttpClientResponse.HttpClientResponse,
	url: string,
	maximum_bytes: number,
) =>
	HttpClientResponse.stream(Effect.succeed(response)).pipe(
		Stream.mapError((cause) => new ToolchainHttpFailure({ cause, code: "network", url })),
		Stream.mapAccumEffect(
			() => 0,
			(total, chunk) =>
				Effect.gen(function* () {
					const next = total + chunk.byteLength;
					if (next > maximum_bytes)
						return yield* new ToolchainHttpFailure({ code: "body_too_large", url });
					return [next, [chunk]] as const;
				}),
		),
	);

const CollectBoundedResponse = (
	response: HttpClientResponse.HttpClientResponse,
	url: string,
	maximum_bytes: number,
) =>
	Effect.gen(function* () {
		const chunks = yield* BoundedResponseStream(response, url, maximum_bytes).pipe(
			Stream.runCollect,
		);
		let total = 0;
		for (const chunk of chunks) total += chunk.byteLength;
		const bytes = new Uint8Array(total);
		let offset = 0;
		for (const chunk of chunks) {
			bytes.set(chunk, offset);
			offset += chunk.byteLength;
		}
		return bytes;
	});

/** Transport-independent adapter; production supplies Effect's fetch client Layer. */
export const ToolchainReleaseHttpFromClient = Layer.effect(
	ToolchainReleaseHttp,
	Effect.gen(function* () {
		const http_client = yield* HttpClient.HttpClient;
		return ToolchainReleaseHttp.of({
			GetStream: (input_url, maximum_bytes) =>
				Stream.unwrap(
					Effect.gen(function* () {
						let url = yield* ParseAllowedUrl(input_url);
						for (let redirects = 0; redirects <= maximum_redirects; redirects += 1) {
							const response = yield* http_client
								.execute(HttpClientRequest.get(url))
								.pipe(
									Effect.provideService(FetchHttpClient.RequestInit, {
										redirect: "manual",
									}),
									Effect.mapError(
										(cause) =>
											new ToolchainHttpFailure({
												cause,
												code: "network",
												url: input_url,
											}),
									),
								);
							if (![301, 302, 303, 307, 308].includes(response.status)) {
								const declared_length = response.headers["content-length"];
								if (
									Number.isSafeInteger(Number(declared_length)) &&
									Number(declared_length) > maximum_bytes
								)
									return yield* new ToolchainHttpFailure({
										code: "body_too_large",
										url: url.toString(),
									});
								if (response.status < 200 || response.status >= 300)
									return yield* new ToolchainHttpFailure({
										code: "http_status",
										url: url.toString(),
									});
								return BoundedResponseStream(
									response,
									url.toString(),
									maximum_bytes,
								);
							}
							const location = response.headers.location;
							if (location === undefined || redirects === maximum_redirects)
								return yield* new ToolchainHttpFailure({
									code: "redirect",
									url: url.toString(),
								});
							url = yield* ParseAllowedUrl(new URL(location, url).toString());
						}
						return yield* new ToolchainHttpFailure({
							code: "redirect",
							url: input_url,
						});
					}),
				),
			Get: (input_url, maximum_bytes) =>
				Effect.gen(function* () {
					let url = yield* ParseAllowedUrl(input_url);
					for (
						let redirect_count = 0;
						redirect_count <= maximum_redirects;
						redirect_count++
					) {
						const request = HttpClientRequest.get(url).pipe(
							HttpClientRequest.setHeader(
								"Accept",
								"application/octet-stream, application/json",
							),
							HttpClientRequest.setHeader("User-Agent", "Artisan-Engine-Toolchain"),
						);
						const response = yield* http_client.execute(request).pipe(
							Effect.provideService(FetchHttpClient.RequestInit, {
								redirect: "manual",
							}),
							Effect.mapError(
								(cause) =>
									new ToolchainHttpFailure({
										cause,
										code: "network",
										url: url.toString(),
									}),
							),
						);
						if ([301, 302, 303, 307, 308].includes(response.status)) {
							const location = response.headers.location;
							if (location === undefined || redirect_count === maximum_redirects)
								return yield* new ToolchainHttpFailure({
									code: "redirect",
									url: url.toString(),
								});
							url = yield* ParseAllowedUrl(new URL(location, url).toString());
							continue;
						}
						if (response.status < 200 || response.status >= 300)
							return yield* new ToolchainHttpFailure({
								code: "http_status",
								url: url.toString(),
							});
						return {
							bytes: yield* CollectBoundedResponse(
								response,
								url.toString(),
								maximum_bytes,
							),
							status: response.status,
							url: response.request.url,
						};
					}
					return yield* new ToolchainHttpFailure({ code: "redirect", url: input_url });
				}),
		});
	}),
);

/** Fetch adapter with manual allowlisted redirects and bounded streaming bodies. */
export const NodeToolchainReleaseHttpLive = ToolchainReleaseHttpFromClient.pipe(
	Layer.provide(FetchHttpClient.layer),
);
