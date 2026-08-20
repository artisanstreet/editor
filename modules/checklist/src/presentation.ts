import {
	apply_checklist_event,
	create_checklist_state,
	format_duration,
	format_progress,
	sanitize_log_line,
	summarize_checklist,
	type ChecklistEvent,
	type ChecklistState,
} from "./model.ts";
import type { Presentation } from "./step.ts";

/**
 * Non-interactive output is a first-class mode, not a fallback. The plain sink
 * keeps full output fidelity for humans and agents reading a scrollback; the
 * json sink is the raw event stream for anything that wants to parse it.
 */

export interface Presenter {
	readonly close: () => void;
	readonly emit: (event: ChecklistEvent) => void;
}

const presentation_flags: Readonly<Record<string, Exclude<Presentation, "auto">>> = {
	"--json": "json",
	"--no-tui": "plain",
	"--plain": "plain",
	"--tui": "tui",
};

/** Consumers parse their own arguments, so give them a way to drop ours first. */
export const strip_presentation_flags = (argv: ReadonlyArray<string>): ReadonlyArray<string> =>
	argv.filter((argument) => presentation_flags[argument] === undefined);

const flag_presentation = (
	argv: ReadonlyArray<string>,
): Exclude<Presentation, "auto"> | undefined => {
	for (const argument of argv) {
		const presentation = presentation_flags[argument];
		if (presentation !== undefined) return presentation;
	}

	return undefined;
};

const environment_disables_tui = (
	environment: Readonly<Record<string, string | undefined>>,
): boolean =>
	environment.NO_TUI === "1" ||
	environment.NO_TUI === "true" ||
	environment.ARTISAN_TUI === "0" ||
	environment.CI !== undefined ||
	environment.TERM === "dumb";

/**
 * An explicit flag beats the call site, which beats the environment, which
 * beats terminal detection. Anything an agent or CI runs therefore lands on
 * plain text without the caller having to know it is not on a terminal.
 */
export const resolve_presentation = (input: {
	readonly argv: ReadonlyArray<string>;
	readonly environment: Readonly<Record<string, string | undefined>>;
	readonly requested: Presentation | undefined;
	readonly stdout_is_tty: boolean | undefined;
}): Exclude<Presentation, "auto"> => {
	const flag = flag_presentation(input.argv);
	const chosen =
		flag ??
		(input.requested !== undefined && input.requested !== "auto"
			? input.requested
			: environment_disables_tui(input.environment)
				? "plain"
				: "tui");

	/**
	 * The dashboard needs a terminal to drive. Asking for it without one is a
	 * mistake worth absorbing rather than honouring: the renderer would take over
	 * a stream that cannot show it and leave escape sequences in the transcript.
	 */
	return chosen === "tui" && input.stdout_is_tty !== true ? "plain" : chosen;
};

const status_marks: Readonly<Record<string, string>> = {
	cancelled: "·",
	failed: "×",
	passed: "✓",
	running: "▶",
	skipped: "-",
};

const node_name = (state: ChecklistState, node_id: string): string =>
	state.nodes.find((node) => node.id === node_id)?.name ?? node_id;

const node_duration = (state: ChecklistState, node_id: string): string => {
	const node = state.nodes.find((candidate) => candidate.id === node_id);

	if (node?.started_at === undefined || node.ended_at === undefined) return "";

	return `  ${format_duration(node.ended_at - node.started_at)}`;
};

export interface PlainPresenterOptions {
	/** Emits GitHub Actions fold markers around each step's output. */
	readonly fold_output?: boolean;
	readonly write: (line: string) => void;
}

/**
 * Streams every output line. Lines stay unprefixed while exactly one task is
 * running so compiler and test output survives copy-paste and downstream
 * parsing untouched; a prefix appears only once steps genuinely overlap.
 */
export const make_plain_presenter = (options: PlainPresenterOptions): Presenter => {
	let state = create_checklist_state();
	let running = 0;
	const fold = options.fold_output ?? false;

	const emit = (event: ChecklistEvent): void => {
		state = apply_checklist_event(state, event);

		if (event.type === "configure") {
			const subtitle = event.subtitle === null ? "" : ` · ${event.subtitle}`;
			const tasks = state.nodes.filter((node) => !node.is_group).length;

			options.write(`── ${event.title}${subtitle} — ${tasks} steps`);
			return;
		}

		if (event.type === "log") {
			const line = sanitize_log_line(event.line);

			options.write(running > 1 ? `[${node_name(state, event.node_id)}] ${line}` : line);
			return;
		}

		if (event.type === "progress") {
			const node = state.nodes.find((candidate) => candidate.id === event.node_id);

			if (node?.progress !== undefined && node.progress.total !== undefined) {
				options.write(`   ${node.name}: ${format_progress(node.progress)}`);
			}
			return;
		}

		if (event.type === "failure") {
			options.write(`   ${node_name(state, event.node_id)}: ${event.reason}`);
			return;
		}

		if (event.type === "status") {
			const node = state.nodes.find((candidate) => candidate.id === event.node_id);

			if (node === undefined || node.is_group) return;

			if (event.status === "running") {
				running += 1;
				options.write(`${status_marks.running} ${node.name}`);
				if (fold) options.write(`::group::${node.name}`);
				return;
			}

			if (event.status === "pending") return;

			running = Math.max(0, running - 1);
			if (fold) options.write("::endgroup::");
			options.write(
				`${status_marks[event.status] ?? "?"} ${node.name}${node_duration(state, event.node_id)}`,
			);
			return;
		}

		if (event.type === "finish") {
			const summary = summarize_checklist(state);
			const counted = (status: string) =>
				summary.steps.filter((entry) => entry.status === status).length;
			const duration =
				summary.duration_ms === undefined
					? ""
					: ` in ${format_duration(summary.duration_ms)}`;

			options.write("");
			options.write(
				`${summary.outcome === "passed" ? "PASSED" : "FAILED"} ${summary.title}${duration} — ` +
					`${counted("passed")} passed, ${counted("failed")} failed, ` +
					`${counted("skipped")} skipped, ${counted("cancelled")} cancelled`,
			);

			for (const entry of summary.steps) {
				if (entry.status !== "failed") continue;
				options.write(`   × ${entry.path}${entry.optional ? " (optional)" : ""}`);
				if (entry.failure !== undefined) options.write(`     ${entry.failure}`);
			}
		}
	};

	return { close: () => {}, emit };
};

export const make_json_presenter = (write: (line: string) => void): Presenter => ({
	close: () => {},
	emit: (event) => write(JSON.stringify(event)),
});
