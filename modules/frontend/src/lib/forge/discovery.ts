import { Data, Effect, Option, Schedule, Schema } from "effect";

import { ForgeHttpUrl } from "../runtime/forge-endpoint";

export const ForgeHealth = Schema.Struct({
	development: Schema.Boolean,
});

export const ReachableForge = Schema.Struct({
	endpoint: Schema.String,
	self: Schema.Boolean,
});

const ForgeInstances = Schema.Struct({
	instances: Schema.Array(ReachableForge),
});

export type ReachableForge = typeof ReachableForge.Type;

export class ForgeDiscoveryError extends Data.TaggedError("ForgeDiscoveryError")<{
	readonly cause: unknown;
	readonly operation: "health" | "instances";
}> {}

const FetchJson = <S extends Schema.ConstraintDecoder<unknown>>(
	operation: ForgeDiscoveryError["operation"],
	path: string,
	schema: S,
) =>
	Effect.gen(function* () {
		const response = yield* Effect.tryPromise({
			try: () => fetch(ForgeHttpUrl(path), { cache: "no-store" }),
			catch: (cause) => new ForgeDiscoveryError({ cause, operation }),
		});
		if (!response.ok)
			return yield* new ForgeDiscoveryError({
				cause: `HTTP ${response.status}`,
				operation,
			});
		const body = yield* Effect.tryPromise({
			try: () => response.json() as Promise<unknown>,
			catch: (cause) => new ForgeDiscoveryError({ cause, operation }),
		});
		return yield* Effect.try({
			try: () => Schema.decodeUnknownSync(schema, { onExcessProperty: "error" })(body),
			catch: (cause) => new ForgeDiscoveryError({ cause, operation }),
		});
	});

/** A bounded probe which remains interruptible when its component scope closes. */
export const DiscoverForgeHealth = FetchJson("health", "/health", ForgeHealth).pipe(
	Effect.retry(Schedule.max([Schedule.recurs(4), Schedule.spaced("1500 millis")])),
	Effect.option,
);

export const DiscoverOtherForges = FetchJson("instances", "/api/instances", ForgeInstances).pipe(
	Effect.map((listing) => listing.instances.filter((instance) => !instance.self)),
	Effect.catch(() => Effect.succeed([] as ReadonlyArray<ReachableForge>)),
);

export const DiscoverForge = Effect.gen(function* () {
	const health = yield* DiscoverForgeHealth;
	const others = Option.isSome(health) ? yield* DiscoverOtherForges : [];
	return { health, others } as const;
});
