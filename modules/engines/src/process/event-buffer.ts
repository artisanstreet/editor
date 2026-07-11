import { Cause, Deferred, Effect, Queue, Ref, Semaphore, Stream } from "effect";

import {
	type EngineObservation,
	type EngineRunTerminalObservation,
	type EngineRunTerminalState,
	EngineBackpressureError,
} from "../engine";

/** Configures bounded ordered delivery and exact-one terminal closure for one engine run. @since 0.4.0 */
export interface EngineEventBufferOptions {
	readonly artisan_run_id: string;
	readonly BeforeEnqueue?: (observation: EngineObservation) => Effect.Effect<void>;
	readonly BeforeFinish?: Effect.Effect<void>;
	readonly capacity: number;
	readonly CloseResource: Effect.Effect<void>;
	readonly make_terminal_observation: (
		terminal_state: EngineRunTerminalState,
		sequence: number,
	) => EngineRunTerminalObservation;
}

interface EngineEventBufferState {
	readonly buffered_count: number;
	readonly closed: boolean;
	readonly next_sequence: number;
}

type EmitReservation =
	| { readonly _tag: "full" }
	| { readonly _tag: "rejected" }
	| { readonly _tag: "reserved"; readonly sequence: number };

/** Owns bounded observations, contiguous sequencing, and exact-one terminal closure. @since 0.4.0 */
export function MakeEngineEventBuffer(options: EngineEventBufferOptions) {
	return Effect.gen(function* () {
		const events = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
		const closed = yield* Deferred.make<EngineRunTerminalState>();
		const lock = yield* Semaphore.make(1);
		const state = yield* Ref.make<EngineEventBufferState>({
			buffered_count: 0,
			closed: false,
			next_sequence: 0,
		});

		const BeginFinishUnlocked = (terminal_state: EngineRunTerminalState) =>
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

				yield* Queue.offer(
					events,
					options.make_terminal_observation(terminal_state, sequence),
				);
				yield* Deferred.succeed(closed, terminal_state);
				yield* Queue.end(events);

				return true;
			});
		const Finish = (terminal_state: EngineRunTerminalState) =>
			Effect.gen(function* () {
				yield* options.BeforeFinish ?? Effect.void;
				const should_close = yield* Semaphore.withPermit(lock)(
					BeginFinishUnlocked(terminal_state),
				);
				if (should_close) yield* options.CloseResource;
			}).pipe(Effect.uninterruptible);
		const Emit = (observation: EngineObservation) =>
			Effect.gen(function* () {
				const decision = yield* Semaphore.withPermit(lock)(
					Effect.gen(function* () {
						const reservation = yield* Ref.modify<
							EngineEventBufferState,
							EmitReservation
						>(state, (current) => {
							if (current.closed) return [{ _tag: "rejected" }, current] as const;
							if (current.buffered_count >= options.capacity)
								return [{ _tag: "full" }, current] as const;
							const sequence = current.next_sequence + 1;
							return [
								{ _tag: "reserved", sequence },
								{
									...current,
									buffered_count: current.buffered_count + 1,
									next_sequence: sequence,
								},
							] as const;
						});
						if (reservation._tag === "rejected")
							return { close_resource: false, rejected: true } as const;
						if (reservation._tag === "full") {
							const should_close = yield* BeginFinishUnlocked("failed");
							return { close_resource: should_close, rejected: true } as const;
						}
						const queued = {
							...observation,
							observation_id: `${observation.observation_id}:sequence:${reservation.sequence}`,
							sequence: reservation.sequence,
						} satisfies EngineObservation;
						yield* options.BeforeEnqueue?.(queued) ?? Effect.void;
						yield* Queue.offer(events, queued);
						return { close_resource: false, rejected: false } as const;
					}),
				);
				if (decision.close_resource) yield* options.CloseResource;
				if (decision.rejected)
					return yield* Effect.fail(
						new EngineBackpressureError({
							artisan_run_id: options.artisan_run_id,
							capacity: options.capacity,
						}),
					);
			}).pipe(Effect.uninterruptible);
		const IsClosed = Ref.get(state).pipe(Effect.map((current) => current.closed));
		const Events = Stream.fromQueue(events).pipe(
			Stream.mapEffect((event) =>
				Ref.update(state, (current) => ({
					...current,
					buffered_count: Math.max(0, current.buffered_count - 1),
				})).pipe(Effect.as(event)),
			),
		);

		return { Closed: Deferred.await(closed), Emit, Events, Finish, IsClosed };
	});
}
