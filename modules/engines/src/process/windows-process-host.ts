import cross_spawn from "cross-spawn";
import { Data, Deferred, Effect, Queue, Schema } from "effect";

const ClaimMessage = Schema.Struct({
	claim_token: Schema.NonEmptyString,
	type: Schema.Literal("claim"),
});

const StartMessage = Schema.Struct({
	input: Schema.Struct({
		args: Schema.Array(Schema.String),
		command: Schema.NonEmptyString,
		cwd: Schema.optional(Schema.String),
		env: Schema.optional(Schema.Record(Schema.String, Schema.String)),
	}),
	type: Schema.Literal("start"),
});

class WindowsProcessHostError extends Data.TaggedError("WindowsProcessHostError")<{
	readonly cause?: unknown;
	readonly message: string;
}> {}

const DecodeClaim = Schema.decodeUnknownEffect(ClaimMessage);
const DecodeStart = Schema.decodeUnknownEffect(StartMessage);

const Send = (message: unknown) =>
	Effect.callback<void, WindowsProcessHostError>((resume) => {
		if (!process.connected || process.send === undefined) {
			resume(Effect.void);
			return;
		}
		process.send(message, (cause) =>
			resume(
				cause
					? Effect.fail(
							new WindowsProcessHostError({
								cause,
								message: "The Windows process host could not send an IPC message",
							}),
						)
					: Effect.void,
			),
		);
	});

const FlushOutput = Effect.callback<void>((resume) => {
	let pending = 2;
	const flushed = () => {
		pending -= 1;
		if (pending === 0) resume(Effect.void);
	};
	process.stdout.write("", flushed);
	process.stderr.write("", flushed);
});

const exit_code = (code: number | null, signal: NodeJS.Signals | null) => {
	if (code !== null) return code;
	if (signal === "SIGINT") return 130;
	if (signal === "SIGTERM") return 143;
	if (signal === "SIGKILL") return 137;
	return 1;
};

export const WindowsProcessHostProgram = Effect.scoped(
	Effect.gen(function* () {
		const messages = yield* Queue.unbounded<unknown>();
		const disconnected = yield* Deferred.make<void>();
		yield* Effect.acquireRelease(
			Effect.sync(() => {
				const receive = (message: unknown) => Queue.offerUnsafe(messages, message);
				const disconnect = () => Deferred.doneUnsafe(disconnected, Effect.void);
				process.on("message", receive);
				process.once("disconnect", disconnect);
				return { disconnect, receive };
			}),
			({ disconnect, receive }) =>
				Effect.sync(() => {
					process.off("message", receive);
					process.off("disconnect", disconnect);
				}),
		);
		const TakeMessage = Queue.take(messages).pipe(
			Effect.raceFirst(
				Deferred.await(disconnected).pipe(
					Effect.andThen(
						Effect.fail(
							new WindowsProcessHostError({
								message: "The Windows process host owner disconnected",
							}),
						),
					),
				),
			),
		);

		const claim = yield* TakeMessage.pipe(
			Effect.flatMap(DecodeClaim),
			Effect.mapError(
				(cause) =>
					new WindowsProcessHostError({
						cause,
						message: "The Windows process host received an invalid claim message",
					}),
			),
		);
		yield* Send({ claim_token: claim.claim_token, type: "claim_ack" });

		const start = yield* TakeMessage.pipe(
			Effect.flatMap(DecodeStart),
			Effect.mapError(
				(cause) =>
					new WindowsProcessHostError({
						cause,
						message: "The Windows process host received an invalid start message",
					}),
			),
		);
		const input = start.input;
		const child = yield* Effect.acquireRelease(
			Effect.try({
				try: () => {
					const spawned = cross_spawn(input.command, [...input.args], {
						...(input.cwd === undefined ? {} : { cwd: input.cwd }),
						...(input.env === undefined ? {} : { env: input.env }),
						shell: false,
						stdio: "pipe",
						windowsHide: true,
					});
					const child_stdin = spawned.stdin;
					const child_stdout = spawned.stdout;
					const child_stderr = spawned.stderr;
					if (child_stdin === null || child_stdout === null || child_stderr === null) {
						spawned.kill("SIGKILL");
						throw new Error("The engine process did not provide piped stdio");
					}
					child_stdin.once("error", () => undefined);
					process.stdin.pipe(child_stdin);
					child_stdout.pipe(process.stdout, { end: false });
					child_stderr.pipe(process.stderr, { end: false });
					return spawned;
				},
				catch: (cause) =>
					new WindowsProcessHostError({
						cause,
						message: `${input.command} ${input.args.join(" ")} failed to spawn`,
					}),
			}),
			(child) =>
				Effect.sync(() => {
					if (child.exitCode === null && child.signalCode === null) {
						child.kill("SIGKILL");
					}
				}),
		);

		const spawned = yield* Deferred.make<number, WindowsProcessHostError>();
		const exited = yield* Deferred.make<{
			readonly code: number | null;
			readonly signal: NodeJS.Signals | null;
		}>();
		yield* Effect.acquireRelease(
			Effect.sync(() => {
				const on_spawned = () => {
					if (child.pid === undefined) {
						Deferred.doneUnsafe(
							spawned,
							Effect.fail(
								new WindowsProcessHostError({
									message: "The engine process did not expose a process id",
								}),
							),
						);
						return;
					}
					Deferred.doneUnsafe(spawned, Effect.succeed(child.pid));
				};
				const on_failed = (cause: Error) =>
					Deferred.doneUnsafe(
						spawned,
						Effect.fail(
							new WindowsProcessHostError({
								cause,
								message: `${input.command} ${input.args.join(" ")} failed to spawn`,
							}),
						),
					);
				const on_closed = (code: number | null, signal: NodeJS.Signals | null) =>
					Deferred.doneUnsafe(exited, Effect.succeed({ code, signal }));
				child.once("spawn", on_spawned);
				child.once("error", on_failed);
				child.once("close", on_closed);
				return { on_closed, on_failed, on_spawned };
			}),
			({ on_closed, on_failed, on_spawned }) =>
				Effect.sync(() => {
					child.off("spawn", on_spawned);
					child.off("error", on_failed);
					child.off("close", on_closed);
				}),
		);
		const process_id = yield* Deferred.await(spawned);
		yield* Send({ process_id, type: "ready" });
		const outcome = yield* Deferred.await(exited).pipe(
			Effect.raceFirst(
				Deferred.await(disconnected).pipe(
					Effect.andThen(
						Effect.fail(
							new WindowsProcessHostError({
								message: "The Windows process host owner disconnected",
							}),
						),
					),
				),
			),
			Effect.raceFirst(
				Queue.take(messages).pipe(
					Effect.andThen(
						Effect.fail(
							new WindowsProcessHostError({
								message:
									"The Windows process host received a message after startup",
							}),
						),
					),
				),
			),
		);
		process.exitCode = exit_code(outcome.code, outcome.signal);
	}),
).pipe(
	Effect.catch((cause) =>
		Send({ message: cause.message, type: "spawn_error" }).pipe(
			Effect.ignore,
			Effect.andThen(Effect.sync(() => (process.exitCode = 127))),
		),
	),
	Effect.ensuring(
		FlushOutput.pipe(
			Effect.andThen(
				Effect.sync(() => {
					const disconnect = process.disconnect;
					if (process.connected && disconnect !== undefined) {
						disconnect.call(process);
					}
				}),
			),
		),
	),
);
