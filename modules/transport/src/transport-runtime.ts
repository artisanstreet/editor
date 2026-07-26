import { Clock, Context, Effect, Layer } from "effect";

import { MakeSnowflakeIdLive, SnowflakeId } from "@artisan/protocol";

/** Supplies transport identifiers and canonical timestamps. */
export class TransportRuntime extends Context.Service<
	TransportRuntime,
	{
		readonly MakeId: (prefix: string) => Effect.Effect<string>;
		readonly Now: Effect.Effect<string>;
	}
>()("Artisan/TransportRuntime") {}

/** Provides Snowflake identifiers and the Effect clock. */
export const TransportRuntimeLive = Layer.effect(
	TransportRuntime,
	Effect.gen(function* () {
		const snowflake_id = yield* SnowflakeId;

		return TransportRuntime.of({
			MakeId: snowflake_id.Make,
			Now: Clock.currentTimeMillis.pipe(
				Effect.map((milliseconds) => new Date(milliseconds).toISOString()),
			),
		});
	}),
).pipe(Layer.provide(MakeSnowflakeIdLive(1)));
