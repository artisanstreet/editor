import { describe, expect, it } from "@effect/vitest";
import { Cause, Effect, Exit, Layer, Stream } from "effect";
import * as HttpClient from "effect/unstable/http/HttpClient";
import * as HttpClientResponse from "effect/unstable/http/HttpClientResponse";

import { ToolchainReleaseHttp, ToolchainReleaseHttpFromClient } from "@artisan/engines";

type Request = Parameters<HttpClient.HttpClient["execute"]>[0];

const response = (
	request: Request,
	chunks: ReadonlyArray<string>,
	options: { readonly headers?: Record<string, string>; readonly status?: number } = {},
) =>
	HttpClientResponse.fromWeb(
		request,
		new Response(
			new ReadableStream<Uint8Array>({
				start(controller) {
					for (const chunk of chunks) controller.enqueue(new TextEncoder().encode(chunk));
					controller.close();
				},
			}),
			{
				...(options.headers === undefined ? {} : { headers: options.headers }),
				status: options.status ?? 200,
			},
		),
	);

const FakeHttpClient = (handle: (request: Request) => Effect.Effect<ReturnType<typeof response>>) =>
	Layer.succeed(HttpClient.HttpClient, {
		execute: handle,
	} as unknown as HttpClient.HttpClient);

const WithHttp = <A>(
	handle: (request: Request) => Effect.Effect<ReturnType<typeof response>>,
	use: (http: typeof ToolchainReleaseHttp.Service) => Effect.Effect<A>,
) =>
	Effect.gen(function* () {
		const http = yield* ToolchainReleaseHttp;
		return yield* use(http);
	}).pipe(
		Effect.provide(ToolchainReleaseHttpFromClient.pipe(Layer.provide(FakeHttpClient(handle)))),
	);

const FailureFrom = (exit: Exit.Exit<unknown, unknown>) =>
	Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;

describe("toolchain release HTTP", () => {
	it.effect("stops a chunked streaming body at the cumulative byte cap", () =>
		WithHttp(
			(request) => Effect.sync(() => response(request, ["ab", "cd"])),
			(http) =>
				Effect.gen(function* () {
					const exit = yield* http
						.GetStream("https://github.com/vendor/release", 3)
						.pipe(Stream.runCollect, Effect.exit);
					expect(FailureFrom(exit)).toMatchObject({
						_tag: "ToolchainHttpFailure",
						code: "body_too_large",
					});
				}),
		),
	);

	it.effect("bounds metadata collection without trusting Content-Length", () =>
		WithHttp(
			(request) => Effect.sync(() => response(request, ["ab", "cd"])),
			(http) =>
				Effect.gen(function* () {
					const exit = yield* http
						.Get("https://api.github.com/repos/openai/codex/releases/latest", 3)
						.pipe(Effect.exit);
					expect(FailureFrom(exit)).toMatchObject({
						_tag: "ToolchainHttpFailure",
						code: "body_too_large",
					});
				}),
		),
	);

	it.effect("follows only allowlisted redirect hops", () => {
		const requested: Array<string> = [];
		return WithHttp(
			(request) =>
				Effect.sync(() => {
					requested.push(request.url.toString());
					return new URL(request.url.toString()).hostname === "github.com"
						? response(request, [], {
								headers: {
									location: "https://objects.githubusercontent.com/release",
								},
								status: 302,
							})
						: response(request, ["ok"]);
				}),
			(http) =>
				Effect.gen(function* () {
					const chunks = yield* http
						.GetStream("https://github.com/vendor/release", 8)
						.pipe(Stream.runCollect, Effect.orDie);
					expect(new TextDecoder().decode(chunks[0])).toBe("ok");
					expect(requested).toEqual([
						"https://github.com/vendor/release",
						"https://objects.githubusercontent.com/release",
					]);
				}),
		);
	});

	it.effect("rejects a redirect outside the vendor boundary before requesting it", () => {
		let request_count = 0;
		return WithHttp(
			(request) =>
				Effect.sync(() => {
					request_count += 1;
					return response(request, [], {
						headers: { location: "https://example.com/payload" },
						status: 302,
					});
				}),
			(http) =>
				Effect.gen(function* () {
					const exit = yield* http
						.GetStream("https://github.com/vendor/release", 8)
						.pipe(Stream.runCollect, Effect.exit);
					expect(FailureFrom(exit)).toMatchObject({
						_tag: "ToolchainHttpFailure",
						code: "disallowed_url",
					});
					expect(request_count).toBe(1);
				}),
		);
	});
});
