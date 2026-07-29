import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber, Layer } from "effect";
import { TestClock } from "effect/testing";

import type { EngineUsageQueryEnvelope, EngineUsageQueryResultEnvelope } from "@artisan/protocol";
import { make_engine_registry_layer, type Engine, type EngineFailure } from "@artisan/engines";

import { MakeEngineUsageQueryHandler } from "../../modules/backend/src/protocol/rpc/query-handlers/engine-usage";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

/** The handler consumes only the descriptor identity and the optional `Usage` effect. */
function make_test_engine(input: {
	readonly display_name?: string;
	readonly id: string;
	readonly usage?: Engine["Usage"];
}): Engine {
	return {
		Descriptor: { display_name: input.display_name ?? input.id, id: input.id },
		...(input.usage === undefined ? {} : { Usage: input.usage }),
	} as unknown as Engine;
}

const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "backend_engine_usage_query",
	MakeId: (prefix) => Effect.succeed(`${prefix}_engine_usage_query`),
	Now: Effect.succeed("2026-07-29T12:00:00.000Z"),
});

const query: EngineUsageQueryEnvelope = {
	kind: "engine.usage.query",
	message_id: "engine_usage_query_1",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-07-29T12:00:00.000Z",
};

function run_handler(engines: ReadonlyArray<Engine>) {
	return Effect.gen(function* () {
		const Handle = yield* MakeEngineUsageQueryHandler;

		return (yield* Handle(query)) as EngineUsageQueryResultEnvelope;
	}).pipe(Effect.provide(Layer.mergeAll(MetadataLive, make_engine_registry_layer(engines))));
}

describe("engine usage query handler", () => {
	it.effect("omits engines that expose no Usage surface", () =>
		Effect.gen(function* () {
			const result = yield* run_handler([
				make_test_engine({ id: "codex" }),
				make_test_engine({
					display_name: "Claude",
					id: "claude",
					usage: Effect.succeed({
						authentication: { state: "authenticated" },
						windows: [],
					}),
				}),
			]);

			expect(result.payload.engines).toHaveLength(1);
			expect(result.payload.engines[0]?.engine_id).toBe("claude");
			expect(result.payload.fetched_at).toBe("2026-07-29T12:00:00.000Z");
		}),
	);

	it.effect("maps a successful usage report's windows and authentication state", () =>
		Effect.gen(function* () {
			const result = yield* run_handler([
				make_test_engine({
					display_name: "Claude",
					id: "claude",
					usage: Effect.succeed({
						authentication: { state: "authenticated" },
						windows: [
							{ id: "five_hour", kind: "session", percent_used: 42 },
							{
								id: "codex_bengalfox",
								kind: "weekly",
								label: "Fable",
								percent_used: 10,
								resets_at: "2026-08-01T00:00:00.000Z",
								window_minutes: 10_080,
							},
						],
					}),
				}),
			]);

			expect(result.payload.engines).toEqual([
				{
					authentication: "authenticated",
					display_name: "Claude",
					engine_id: "claude",
					windows: [
						{ id: "five_hour", kind: "session", percent_used: 42 },
						{
							id: "codex_bengalfox",
							kind: "weekly",
							label: "Fable",
							percent_used: 10,
							resets_at: "2026-08-01T00:00:00.000Z",
							window_minutes: 10_080,
						},
					],
				},
			]);
		}),
	);

	it.effect("reports unknown authentication and a short failure message when Usage fails", () =>
		Effect.gen(function* () {
			const failure = {
				_tag: "EngineUnavailableError",
				engine_id: "claude",
				message: "Claude CLI not found",
			} as unknown as EngineFailure;
			const result = yield* run_handler([
				make_test_engine({
					display_name: "Claude",
					id: "claude",
					usage: Effect.fail(failure),
				}),
			]);

			expect(result.payload.engines).toEqual([
				{
					authentication: "unknown",
					display_name: "Claude",
					engine_id: "claude",
					failure: "Claude CLI not found",
					windows: [],
				},
			]);
		}),
	);

	it("reports unknown authentication when Usage exceeds its 15-second timeout", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const Handle = yield* MakeEngineUsageQueryHandler;
				const fiber = yield* Handle(query).pipe(Effect.forkChild);

				yield* TestClock.adjust("15 seconds");

				return (yield* Fiber.join(fiber)) as EngineUsageQueryResultEnvelope;
			}).pipe(
				Effect.provide(
					Layer.mergeAll(
						MetadataLive,
						make_engine_registry_layer([
							make_test_engine({
								display_name: "Claude",
								id: "claude",
								usage: Effect.never,
							}),
						]),
						TestClock.layer(),
					),
				),
			),
		);

		expect(result.payload.engines).toEqual([
			{
				authentication: "unknown",
				display_name: "Claude",
				engine_id: "claude",
				failure: "Usage lookup timed out.",
				windows: [],
			},
		]);
	});
});
