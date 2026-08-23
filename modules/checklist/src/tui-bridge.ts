import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";

import { Effect, type Scope } from "effect";

import type { ChecklistEvent } from "./model.ts";
import { make_plain_presenter, type Presenter } from "./presentation.ts";

/**
 * Parent half of the renderer. OpenTUI needs Bun, so the dashboard runs as a
 * child process fed newline-delimited events over fd 3, with fd 4 carrying the
 * quit request back. Anything that goes wrong degrades to plain text rather
 * than taking the run down with it.
 */

const dependency_require = createRequire(import.meta.url);

/**
 * Terminal modes that a renderer may leave behind if its process is killed
 * between setup and destroy. Mouse modes are the visible failure: every
 * pointer move becomes an SGR escape sequence typed into the resumed shell.
 */
export const terminal_presentation_reset = [
	"\u001b[?1000l",
	"\u001b[?1002l",
	"\u001b[?1003l",
	"\u001b[?1004l",
	"\u001b[?1005l",
	"\u001b[?1006l",
	"\u001b[?1007l",
	"\u001b[?1015l",
	"\u001b[?1016l",
	"\u001b[?2004l",
	"\u001b[?1049l",
	"\u001b[<u",
	"\u001b[?25h",
	"\u001b[0m",
].join("");

interface TerminalInput {
	readonly isTTY?: boolean;
	readonly setRawMode?: (enabled: boolean) => unknown;
}

interface TerminalOutput {
	readonly isTTY?: boolean;
	readonly write: (text: string) => unknown;
}

/** Idempotent parent-side recovery for graceful and abnormal dashboard exits. */
export const restore_terminal_presentation = (
	input: TerminalInput = process.stdin,
	output: TerminalOutput = process.stdout,
): void => {
	if (input.isTTY === true && input.setRawMode !== undefined) {
		try {
			input.setRawMode(false);
		} catch {
			/** The console handle may already be closing. */
		}
	}

	if (output.isTTY !== true) return;
	try {
		output.write(terminal_presentation_reset);
	} catch {
		/** The process is already past writable shutdown. */
	}
};

const resolve_bun_executable = (): string => {
	const package_path = dependency_require.resolve("bun/package.json");
	const manifest = JSON.parse(readFileSync(package_path, "utf8")) as {
		readonly bin: { readonly bun: string };
	};

	return join(dirname(package_path), manifest.bin.bun);
};

const resolve_entry = (): string => dependency_require.resolve("@artisanstreet/checklist/entry");

const start_tui_presenter = (write: (line: string) => void): Presenter => {
	const fall_back = (reason: string): Presenter => {
		write(`checklist: dashboard unavailable (${reason}); using plain output`);

		return make_plain_presenter({ write });
	};

	let child: ChildProcess;

	try {
		child = spawn(resolve_bun_executable(), [resolve_entry()], {
			env: process.env,
			stdio: ["inherit", "inherit", "inherit", "pipe", "pipe"],
			windowsHide: true,
		});
	} catch (cause) {
		return fall_back(cause instanceof Error ? cause.message : String(cause));
	}

	const event_stream = child.stdio[3] as Writable | null;
	const command_stream = child.stdio[4] as Readable | null;

	if (event_stream === null || command_stream === null) {
		child.kill();

		return fall_back("the dashboard did not expose its event pipes");
	}

	const pending: ChecklistEvent[] = [];
	/**
	 * The structural events, kept so a dashboard that dies mid-run can hand its
	 * remaining output to a plain presenter that still knows the step tree.
	 * Everything else is display-only and not worth retaining.
	 */
	const structure: ChecklistEvent[] = [];
	let active = true;
	let backpressured = false;
	let closing = false;
	let fallback: Presenter | undefined;
	let close_promise: Promise<void> | undefined;
	let force_close_timer: ReturnType<typeof setTimeout> | undefined;

	const restore_terminal = () => restore_terminal_presentation();
	const emergency_exit = () => {
		if (child.exitCode === null && !child.killed) child.kill();
		restore_terminal();
	};
	process.once("exit", emergency_exit);
	const release_emergency_exit = () => process.removeListener("exit", emergency_exit);

	const degrade = (reason: string): void => {
		if (fallback !== undefined || closing) return;

		write(`checklist: dashboard ${reason}; using plain output`);
		fallback = make_plain_presenter({ write });
		for (const event of structure) fallback.emit(event);
	};

	const write_event = (event: ChecklistEvent): boolean =>
		event_stream.write(`${JSON.stringify(event)}\n`);

	const flush = (): void => {
		backpressured = false;

		try {
			while (active && pending.length > 0) {
				const event = pending.shift();

				if (event === undefined) return;
				if (write_event(event)) continue;

				backpressured = true;
				return;
			}
		} catch {
			active = false;
		}
	};

	event_stream.on("drain", flush);
	event_stream.once("error", () => {
		active = false;
		pending.length = 0;
	});
	child.once("error", (cause) => {
		active = false;
		pending.length = 0;
		restore_terminal();
		degrade(`failed (${cause.message})`);
	});
	child.once("exit", (code) => {
		active = false;
		pending.length = 0;
		if (force_close_timer !== undefined) clearTimeout(force_close_timer);
		release_emergency_exit();
		restore_terminal();
		degrade(`exited with code ${code ?? "unknown"}`);
	});

	/**
	 * A quit request has to interrupt the run, not just close the window; the
	 * signal path is what already tears down in-flight child processes.
	 */
	createInterface({ input: command_stream }).on("line", (line) => {
		try {
			const command = JSON.parse(line) as { readonly type?: unknown };

			if (command.type === "shutdown") process.emit("SIGINT");
		} catch {
			/** A malformed display command is ignored; the run keeps ownership. */
		}
	});

	return {
		close: () => {
			if (close_promise !== undefined) return close_promise;
			closing = true;
			close_promise = new Promise<void>((resolve) => {
				const finish = () => {
					if (force_close_timer !== undefined) clearTimeout(force_close_timer);
					release_emergency_exit();
					restore_terminal();
					resolve();
				};

				if (child.exitCode !== null) {
					finish();
					return;
				}

				child.once("exit", finish);
				try {
					if (!event_stream.destroyed && !event_stream.writableEnded) {
						event_stream.end(`${JSON.stringify({ type: "shutdown" })}\n`);
					}
				} catch {
					/** The dashboard is already gone; the exit waiter still restores. */
				}

				force_close_timer = setTimeout(() => {
					if (child.exitCode === null && !child.killed) child.kill();
					/** A broken child must not hold the failed build open forever. */
					setTimeout(finish, 500).unref();
				}, 500);
			});

			active = false;
			pending.length = 0;
			return close_promise;
		},
		emit: (event) => {
			if (event.type === "configure" || event.type === "expand") structure.push(event);

			if (fallback !== undefined) {
				fallback.emit(event);
				return;
			}

			if (!active || event_stream.destroyed || event_stream.writableEnded) return;

			if (backpressured) {
				pending.push(event);
				return;
			}

			try {
				backpressured = !write_event(event);
			} catch {
				active = false;
				degrade("stopped accepting events");
			}
		},
		transient: true,
	};
};

export const MakeTuiPresenter = (
	write: (line: string) => void,
): Effect.Effect<Presenter, never, Scope.Scope> =>
	Effect.acquireRelease(
		Effect.sync(() => start_tui_presenter(write)),
		(presenter) => Effect.promise(async () => await presenter.close()),
	);
