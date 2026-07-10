import { randomUUID } from "node:crypto";

import { Context, Effect, Layer } from "effect";

export type RuntimeIdPrefix = "event" | "message";

export class RuntimeMetadata extends Context.Service<
	RuntimeMetadata,
	{
		readonly MakeId: (prefix: RuntimeIdPrefix) => Effect.Effect<string>;
		readonly Now: Effect.Effect<string>;
	}
>()("Artisan/RuntimeMetadata") {}

export const RuntimeMetadataLive = Layer.succeed(RuntimeMetadata, {
	MakeId: (prefix) => Effect.sync(() => `${prefix}_${randomUUID()}`),
	Now: Effect.sync(() => new Date().toISOString()),
});
