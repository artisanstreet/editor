import { Data, Effect, Schema, Stream } from "effect";

import { PositiveInt, type OutboundControlEnvelope } from "@artisan/protocol";

/** Validates heartbeat settings before a connection can open. */
export const ProtocolConnectionOptionsSchema = Schema.Struct({
	heartbeat_interval_ms: PositiveInt,
	heartbeat_timeout_ms: PositiveInt,
}).check(
	Schema.makeFilter((options) =>
		options.heartbeat_timeout_ms < options.heartbeat_interval_ms
			? "Expected the heartbeat timeout to be at least the heartbeat interval"
			: undefined,
	),
);

export type ProtocolConnectionOptions = typeof ProtocolConnectionOptionsSchema.Type;

/** Reports invalid backend connection settings at layer construction. */
export class ProtocolConfigurationError extends Data.TaggedError("ProtocolConfigurationError")<{
	readonly cause: unknown;
}> {}

/** Decodes connection settings at the backend configuration boundary. */
export const DecodeProtocolConnectionOptions = (input: unknown) =>
	Schema.decodeUnknownEffect(ProtocolConnectionOptionsSchema, {
		onExcessProperty: "error",
	})(input).pipe(Effect.mapError((cause) => new ProtocolConfigurationError({ cause })));

/** Exposes a transport-neutral, scoped Artisan control connection. */
export interface ProtocolConnection {
	readonly Close: Effect.Effect<void>;
	readonly Closed: Effect.Effect<void>;
	readonly Outbound: Stream.Stream<OutboundControlEnvelope>;
	readonly Receive: (input: unknown) => Effect.Effect<void>;
}

/** Supplies conservative local-desktop defaults for control connections. */
export const DefaultProtocolConnectionOptions: ProtocolConnectionOptions = {
	heartbeat_interval_ms: 15_000,
	heartbeat_timeout_ms: 45_000,
};
