import { Clock, Context, Data, Effect, Layer, Ref, Schema } from "effect";

/** The UTC epoch reserved for Artisan Snowflake identifiers. */
export const SnowflakeEpochMilliseconds = Date.UTC(2026, 5, 19);

const WorkerId = Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 1_023 }));

export type WorkerId = typeof WorkerId.Type;

export class InvalidSnowflakeWorkerId extends Data.TaggedError("InvalidSnowflakeWorkerId")<{
	readonly worker_id: number;
}> {}

interface SnowflakeState {
	readonly sequence: number;
	readonly timestamp_ms: number;
}

/** Generates prefix-preserving, process-unique identifiers without number precision loss. */
export class SnowflakeId extends Context.Service<
	SnowflakeId,
	{
		readonly Make: (prefix: string) => Effect.Effect<string>;
	}
>()("Artisan/SnowflakeId") {}

/**
 * Provides a concurrency-safe Snowflake generator for one explicitly assigned worker.
 *
 * The 64-bit payload uses 41 timestamp bits, 10 worker bits, and 12 sequence bits.
 * Clock rollback is absorbed by retaining the last logical millisecond. Exhausting a
 * millisecond advances that logical clock by one millisecond rather than blocking.
 */
export const MakeSnowflakeIdLive = (
	worker_id: number,
): Layer.Layer<SnowflakeId, InvalidSnowflakeWorkerId> =>
	Layer.effect(
		SnowflakeId,
		Effect.gen(function* () {
			const decoded_worker_id = yield* Schema.decodeUnknownEffect(WorkerId)(worker_id).pipe(
				Effect.mapError(() => new InvalidSnowflakeWorkerId({ worker_id })),
			);
			const state = yield* Ref.make<SnowflakeState>({
				sequence: -1,
				timestamp_ms: SnowflakeEpochMilliseconds,
			});

			const Make = (prefix: string) =>
				Effect.gen(function* () {
					const now_ms = yield* Clock.currentTimeMillis;
					const payload = yield* Ref.modify(state, (current) => {
						const observed_ms = Math.max(now_ms, SnowflakeEpochMilliseconds);
						const timestamp_ms = Math.max(observed_ms, current.timestamp_ms);
						const same_millisecond = timestamp_ms === current.timestamp_ms;
						const next_sequence = same_millisecond ? current.sequence + 1 : 0;
						const sequence_overflow = next_sequence > 4_095;
						const logical_timestamp_ms = sequence_overflow
							? timestamp_ms + 1
							: timestamp_ms;
						const sequence = sequence_overflow ? 0 : next_sequence;
						const timestamp_component =
							BigInt(logical_timestamp_ms - SnowflakeEpochMilliseconds) << 22n;
						const worker_component = BigInt(decoded_worker_id) << 12n;
						const snowflake = timestamp_component | worker_component | BigInt(sequence);

						return [
							snowflake,
							{ sequence, timestamp_ms: logical_timestamp_ms },
						] as const;
					});

					return `${prefix}_${payload.toString(10)}`;
				});

			return SnowflakeId.of({ Make });
		}),
	);
