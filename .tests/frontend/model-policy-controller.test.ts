import { Deferred, Effect, Fiber, Ref } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadSessionPolicy } from "@artisan/protocol";
import {
	MakeModelPolicyController,
	ModelPolicyMutationError,
} from "../../modules/frontend/src/routes/components/model-selector/policy-controller";

const initial_policy: ThreadSessionPolicy = {
	engine_id: "codex",
	model: "gpt-5.6-codex",
	permission: "supervised",
	permission_mode: "on_request",
	reasoning_effort: "medium",
	sandbox_mode: "workspace_write",
	service_tier: "standard",
	strict_clarification: false,
	web_search_enabled: false,
};

describe("model policy controller", () => {
	it("coalesces rapid reasoning, speed, and model changes from the in-flight intent", async () => {
		const writes = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeModelPolicyController;
				const started = yield* Deferred.make<void>();
				const release = yield* Deferred.make<void>();
				const observed = yield* Ref.make<ReadonlyArray<ThreadSessionPolicy>>([]);
				yield* controller.SetAuthoritative(initial_policy);
				yield* controller.Patch({ reasoning_effort: "high" });

				const flush = yield* controller
					.Flush((desired) =>
						Effect.gen(function* () {
							yield* Ref.update(observed, (current) => [...current, desired]);
							if ((yield* Ref.get(observed)).length === 1) {
								yield* Deferred.succeed(started, undefined);
								yield* Deferred.await(release);
							}
							return desired;
						}),
					)
					.pipe(Effect.forkChild);

				yield* Deferred.await(started);
				yield* controller.Patch({ service_tier: "priority" });
				yield* controller.Patch({ engine_id: "claude", model: "claude-opus-4-1" });
				yield* Deferred.succeed(release, undefined);
				yield* Fiber.join(flush);

				return yield* Ref.get(observed);
			}),
		);

		expect(writes).toHaveLength(2);
		expect(writes[0]).toMatchObject({ reasoning_effort: "high" });
		expect(writes[1]).toMatchObject({
			engine_id: "claude",
			model: "claude-opus-4-1",
			reasoning_effort: "high",
			service_tier: "priority",
		});
	});

	it("accepts an authoritative reconciliation result after an ambiguous write", async () => {
		const reconciled = { ...initial_policy, reasoning_effort: "low" as const };
		const current = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeModelPolicyController;
				yield* controller.SetAuthoritative(initial_policy);
				yield* controller.Patch({ reasoning_effort: "high" });
				const result = yield* controller.Flush(() =>
					Effect.gen(function* () {
						/** Simulates an ambiguous command failure followed by a successful query. */
						yield* Effect.void;
						return reconciled;
					}),
				);
				expect(result.confirmed).toEqual([reconciled]);
				return result.current;
			}),
		);

		expect(current).toEqual(reconciled);
	});

	it("restores the last authoritative policy when persistence and reconciliation fail", async () => {
		const current = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeModelPolicyController;
				yield* controller.SetAuthoritative(initial_policy);
				yield* controller.Patch({ reasoning_effort: "high" });
				const exit = yield* Effect.exit(
					controller.Flush(() =>
						Effect.gen(function* () {
							return yield* Effect.fail({
								message: "authoritative query unavailable",
							});
						}),
					),
				);
				expect(exit._tag).toBe("Failure");
				if (exit._tag === "Failure") {
					expect(exit.cause.toString()).toContain(ModelPolicyMutationError.name);
				}
				return yield* controller.Current;
			}),
		);

		expect(current).toEqual(initial_policy);
	});

	it("suppresses duplicate repair requests for the same stale policy", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeModelPolicyController;
				yield* controller.SetAuthoritative(initial_policy);
				const repaired = { ...initial_policy, permission_mode: "never" as const };
				const first = yield* controller.RequestRepair(repaired);
				const duplicate = yield* controller.RequestRepair(repaired);
				const writes = yield* Ref.make(0);
				yield* controller.Flush((desired) =>
					Effect.gen(function* () {
						yield* Ref.update(writes, (current) => current + 1);
						return desired;
					}),
				);
				const after_confirmation = yield* controller.RequestRepair(repaired);
				return { after_confirmation, duplicate, first, writes: yield* Ref.get(writes) };
			}),
		);

		expect(result).toEqual({
			after_confirmation: false,
			duplicate: false,
			first: true,
			writes: 1,
		});
	});
});
