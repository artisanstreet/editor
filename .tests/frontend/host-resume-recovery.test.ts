import { describe, expect, it } from "vitest";
import { Effect, Layer, Ref } from "effect";
import { TestClock } from "effect/testing";
import { ArtisanClient } from "@artisan/transport/client";

import { MakeHostResumeRecoveryLayer } from "../../modules/frontend/src/lib/runtime/host-resume-recovery";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const heartbeat_ms = 1_000;
const minimum_gap_ms = 30_000;

/** Runs a monitor against clocks whose disagreement precisely describes sleep. */
const Observe = (wall_clock_script: ReadonlyArray<number>): Promise<number> => {
	let retries = 0;

	return Effect.runPromise(
		Effect.gen(function* () {
			const wall_clock = yield* Ref.make(0);
			const client_layer = Layer.succeed(ArtisanClient, {
				...FixtureArtisanClientService,
				RetryConnection: Effect.sync(() => {
					retries += 1;
				}),
			});
			const recovery_layer = MakeHostResumeRecoveryLayer({
				heartbeat_ms,
				minimum_gap_ms,
				Now: Ref.get(wall_clock),
			}).pipe(Layer.provide(client_layer));

			yield* Effect.scoped(
				Effect.gen(function* () {
					yield* Layer.build(recovery_layer);

					for (const wall_clock_ms of wall_clock_script) {
						yield* Ref.set(wall_clock, wall_clock_ms);
						yield* TestClock.adjust(heartbeat_ms);
					}
				}),
			);

			return retries;
		}).pipe(Effect.provide(TestClock.layer())),
	);
};

describe("browser host resume recovery", () => {
	it("stays quiet while the renderer clocks agree", async () => {
		expect(await Observe([1_000, 2_000, 3_000, 4_000])).toBe(0);
	});

	it("retries the existing client exactly once after a large gap", async () => {
		expect(await Observe([1_000, 2_000, 3_000, 604_000])).toBe(1);
	});

	it("opens one recovery epoch for each separate host resume", async () => {
		expect(await Observe([1_000, 604_000, 605_000, 1_206_000])).toBe(2);
	});

	it("interrupts the monitor when its layer scope closes", async () => {
		let retries = 0;

		await Effect.runPromise(
			Effect.gen(function* () {
				const wall_clock = yield* Ref.make(0);
				const client_layer = Layer.succeed(ArtisanClient, {
					...FixtureArtisanClientService,
					RetryConnection: Effect.sync(() => {
						retries += 1;
					}),
				});
				const recovery_layer = MakeHostResumeRecoveryLayer({
					heartbeat_ms,
					minimum_gap_ms,
					Now: Ref.get(wall_clock),
				}).pipe(Layer.provide(client_layer));

				yield* Effect.scoped(
					Effect.gen(function* () {
						yield* Layer.build(recovery_layer);
					}),
				);
				yield* Ref.set(wall_clock, 604_000);
				yield* TestClock.adjust(heartbeat_ms);
			}).pipe(Effect.provide(TestClock.layer())),
		);

		expect(retries).toBe(0);
	});
});
