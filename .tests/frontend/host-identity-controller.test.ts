import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Deferred, Effect, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { HostIdentitySnapshot } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import {
	HostIdentityController,
	HostIdentityControllerLive,
} from "../../modules/frontend/src/lib/identity/host-identity-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const identity: HostIdentitySnapshot = {
	display_name: "Fixture User",
	hostname: "FIXTURE-HOST",
	platform: "win32",
	username: "fixture",
};

describe("host identity controller", () => {
	it("coalesces callers and retains a layer-owned request after its caller is interrupted", async () => {
		let requests = 0;
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reply = yield* Deferred.make<HostIdentitySnapshot>();
					const services = yield* Layer.build(
						HostIdentityControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetHostIdentity: Effect.gen(function* () {
										requests += 1;
										return yield* Deferred.await(reply);
									}),
								}),
							),
						),
					);
					yield* Effect.gen(function* () {
						const controller = yield* HostIdentityController;
						const caller = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* controller.Refresh;
						yield* Effect.yieldNow;
						expect(requests).toBe(1);
						yield* Fiber.interrupt(caller);
						yield* Deferred.succeed(reply, identity);
						yield* Effect.yieldNow;
						yield* Effect.yieldNow;
						expect(yield* controller.Current).toEqual(identity);
						yield* controller.Refresh;
						expect(requests).toBe(1);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result).toBeUndefined();
	});

	it("keeps identity and cold component work out of their top-level wait path", () => {
		const sidebar = readFileSync(
			resolve(
				process.cwd(),
				"modules/frontend/src/routes/components/sidebar-identity.svelte",
			),
			"utf8",
		);
		const environment = readFileSync(
			resolve(
				process.cwd(),
				"modules/frontend/src/routes/components/thread-environment-card.svelte",
			),
			"utf8",
		);

		expect(sidebar).toContain("identity_controller.Current");
		expect(sidebar).toContain("identity_controller.Changes");
		expect(sidebar).toContain("identity_controller.Refresh.pipe(Effect.forkScoped)");
		expect(sidebar).not.toContain("yield* LoadIdentity");
		expect(environment).toContain("identity_controller.Current");
		expect(environment).toContain("identity_controller.Changes");
		expect(environment).toContain(".pipe(Effect.forkScoped)");
		expect(environment).not.toContain("client.GetHostIdentity");
	});
});
