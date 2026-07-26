import { Buffer } from "node:buffer";
import { request as request_http, type ClientRequest, type IncomingMessage } from "node:http";
import { request as request_https } from "node:https";

import { Effect, Fiber, Layer } from "effect";

import {
	RichLinkHttpTransport,
	RichLinkTransportError,
	type RichLinkHttpRequest,
	type RichLinkHttpResponse,
} from "./rich-link-metadata";

function transport_error(
	input: RichLinkHttpRequest,
	code: RichLinkTransportError["code"],
	cause: unknown,
) {
	return new RichLinkTransportError({ cause, code, url: input.url });
}

function normalize_headers(response: IncomingMessage) {
	return Object.fromEntries(
		Object.entries(response.headers).flatMap(([name, value]) => {
			if (value === undefined) {
				return [];
			}

			return [[name.toLowerCase(), Array.isArray(value) ? value.join(", ") : value]];
		}),
	);
}

const AwaitConnection = (request: ClientRequest, url: URL) =>
	Effect.callback<void>((resume) => {
		const on_socket = (socket: Parameters<Parameters<ClientRequest["once"]>[1]>[0]) => {
			const event = url.protocol === "https:" ? "secureConnect" : "connect";
			const on_connected = () => resume(Effect.void);

			if (!socket.connecting) {
				on_connected();
			} else {
				socket.once(event, on_connected);
			}
		};

		request.once("socket", on_socket);

		return Effect.sync(() => request.off("socket", on_socket));
	});

const ReadResponse = (request: ClientRequest, input: RichLinkHttpRequest) =>
	Effect.callback<RichLinkHttpResponse, RichLinkTransportError>((resume) => {
		let response: IncomingMessage | undefined;
		let completed = false;
		let body_bytes = 0;
		const chunks: Array<Buffer> = [];
		const complete = (result: Effect.Effect<RichLinkHttpResponse, RichLinkTransportError>) => {
			if (!completed) {
				completed = true;
				resume(result);
			}
		};
		const on_request_error = (cause: Error) =>
			complete(Effect.fail(transport_error(input, "request", cause)));
		const on_response = (incoming: IncomingMessage) => {
			response = incoming;
			incoming.once("error", on_request_error);
			const encoding = incoming.headers["content-encoding"];

			if (encoding !== undefined && String(encoding).toLowerCase() !== "identity") {
				incoming.destroy();
				complete(
					Effect.fail(
						transport_error(
							input,
							"unsupported_encoding",
							new Error("response was not identity encoded"),
						),
					),
				);
				return;
			}

			incoming.on("data", (chunk: Buffer) => {
				if (completed) {
					return;
				}

				body_bytes += chunk.byteLength;
				if (body_bytes > input.max_bytes) {
					incoming.destroy();
					complete(
						Effect.fail(
							transport_error(
								input,
								"response_size",
								new Error("response exceeded its byte limit"),
							),
						),
					);
					return;
				}

				chunks.push(chunk);
			});
			incoming.once("end", () =>
				complete(
					Effect.succeed({
						body: Buffer.concat(chunks, body_bytes),
						headers: normalize_headers(incoming),
						status: incoming.statusCode ?? 0,
					}),
				),
			);
		};

		request.once("error", on_request_error);
		request.once("response", on_response);

		return Effect.sync(() => {
			completed = true;
			request.off("response", on_response);
			request.destroy();
			response?.destroy();
		});
	});

/** Provides bounded Node HTTP(S) requests pinned to prevalidated addresses. */
export const NodeRichLinkHttpTransportLive = Layer.succeed(RichLinkHttpTransport, {
	Request: (input) =>
		Effect.gen(function* () {
			if (
				!Number.isSafeInteger(input.max_bytes) ||
				input.max_bytes < 0 ||
				!Number.isSafeInteger(input.connect_timeout_ms) ||
				input.connect_timeout_ms <= 0 ||
				!Number.isSafeInteger(input.response_timeout_ms) ||
				input.response_timeout_ms <= 0
			) {
				return yield* Effect.fail(
					transport_error(
						input,
						"request",
						new Error("transport limits must be positive safe integers"),
					),
				);
			}

			return yield* Effect.scoped(
				Effect.gen(function* () {
					const url = new URL(input.url);
					const request_fn = url.protocol === "https:" ? request_https : request_http;
					const request = yield* Effect.acquireRelease(
						Effect.sync(() =>
							request_fn({
								agent: false,
								family: input.pinned_address.family,
								headers: {
									Accept: input.accept,
									"Accept-Encoding": "identity",
									Host: input.host_header,
									"User-Agent": "ArtisanEditor-LinkPreview/1.0",
								},
								hostname: input.pinned_address.address,
								method: "GET",
								path: `${url.pathname}${url.search}`,
								port: url.port || undefined,
								...(url.protocol === "https:"
									? { servername: input.tls_server_name }
									: {}),
							}),
						),
						(request) => Effect.sync(() => request.destroy()),
					);
					const connection_fiber = yield* AwaitConnection(request, url).pipe(
						Effect.timeoutOrElse({
							duration: input.connect_timeout_ms,
							orElse: () =>
								Effect.fail(
									transport_error(
										input,
										"connect_timeout",
										new Error("connection timed out"),
									),
								),
						}),
						Effect.forkScoped({ startImmediately: true }),
					);
					const response_fiber = yield* ReadResponse(request, input).pipe(
						Effect.timeoutOrElse({
							duration: input.response_timeout_ms,
							orElse: () =>
								Effect.fail(
									transport_error(
										input,
										"response_timeout",
										new Error("response timed out"),
									),
								),
						}),
						Effect.forkScoped({ startImmediately: true }),
					);

					yield* Effect.sync(() => request.end());
					yield* Fiber.join(connection_fiber);

					return yield* Fiber.join(response_fiber);
				}),
			);
		}),
});
