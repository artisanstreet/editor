import { goto, type GotoOptions } from "$app/navigation";
import { Context, Data, Effect, Layer } from "effect";

/** A SvelteKit navigation could not be completed. */
export class RouteNavigationFailure extends Data.TaggedError("RouteNavigationFailure")<{
	readonly cause: unknown;
}> {}

/** Browser navigation is a capability so routed components only describe it. */
export class RouteNavigation extends Context.Service<
	RouteNavigation,
	{
		readonly Navigate: (
			path: string | URL,
			options?: GotoOptions,
		) => Effect.Effect<void, RouteNavigationFailure>;
	}
>()("Artisan/RouteNavigation") {}

/** The only browser adapter for SvelteKit's imperative `goto` API. */
export const RouteNavigationLive = Layer.effect(
	RouteNavigation,
	Effect.gen(function* () {
		const Navigate = (path: string | URL, options?: GotoOptions) =>
			Effect.gen(function* () {
				yield* Effect.tryPromise({
					try: () => goto(path, options),
					catch: (cause) => new RouteNavigationFailure({ cause }),
				});
			});

		return RouteNavigation.of({ Navigate });
	}),
);
