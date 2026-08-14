import { Cause, Deferred, Effect, Queue, Ref, Semaphore, Stream } from "effect";

import {
	type EngineObservation,
	type EngineErrorRef,
	type EngineRunTerminalObservation,
	type EngineRunTerminalState,
	EngineBackpressureError,
} from "../engine";

/** Configures bounded ordered delivery and exact-one terminal closure for one engine run. @since 0.4.0 */
export interface EngineEventBufferOptions {
	readonly artisan_run_id: string;
	/** Potentially suspending, non-canonical preparation that never owns queue capacity. */
	readonly BeforePrepare?: (observation: EngineObservation) => Effect.Effect<void>;
	/** Atomic commit fence receiving the final sequence and observation id. */
	readonly BeforeEnqueue?: (observation: EngineObservation) => Effect.Effect<void>;
	readonly BeforeFinish?: Effect.Effect<void>;
	/** Maximum time a producer may wait for the canonical consumer before failing the run. */
	readonly backpressure_timeout_ms?: number;
	readonly capacity: number;
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

interface BufferedEvent {
	readonly event: EngineObservation;
	readonly releases_slot: boolean;
}

type EmitReservation =
	| { readonly _tag: "rejected" }
	| { readonly _tag: "reserved"; readonly sequence: number };

const default_backpressure_timeout_ms = 30_000;

/** Never let a failed run reach persistence without a reader-safe explanation. */
const generic_failed_run_error: EngineErrorRef = {
	artisan_code: "AE-RUN-301",
	detail: "The engine stopped before the run could finish.",
};

/** Owns bounded observations, contiguous sequencing, and exact-one terminal closure. @since 0.4.0 */
export function MakeEngineEventBuffer(options: EngineEventBufferOptions) {
	return Effect.gen(function* () {
		const backpressure_timeout_ms =
			options.backpressure_timeout_ms ?? default_backpressure_timeout_ms;
		const events = yield* Queue.unbounded<BufferedEvent, Cause.Done<void>>();
		const closed = yield* Deferred.make<EngineRunTerminalState>();
		const lock = yield* Semaphore.make(1);
		const preparation_lock = yield* Semaphore.make(1);
		const slots = yield* Semaphore.make(options.capacity);
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
					event: {
						...options.make_terminal_observation(terminal_state, sequence, failure),
						...(failure === undefined ? {} : { error_ref: failure }),
					},
					releases_slot: false,
				});
				yield* Deferred.succeed(closed, terminal_state);
				yield* Queue.end(events);

				return true;
			});
		const BeginBackpressureFinishUnlocked = () =>
			Effect.gen(function* () {
				const sequences = yield* Ref.modify(state, (current) =>
					current.closed
						? ([undefined, current] as const)
						: ([
								[current.next_sequence + 1, current.next_sequence + 2] as const,
								{
									...current,
									closed: true,
									next_sequence: current.next_sequence + 2,
								},
							] as const),
				);

				if (sequences === undefined) return false;

				const [diagnostic_sequence, terminal_sequence] = sequences;
				yield* Queue.offer(events, {
					event: {
						_tag: "process_diagnostic",
						artisan_run_id: options.artisan_run_id,
						level: "error",
						message: "Engine event buffer remained full and the run was stopped.",
						observation_id: `${options.artisan_run_id}:event-buffer-backpressure:${diagnostic_sequence}`,
						raw: {
							engine_id: "event-buffer",
							frame: {
								backpressure_timeout_ms,
								capacity: options.capacity,
								source: "engine-event-buffer",
							},
							transport: "internal",
						},
						sequence: diagnostic_sequence,
					},
					releases_slot: false,
				});
				const failure: EngineErrorRef = {
					artisan_code: "AE-RUN-301",
					detail: "The engine could not deliver observations fast enough to continue safely.",
				};
				yield* Queue.offer(events, {
					event: {
						...options.make_terminal_observation("failed", terminal_sequence, failure),
						error_ref: failure,
					},
					releases_slot: false,
				});
				yield* Deferred.succeed(closed, "failed");
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
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const prepared = yield* Semaphore.withPermit(preparation_lock)(
						Effect.gen(function* () {
							const current = yield* Ref.get(state);
							if (current.closed) return false;
							/** Preparation never owns a queued-event capacity slot. */
							yield* options.BeforePrepare?.(observation) ?? Effect.void;
							return true;
						}),
					);
					if (!prepared)
						return yield* Effect.fail(
							new EngineBackpressureError({
								artisan_run_id: options.artisan_run_id,
								capacity: options.capacity,
							}),
						);

					const availability = yield* restore(
						Effect.race(
							slots.take(1).pipe(Effect.as("acquired" as const)),
							Deferred.await(closed).pipe(Effect.as("closed" as const)),
						).pipe(
							Effect.timeoutOrElse({
								duration: backpressure_timeout_ms,
								orElse: () => Effect.succeed("timed_out" as const),
							}),
						),
					);

					if (availability === "closed")
						return yield* Effect.fail(
							new EngineBackpressureError({
								artisan_run_id: options.artisan_run_id,
								capacity: options.capacity,
							}),
						);
					if (availability === "timed_out") {
						const should_close = yield* Semaphore.withPermit(lock)(
							BeginBackpressureFinishUnlocked(),
						);
						if (should_close) yield* options.CloseResource;
						return yield* Effect.fail(
							new EngineBackpressureError({
								artisan_run_id: options.artisan_run_id,
								capacity: options.capacity,
							}),
						);
					}

					const reservation = yield* Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const reservation = yield* Ref.modify<
								EngineEventBufferState,
								EmitReservation
							>(state, (current) => {
								if (current.closed) return [{ _tag: "rejected" }, current] as const;
								const sequence = current.next_sequence + 1;
								return [
									{ _tag: "reserved", sequence },
									{ ...current, next_sequence: sequence },
								] as const;
							});
							if (reservation._tag === "reserved") {
								const queued = {
									...observation,
									observation_id: `${observation.observation_id}:sequence:${reservation.sequence}`,
									sequence: reservation.sequence,
								} satisfies EngineObservation;
								yield* options.BeforeEnqueue?.(queued) ?? Effect.void;
								yield* Queue.offer(events, { event: queued, releases_slot: true });
							}
							return reservation;
						}),
					);

					if (reservation._tag === "reserved") return;

					yield* slots.release(1);
					return yield* Effect.fail(
						new EngineBackpressureError({
							artisan_run_id: options.artisan_run_id,
							capacity: options.capacity,
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
		const Events = Stream.fromQueue(events).pipe(
			Stream.mapEffect(({ event, releases_slot }) =>
				Effect.gen(function* () {
					if (releases_slot) yield* slots.release(1);
					return event;
				}),
			),
		);

		return { Activity, Closed: Deferred.await(closed), Emit, Events, Finish, IsClosed };
	});
}
