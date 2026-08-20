// oxlint-disable-next-line typescript/triple-slash-reference
/// <reference path="../../../app.d.ts" />
import {
	Context,
	Data,
	Effect,
	Fiber,
	Layer,
	Ref,
	Semaphore,
	Stream,
	SubscriptionRef,
} from "effect";

import type { MathRenderResult } from "./math-rendering";

export type ConversationMathRenderer = (source: string, display_mode: boolean) => MathRenderResult;

/** A dynamic KaTeX import failed; the escaped math fallback remains authoritative. */
export class MathRendererLoadFailure extends Data.TaggedError("MathRendererLoadFailure")<{
	readonly cause: unknown;
}> {}

export type MathRendererState =
	| { readonly _tag: "Loading" }
	| { readonly _tag: "Ready"; readonly render: ConversationMathRenderer }
	| { readonly _tag: "Unavailable" };

export interface MathRendererLoader {
	readonly LoadCss: Effect.Effect<void, MathRendererLoadFailure>;
	readonly LoadRenderer: Effect.Effect<ConversationMathRenderer, MathRendererLoadFailure>;
}

type ActiveLoad = {
	readonly fiber?: Fiber.Fiber<void, unknown>;
	readonly generation: number;
};

/**
 * Owns the one lazy KaTeX load for all transcript math. Individual nodes render
 * their escaped source while this app-scoped fiber resolves the renderer.
 */
export class MathRendererController extends Context.Service<
	MathRendererController,
	{
		readonly Changes: Stream.Stream<MathRendererState>;
		readonly Current: Effect.Effect<MathRendererState>;
		/** Retries only after an unavailable result; ready and admitted loads are retained. */
		readonly Refresh: Effect.Effect<void>;
	}
>()("Artisan/MathRendererController") {}

export const MakeMathRendererControllerLive = (loader: MathRendererLoader) =>
	Layer.effect(
		MathRendererController,
		Effect.gen(function* () {
			const controller_scope = yield* Effect.scope;
			const active = yield* Ref.make<ActiveLoad | undefined>(undefined);
			const generation = yield* Ref.make(0);
			const refresh_lock = yield* Semaphore.make(1);
			const state = yield* SubscriptionRef.make<MathRendererState>({ _tag: "Loading" });

			const Complete = (request_generation: number) =>
				Ref.update(active, (current) =>
					current?.generation === request_generation ? undefined : current,
				);

			const Load = (request_generation: number) =>
				Effect.all([loader.LoadCss, loader.LoadRenderer], {
					concurrency: "unbounded",
				}).pipe(
					Effect.flatMap(([, render]) =>
						Effect.gen(function* () {
							if ((yield* Ref.get(generation)) !== request_generation) return;
							yield* SubscriptionRef.set(state, { _tag: "Ready", render });
						}),
					),
					Effect.catch(() =>
						Effect.gen(function* () {
							if ((yield* Ref.get(generation)) !== request_generation) return;
							yield* SubscriptionRef.set(state, { _tag: "Unavailable" });
						}),
					),
					Effect.ensuring(Complete(request_generation)),
				);

			const Refresh = Effect.uninterruptible(
				refresh_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* SubscriptionRef.get(state);
						if (current._tag === "Ready" || (yield* Ref.get(active)) !== undefined)
							return;
						const request_generation = yield* Ref.updateAndGet(
							generation,
							(current) => current + 1,
						);
						yield* SubscriptionRef.set(state, { _tag: "Loading" });
						yield* Ref.set(active, { generation: request_generation });
						const fiber = yield* Effect.forkIn(
							Load(request_generation),
							controller_scope,
						);
						yield* Ref.update(active, (active_load) =>
							active_load?.generation === request_generation
								? { fiber, generation: request_generation }
								: active_load,
						);
					}),
				),
			);

			return MathRendererController.of({
				Changes: SubscriptionRef.changes(state),
				Current: SubscriptionRef.get(state),
				Refresh,
			});
		}),
	);

const BrowserMathRendererLoader: MathRendererLoader = {
	LoadCss: Effect.tryPromise({
		catch: (cause) => new MathRendererLoadFailure({ cause }),
		try: () => import("katex/dist/katex.min.css"),
	}).pipe(Effect.asVoid),
	LoadRenderer: Effect.tryPromise({
		catch: (cause) => new MathRendererLoadFailure({ cause }),
		try: async () => {
			const module = await import("./math-rendering");
			return module.render_conversation_math;
		},
	}),
};

export const MathRendererControllerLive = MakeMathRendererControllerLive(BrowserMathRendererLoader);
