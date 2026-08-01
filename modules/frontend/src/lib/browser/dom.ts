import { Data, Effect } from "effect";

/** A narrowly scoped synchronous browser-DOM operation failed. */
export class BrowserDomFailure extends Data.TaggedError("BrowserDomFailure")<{
	readonly cause: unknown;
	readonly message: string;
}> {}

/**
 * Describes one fallible host-DOM operation. Callers yield this at their SER
 * boundary rather than performing browser work eagerly in component code.
 */
export const RunBrowserDom = <Value>(operation: () => Value) =>
	Effect.gen(function* () {
		return yield* Effect.try({
			catch: (cause) =>
				new BrowserDomFailure({
					cause,
					message:
						cause instanceof Error ? cause.message : "Browser DOM operation failed.",
				}),
			try: operation,
		});
	});
