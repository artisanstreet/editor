import { Deferred, Effect, Option, Schedule, Stream } from "effect";
import { HttpClient } from "effect/unstable/http";

import { ReadinessError } from "./error.ts";
import type { Readiness } from "./model.ts";

export interface OutputLine {
	readonly line: string;
	readonly stream: "stderr" | "stdout";
}

const MatchOutput = (
	readiness: Extract<Readiness, { readonly _tag: "Output" }>,
	output: OutputLine,
) => {
	if (readiness.stream !== "either" && readiness.stream !== output.stream) return false;
	/** RegExp instances with g/y retain state across calls. */
	readiness.pattern.lastIndex = 0;
	return readiness.pattern.test(output.line);
};

/** Waits for one eligible decoded output line without consuming the process streams twice. */
export const MatchesOutputReadiness = (
	readiness: Extract<Readiness, { readonly _tag: "Output" }>,
	output: OutputLine,
): boolean => MatchOutput(readiness, output);

export const AwaitOutputReadiness = (
	process_id: string,
	readiness: Extract<Readiness, { readonly _tag: "Output" }>,
	ready: Deferred.Deferred<void, ReadinessError>,
): Effect.Effect<void, ReadinessError> =>
	Deferred.await(ready).pipe(
		Effect.timeoutOption(readiness.timeout),
		Effect.flatMap((outcome) =>
			Option.match(outcome, {
				onNone: () =>
					Effect.fail(
						new ReadinessError({
							cause: new Error("Timed out waiting for process output"),
							process_id,
							readiness: "output",
						}),
					),
				onSome: () => Effect.void,
			}),
		),
	);

const AwaitHttpAttempt = (url: string) =>
	HttpClient.get(url).pipe(
		Effect.flatMap((response) =>
			response.status >= 200 && response.status < 300
				? Effect.void
				: Effect.fail(new Error(`HTTP readiness returned ${response.status}`)),
		),
	);

export const AwaitHttpReadiness = (
	process_id: string,
	readiness: Extract<Readiness, { readonly _tag: "Http" }>,
): Effect.Effect<void, ReadinessError, HttpClient.HttpClient> =>
	AwaitHttpAttempt(readiness.url).pipe(
		Effect.retry(Schedule.spaced(readiness.interval)),
		Effect.timeoutOption(readiness.timeout),
		Effect.flatMap((outcome) =>
			Option.match(outcome, {
				onNone: () =>
					Effect.fail(
						new ReadinessError({
							cause: new Error("Timed out waiting for HTTP readiness"),
							process_id,
							readiness: "http",
						}),
					),
				onSome: () => Effect.void,
			}),
		),
		Effect.mapError((cause) =>
			cause instanceof ReadinessError
				? cause
				: new ReadinessError({ cause, process_id, readiness: "http" }),
		),
	);

export const DecodeOutput = <E, R>(
	stream: "stderr" | "stdout",
	bytes: Stream.Stream<Uint8Array, E, R>,
): Stream.Stream<OutputLine, E, R> =>
	bytes.pipe(
		Stream.decodeText,
		Stream.splitLines,
		Stream.map((line) => ({ line, stream })),
	);
