import { randomUUID } from "node:crypto";

import { Context, Effect, Layer } from "effect";

export type RuntimeIdPrefix =
	| "agent"
	| "approval"
	| "backend"
	| "connection"
	| "event"
	| "heartbeat"
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

export const RuntimeMetadataLive = Layer.sync(RuntimeMetadata, () => ({
	instance_id: `backend_${randomUUID()}`,
	MakeId: (prefix) => Effect.sync(() => `${prefix}_${randomUUID()}`),
	Now: Effect.sync(() => new Date().toISOString()),
}));
