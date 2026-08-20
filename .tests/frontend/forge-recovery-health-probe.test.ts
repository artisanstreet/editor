import { Deferred, Effect, Fiber, Ref } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import {
	forge_recovery_health_deadline,
	ProbeForgeRecoveryHealth,
} from "../../modules/frontend/src/lib/forge/recovery-health-probe";

describe("Forge recovery health probe", () => {
	it("bounds an indefinitely pending request and leaves a later probe free to succeed", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const started = yield* Deferred.make<void>();
				const calls = yield* Ref.make(0);
				const pending = yield* ProbeForgeRecoveryHealth(
					Effect.gen(function* () {
						yield* Ref.update(calls, (current) => current + 1);
						yield* Deferred.succeed(started, undefined);
						return yield* Effect.never;
					}),
				).pipe(Effect.forkChild);

				yield* Deferred.await(started);
				yield* TestClock.adjust(forge_recovery_health_deadline);
				const unreachable = yield* Fiber.join(pending);
				const reachable = yield* ProbeForgeRecoveryHealth(Effect.succeed({ status: 204 }));

				return { calls: yield* Ref.get(calls), reachable, unreachable };
			}).pipe(Effect.provide(TestClock.layer())),
		);

		expect(result).toEqual({ calls: 1, reachable: true, unreachable: false });
	});

	it("treats non-success HTTP status and request failure as unreachable", async () => {
		const result = await Effect.runPromise(
			Effect.all([
				ProbeForgeRecoveryHealth(Effect.succeed({ status: 503 })),
				ProbeForgeRecoveryHealth(Effect.fail("connection refused")),
			]),
		);

		expect(result).toEqual([false, false]);
	});
});
