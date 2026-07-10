import { Schema } from "effect";

/** Describes a process failure deliberately replayed by a transcript. @since 0.3.0 */
export const EngineTranscriptFault = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("backpressure"),
		write_capacity: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	}),
	Schema.Struct({
		_tag: Schema.Literal("crash"),
		at_ms: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
		exit_code: Schema.Int,
	}),
	Schema.Struct({
		_tag: Schema.Literal("early_eof"),
		at_ms: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	}),
]);

/** Preserves one byte-exact process transcript chunk and its relative timing. @since 0.1.0 */
export const EngineTranscriptChunk = Schema.Struct({
	at_ms: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	chunk_base64: Schema.String.check(
		Schema.isPattern(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/),
	),
	stream: Schema.Literals(["stdin", "stdout", "stderr"]),
});

/** Preserves the invocation and complete byte-level transcript of an engine process. @since 0.1.0 */
export const EngineTranscriptRecord = Schema.Struct({
	args: Schema.Array(Schema.String),
	chunks: Schema.Array(EngineTranscriptChunk),
	command: Schema.NonEmptyString,
	exit_at_ms: Schema.optional(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))),
	exit_code: Schema.NullOr(Schema.Int),
	exit_signal: Schema.NullOr(Schema.String),
	fault: Schema.optional(EngineTranscriptFault),
});

/** Preserves the ordered process invocations required by one engine scenario. @since 0.3.0 */
export const EngineTranscriptSequence = Schema.NonEmptyArray(EngineTranscriptRecord);

/** Represents a validated fault deliberately replayed by a process transcript. @since 0.3.0 */
export type EngineTranscriptFault = Schema.Schema.Type<typeof EngineTranscriptFault>;

/** Represents a validated byte-exact transcript chunk. @since 0.1.0 */
export type EngineTranscriptChunk = Schema.Schema.Type<typeof EngineTranscriptChunk>;

/** Represents a validated process transcript record. @since 0.1.0 */
export type EngineTranscriptRecord = Schema.Schema.Type<typeof EngineTranscriptRecord>;

/** Represents a validated ordered process transcript sequence. @since 0.3.0 */
export type EngineTranscriptSequence = Schema.Schema.Type<typeof EngineTranscriptSequence>;
