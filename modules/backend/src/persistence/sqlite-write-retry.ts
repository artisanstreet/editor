import { Cause, Effect, Schedule } from "effect";
import { EffectDrizzleQueryError } from "drizzle-orm/effect-core";
import { isSqlError } from "effect/unstable/sql/SqlError";

const WriteContentionSchedule = Schedule.exponential("5 millis").pipe(
	Schedule.upTo({ duration: "1 second", times: 8 }),
);

function is_retryable_database_error(error: unknown): boolean {
	if (isSqlError(error)) {
		return error.isRetryable;
	}

	if (!(error instanceof EffectDrizzleQueryError) || !Cause.isCause(error.cause)) {
		return false;
	}

	return error.cause.reasons.some(
		(reason) => Cause.isFailReason(reason) && is_retryable_database_error(reason.error),
	);
}

/** Retries one complete SQLite write transaction after bounded lock contention. */
export function RetrySqliteWrite<A, E, R>(operation: Effect.Effect<A, E, R>) {
	return operation.pipe(
		Effect.retry({
			schedule: WriteContentionSchedule,
			while: is_retryable_database_error,
		}),
	);
}
