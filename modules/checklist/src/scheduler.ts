import { Data, Effect, Stream, type Scope } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";

import type { ChecklistEvent, NodeDefinition, StepStatus } from "./model.ts";
import {
	is_command_request,
	is_group_step,
	type Awaitable,
	type ChecklistOptions,
	type GroupStep,
	type RetryOptions,
	type Step,
	type StepHandle,
	type TaskStep,
	type Value,
} from "./step.ts";

/**
 * Walks the step tree and drives it. Structure is the graph: an array runs in
 * declaration order, a group with concurrency overlaps its children, and values
 * flow through a scope layered to match that shape.
 */

export type EmitChecklistEvent = (event: ChecklistEvent) => void;

export class StepFailure extends Data.TaggedError("StepFailure")<{
	readonly cause: unknown;
	readonly node_id: string;
	readonly reason: string;
	readonly step_name: string;
}> {}

/**
 * A value scope layered over its parent. Sequential siblings share one scope so
 * later steps observe earlier writes; concurrent siblings each get a private
 * child that merges upward only once the whole group settles, which is what
 * keeps them from racing on one another's output.
 */
interface ValueScope {
	readonly own: Map<string, unknown>;
	readonly parent: ValueScope | undefined;
}

export const make_value_scope = (parent?: ValueScope): ValueScope => ({
	own: new Map<string, unknown>(),
	parent,
});

const read_value = (
	scope: ValueScope,
	key: string,
): { readonly present: boolean; readonly value: unknown } => {
	let current: ValueScope | undefined = scope;

	while (current !== undefined) {
		if (current.own.has(key)) return { present: true, value: current.own.get(key) };
		current = current.parent;
	}

	return { present: false, value: undefined };
};

const merge_value_scope = (target: ValueScope, source: ValueScope): void => {
	for (const [key, value] of source.own) target.own.set(key, value);
};

interface PlannedNode {
	/** Undefined marks a group whose children are resolved at schedule time. */
	readonly children: ReadonlyArray<PlannedNode> | undefined;
	readonly definition: NodeDefinition;
	readonly step: Step;
}

export const plan_steps = (
	steps: ReadonlyArray<Step>,
	parent_id: string | undefined,
	depth: number,
): ReadonlyArray<PlannedNode> =>
	steps.map((step, index) => {
		const id = parent_id === undefined ? `${index + 1}` : `${parent_id}.${index + 1}`;
		const group = is_group_step(step);

		return {
			children:
				group && typeof step.steps !== "function"
					? plan_steps(step.steps, id, depth + 1)
					: undefined,
			definition: {
				depth,
				id,
				is_group: group,
				name: step.name,
				optional: step.optional ?? false,
				parent_id: parent_id ?? null,
			},
			step,
		};
	});

export const flatten_plan = (nodes: ReadonlyArray<PlannedNode>): ReadonlyArray<NodeDefinition> =>
	nodes.flatMap((node) => [node.definition, ...flatten_plan(node.children ?? [])]);

interface RunContext {
	readonly emit: EmitChecklistEvent;
	readonly now: () => number;
}

const make_handle = (input: {
	readonly context: RunContext;
	readonly node_id: string;
	readonly scope: ValueScope;
	readonly step_name: string;
}): StepHandle => ({
	detail: (text) => input.context.emit({ detail: text, node_id: input.node_id, type: "detail" }),
	get: <A>(value: Value<A>): A => {
		const found = read_value(input.scope, value.key);

		if (!found.present) {
			throw new Error(
				`step "${input.step_name}" read value "${value.key}", which no completed step produced`,
			);
		}

		return found.value as A;
	},
	log: (line) => input.context.emit({ line, node_id: input.node_id, type: "log" }),
	peek: <A>(value: Value<A>): A | undefined => read_value(input.scope, value.key).value as A,
	progress: (done, total) =>
		input.context.emit({
			done,
			node_id: input.node_id,
			total: total ?? null,
			type: "progress",
		}),
	set: (value, next) => {
		input.scope.own.set(value.key, next);
	},
	warn: (text) =>
		input.context.emit({ line: `warning: ${text}`, node_id: input.node_id, type: "log" }),
});

/** Normalizes the three shapes a step body may return into one Effect. */
const Resolve = <A>(result: Awaitable<A>): Effect.Effect<A, unknown, Scope.Scope> => {
	if (Effect.isEffect(result)) return result as Effect.Effect<A, unknown>;
	if (result instanceof Promise) {
		return Effect.tryPromise({ catch: (cause: unknown) => cause, try: () => result });
	}

	return Effect.succeed(result);
};

/** Sync throws (a `get` on a missing value) belong in the error channel, not as defects. */
const Invoke = <A>(call: () => Awaitable<A>): Effect.Effect<A, unknown, Scope.Scope> =>
	Effect.try({ catch: (cause: unknown) => cause, try: call }).pipe(Effect.flatMap(Resolve));

const failure_reason = (cause: unknown): string =>
	cause instanceof Error ? cause.message : String(cause);

const resolve_environment = (
	step: TaskStep,
	handle: StepHandle,
): Readonly<Record<string, string>> | undefined =>
	typeof step.env === "function" ? step.env(handle) : step.env;

const resolve_cwd = (step: TaskStep, handle: StepHandle): string | undefined =>
	typeof step.cwd === "function" ? step.cwd(handle) : step.cwd;

/**
 * A string body is one shell command line; splitting it would break the
 * package-manager shims on Windows, which are command scripts. An array body is
 * argv and never reaches a shell. Standard input is closed either way: an
 * interactive step would fight the renderer for the terminal.
 */
const RunCommand = (
	run: ReadonlyArray<string> | string,
	step: TaskStep,
	handle: StepHandle,
	node_id: string,
	context: RunContext,
): Effect.Effect<void, unknown, ChildProcessSpawner.ChildProcessSpawner> =>
	Effect.scoped(
		Effect.gen(function* () {
			const cwd = resolve_cwd(step, handle);
			const environment = resolve_environment(step, handle);
			const options = {
				stderr: "pipe",
				stdin: "ignore",
				stdout: "pipe",
				...(cwd === undefined ? {} : { cwd }),
				/** A step's `env` augments the inherited environment; PATH must survive. */
				...(environment === undefined ? {} : { env: environment, extendEnv: true }),
			} as const;
			const command =
				typeof run === "string"
					? ChildProcess.make(run, { ...options, shell: true })
					: ChildProcess.make(run[0] ?? "", run.slice(1), options);
			const child = yield* command;

			yield* child.all.pipe(
				Stream.decodeText(),
				Stream.splitLines,
				Stream.runForEach((line) =>
					Effect.sync(() => {
						context.emit({ line, node_id, type: "log" });
						step.watch?.(line, handle);
					}),
				),
			);

			const exit_code = yield* child.exitCode;

			if (exit_code !== 0) {
				const label = typeof run === "string" ? run : run.join(" ");

				return yield* Effect.fail(new Error(`${label} exited with code ${exit_code}`));
			}
		}),
	);

const retry_settings = (retry: number | RetryOptions | undefined): RetryOptions =>
	retry === undefined ? { attempts: 1 } : typeof retry === "number" ? { attempts: retry } : retry;

/**
 * Hand-rolled rather than scheduled so each attempt re-invokes the body from
 * scratch and the retry notice lands in that step's own output.
 */
const WithRetry = <R>(
	body: Effect.Effect<void, unknown, R>,
	settings: RetryOptions,
	handle: StepHandle,
): Effect.Effect<void, unknown, R> => {
	const Attempt = (remaining: number): Effect.Effect<void, unknown, R> =>
		body.pipe(
			Effect.catch((cause: unknown) => {
				if (remaining <= 0) return Effect.fail(cause);

				handle.warn(`${failure_reason(cause)} — retrying (${remaining} left)`);

				return settings.delay === undefined
					? Attempt(remaining - 1)
					: Effect.sleep(settings.delay).pipe(
							Effect.flatMap(() => Attempt(remaining - 1)),
						);
			}),
		);

	return Attempt(Math.max(0, settings.attempts - 1));
};

const set_status = (context: RunContext, node_id: string, status: StepStatus): void => {
	context.emit({ at: context.now(), node_id, status, type: "status" });
};

const mark_subtree_skipped = (node: PlannedNode, context: RunContext): void => {
	set_status(context, node.definition.id, "skipped");
	for (const child of node.children ?? []) mark_subtree_skipped(child, context);
};

const EvaluateWhen = (
	step: Step,
	handle: StepHandle,
): Effect.Effect<boolean, unknown, Scope.Scope> =>
	step.when === undefined ? Effect.succeed(true) : Invoke(() => step.when?.(handle) ?? true);

type RunEnvironment = ChildProcessSpawner.ChildProcessSpawner | Scope.Scope;

const RunTask = (
	node: PlannedNode,
	step: TaskStep,
	scope: ValueScope,
	context: RunContext,
): Effect.Effect<void, StepFailure, RunEnvironment> =>
	Effect.gen(function* () {
		const node_id = node.definition.id;
		const handle = make_handle({ context, node_id, scope, step_name: step.name });
		const fail = (cause: unknown) =>
			new StepFailure({
				cause,
				node_id,
				reason: failure_reason(cause),
				step_name: step.name,
			});

		const enabled = yield* EvaluateWhen(step, handle).pipe(Effect.mapError(fail));

		if (!enabled) {
			set_status(context, node_id, "skipped");
			return;
		}

		set_status(context, node_id, "running");

		const body: Effect.Effect<void, unknown, RunEnvironment> =
			typeof step.run === "function"
				? Invoke(() =>
						(step.run as (handle: StepHandle) => Awaitable<unknown>)(handle),
					).pipe(
						Effect.flatMap((produced) =>
							is_command_request(produced)
								? RunCommand(produced.run, step, handle, node_id, context)
								: Effect.sync(() => {
										if (step.provides !== undefined) {
											scope.own.set(step.provides.key, produced);
										}
									}),
						),
					)
				: RunCommand(step.run, step, handle, node_id, context);

		const succeeded = yield* WithRetry(body, retry_settings(step.retry), handle).pipe(
			Effect.as(true),
			Effect.catch((cause: unknown) => {
				const failure = fail(cause);

				context.emit({ node_id, reason: failure.reason, type: "failure" });
				set_status(context, node_id, "failed");

				/** An optional step reports its failure but does not end the run. */
				return step.optional === true ? Effect.succeed(false) : Effect.fail(failure);
			}),
		);

		if (succeeded) set_status(context, node_id, "passed");
	});

const RunGroup = (
	node: PlannedNode,
	group: GroupStep,
	scope: ValueScope,
	context: RunContext,
): Effect.Effect<void, StepFailure, RunEnvironment> =>
	Effect.gen(function* () {
		const node_id = node.definition.id;
		const handle = make_handle({ context, node_id, scope, step_name: group.name });
		const fail = (cause: unknown) =>
			new StepFailure({
				cause,
				node_id,
				reason: failure_reason(cause),
				step_name: group.name,
			});

		const enabled = yield* EvaluateWhen(group, handle).pipe(Effect.mapError(fail));

		if (!enabled) {
			mark_subtree_skipped(node, context);
			return;
		}

		set_status(context, node_id, "running");

		const children =
			node.children ??
			(yield* Invoke(() =>
				(group.steps as (handle: StepHandle) => Awaitable<ReadonlyArray<Step>>)(handle),
			).pipe(
				Effect.mapError(fail),
				Effect.map((resolved) => {
					const planned = plan_steps(resolved, node_id, node.definition.depth + 1);

					context.emit({
						node_id,
						nodes: flatten_plan(planned),
						type: "expand",
					});

					return planned;
				}),
			));

		const concurrency = group.concurrency ?? 1;

		if (concurrency === 1) {
			for (const child of children) yield* RunNode(child, scope, context);
		} else {
			const child_scopes = children.map(() => make_value_scope(scope));

			yield* Effect.forEach(
				children,
				(child, index) => RunNode(child, child_scopes[index] ?? scope, context),
				{ concurrency },
			);

			for (const child_scope of child_scopes) merge_value_scope(scope, child_scope);
		}

		set_status(context, node_id, "passed");
	});

const RunNode = (
	node: PlannedNode,
	scope: ValueScope,
	context: RunContext,
): Effect.Effect<void, StepFailure, RunEnvironment> =>
	is_group_step(node.step)
		? RunGroup(node, node.step, scope, context)
		: RunTask(node, node.step, scope, context);

export interface ScheduleInput {
	readonly emit: EmitChecklistEvent;
	readonly now?: () => number;
	readonly options: ChecklistOptions;
	readonly steps: ReadonlyArray<Step>;
}

/**
 * Emits `configure`, drives the tree, and always emits `finish` so every
 * presentation sees a terminal event even when the run fails or is interrupted.
 */
export const ScheduleChecklist = (
	input: ScheduleInput,
): Effect.Effect<boolean, never, RunEnvironment> =>
	Effect.gen(function* () {
		const now = input.now ?? (() => Date.now());
		const context: RunContext = { emit: input.emit, now };
		const planned = plan_steps(input.steps, undefined, 0);
		const scope = make_value_scope();

		context.emit({
			max_log_lines: input.options.max_log_lines ?? 500,
			nodes: flatten_plan(planned),
			started_at: now(),
			subtitle: input.options.subtitle ?? null,
			title: input.options.title ?? "Checklist",
			type: "configure",
		});

		const concurrency = input.options.concurrency ?? 1;
		const Body =
			concurrency === 1
				? Effect.forEach(planned, (node) => RunNode(node, scope, context), {
						discard: true,
					})
				: Effect.gen(function* () {
						const scopes = planned.map(() => make_value_scope(scope));

						yield* Effect.forEach(
							planned,
							(node, index) => RunNode(node, scopes[index] ?? scope, context),
							{ concurrency, discard: true },
						);
					});

		const outcome = yield* Body.pipe(
			Effect.as(true),
			Effect.catch(() => Effect.succeed(false)),
			Effect.onInterrupt(() =>
				Effect.sync(() => {
					context.emit({ at: now(), outcome: "failed", type: "finish" });
				}),
			),
		);

		context.emit({ at: now(), outcome: outcome ? "passed" : "failed", type: "finish" });

		return outcome;
	});
