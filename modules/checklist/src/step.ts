import type { Duration, Effect, Scope } from "effect";

/**
 * The authoring surface. A step either runs something (`run`) or contains
 * steps (`steps`); nothing else distinguishes the two. Ordering is structural,
 * so there are no ids to declare and no edges to keep in sync.
 */

declare const value_variance: unique symbol;

/**
 * A named slot in the run's value scope. The phantom parameter keeps `get`
 * honest without forcing a runtime carrier.
 */
export interface Value<A> {
	readonly key: string;
	readonly [value_variance]: (_: never) => A;
}

export const value = <A>(key: string): Value<A> => ({ key }) as unknown as Value<A>;

export interface StepHandle {
	/** Right-hand annotation on this step's row. */
	readonly detail: (text: string) => void;
	/** Throws a named error when no completed step produced the value. */
	readonly get: <A>(value: Value<A>) => A;
	readonly log: (line: string) => void;
	readonly peek: <A>(value: Value<A>) => A | undefined;
	readonly progress: (done: number, total?: number) => void;
	readonly set: <A>(value: Value<A>, next: A) => void;
	readonly warn: (text: string) => void;
}

/**
 * An Effect body may require the run's Scope, so a step can acquire something
 * that stays alive for the rest of the checklist and is released even when the
 * run fails or is interrupted.
 */
export type Awaitable<A> = A | Effect.Effect<A, unknown, Scope.Scope> | Promise<A>;

/**
 * A string is a shell command line, an array is argv with no shell, and a
 * function is arbitrary work. The string form exists because package-manager
 * shims on Windows are command scripts that must not have their argv split.
 */
const command_marker = "~@artisanstreet/checklist/CommandRequest";

export interface CommandRequest {
	readonly [command_marker]: true;
	readonly run: ReadonlyArray<string> | string;
}

/**
 * Marks a value as a command to execute rather than a result to store. Only
 * needed when the command line itself depends on an upstream value: a function
 * returning a bare array is otherwise indistinguishable from a step whose
 * output happens to be an array.
 */
export const command = (run: ReadonlyArray<string> | string): CommandRequest => ({
	[command_marker]: true,
	run,
});

export const is_command_request = (value: unknown): value is CommandRequest =>
	typeof value === "object" && value !== null && command_marker in value;

export type StepRun<A> =
	| ReadonlyArray<string>
	| string
	| ((step: StepHandle) => Awaitable<A> | CommandRequest);

export interface RetryOptions {
	readonly attempts: number;
	readonly delay?: Duration.Input;
}

export interface StepBase {
	readonly name: string;
	/** Failure is recorded and reported, but does not fail the run. */
	readonly optional?: boolean;
	/** A false result marks the step skipped, not failed. */
	readonly when?: (step: StepHandle) => Awaitable<boolean>;
}

export interface TaskStep<A = unknown> extends StepBase {
	readonly cwd?: string | ((step: StepHandle) => string);
	readonly env?:
		| Readonly<Record<string, string>>
		| ((step: StepHandle) => Readonly<Record<string, string>>);
	/** Declares this step's output; constrains `run`'s return type when authored through `task`. */
	readonly provides?: Value<A>;
	readonly retry?: number | RetryOptions;
	readonly run: StepRun<A>;
	/** Called per output line. The dominant use is pulling progress out of tool chatter. */
	readonly watch?: (line: string, step: StepHandle) => void;
}

export interface GroupStep extends StepBase {
	/** 1 (the default) runs children in order; anything else overlaps them. */
	readonly concurrency?: "unbounded" | number;
	readonly steps: ReadonlyArray<Step> | ((step: StepHandle) => Awaitable<ReadonlyArray<Step>>);
}

export type Step = GroupStep | TaskStep;

export const is_group_step = (step: Step): step is GroupStep => "steps" in step;

/**
 * Identity helper. Object literals inside a `ReadonlyArray<Step>` are checked
 * against `TaskStep<unknown>`, which loses the `provides`/`run` correlation;
 * authoring a step through this recovers it at zero runtime cost.
 */
export const task = <A>(step: TaskStep<A>): TaskStep<A> => step;

export type Presentation = "auto" | "json" | "plain" | "tui";

export interface ChecklistOptions {
	/** Concurrency for the root group. */
	readonly concurrency?: "unbounded" | number;
	readonly max_log_lines?: number;
	readonly presentation?: Presentation;
	readonly subtitle?: string;
	readonly title?: string;
}
