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
			windowsHide: false,
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
		degrade(`failed (${cause.message})`);
	});
	child.once("exit", (code) => {
		active = false;
		pending.length = 0;
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
			if (closing) return;
			closing = true;

			try {
				if (!event_stream.destroyed && !event_stream.writableEnded) {
					event_stream.end(`${JSON.stringify({ type: "shutdown" })}\n`);
				}
			} catch {
				/** The dashboard is already gone; nothing left to notify. */
			}

			active = false;
			pending.length = 0;
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
	};
};

export const MakeTuiPresenter = (
	write: (line: string) => void,
): Effect.Effect<Presenter, never, Scope.Scope> =>
	Effect.acquireRelease(
		Effect.sync(() => start_tui_presenter(write)),
		(presenter) => Effect.sync(presenter.close),
	);
