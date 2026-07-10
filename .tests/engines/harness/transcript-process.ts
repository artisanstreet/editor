import { Buffer } from "node:buffer";

import { Effect, Layer, Schema } from "effect";

import {
	CodexProcessFactory,
	type CodexProcessExit,
	type CodexProcessHandle,
	type CodexProcessSpawnInput,
	EngineProcessError,
} from "@artisan/engines";

import {
	type EngineTranscriptChunk,
	EngineTranscriptRecord as EngineTranscriptRecordSchema,
	type EngineTranscriptRecord as TranscriptRecord,
	EngineTranscriptSequence as EngineTranscriptSequenceSchema,
	type EngineTranscriptSequence as TranscriptSequence,
} from "../transcript";

/** Validates unknown transcript data before it reaches the process replay boundary. @since 0.3.0 */
export const DecodeEngineTranscriptRecord = Schema.decodeUnknownEffect(
	EngineTranscriptRecordSchema,
);

/** Validates an ordered process sequence before engine-level replay. @since 0.3.0 */
export const DecodeEngineTranscriptSequence = Schema.decodeUnknownEffect(
	EngineTranscriptSequenceSchema,
);

/** Owns a replay factory and assertions for one byte-faithful process transcript. @since 0.3.0 */
export interface TranscriptReplay {
	readonly Assert: Effect.Effect<void, EngineProcessError>;
	readonly AssertClosed: Effect.Effect<void, EngineProcessError>;
	readonly Layer: Layer.Layer<CodexProcessFactory>;
}

/** Tracks the scoped lifecycle of one replayed process. */
interface ReplayProcessState {
	closed: boolean;
	readonly started_at_ms: number;
}

function process_error(operation: EngineProcessError["operation"], cause: unknown) {
	return new EngineProcessError({ cause, operation });
}

function delay(milliseconds: number) {
	return new Promise<void>((resolve) => setTimeout(resolve, milliseconds));
}

function delay_until(state: ReplayProcessState, at_ms: number) {
	return delay(Math.max(0, state.started_at_ms + at_ms - Date.now()));
}

function exit_for(transcript: TranscriptRecord): {
	readonly at_ms: number;
	readonly exit: CodexProcessExit;
} {
	if (transcript.fault?._tag === "crash") {
		return {
			at_ms: transcript.fault.at_ms,
			exit: { code: transcript.fault.exit_code, signal: null },
		};
	}

	if (transcript.fault?._tag === "early_eof") {
		return { at_ms: transcript.fault.at_ms, exit: { code: 0, signal: null } };
	}

	const at_ms =
		transcript.exit_at_ms ??
		transcript.chunks.reduce((latest, chunk) => Math.max(latest, chunk.at_ms), 0);

	return {
		at_ms,
		exit: {
			code: transcript.exit_code,
			signal: transcript.exit_signal as NodeJS.Signals | null,
		},
	};
}

/** Replays every process stream against the same spawn-relative transcript clock. */
function stream_chunks(
	chunks: ReadonlyArray<EngineTranscriptChunk>,
	state: ReplayProcessState,
): AsyncIterable<Uint8Array> {
	return {
		async *[Symbol.asyncIterator]() {
			for (const chunk of [...chunks].sort((left, right) => left.at_ms - right.at_ms)) {
				await delay_until(state, chunk.at_ms);

				yield new Uint8Array(Buffer.from(chunk.chunk_base64, "base64"));
			}
		},
	};
}

function make_handle(
	transcript: TranscriptRecord,
	writes: Array<Uint8Array>,
	state: ReplayProcessState,
): CodexProcessHandle {
	const expected_stdin = transcript.chunks
		.filter((chunk) => chunk.stream === "stdin")
		.sort((left, right) => left.at_ms - right.at_ms);
	const stdout = transcript.chunks.filter((chunk) => chunk.stream === "stdout");
	const stderr = transcript.chunks.filter((chunk) => chunk.stream === "stderr");
	const terminal = exit_for(transcript);
	let exit_settled = false;
	let resolve_exit: (exit: CodexProcessExit) => void = () => undefined;
	const exit = new Promise<CodexProcessExit>((resolve) => {
		resolve_exit = resolve;
	});
	const SettleExit = () => {
		if (exit_settled) {
			return;
		}

		exit_settled = true;
		clearTimeout(exit_timeout);
		resolve_exit(terminal.exit);
	};
	const exit_timeout = setTimeout(
		SettleExit,
		Math.max(0, state.started_at_ms + terminal.at_ms - Date.now()),
	);

	return {
		Close: Effect.sync(() => {
			state.closed = true;
			SettleExit();
		}),
		Exit: Effect.promise(() => exit),
		Kill: () => Effect.sync(SettleExit),
		Stderr: stream_chunks(stderr, state),
		Stdout: stream_chunks(stdout, state),
		Write: (chunk) => {
			const expected = expected_stdin[writes.length];

			return Effect.promise(() => delay_until(state, expected?.at_ms ?? 0)).pipe(
				Effect.andThen(
					Effect.try({
						try: () => {
							const capacity =
								transcript.fault?._tag === "backpressure"
									? transcript.fault.write_capacity
									: undefined;

							if (capacity !== undefined && writes.length >= capacity) {
								throw process_error(
									"write",
									new Error(`Transcript write capacity ${capacity} exceeded`),
								);
							}

							if (!expected) {
								throw process_error(
									"write",
									new Error("Unexpected outbound process frame"),
								);
							}

							const expected_bytes = Buffer.from(expected.chunk_base64, "base64");

							if (!Buffer.from(chunk).equals(expected_bytes)) {
								throw process_error(
									"write",
									new Error("Outbound process frame did not match transcript"),
								);
							}

							writes.push(new Uint8Array(chunk));
						},
						catch: (cause) =>
							cause instanceof EngineProcessError
								? cause
								: process_error("write", cause),
					}),
				),
			);
		},
	};
}

function make_replay(transcripts: TranscriptSequence): TranscriptReplay {
	const writes = transcripts.map(() => new Array<Uint8Array>());
	const handles: Array<ReplayProcessState> = [];
	let spawn_count = 0;
	const Assert = Effect.try({
		try: () => {
			if (spawn_count !== transcripts.length) {
				throw process_error(
					"spawn",
					new Error(
						`Expected ${transcripts.length} process spawns, received ${spawn_count}`,
					),
				);
			}

			for (const [index, transcript] of transcripts.entries()) {
				const expected_count = transcript.chunks.filter(
					(chunk) => chunk.stream === "stdin",
				).length;
				const received_count = writes[index]!.length;

				if (received_count !== expected_count) {
					throw process_error(
						"write",
						new Error(
							`Process ${index + 1} expected ${expected_count} outbound frames, received ${received_count}`,
						),
					);
				}
			}
		},
		catch: (cause) =>
			cause instanceof EngineProcessError ? cause : process_error("write", cause),
	});
	const AssertClosed = Effect.try({
		try: () => {
			if (handles.some((handle) => !handle.closed)) {
				throw process_error("close", new Error("A replayed process handle was not closed"));
			}
		},
		catch: (cause) =>
			cause instanceof EngineProcessError ? cause : process_error("close", cause),
	});
	const ReplayLayer = Layer.succeed(CodexProcessFactory, {
		Spawn: (spawn: CodexProcessSpawnInput) =>
			Effect.try({
				try: () => {
					const transcript = transcripts[spawn_count];
					const invocation_index = spawn_count;

					if (!transcript) {
						throw process_error(
							"spawn",
							new Error("Unexpected extra process invocation"),
						);
					}

					if (
						spawn.command !== transcript.command ||
						JSON.stringify(spawn.args) !== JSON.stringify(transcript.args)
					) {
						throw process_error(
							"spawn",
							new Error(
								`Process invocation ${invocation_index + 1} did not match transcript`,
							),
						);
					}

					const state = { closed: false, started_at_ms: Date.now() };

					spawn_count += 1;
					handles.push(state);

					return make_handle(transcript, writes[invocation_index]!, state);
				},
				catch: (cause) =>
					cause instanceof EngineProcessError ? cause : process_error("spawn", cause),
			}),
	});

	return { Assert, AssertClosed, Layer: ReplayLayer };
}

/** Creates a validated replay for one byte-faithful process transcript. @since 0.3.0 */
export function make_transcript_replay(input: unknown) {
	return DecodeEngineTranscriptRecord(input).pipe(
		Effect.map((transcript) => make_replay([transcript])),
	);
}

/** Creates a validated replay for every process in one engine scenario. @since 0.3.0 */
export function make_transcript_sequence_replay(input: unknown) {
	return DecodeEngineTranscriptSequence(input).pipe(Effect.map(make_replay));
}
