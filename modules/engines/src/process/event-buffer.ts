import { Cause, Deferred, Effect, Queue, Ref, Semaphore, Stream } from "effect";

import {
	type EngineObservation,
	type EngineErrorRef,
	type EngineRunTerminalObservation,
	type EngineRunTerminalState,
} from "../engine";

/** Configures ordered lossless delivery and exact-one terminal closure for one engine run. @since 0.4.0 */
export interface EngineEventBufferOptions {
	readonly artisan_run_id: string;
	/** Potentially suspending, non-canonical preparation before an observation is queued. */
	readonly BeforePrepare?: (observation: EngineObservation) => Effect.Effect<void>;
	/** Atomic commit fence receiving the final sequence and observation id. */
	readonly BeforeEnqueue?: (observation: EngineObservation) => Effect.Effect<void>;
	readonly BeforeFinish?: Effect.Effect<void>;
	readonly CloseResource: Effect.Effect<void>;
	readonly make_terminal_observation: (
		terminal_state: EngineRunTerminalState,
		sequence: number,
		error_ref?: EngineErrorRef,
	) => EngineRunTerminalObservation;
}

interface EngineEventBufferState {
	readonly closed: boolean;
	readonly next_sequence: number;
}

/** Never let a failed run reach persistence without a reader-safe explanation. */
const generic_failed_run_error: EngineErrorRef = {
	artisan_code: "AE-RUN-301",
	detail: "The engine stopped before the run could finish.",
};

/** Owns lossless observations, contiguous sequencing, and exact-one terminal closure. @since 0.4.0 */
export function MakeEngineEventBuffer(options: EngineEventBufferOptions) {
	return Effect.gen(function* () {
		const events = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
		const closed = yield* Deferred.make<EngineRunTerminalState>();
		const lock = yield* Semaphore.make(1);
		const preparation_lock = yield* Semaphore.make(1);
		const state = yield* Ref.make<EngineEventBufferState>({
			closed: false,
			next_sequence: 0,
		});

		const BeginFinishUnlocked = (
			terminal_state: EngineRunTerminalState,
			error_ref?: EngineErrorRef,
		) =>
			Effect.gen(function* () {
				const sequence = yield* Ref.modify(state, (current) =>
					current.closed
						? ([undefined, current] as const)
						: ([
								current.next_sequence + 1,
								{
									...current,
									closed: true,
									next_sequence: current.next_sequence + 1,
								},
							] as const),
				);

				if (sequence === undefined) return false;

				const failure =
					terminal_state === "failed"
						? (error_ref ?? generic_failed_run_error)
						: undefined;
				yield* Queue.offer(events, {
					...options.make_terminal_observation(terminal_state, sequence, failure),
					...(failure === undefined ? {} : { error_ref: failure }),
				});
				yield* Deferred.succeed(closed, terminal_state);
				yield* Queue.end(events);

				return true;
			});
		const Finish = (terminal_state: EngineRunTerminalState, error_ref?: EngineErrorRef) =>
			Effect.gen(function* () {
				yield* options.BeforeFinish ?? Effect.void;
				const should_close = yield* Semaphore.withPermit(lock)(
					BeginFinishUnlocked(terminal_state, error_ref),
				);
				if (should_close) yield* options.CloseResource;
			}).pipe(Effect.uninterruptible);
		const Emit = (observation: EngineObservation) =>
			Effect.uninterruptible(
				Effect.gen(function* () {
					const prepared = yield* Semaphore.withPermit(preparation_lock)(
						Effect.gen(function* () {
							const current = yield* Ref.get(state);
							if (current.closed) return false;
							yield* options.BeforePrepare?.(observation) ?? Effect.void;
							return true;
						}),
					);
					if (!prepared) return;

					yield* Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const sequence = yield* Ref.modify(state, (current) => {
								if (current.closed) return [undefined, current] as const;
								const sequence = current.next_sequence + 1;
								return [sequence, { ...current, next_sequence: sequence }] as const;
							});
							if (sequence === undefined) return;
							const queued = {
								...observation,
								observation_id: `${observation.observation_id}:sequence:${sequence}`,
								sequence,
							} satisfies EngineObservation;
							yield* options.BeforeEnqueue?.(queued) ?? Effect.void;
							yield* Queue.offer(events, queued);
						}),
					);
				}),
			);
		/**
		 * Advances on every reserved observation and never resets, so a reader
		 * can tell whether the run produced anything between two points in time
		 * without holding the observations themselves.
		 */
		const Activity = Ref.get(state).pipe(Effect.map((current) => current.next_sequence));
		const IsClosed = Ref.get(state).pipe(Effect.map((current) => current.closed));
		const Events = Stream.fromQueue(events);

		return { Activity, Closed: Deferred.await(closed), Emit, Events, Finish, IsClosed };
	});
}
