import { Clock, Effect, Layer } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

/** Configures browser recovery after the host returns from suspend. @since 0.2.16 */
export interface HostResumeRecoveryOptions {
	/** Spacing between checks, and the upper bound on resume detection latency. */
	readonly heartbeat_ms?: number;
	/** Smallest wall-clock gap treated as a suspend rather than scheduler drift. */
	readonly minimum_gap_ms?: number;
	/** Reads wall-clock epoch milliseconds; injectable for deterministic clock tests. */
	readonly Now?: Effect.Effect<number>;
}

/**
 * Starts one browser-lifetime monitor. Its scoped child fiber is interrupted
 * with runtime disposal, so a discarded renderer cannot reconnect afterwards.
 *
 * @since 0.2.16
 */
export const MakeHostResumeRecoveryLayer = (options: HostResumeRecoveryOptions = {}) =>
	Layer.effectDiscard(
		Effect.gen(function* () {
			const client = yield* ArtisanClient;
			const heartbeat_ms = options.heartbeat_ms ?? 10_000;
			const minimum_gap_ms = options.minimum_gap_ms ?? 30_000;
			const Now =
				options.Now ??
				Effect.gen(function* () {
					return yield* Clock.currentTimeMillis;
				});

			/**
			 * Compares Effect's monotonic sleep with wall time. A suspended renderer
			 * does not spend the heartbeat on its monotonic clock, while wall time
			 * continues; the disagreement is one recovery epoch, not a retry policy.
			 */
			const WatchForHostResume = Effect.gen(function* () {
				while (true) {
					const started_at_ms = yield* Now;

					yield* Effect.sleep(heartbeat_ms);

					const resumed_at_ms = yield* Now;
					const suspended_ms = resumed_at_ms - started_at_ms - heartbeat_ms;

					if (suspended_ms < minimum_gap_ms) continue;

					/**
					 * The client's retry remains the only reconnect policy. A failed
					 * bounded retry must not turn this heartbeat into an unbounded loop.
					 */
					yield* client.RetryConnection.pipe(Effect.ignore);
				}
			});

			yield* WatchForHostResume.pipe(Effect.forkScoped);
		}),
	);

/** Browser production defaults. @since 0.2.16 */
export const HostResumeRecoveryLive = MakeHostResumeRecoveryLayer();
