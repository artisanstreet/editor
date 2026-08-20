import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import { TerminalSession } from "@artisan/protocol";

import {
	AdoptObservedTerminalActivity,
	ObservedTerminalId,
	SplitObservedCommand,
	unknown_observed_shell,
	type ObservedTerminalContext,
} from "../../modules/backend/src/terminal/observed";

const context: ObservedTerminalContext = {
	observed_at: "2026-08-16T09:00:00.000Z",
	owner: { agent_id: "agent_root", run_id: "run_1" },
	thread_id: "thread_1",
	working_directory: "C:\\Users\\sander\\Desktop\\artisan-editor",
	workspace_id: "project_1",
};

/**
 * Artisan runs over another harness. The engine's own shell tool is what runs
 * `npm run dev`, and the normalizer already reports it — but that report only
 * ever became a transcript row, so the Terminals card stayed empty no matter
 * what the engine was running.
 */
describe("observed terminal adoption", () => {
	it("adopts a started command as a live session the card will show", () => {
		const session = AdoptObservedTerminalActivity(
			{ activity_id: "toolu_01", command: "npm run dev", shell: "pwsh", state: "started" },
			context,
		);

		expect(session?.state).toBe("active");
		/** Titled by what runs it, identified by what it runs. */
		expect(session?.executable).toBe("pwsh");
		expect(session?.args).toEqual(["npm", "run", "dev"]);
		/** Ownership is true: the agent really did run this during this run. */
		expect(session?.ownership).toEqual({
			agent_id: "agent_root",
			kind: "agent",
			run_id: "run_1",
		});
		/** Nothing spawned it here, so there is no process to claim. */
		expect(session?.pid).toBeUndefined();
	});

	it("produces a session the protocol accepts unchanged", () => {
		const session = AdoptObservedTerminalActivity(
			{ activity_id: "toolu_02", command: "pnpm vitest run", state: "started" },
			context,
		);
		const decoded = Effect.runSync(
			Effect.exit(
				Schema.decodeUnknownEffect(TerminalSession, { onExcessProperty: "error" })(session),
			),
		);

		/** No migration and no contract change: an observed session is a session. */
		expect(decoded._tag).toBe("Success");
	});

	it("keeps one session across the frames of a single command", () => {
		const started = AdoptObservedTerminalActivity(
			{ activity_id: "toolu_03", command: "npm test", state: "started" },
			context,
		);
		const completed = AdoptObservedTerminalActivity(
			{ activity_id: "toolu_03", command: "npm test", exit_code: 0, state: "completed" },
			context,
		);

		expect(started?.terminal_id).toBe(ObservedTerminalId("toolu_03"));
		expect(completed?.terminal_id).toBe(started?.terminal_id);
		/** Exiting is what removes the row: the card shows only live terminals. */
		expect(completed?.state).toBe("closed");
		expect(completed?.exit_code).toBe(0);
		expect(completed?.exit_reason).toBe("exited");
	});

	it("carries a failure through as a failed session", () => {
		const session = AdoptObservedTerminalActivity(
			{ activity_id: "toolu_04", command: "npm test", exit_code: 1, state: "failed" },
			context,
		);

		expect(session?.state).toBe("failed");
		expect(session?.exit_code).toBe(1);
	});

	it("adopts nothing from an output frame or a command with no text", () => {
		/** Output belongs to the session its start frame already opened. */
		expect(
			AdoptObservedTerminalActivity(
				{ activity_id: "toolu_05", command: "npm test", state: "output" },
				context,
			),
		).toBeUndefined();
		/** A row the card would render as an empty command line is worse than none. */
		expect(
			AdoptObservedTerminalActivity(
				{ activity_id: "toolu_06", command: "   ", state: "started" },
				context,
			),
		).toBeUndefined();
		expect(
			AdoptObservedTerminalActivity({ activity_id: "toolu_07", state: "started" }, context),
		).toBeUndefined();
	});

	it("splits a command into the shell that ran it and the command it ran", () => {
		expect(SplitObservedCommand("  git   status --short ", "bash")).toEqual({
			args: ["git", "status", "--short"],
			executable: "bash",
		});
	});

	/**
	 * Codex names no shell out of band, but hands over the whole invocation, so
	 * the interpreter is already the leading program and only needs reading.
	 * The script flags belong to the invocation rather than to what was run — a
	 * row reading `-Command 'git status'` buries the part worth seeing.
	 */
	it("reads the shell off a full invocation and keeps only what it ran", () => {
		/** Verbatim from a recorded Codex run; raw so the path keeps its separators. */
		expect(
			SplitObservedCommand(
				String.raw`"C:\Users\sander\AppData\Local\Microsoft\WindowsApps\pwsh.exe" -Command 'git status --short --branch'`,
			),
		).toEqual({ args: ["git status --short --branch"], executable: "pwsh" });

		expect(SplitObservedCommand("/bin/bash -lc 'npm run dev'")).toEqual({
			args: ["npm run dev"],
			executable: "bash",
		});
	});

	/**
	 * Codex sends Windows paths with their separators already doubled, so the
	 * text that arrives really does contain two backslashes between segments.
	 * Reading the program off it must survive that rather than assume the path
	 * a person would type.
	 */
	it("reads the shell off a path whose separators arrive doubled", () => {
		expect(
			SplitObservedCommand(
				String.raw`"C:\\Users\\sander\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe" -NoProfile -Command "git status"`,
			),
		).toEqual({ args: ["git status"], executable: "pwsh" });
	});

	/**
	 * A command whose first word is a program rather than a shell must not be
	 * mistaken for one, and nothing is inferred from the command's syntax:
	 * almost everything an agent runs reads identically in every shell.
	 */
	it("marks the shell unknown rather than guessing one", () => {
		expect(SplitObservedCommand("git status")).toEqual({
			args: ["git", "status"],
			executable: unknown_observed_shell,
		});
		expect(SplitObservedCommand("npm run dev", "   ")).toEqual({
			args: ["npm", "run", "dev"],
			executable: unknown_observed_shell,
		});
	});
});
