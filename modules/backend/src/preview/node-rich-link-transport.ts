import { Buffer } from "node:buffer";
import { request as request_http, type IncomingMessage } from "node:http";
import { request as request_https } from "node:https";

import { Effect, Layer } from "effect";

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

			return yield* Effect.callback<RichLinkHttpResponse, RichLinkTransportError>(
				(resume) => {
					const url = new URL(input.url);
					const chunks: Array<Buffer> = [];
					let body_bytes = 0;
					let completed = false;
					let response: IncomingMessage | undefined;
					let connect_timeout: ReturnType<typeof setTimeout> | undefined;
					let response_timeout: ReturnType<typeof setTimeout> | undefined;

					const request_fn = url.protocol === "https:" ? request_https : request_http;
					const request = request_fn(
						{
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
								? {
										servername: input.tls_server_name,
									}
								: {}),
						},
						(incoming) => {
							response = incoming;
							incoming.once("error", (cause) => {
								complete(Effect.fail(transport_error(input, "request", cause)));
							});
							const encoding = incoming.headers["content-encoding"];

							if (
								encoding !== undefined &&
								String(encoding).toLowerCase() !== "identity"
							) {
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
							incoming.once("end", () => {
								complete(
									Effect.succeed({
										body: Buffer.concat(chunks, body_bytes),
										headers: normalize_headers(incoming),
										status: incoming.statusCode ?? 0,
									}),
								);
							});
						},
					);

					const clear_timers = () => {
						if (connect_timeout !== undefined) {
							clearTimeout(connect_timeout);
						}

						if (response_timeout !== undefined) {
							clearTimeout(response_timeout);
						}
					};
					const complete = (
						result: Effect.Effect<RichLinkHttpResponse, RichLinkTransportError>,
					) => {
						if (completed) {
							return;
						}

						completed = true;
						clear_timers();
						resume(result);
					};

					request.once("socket", (socket) => {
						const connected = () => {
							if (connect_timeout !== undefined) {
								clearTimeout(connect_timeout);
							}
						};

						if (!socket.connecting) {
							connected();

							return;
						}

						socket.once(
							url.protocol === "https:" ? "secureConnect" : "connect",
							connected,
						);
					});
					request.once("error", (cause) => {
						complete(Effect.fail(transport_error(input, "request", cause)));
					});

					connect_timeout = setTimeout(() => {
						request.destroy();
						complete(
							Effect.fail(
								transport_error(
									input,
									"connect_timeout",
									new Error("connection timed out"),
								),
							),
						);
					}, input.connect_timeout_ms);
					response_timeout = setTimeout(() => {
						request.destroy();
						response?.destroy();
						complete(
							Effect.fail(
								transport_error(
									input,
									"response_timeout",
									new Error("response timed out"),
								),
							),
						);
					}, input.response_timeout_ms);

					request.end();

					return Effect.sync(() => {
						if (completed) {
							return;
						}

						completed = true;
						clear_timers();
						request.destroy();
						response?.destroy();
					});
				},
			);
		}),
});
