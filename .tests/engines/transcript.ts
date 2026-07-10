import { Schema } from "effect";

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
	exit_code: Schema.NullOr(Schema.Int),
	exit_signal: Schema.NullOr(Schema.String),
});

/** Represents a validated byte-exact transcript chunk. @since 0.1.0 */
export type EngineTranscriptChunk = Schema.Schema.Type<typeof EngineTranscriptChunk>;

/** Represents a validated process transcript record. @since 0.1.0 */
export type EngineTranscriptRecord = Schema.Schema.Type<typeof EngineTranscriptRecord>;
