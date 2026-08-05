import { Effect } from "effect";

import type { EngineRunTerminalState } from "../engine";

/** Configures one engine run's stalled-stream deadline. @since 0.7.0 */
export interface EngineInactivityOptions<E, R> {
	/** Reads the run's monotonic observation counter. */
	readonly Activity: Effect.Effect<number>;
	/** Cancels the deadline as soon as any terminal state is observed. */
	readonly Closed: Effect.Effect<EngineRunTerminalState>;
	/**
	 * Reports whether the run owes output right now. A session that stays open
	 * between turns is legitimately silent while idle, so silence only counts
	 * against a run that is expected to be producing.
	 */
	readonly Expecting: Effect.Effect<boolean>;
	/** Silence, measured only while the host is awake, that settles the run. */
	readonly inactivity_ms: number;
	/** Settles the run once a whole window closes with nothing observed. */
	readonly OnStall: Effect.Effect<void, E, R>;
}

/**
 * Sub-windows per deadline. More probes tighten when a stall is noticed
 * without shortening the silence a healthy run is allowed.
 */
const probe_count = 4;

const AttendProbe = <E, R>(
	options: EngineInactivityOptions<E, R>,
	observed: number,
	idle_probes: number,
): Effect.Effect<void, E, R> =>
	Effect.gen(function* () {
		yield* Effect.sleep(options.inactivity_ms / probe_count);

		const expecting = yield* options.Expecting;
		const current = yield* options.Activity;

		if (!expecting || current !== observed) return yield* AttendProbe(options, current, 0);

		return yield* idle_probes + 1 >= probe_count
			? options.OnStall
			: AttendProbe(options, observed, idle_probes + 1);
	});

/**
 * Watches one engine run for a stream that has stopped producing.
 *
 * An engine that survives a host suspend keeps a live process and an open
 * stdio pipe while its own connection to the provider is silently dead, so a
 * total-run deadline cannot tell a healthy long run from a stalled one. This
 * deadline measures silence instead, and every observation re-arms it.
 *
 * Only silence while the run is `Expecting` output counts, so a transport
 * whose session outlives a single turn is not settled for idling between
 * them.
 *
 * The window is measured with `Effect.sleep` alone and never the wall clock,
 * because libuv's monotonic clock stops while the host is suspended: a
 * three-hour hibernate must not be charged to the run as three hours of
 * silence. The cost is granularity — a stall is noticed somewhere between one
 * window and one window plus a probe after the last observation. That slack is
 * deliberate, since a run killed for a stall it never had is worse than one
 * settled a little late.
 *
 * @since 0.7.0
 */
export const WatchEngineInactivity = <E, R>(options: EngineInactivityOptions<E, R>) =>
	Effect.raceFirst(
		options.Closed.pipe(Effect.asVoid),
		Effect.gen(function* () {
			const observed = yield* options.Activity;

			return yield* AttendProbe(options, observed, 0);
		}),
	);
