import { Schema } from "effect";

/** Validates a non-empty identifier that is safe to carry across transports. */
export const Identifier = Schema.String.check(
	Schema.isPattern(/^\S+$/, {
		message: "Expected a non-empty identifier without whitespace",
	}),
);

/** Validates a positive integer protocol version offered during negotiation. */
export const ProtocolVersion = Schema.Int.check(
	Schema.isGreaterThan(0, {
		message: "Expected a positive protocol version",
	}),
);

/** Validates a positive integer used for bounded timing and capacity values. */
export const PositiveInt = Schema.Int.check(
	Schema.isGreaterThan(0, {
		message: "Expected a positive integer",
	}),
);

/** Lists the protocol versions supported by this Artisan V1 implementation. */
export const SupportedProtocolVersions = [1] as const;

/** Validates a protocol version selected by the V1 negotiation. */
export const NegotiatedProtocolVersion = Schema.Literals(SupportedProtocolVersions);

/** Validates the version of the schema carried by every V1 frame. */
export const SchemaVersion = Schema.Literal(1);

/** Validates a canonical UTC timestamp used by protocol trace metadata. */
export const IsoDateTime = Schema.String.check(
	Schema.isPattern(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/, {
		message: "Expected an ISO 8601 UTC timestamp",
	}),
	Schema.makeFilter<string>((input) => {
		const date = new Date(input);
		const normalized = input.includes(".") ? input : input.replace("Z", ".000Z");

		return Number.isNaN(date.getTime()) || date.toISOString() !== normalized
			? "Expected a real ISO 8601 UTC timestamp"
			: undefined;
	}),
);

/** Validates a non-negative durable journal position. */
export const JournalSequence = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0, {
		message: "Expected a non-negative journal sequence",
	}),
);

/** Validates a non-negative sequence within an individual ordered stream. */
export const StreamSequence = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0, {
		message: "Expected a non-negative stream sequence",
	}),
);

/** Identifies the native provider payload that produced an Artisan frame. */
export const RawOrigin = Schema.Struct({
	provider: Identifier,
	reference: Identifier,
});

export type RawOrigin = typeof RawOrigin.Type;

/** Records the last contiguous sequence applied for one logical stream. */
export const StreamCursor = Schema.Struct({
	sequence: StreamSequence,
	stream_id: Identifier,
});

export type StreamCursor = typeof StreamCursor.Type;
