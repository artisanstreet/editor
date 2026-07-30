import { request as request_http, type IncomingMessage } from "node:http";
import { request as request_https } from "node:https";

import { Clock, Effect, Layer, Option } from "effect";

import {
	PreviewHealthProbe,
	PreviewHealthProbeError,
	type PreviewHealthProbeResult,
	type PreviewTargetRecord,
} from "./target";
import { is_valid_loopback_preview_url } from "./runtime";

const maximum_response_bytes = 8 * 1024;
const default_timeout_ms = 3_000;

/** Configures the fixed resource limits for a local health observation. */
export interface NodePreviewHealthProbeOptions {
	readonly maximum_response_bytes?: number;
	readonly timeout_ms?: number;
}

function probe_error(target: PreviewTargetRecord, cause: unknown) {
	return new PreviewHealthProbeError({ cause, target_id: target.id });
}

function valid_limit(value: number) {
	return Number.isSafeInteger(value) && value > 0;
}

/** Pins local names to a loopback literal so a hostile resolver cannot redirect a probe. */
function loopback_address(url: URL) {
	const hostname = url.hostname.toLowerCase().replace(/^\[|\]$/gu, "");

	if (hostname === "localhost" || hostname.endsWith(".localhost")) {
		return { family: 4 as const, hostname: "127.0.0.1" };
	}

	return { family: hostname.includes(":") ? (6 as const) : (4 as const), hostname };
}

function message_from_response(response: IncomingMessage, body: Uint8Array) {
	const content_type = response.headers["content-type"]?.toLowerCase() ?? "";

	if (!content_type.startsWith("text/") && !content_type.includes("json")) {
		return Option.none<string>();
	}

	const message = new TextDecoder().decode(body).trim().slice(0, 512);

	return message.length === 0 ? Option.none<string>() : Option.some(message);
}

/**
 * Provides a bounded direct HTTP(S) health check for already-registered loopback
 * targets. Node's request API is retained as a custom boundary because the current
 * Effect HTTP client does not provide address pinning/loopback-only dialing.
 */
export const make_node_preview_health_probe_layer = (
	options: NodePreviewHealthProbeOptions = {},
) => {
	const maximum_bytes = options.maximum_response_bytes ?? maximum_response_bytes;
	const timeout_ms = options.timeout_ms ?? default_timeout_ms;

	return Layer.succeed(PreviewHealthProbe, {
		Probe: (target) => {
			if (!valid_limit(maximum_bytes) || !valid_limit(timeout_ms)) {
				return Effect.fail(
					probe_error(
						target,
						new Error("health probe limits must be positive safe integers"),
					),
				);
			}

			if (!is_valid_loopback_preview_url(target.url)) {
				return Effect.fail(
					probe_error(target, new Error("health target must be loopback HTTP(S)")),
				);
			}

			return Effect.gen(function* () {
				const url = new URL(target.url);
				const pinned = loopback_address(url);
				const started_at_ms = yield* Clock.currentTimeMillis;
				const request_fn = url.protocol === "https:" ? request_https : request_http;
				const result = yield* Effect.callback<
					Omit<PreviewHealthProbeResult, "latency_ms">,
					PreviewHealthProbeError
				>((resume) => {
					let completed = false;
					let response: IncomingMessage | undefined;
					const complete = (
						outcome: Effect.Effect<
							Omit<PreviewHealthProbeResult, "latency_ms">,
							PreviewHealthProbeError
						>,
					) => {
						if (completed) {
							return;
						}

						completed = true;
						resume(outcome);
					};
					const request = request_fn(
						{
							agent: false,
							headers: {
								Accept: "text/plain, application/json;q=0.9, */*;q=0.1",
								"Accept-Encoding": "identity",
								Host: url.host,
								"User-Agent": "ArtisanEditor-PreviewHealth/1.0",
							},
							family: pinned.family,
							hostname: pinned.hostname,
							method: "GET",
							path: `${url.pathname}${url.search}`,
							port: url.port || undefined,
							...(url.protocol === "https:" ? { servername: url.hostname } : {}),
						},
						(incoming) => {
							response = incoming;
							const chunks: Array<Uint8Array> = [];
							let received_bytes = 0;

							incoming.once("error", (cause) =>
								complete(Effect.fail(probe_error(target, cause))),
							);
							incoming.on("data", (chunk: Uint8Array) => {
								if (completed) {
									return;
								}

								received_bytes += chunk.byteLength;
								if (received_bytes > maximum_bytes) {
									incoming.destroy();
									complete(
										Effect.fail(
											probe_error(
												target,
												new Error("health response exceeded byte limit"),
											),
										),
									);

									return;
								}

								chunks.push(chunk);
							});
							incoming.once("end", () => {
								const body = new Uint8Array(received_bytes);
								let offset = 0;

								for (const chunk of chunks) {
									body.set(chunk, offset);
									offset += chunk.byteLength;
								}

								const status_code = incoming.statusCode ?? 0;
								complete(
									Effect.succeed({
										message: message_from_response(incoming, body),
										status:
											status_code >= 200 && status_code < 400
												? "healthy"
												: "unhealthy",
										status_code: Option.some(status_code),
									}),
								);
							});
						},
					);

					request.once("error", (cause) =>
						complete(Effect.fail(probe_error(target, cause))),
					);
					request.end();

					return Effect.sync(() => {
						if (completed) {
							return;
						}

						completed = true;
						request.destroy();
						response?.destroy();
					});
				}).pipe(
					Effect.timeoutOrElse({
						duration: timeout_ms,
						orElse: () =>
							Effect.fail(probe_error(target, new Error("health request timed out"))),
					}),
				);
				const completed_at_ms = yield* Clock.currentTimeMillis;

				return {
					...result,
					latency_ms: Math.max(0, completed_at_ms - started_at_ms),
				};
			});
		},
	});
};

/** Provides the production bounded loopback-only preview health probe. */
export const NodePreviewHealthProbeLive = make_node_preview_health_probe_layer();
