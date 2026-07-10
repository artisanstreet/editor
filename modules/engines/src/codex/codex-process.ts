import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";

import { Context, Effect, Layer } from "effect";

import { EngineProcessError } from "../engine";

/** Describes the terminal state reported by a spawned child process. @since 0.1.0 */
export interface CodexProcessExit {
	readonly code: number | null;
	readonly signal: NodeJS.Signals | null;
}

/** Supplies the executable and environment used to spawn a Codex process. @since 0.1.0 */
export interface CodexProcessSpawnInput {
	readonly args: ReadonlyArray<string>;
	readonly command: string;
	readonly cwd?: string;
	readonly env?: NodeJS.ProcessEnv;
	readonly shell?: boolean;
}

/** Owns the byte streams and lifecycle controls for one spawned Codex process. @since 0.1.0 */
export interface CodexProcessHandle {
	readonly Exit: Effect.Effect<CodexProcessExit, EngineProcessError>;
	readonly Kill: (signal?: NodeJS.Signals) => Effect.Effect<void, EngineProcessError>;
	readonly Close: Effect.Effect<void>;
	readonly Stderr: AsyncIterable<Uint8Array>;
	readonly Stdout: AsyncIterable<Uint8Array>;
	readonly Write: (chunk: Uint8Array) => Effect.Effect<void, EngineProcessError>;
}

/** Spawns and owns Codex CLI process handles. @since 0.1.0 */
export class CodexProcessFactory extends Context.Service<
	CodexProcessFactory,
	{
		readonly Spawn: (
			input: CodexProcessSpawnInput,
		) => Effect.Effect<CodexProcessHandle, EngineProcessError>;
	}
>()("Artisan/CodexProcessFactory") {}

function make_process_exit(child: ChildProcessWithoutNullStreams) {
	const exit = new Promise<CodexProcessExit>((resolve, reject) => {
		child.once("error", reject);
		child.once("exit", (code, signal) => resolve({ code, signal }));
	});

	return Effect.tryPromise({
		try: () => exit,
		catch: (cause) => new EngineProcessError({ cause, operation: "exit" }),
	});
}

function make_write(child: ChildProcessWithoutNullStreams, chunk: Uint8Array) {
	return Effect.tryPromise({
		try: () =>
			new Promise<void>((resolve, reject) => {
				child.stdin.write(chunk, (error) => (error ? reject(error) : resolve()));
			}),
		catch: (cause) => new EngineProcessError({ cause, operation: "write" }),
	});
}

function make_kill(child: ChildProcessWithoutNullStreams, signal: NodeJS.Signals = "SIGTERM") {
	return Effect.try({
		try: () => {
			if (!child.killed && child.exitCode === null) {
				child.kill(signal);
			}
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "kill" }),
	});
}

/** Provides a Node child-process implementation of the Codex process seam. @since 0.1.0 */
export const CodexProcessFactoryLive = Layer.succeed(CodexProcessFactory, {
	Spawn: (input) =>
		Effect.try({
			try: () => {
				const child = spawn(input.command, input.args, {
					cwd: input.cwd,
					env: input.env,
					shell: input.shell,
					stdio: "pipe",
				});
				const Exit = make_process_exit(child);
				const Kill = (signal?: NodeJS.Signals) => make_kill(child, signal);
				const Close = Effect.gen(function* () {
					if (!child.stdin.destroyed) {
						child.stdin.end();
					}

					yield* Kill().pipe(Effect.ignore);
					yield* Exit.pipe(Effect.ignore);
				});

				return {
					Close,
					Exit,
					Kill,
					Stderr: child.stderr,
					Stdout: child.stdout,
					Write: (chunk) => make_write(child, chunk),
				};
			},
			catch: (cause) => new EngineProcessError({ cause, operation: "spawn" }),
		}),
});
