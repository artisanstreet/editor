import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it, vi } from "vitest";

import {
	restore_terminal_presentation,
	terminal_presentation_reset,
} from "../../modules/checklist/src/tui-bridge";
import {
	apply_checklist_event,
	create_checklist_state,
	type ChecklistEvent,
} from "../../modules/checklist/src/model";
import { format_persistent_failure_report } from "../../modules/checklist/src";

describe("checklist TUI terminal cleanup", () => {
	it("disables every mouse protocol and restores ordinary terminal input", () => {
		const set_raw_mode = vi.fn();
		const write = vi.fn();

		restore_terminal_presentation(
			{ isTTY: true, setRawMode: set_raw_mode },
			{ isTTY: true, write },
		);

		expect(set_raw_mode).toHaveBeenCalledWith(false);
		expect(write).toHaveBeenCalledWith(terminal_presentation_reset);
		for (const mode of [1000, 1002, 1003, 1004, 1005, 1006, 1007, 1015, 1016, 1049, 2004]) {
			expect(terminal_presentation_reset).toContain(`\u001b[?${mode}l`);
		}
		expect(terminal_presentation_reset).toContain("\u001b[?25h");
	});

	it("never asks OpenTUI to enable pointer or extended keyboard reporting", () => {
		const source = readFileSync(resolve("modules/checklist/src/tui.ts"), "utf8");
		const bridge = readFileSync(resolve("modules/checklist/src/tui-bridge.ts"), "utf8");

		expect(source).toContain("enableMouseMovement: false");
		expect(source).toContain("useMouse: false");
		expect(source).toContain("autoFocus: false");
		expect(source).toContain("useKittyKeyboard: null");
		expect(bridge).toContain("windowsHide: true");
		expect(bridge).not.toContain("windowsHide: false");
	});

	it("awaits graceful dashboard shutdown and retains an emergency exit reset", () => {
		const source = readFileSync(resolve("modules/checklist/src/tui-bridge.ts"), "utf8");

		expect(source).toContain('process.once("exit", emergency_exit)');
		expect(source).toContain('event_stream.end(`${JSON.stringify({ type: "shutdown" })}\\n`)');
		expect(source).toContain("Effect.promise(async () => await presenter.close())");
		expect(source).toContain("if (child.exitCode === null && !child.killed) child.kill()");
	});

	it("replays the failed command's useful output after the dashboard closes", () => {
		const events: ChecklistEvent[] = [
			{
				max_log_lines: 500,
				nodes: [
					{
						depth: 0,
						id: "desktop",
						is_group: false,
						name: "desktop package",
						optional: false,
						parent_id: null,
					},
				],
				started_at: 0,
				subtitle: null,
				title: "build",
				type: "configure",
			},
			{ at: 1, node_id: "desktop", status: "running", type: "status" },
			{ line: "compiler context", node_id: "desktop", type: "log" },
			{ line: "Unexpected token", node_id: "desktop", type: "log" },
			{ node_id: "desktop", reason: "Process exited with code 1", type: "failure" },
			{ at: 2, node_id: "desktop", status: "failed", type: "status" },
			{ at: 2, outcome: "failed", type: "finish" },
		];
		const state = events.reduce(apply_checklist_event, create_checklist_state());

		expect(format_persistent_failure_report(state)).toEqual([
			"── desktop package failed",
			"compiler context",
			"Unexpected token",
			"Process exited with code 1",
		]);
	});
});
