import { Context, Effect, Layer, Stream, SubscriptionRef } from "effect";

import { RunBrowserDom } from "./dom";

/** A page is being watched only while its document is both focused and visible. */
export const ReaderIsWatching = (has_focus: boolean, visibility_state: string): boolean =>
	has_focus && visibility_state === "visible";

/** Root activity is read only while the root transcript itself is on screen. */
export const reader_can_acknowledge_root_conversation = (
	watching: boolean,
	inspecting_agent: boolean,
): boolean => watching && !inspecting_agent;

interface ReaderAttentionDocument extends Stream.EventListener<unknown> {
	hasFocus(): boolean;
	readonly visibilityState: string;
}

interface ReaderAttentionTargets {
	readonly document: ReaderAttentionDocument;
	readonly window: Stream.EventListener<unknown>;
}

/**
 * The renderer is also typechecked by Node-side tooling, so this browser-only
 * adapter describes exactly the host surface it consumes instead of importing
 * the full DOM ambient library into shared workspace TypeScript.
 */
const BrowserReaderAttentionTargets = (): ReaderAttentionTargets => {
	const host = globalThis as {
		readonly document?: ReaderAttentionDocument;
		readonly window?: Stream.EventListener<unknown>;
	};
	if (host.document === undefined || host.window === undefined)
		throw new Error("Browser reader-attention targets are unavailable.");
	return { document: host.document, window: host.window };
};

const ReadBrowserReaderAttention = Effect.gen(function* () {
	return yield* RunBrowserDom(() => {
		const targets = BrowserReaderAttentionTargets();
		return ReaderIsWatching(targets.document.hasFocus(), targets.document.visibilityState);
	});
});

/** Owns the renderer-wide focus/visibility signal used by reads and notifications. */
export class BrowserReaderAttention extends Context.Service<
	BrowserReaderAttention,
	{
		readonly Changes: Stream.Stream<boolean>;
		readonly Current: Effect.Effect<boolean>;
	}
>()("Artisan/BrowserReaderAttention") {}

export const BrowserReaderAttentionLive = Layer.effect(
	BrowserReaderAttention,
	Effect.gen(function* () {
		const attention = yield* SubscriptionRef.make(yield* ReadBrowserReaderAttention);
		const targets = yield* RunBrowserDom(BrowserReaderAttentionTargets);
		const Refresh = Effect.gen(function* () {
			yield* SubscriptionRef.set(attention, yield* ReadBrowserReaderAttention);
		});
		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(attention);
		});

		yield* Stream.mergeAll(
			[
				Stream.fromEventListener(targets.window, "focus"),
				Stream.fromEventListener(targets.window, "blur"),
				Stream.fromEventListener(targets.document, "visibilitychange"),
			],
			{ concurrency: "unbounded" },
		).pipe(
			Stream.runForEach(() => Refresh),
			Effect.forkScoped({ startImmediately: true }),
		);

		return BrowserReaderAttention.of({
			Changes: SubscriptionRef.changes(attention),
			Current,
		});
	}),
);
