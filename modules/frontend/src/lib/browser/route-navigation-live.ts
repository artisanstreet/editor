import { goto } from "$app/navigation";
import { Effect, Layer } from "effect";

import {
	RouteNavigation,
	RouteNavigationFailure,
	type RouteNavigationOptions,
} from "./route-navigation";

/** The only browser adapter for SvelteKit's imperative `goto` API. */
export const RouteNavigationLive = Layer.effect(
	RouteNavigation,
	Effect.gen(function* () {
		const Navigate = (path: string | URL, options?: RouteNavigationOptions) =>
			Effect.gen(function* () {
				yield* Effect.tryPromise({
					try: () => goto(path, options),
					catch: (cause) => new RouteNavigationFailure({ cause }),
				});
			});

		return RouteNavigation.of({ Navigate });
	}),
);
