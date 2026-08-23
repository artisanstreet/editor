import { Data, Effect, type Scope } from "effect";
import type { ChildProcessSpawner } from "effect/unstable/process";

import {
	apply_checklist_event,
	create_checklist_state,
	summarize_checklist,
	type ChecklistEvent,
	type ChecklistState,
	type ChecklistSummary,
} from "./model.ts";
import {
	make_json_presenter,
	make_plain_presenter,
	resolve_presentation,
	type Presenter,
} from "./presentation.ts";
import { ScheduleChecklist } from "./scheduler.ts";
import { command, task, value, type ChecklistOptions, type Step } from "./step.ts";
import { MakeTuiPresenter } from "./tui-bridge.ts";

export type {
	ChecklistEvent,
	ChecklistNode,
	ChecklistState,
	ChecklistSummary,
	ChecklistSummaryEntry,
	LogChunk,
	LogLine,
	LogStyle,
	StepStatus,
} from "./model.ts";
export {
	apply_checklist_event,
	create_checklist_state,
	format_duration,
	is_checklist_event,
	summarize_checklist,
} from "./model.ts";
export type { Presenter } from "./presentation.ts";
export {
	make_json_presenter,
	make_plain_presenter,
	resolve_presentation,
	strip_presentation_flags,
} from "./presentation.ts";
export { StepFailure } from "./scheduler.ts";
export type {
	ChecklistOptions,
	CommandRequest,
	GroupStep,
	Presentation,
	RetryOptions,
	Step,
	StepHandle,
	StepRun,
	TaskStep,
	Value,
} from "./step.ts";
export { command, is_command_request, task, value } from "./step.ts";
export { restore_terminal_presentation, terminal_presentation_reset } from "./tui-bridge.ts";

/**
 * Carries the whole summary so a failed run stays reportable, and a message so
 * the runtime's own failure log names the steps instead of printing an empty
 * tag underneath the summary that was just rendered.
 */
export class ChecklistFailed extends Data.TaggedError("ChecklistFailed")<{
	readonly message: string;
	readonly summary: ChecklistSummary;
}> {}

const failure_message = (summary: ChecklistSummary): string => {
	const failed = summary.steps.filter((entry) => entry.status === "failed" && !entry.optional);

	return failed.length === 0
		? `${summary.title} failed`
		: `${summary.title} failed at ${failed.map((entry) => entry.path).join(", ")}`;
};

const write_line = (line: string): void => {
	process.stdout.write(`${line}\n`);
};

const persistent_failure_log_limit = 80;

/**
 * Replays the useful tail after a full-screen presenter has been torn down.
 * Without this, a failed build returns to the shell with only the wrapper's
 * step name while the compiler diagnostic disappears with the alternate screen.
 */
export const format_persistent_failure_report = (state: ChecklistState): ReadonlyArray<string> =>
	state.nodes
		.filter((node) => !node.is_group && node.status === "failed")
		.flatMap((node) => [
			`── ${node.name} failed`,
			...node.log_lines.slice(-persistent_failure_log_limit).map((line) => line.text),
			...(node.failure === undefined ? [] : [node.failure]),
		]);

const MakePresenter = (options: ChecklistOptions): Effect.Effect<Presenter, never, Scope.Scope> =>
	Effect.suspend(() => {
		const presentation = resolve_presentation({
			argv: process.argv.slice(2),
			environment: process.env,
			requested: options.presentation,
			stdout_is_tty: process.stdout.isTTY,
		});

		if (presentation === "json") return Effect.succeed(make_json_presenter(write_line));
		if (presentation === "tui") return MakeTuiPresenter(write_line);

		return Effect.succeed(
			make_plain_presenter({
				fold_output: process.env.GITHUB_ACTIONS !== undefined,
				write: write_line,
			}),
		);
	});

/**
 * Runs the checklist and resolves with its summary. Failure carries the same
 * summary so a failed run stays fully reportable rather than just an exit code,
 * and `NodeRuntime.runMain` turns it into a non-zero exit on its own.
 */
export const make = (
	steps: ReadonlyArray<Step>,
	options: ChecklistOptions = {},
): Effect.Effect<ChecklistSummary, ChecklistFailed, ChildProcessSpawner.ChildProcessSpawner> =>
	Effect.scoped(
		Effect.gen(function* () {
			const presenter = yield* MakePresenter(options);
			let state = create_checklist_state(options.title ?? "Checklist");

			const emit = (event: ChecklistEvent): void => {
				state = apply_checklist_event(state, event);
				presenter.emit(event);
			};

			const passed = yield* ScheduleChecklist({ emit, options, steps });
			const summary = summarize_checklist(state);

			if (!passed) {
				if (presenter.transient === true) {
					yield* Effect.promise(async () => await presenter.close());
					for (const line of format_persistent_failure_report(state)) write_line(line);
				}
				return yield* Effect.fail(
					new ChecklistFailed({ message: failure_message(summary), summary }),
				);
			}

			return summary;
		}),
	);

export const Checklist = { command, make, task, value } as const;
