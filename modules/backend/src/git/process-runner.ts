import { Context, Data, Effect } from "effect";

/** Identifies which process execution phase failed. */
export type ProcessRunnerOperation = "configuration" | "spawn" | "stdin";

/** Reports a process-level failure independently from any Git operation. */
export class ProcessRunnerError extends Data.TaggedError("ProcessRunnerError")<{
	readonly cause: unknown;
	readonly command: string;
	readonly operation: ProcessRunnerOperation;
}> {}

/** Configures one shell-free process invocation. */
export interface ProcessRunnerInput {
	readonly args: ReadonlyArray<string>;
	readonly command: string;
	readonly cwd: string;
	readonly environment?: Readonly<Record<string, string | undefined>>;
	readonly max_stderr_bytes?: number;
	readonly max_stdout_bytes?: number;
	readonly stdin?: Uint8Array;
}

/** Contains bounded process output and total observed byte counts. */
export interface ProcessRunnerResult {
	readonly exit_code: number;
	readonly stderr: Uint8Array;
	readonly stderr_bytes: number;
	readonly stderr_truncated: boolean;
	readonly stdout: Uint8Array;
	readonly stdout_bytes: number;
	readonly stdout_truncated: boolean;
}

/** Defines injectable, cancellable process execution without shell interpolation. */
export interface ProcessRunnerShape {
	readonly Run: (
		input: ProcessRunnerInput,
	) => Effect.Effect<ProcessRunnerResult, ProcessRunnerError>;
}

/** Abstracts process execution from repository-specific behavior. */
export class ProcessRunner extends Context.Service<ProcessRunner, ProcessRunnerShape>()(
	"Artisan/ProcessRunner",
) {}
