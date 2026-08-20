import { Context, Data, Effect, Scope, Stream } from "effect";

/** Identifies one operation rejected or failed by a native terminal driver. */
export type TerminalDriverOperation =
	| "clear"
	| "close"
	| "configuration"
	| "kill"
	| "output"
	| "resize"
	| "spawn"
	| "write";

/** Describes a native terminal failure without leaking adapter-specific errors. */
export class TerminalDriverError extends Data.TaggedError("TerminalDriverError")<{
	readonly cause: unknown;
	readonly operation: TerminalDriverOperation;
}> {}

/** Defines the dimensions and process environment for one pseudoterminal. */
export interface TerminalDriverOpenInput {
	readonly args: ReadonlyArray<string>;
	readonly cols: number;
	readonly cwd: string;
	readonly env?: Readonly<Record<string, string | undefined>>;
	readonly executable: string;
	readonly rows: number;
}

/**
 * Reports why a native pseudoterminal stopped producing output. The native
 * driver no longer emits `output_overflow`, but sessions persisted by older
 * backends still carry it.
 */
export interface TerminalDriverExit {
	readonly exit_code: number | null;
	readonly reason: "closed" | "exited" | "killed" | "output_overflow";
	readonly signal: number | null;
}

/** Owns one live pseudoterminal until its parent scope closes. */
export interface TerminalDriverHandle {
	readonly Clear: Effect.Effect<void, TerminalDriverError>;
	readonly Close: Effect.Effect<void, TerminalDriverError>;
	readonly Exit: Effect.Effect<TerminalDriverExit>;
	readonly Kill: (signal?: string) => Effect.Effect<void, TerminalDriverError>;
	readonly Output: Stream.Stream<Uint8Array>;
	readonly Resize: (cols: number, rows: number) => Effect.Effect<void, TerminalDriverError>;
	readonly Write: (chunk: Uint8Array) => Effect.Effect<void, TerminalDriverError>;
	readonly pid: number;
}

/** Abstracts native pseudoterminal ownership from orchestration and protocol code. */
export class TerminalDriver extends Context.Service<
	TerminalDriver,
	{
		readonly Open: (
			input: TerminalDriverOpenInput,
		) => Effect.Effect<TerminalDriverHandle, TerminalDriverError, Scope.Scope>;
	}
>()("Artisan/TerminalDriver") {}
