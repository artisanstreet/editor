import { Effect, Layer, Ref } from "effect";
import { describe, expect, it } from "vitest";

import type { SessionDefaultsUpdateInput } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	SessionDefaultsController,
	SessionDefaultsControllerLive,
} from "../../modules/frontend/src/lib/settings/session-defaults-controller";

describe("onboarding completion", () => {
	it("persists and publishes completion through session defaults", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const updates = yield* Ref.make<ReadonlyArray<SessionDefaultsUpdateInput>>([]);
					const services = yield* Layer.build(
						Layer.provide(
							SessionDefaultsControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								UpdateSessionDefaults: (input) =>
									Ref.update(updates, (current) => [...current, input]).pipe(
										Effect.andThen(
											FixtureArtisanClientService.UpdateSessionDefaults(),
										),
									),
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const state = yield* controller.SetOnboardingCompleted(true);
						return { state, updates: yield* Ref.get(updates) };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.updates).toEqual([{ onboarding_completed: true }]);
		expect(result.state.defaults.onboarding_completed).toBe(true);
	});
});
