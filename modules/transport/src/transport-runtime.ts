import { Clock, Context, Effect, Layer } from "effect";

/** Supplies transport identifiers and canonical timestamps. */
export class TransportRuntime extends Context.Service<
	TransportRuntime,
	{
		readonly MakeId: (prefix: string) => Effect.Effect<string>;
		readonly Now: Effect.Effect<string>;
	}
>()("Artisan/TransportRuntime") {}

/** Provides cryptographically random identifiers and the Effect clock. */
export const TransportRuntimeLive = Layer.succeed(TransportRuntime, {
	MakeId: (prefix) => Effect.sync(() => `${prefix}_${globalThis.crypto.randomUUID()}`),
	Now: Clock.currentTimeMillis.pipe(
		Effect.map((milliseconds) => new Date(milliseconds).toISOString()),
	),
});
