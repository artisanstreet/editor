import { describe, expect, it } from "@effect/vitest";
import { Context, Effect, Layer, Scope } from "effect";

import { FrontendComponentScopeLive } from "../../modules/frontend/src/lib/runtime/frontend-runtime";

describe("frontend component scope", () => {
	it.effect("provides an app-lifetime Scope and closes registered component finalizers", () =>
		Effect.gen(function* () {
			let finalized = false;
			yield* Effect.scoped(
				Effect.gen(function* () {
					const context = yield* Layer.build(FrontendComponentScopeLive);
					const component_scope = Context.get(context, Scope.Scope);
					yield* Scope.addFinalizer(
						component_scope,
						Effect.sync(() => {
							finalized = true;
						}),
					);
				}),
			);
			expect(finalized).toBe(true);
		}),
	);
});
