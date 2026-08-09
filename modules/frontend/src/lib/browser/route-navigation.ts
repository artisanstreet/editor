import { Context, Data, Effect } from "effect";

/** A SvelteKit navigation could not be completed. */
export class RouteNavigationFailure extends Data.TaggedError("RouteNavigationFailure")<{
	readonly cause: unknown;
}> {}

/**
 * The navigation options the app actually asks for, declared here rather than
 * imported from `$app/navigation`. Each is a `goto` option the adapter passes
 * straight through; naming them is what keeps this module free of SvelteKit
 * and so importable from a test.
 */
export interface RouteNavigationOptions {
	readonly keepFocus?: boolean;
	readonly noScroll?: boolean;
	readonly replaceState?: boolean;
}

/**
 * Browser navigation is a capability so routed components only describe it.
 *
 * The capability is declared apart from its adapter for the same reason the
 * banner presenter is: consumers — and the tests that drive them — need the
 * tag without `$app/navigation`, which resolves only inside a built SvelteKit
 * app and so cannot be reached from the workspace test runner.
 */
export class RouteNavigation extends Context.Service<
	RouteNavigation,
	{
		readonly Navigate: (
			path: string | URL,
			options?: RouteNavigationOptions,
		) => Effect.Effect<void, RouteNavigationFailure>;
	}
>()("Artisan/RouteNavigation") {}
