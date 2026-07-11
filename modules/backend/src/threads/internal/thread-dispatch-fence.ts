import { Deferred, Effect, Exit, Option, Ref } from "effect";

interface ThreadDispatchState {
	readonly active: number;
	readonly completion?: Deferred.Deferred<Exit.Exit<void, never>>;
	readonly drained?: Deferred.Deferred<void>;
	readonly quiesced: boolean;
}

interface ThreadQuiesceDecision {
	readonly completion: Deferred.Deferred<Exit.Exit<void, never>>;
	readonly drained: Deferred.Deferred<void> | undefined;
	readonly owner: boolean;
}

/** Serializes permanent thread quiescence after all admitted dispatch side effects drain. */
export interface ThreadDispatchFence {
	readonly Quiesce: (
		thread_id: string,
		after_drained: Effect.Effect<void>,
	) => Effect.Effect<void>;
	readonly Run: <A, E, R>(
		thread_id: string,
		operation: Effect.Effect<A, E, R>,
	) => Effect.Effect<Option.Option<A>, E, R>;
}

/** Creates a per-runtime dispatch fence which preserves concurrent work within active threads. */
export const MakeThreadDispatchFence = Effect.gen(function* () {
	const states = yield* Ref.make(new Map<string, ThreadDispatchState>());

	const Enter = (thread_id: string) =>
		Ref.modify(states, (current) => {
			const state = current.get(thread_id);

			if (state?.quiesced) {
				return [false, current] as const;
			}

			const next = new Map(current);

			next.set(thread_id, {
				active: (state?.active ?? 0) + 1,
				quiesced: false,
			});

			return [true, next] as const;
		});

	const Leave = (thread_id: string) =>
		Ref.modify(states, (current) => {
			const state = current.get(thread_id);

			if (!state || state.active === 0) {
				return [undefined, current] as const;
			}

			const active = state.active - 1;
			const next = new Map(current);

			if (active === 0 && !state.quiesced) {
				next.delete(thread_id);
			} else {
				next.set(thread_id, { ...state, active });
			}

			return [active === 0 ? state.drained : undefined, next] as const;
		}).pipe(
			Effect.flatMap((drained) =>
				drained ? Deferred.succeed(drained, undefined).pipe(Effect.asVoid) : Effect.void,
			),
		);

	const Run: ThreadDispatchFence["Run"] = (thread_id, operation) =>
		Effect.uninterruptibleMask((restore) =>
			Effect.flatMap(Enter(thread_id), (entered) =>
				entered
					? restore(operation).pipe(
							Effect.map(Option.some),
							Effect.ensuring(Leave(thread_id)),
						)
					: Effect.succeed(Option.none()),
			),
		);

	const Quiesce: ThreadDispatchFence["Quiesce"] = (thread_id, after_drained) =>
		Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const completion_candidate = yield* Deferred.make<Exit.Exit<void, never>>();
				const drain_candidate = yield* Deferred.make<void>();
				const decision = yield* Ref.modify(
					states,
					(
						current,
					): readonly [ThreadQuiesceDecision, Map<string, ThreadDispatchState>] => {
						const state = current.get(thread_id);

						if (state?.quiesced && state.completion) {
							return [
								{
									completion: state.completion,
									drained: state.drained,
									owner: false,
								},
								current,
							] as const;
						}

						const active = state?.active ?? 0;
						const next = new Map(current);

						next.set(thread_id, {
							active,
							completion: completion_candidate,
							...(active > 0 ? { drained: drain_candidate } : {}),
							quiesced: true,
						});

						return [
							{
								completion: completion_candidate,
								drained: active > 0 ? drain_candidate : undefined,
								owner: true,
							},
							next,
						] as const;
					},
				);

				if (!decision.owner) {
					const completed = yield* restore(Deferred.await(decision.completion));

					return yield* Exit.isSuccess(completed)
						? Effect.void
						: Effect.failCause(completed.cause);
				}

				const completed = yield* restore(
					Effect.gen(function* () {
						if (decision.drained) {
							yield* Deferred.await(decision.drained);
						}

						yield* after_drained;
					}),
				).pipe(Effect.exit);

				yield* Deferred.succeed(decision.completion, completed);

				return yield* Exit.isSuccess(completed)
					? Effect.void
					: Effect.failCause(completed.cause);
			}),
		);

	return { Quiesce, Run } satisfies ThreadDispatchFence;
});
