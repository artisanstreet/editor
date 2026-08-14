import { Data, Effect } from "effect";
import { RunBrowserDom } from "$lib/browser/dom";

/** A renderer chunk failed to warm; first-sight rendering pays it instead. */
class ConversationRendererWarmupFailure extends Data.TaggedError(
	"ConversationRendererWarmupFailure",
)<{
	readonly cause: unknown;
}> {}

/**
 * The exact dynamic imports a settled message pays at first sight: Shiki's
 * core-and-theme graph behind code fences, KaTeX behind math, and the Mermaid
 * renderer behind diagrams. Warming fetches and evaluates those chunks;
 * nothing is constructed — highlighters, grammars, and renders still wait for
 * a message that proves which ones it needs.
 */
const conversation_renderer_loads: ReadonlyArray<() => Promise<unknown>> = [
	() => import("./settled-highlighting"),
	() => import("./math-rendering"),
	() => import("katex/dist/katex.min.css"),
	() => import("./mermaid-rendering"),
];

/**
 * One idle slice per chunk keeps each module's main-thread evaluation out of
 * hydration and reveal-animation frames. The timeout is the starvation valve:
 * a renderer that never reports idle still warms eventually. Safari has no
 * requestIdleCallback; a paced sleep is the closest quiet moment it offers.
 */
const WaitForRendererIdle = Effect.gen(function* () {
	const supports_idle = yield* RunBrowserDom(
		() => typeof globalThis.requestIdleCallback === "function",
	).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return false;
			}),
		),
	);
	if (!supports_idle) {
		yield* Effect.sleep("1 second");
		return;
	}
	yield* Effect.callback<void>((resume) => {
		const handle = globalThis.requestIdleCallback(() => resume(Effect.gen(function* () {})), {
			timeout: 10_000,
		});
		return Effect.gen(function* () {
			globalThis.cancelIdleCallback(handle);
		});
	});
});

/**
 * Pre-loads every conversation renderer a first settled message would
 * otherwise pause on. Warming is best effort: a chunk that fails here is
 * simply paid again, with the same graceful degradation, at first real use.
 */
export const WarmConversationRenderers = Effect.gen(function* () {
	for (const load of conversation_renderer_loads) {
		yield* WaitForRendererIdle;
		yield* Effect.tryPromise({
			catch: (cause) => new ConversationRendererWarmupFailure({ cause }),
			try: load,
		}).pipe(Effect.ignore);
	}
});
