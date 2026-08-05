import { Clock, Context, Effect, Layer, PubSub, Scope } from "effect";

/** Reports one host suspend, measured between two heartbeat ticks. @since 0.7.0 */
export interface HostResume {
	/** Wall-clock instant the gap was noticed, in epoch milliseconds. */
	readonly resumed_at_ms: number;
	/** Wall-clock milliseconds the host spent away. */
	readonly suspended_ms: number;
}

/** Configures the heartbeat that detects a host suspend. @since 0.7.0 */
export interface HostSuspendMonitorOptions {
	/** Spacing between ticks, and so the worst-case delay before a resume is noticed. */
	readonly heartbeat_ms?: number;
	/** Smallest gap reported as a suspend rather than ordinary scheduler drift. */
	readonly minimum_gap_ms?: number;
	/** Reads wall-clock epoch milliseconds; the detector's second clock. */
	readonly Now?: Effect.Effect<number>;
}

/**
 * Broadcasts host suspends so long-lived work can distrust what it was holding
 * when the machine went away.
 *
 * @since 0.7.0
 */
export class HostSuspendMonitor extends Context.Service<
	HostSuspendMonitor,
	{
		readonly Subscribe: Effect.Effect<PubSub.Subscription<HostResume>, never, Scope.Scope>;
	}
>()("Artisan/HostSuspendMonitor") {}

/**
 * Detects a suspend by disagreement between two clocks.
 *
 * `Effect.sleep` runs on libuv's monotonic clock, which stops while the host is
 * suspended, whereas the wall clock keeps counting. Sleeping one heartbeat and
 * finding that far more wall-clock time passed therefore measures, with no
 * native code on any platform, exactly how long the host was away.
 *
 * A large clock correction reads as a suspend here. That is acceptable: every
 * consumer treats a resume as a reason to re-check work it already holds, so
 * the cost of a false positive is one redundant check.
 */
const WatchForClockGaps = (
	options: Required<HostSuspendMonitorOptions>,
	Publish: (resume: HostResume) => Effect.Effect<void>,
) =>
	Effect.gen(function* () {
		const started_at_ms = yield* options.Now;

		yield* Effect.sleep(options.heartbeat_ms);

		const resumed_at_ms = yield* options.Now;
		const suspended_ms = resumed_at_ms - started_at_ms - options.heartbeat_ms;

		if (suspended_ms < options.minimum_gap_ms) return;

		yield* Effect.logInfo("Host resumed from suspend", { resumed_at_ms, suspended_ms });
		yield* Publish({ resumed_at_ms, suspended_ms });
	}).pipe(Effect.forever);

/** @since 0.7.0 */
export const MakeHostSuspendMonitorLayer = (options: HostSuspendMonitorOptions = {}) =>
	Layer.effect(
		HostSuspendMonitor,
		Effect.gen(function* () {
			const configured = {
				heartbeat_ms: options.heartbeat_ms ?? 10_000,
				minimum_gap_ms: options.minimum_gap_ms ?? 30_000,
				Now: options.Now ?? Clock.currentTimeMillis,
			};
			/**
			 * Sliding, because a resume is a hint rather than a record: a slow
			 * subscriber must never hold up the heartbeat, and only the newest
			 * gap is worth acting on.
			 */
			const pubsub = yield* Effect.acquireRelease(
				PubSub.sliding<HostResume>(8),
				PubSub.shutdown,
			);

			yield* Effect.forkScoped(
				WatchForClockGaps(configured, (resume) =>
					PubSub.publish(pubsub, resume).pipe(Effect.asVoid),
				),
			);

			return { Subscribe: PubSub.subscribe(pubsub) };
		}),
	);

/** @since 0.7.0 */
export const HostSuspendMonitorLive = MakeHostSuspendMonitorLayer();
