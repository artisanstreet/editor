import { Clock, Context, Effect, Layer } from "effect";

import { MakeSnowflakeIdLive, SnowflakeId } from "@artisan/protocol";

export type RuntimeIdPrefix =
	| "agent"
	| "attachment"
	| "backend"
	| "connection"
	| "event"
	| "heartbeat"
	| "intake"
	| "message"
	| "run"
	| "stream_ticket";

export class RuntimeMetadata extends Context.Service<
	RuntimeMetadata,
	{
		readonly instance_id: string;
		readonly MakeId: (prefix: RuntimeIdPrefix) => Effect.Effect<string>;
		readonly Now: Effect.Effect<string>;
	}
>()("Artisan/RuntimeMetadata") {}

export const RuntimeMetadataLive = Layer.effect(
	RuntimeMetadata,
	Effect.gen(function* () {
		const snowflake_id = yield* SnowflakeId;
		const instance_id = yield* snowflake_id.Make("backend");

		return RuntimeMetadata.of({
			instance_id,
			MakeId: snowflake_id.Make,
			Now: Clock.currentTimeMillis.pipe(
				Effect.map((milliseconds) => new Date(milliseconds).toISOString()),
			),
		});
	}),
).pipe(Layer.provide(MakeSnowflakeIdLive(0)));
