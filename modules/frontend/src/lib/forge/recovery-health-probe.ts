import { Effect, Option } from "effect";

/**
 * Recovery runs every two seconds, so an individual liveness read must leave
 * room for the next cadence rather than permanently owning the watcher.
 */
export const forge_recovery_health_deadline = "1500 millis";

/**
 * Converts one cheap origin health request into the recovery watcher's narrow
 * reachability fact. Timeout and transport failure are equivalent here: this
 * iteration cannot prove Forge is back, and the scoped watcher may probe again.
 */
export const ProbeForgeRecoveryHealth = <E, R>(
	request: Effect.Effect<{ readonly status: number }, E, R>,
): Effect.Effect<boolean, never, R> =>
	request.pipe(
		Effect.map((response) => response.status >= 200 && response.status < 300),
		Effect.timeoutOption(forge_recovery_health_deadline),
		Effect.map(Option.getOrElse(() => false)),
		Effect.catch(() => Effect.succeed(false)),
	);
