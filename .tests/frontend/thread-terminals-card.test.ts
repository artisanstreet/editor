import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import type { TerminalSession } from "@artisan/protocol";
import {
	append_terminal_output,
	apply_terminal_lifecycle,
	is_live_terminal,
	present_terminal_output,
	terminal_command_line,
	terminal_display_name,
} from "../../modules/frontend/src/lib/terminal/presentation";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

const session = (overrides: Partial<TerminalSession>): TerminalSession => ({
	args: ["dev"],
	cols: 100,
	created_at: "2026-08-14T08:00:00.000Z",
	executable: "C:\\tools\\vp.exe",
	generation: 1,
	rows: 30,
	state: "active",
	terminal_id: "terminal_1",
	thread_id: "thread_1",
	updated_at: "2026-08-14T08:00:00.000Z",
	working_directory: "C:\\repo",
	workspace_id: "workspace_1",
	...overrides,
});

describe("terminal presentation", () => {
	/**
	 * The row is titled by the interpreter and identified by what it was told to
	 * run, so the command never repeats the shell already named on the left.
	 */
	it("titles a terminal by its shell and identifies it by its command", () => {
		const powershell = session({
			args: ["npm", "run", "dev", "--host", "editor local"],
			executable: "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
		});

		expect(terminal_display_name(powershell)).toBe("PowerShell");
		expect(terminal_command_line(powershell)).toBe('npm run dev --host "editor local"');
	});

	/** One shell ships under two filenames; people call both PowerShell. */
	it("gives known shells the names people call them", () => {
		expect(terminal_display_name(session({ executable: "/usr/bin/bash" }))).toBe("Bash");
		expect(terminal_display_name(session({ executable: "powershell.exe" }))).toBe("PowerShell");
		expect(terminal_display_name(session({ executable: "nu" }))).toBe("Nushell");
		expect(terminal_display_name(session({ executable: "cmd.exe" }))).toBe("Command Prompt");
		expect(terminal_display_name(session({ executable: "shell" }))).toBe("Shell");
	});

	/**
	 * An unfamiliar row that names the program is more use than one flattened
	 * into a generic word, so an unmapped shell keeps its own name.
	 */
	it("keeps the program name of a shell it has no name for", () => {
		expect(terminal_display_name(session({ executable: "/opt/oil/osh" }))).toBe("osh");
	});

	it("strips ANSI sequences and collapses carriage-return progress frames", () => {
		const raw = "\u001b[32mready\u001b[0m in 300ms\nbuild 10%\rbuild 60%\rdone\n";

		expect(present_terminal_output(raw)).toBe("ready in 300ms\ndoned 60%\n");
	});

	it("keeps only the newest window of an unbounded log", () => {
		const grown = append_terminal_output("a".repeat(200_000), "TAIL");

		expect(grown.length).toBe(200_000);
		expect(grown.endsWith("TAIL")).toBe(true);
	});

	it("shows opening and active terminals and drops exited ones like finished agents", () => {
		const opening = session({ state: "opening", terminal_id: "terminal_opening" });
		const closed = session({ state: "closed", terminal_id: "terminal_closed" });
		const failed = session({ state: "failed", terminal_id: "terminal_failed" });

		expect([opening, session({}), closed, failed].filter(is_live_terminal)).toEqual([
			opening,
			session({}),
		]);
	});

	it("applies lifecycle events as upserts keyed by terminal id", () => {
		const opened = session({});
		const exited = session({ exit_code: 1, state: "closed" });
		const other = session({ terminal_id: "terminal_2" });

		expect(apply_terminal_lifecycle([], opened)).toEqual([opened]);
		expect(apply_terminal_lifecycle([opened], other)).toEqual([opened, other]);
		expect(apply_terminal_lifecycle([opened, other], exited)).toEqual([exited, other]);
	});
});

describe("thread terminals card", () => {
	const panel = ReadSource("modules/frontend/src/routes/components/thread-panel.svelte");
	const card = ReadSource("modules/frontend/src/routes/components/thread-terminals-card.svelte");
	const list = ReadSource("modules/frontend/src/routes/components/thread-terminals.svelte");

	it("mounts in the right rail beside the agents card", () => {
		expect(panel).toContain("ThreadTerminalsCard");
		expect(panel).toContain("<ThreadTerminalsCard {hover} {thread_id} {workspace_id} />");
	});

	it("lists live terminals from public contracts and refreshes on lifecycle events", () => {
		expect(card).toContain("const terminals_controller = yield* ThreadTerminalsController");
		expect(card).toContain("yield* terminals_controller.Current(");
		expect(card).toContain("terminals_controller.Changes.pipe(");
		expect(card).toContain("terminals_controller.Refresh(");
		expect(card).toContain("Effect.forkScoped");
		expect(card).not.toContain(".ListTerminals(");
		expect(card).toContain('"terminal.lifecycle"');
		expect(card).toContain("filter(is_live_terminal)");
	});

	it("shows the command line beside each terminal so nothing has to be guessed", () => {
		expect(list).toContain("terminal_command_line(entry)");
		expect(list).toContain("terminal_display_name(entry)");
		/**
		 * One line rather than a name over a subtext: the row reads as a single
		 * fact, with the program on the left and the command it is running on the
		 * right. The command yields its width first and leaves entirely once the
		 * column cannot carry both, since a few clipped characters of a path say
		 * less than nothing.
		 */
		expect(list).toContain("@container");
		expect(list).toContain("items-center justify-between gap-4");
		expect(list).toContain("@min-[14rem]:block");
		expect(list).not.toContain("flex-col gap-0.5");
	});

	it("opens output read-only and never writes to the PTY", () => {
		expect(card).toContain("OpenTerminalOutput");
		expect(card).toContain('viewer_state = "ready"');
		expect(card).toContain('viewer_state = "failed"');
		expect(card).toContain("AppendOutput(decoder.decode())");
		expect(card).toContain('"Waiting for output…"');
		expect(card).toContain('"Terminal output is unavailable."');
		/** Replacing or closing a viewer owns interruption, not merely stale rendering. */
		expect(card).toContain("Scope.close(scope, Exit.void)");
		expect(card).toContain(
			"Effect.forkIn(FollowOutput(terminal, generation), next_output_scope)",
		);
		expect(card).toContain("onOpenChange={yield* HandleViewerOpenChange(event)}");
		for (const source of [card, list]) {
			expect(source).not.toContain("terminal.write");
			expect(source).not.toContain("TerminalWrite");
			expect(source).not.toContain("client.Command");
		}
	});
});
