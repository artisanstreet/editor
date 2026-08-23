import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";

import type { EngineUsageQueryResultEnvelope, EngineUsageReport } from "@artisan/protocol";
import { ClientApiContext } from "../../modules/transport/src/internal/api/context";
import { MakeQueryApi } from "../../modules/transport/src/internal/api/queries";
import { client_error, RequestDelivered } from "../../modules/transport/src/internal/client-common";
import { make_client_request_coordinator } from "../../modules/transport/src/internal/client-request-coordinator";

const report: EngineUsageReport = {
	authentication: "unknown",
	display_name: "Claude",
	engine_id: "claude",
	windows: [],
};

const usage_result = (correlation_id: string): EngineUsageQueryResultEnvelope => ({
	correlation_id,
	kind: "engine.usage.query.result",
	message_id: `result_${correlation_id}`,
	origin: "backend" as const,
	payload: { engines: [report], fetched_at: "2026-08-18T12:00:00.000Z" },
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: "2026-08-18T12:00:00.000Z",
});

/** Builds the query API over a coordinator whose sends the caller controls. */
const make_query_api = (send: Parameters<typeof make_client_request_coordinator>[0]) =>
	Effect.gen(function* () {
		const requests = yield* make_client_request_coordinator(send);
		let trace_number = 0;
		const api = yield* MakeQueryApi.pipe(
			Effect.provideService(ClientApiContext, {
				MakeId: (prefix: string) => Effect.succeed(`${prefix}_test`),
				MakeTrace: Effect.sync(() => ({
					message_id: `message_${++trace_number}`,
					origin: "frontend" as const,
					protocol_version: 1 as const,
					schema_version: 1 as const,
					sent_at: "2026-08-18T12:00:00.000Z",
				})),
				Request: requests.Request,
			}),
		);

		return { api, requests };
	});

describe("engine usage read retry", () => {
	it("retries a usage read nothing answered, carrying a fresh correlation each attempt", async () => {
		const attempted: Array<string> = [];
		const program = Effect.gen(function* () {
			const landed = yield* Deferred.make<string>();
			const { api, requests } = yield* make_query_api((envelope) =>
				Effect.gen(function* () {
					if (envelope.kind !== "engine.usage.query") {
						return yield* Effect.die(`unexpected request ${envelope.kind}`);
					}

					attempted.push(envelope.message_id);

					/** The incident: the session drops before anything can answer. */
					if (attempted.length === 1) {
						return yield* Effect.fail(
							client_error(
								"connection",
								"The MessagePort connection closed.",
								new Error("closed"),
								true,
							),
						);
					}

					yield* Deferred.succeed(landed, envelope.message_id);

					return RequestDelivered("connection_usage_retry");
				}),
			);
			const pending = yield* Effect.forkScoped(api.get_engine_usage());

			yield* requests.Resolve(usage_result(yield* Deferred.await(landed)));

			return yield* Fiber.join(pending);
		}).pipe(Effect.scoped);

		const snapshot = await Effect.runPromise(program);

		expect(snapshot).toEqual({
			engines: [report],
			fetched_at: "2026-08-18T12:00:00.000Z",
		});
		/** The retry happened, and it did not resend the id the coordinator already holds. */
		expect(attempted).toHaveLength(2);
		expect(new Set(attempted).size).toBe(2);
	});

	it("does not retry a backend rejection, even one the protocol marks retryable", async () => {
		const attempted: Array<string> = [];
		const program = Effect.gen(function* () {
			const landed = yield* Deferred.make<string>();
			const { api, requests } = yield* make_query_api((envelope) =>
				Effect.gen(function* () {
					attempted.push(envelope.message_id);
					yield* Deferred.succeed(landed, envelope.message_id);

					return RequestDelivered("connection_usage_rejection");
				}),
			);
			const pending = yield* Effect.forkScoped(Effect.exit(api.get_engine_usage()));

			yield* requests.Reject(yield* Deferred.await(landed), {
				code: "engine_usage_unavailable",
				message: "The engine registry is unavailable.",
				retryable: true,
			});

			return yield* Fiber.join(pending);
		}).pipe(Effect.scoped);

		const exit = await Effect.runPromise(program);

		/**
		 * A rejection is the backend having answered. Asking again would only put
		 * the same question to the same decision, so the caller sees it once.
		 */
		expect(exit._tag).toBe("Failure");
		expect(attempted).toHaveLength(1);
	});
});
