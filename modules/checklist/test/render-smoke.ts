import { strict as assert } from "node:assert";

import type { KeyEvent } from "@opentui/core";
import { createTestRenderer } from "@opentui/core/testing";

import { create_checklist_tui } from "../src/tui.ts";

const escape_character = String.fromCodePoint(0x1b);

interface BunTestModule {
	readonly test: (name: string, run_test: () => Promise<void> | void) => void;
}

const bun_test_specifier: string = "bun:test";
const { test } = (await import(bun_test_specifier)) as BunTestModule;

test("renders the checklist, follows the running step, and handles quit", async () => {
	const harness = await createTestRenderer({ height: 30, width: 110 });
	let quit_requests = 0;
	const tui = await create_checklist_tui({
		on_quit: () => {
			quit_requests += 1;
		},
		renderer: harness.renderer,
	});

	try {
		tui.dispatch({
			max_log_lines: 100,
			nodes: [
				{
					depth: 0,
					id: "1",
					is_group: false,
					name: "native build",
					optional: false,
					parent_id: null,
				},
				{
					depth: 0,
					id: "2",
					is_group: true,
					name: "package",
					optional: false,
					parent_id: null,
				},
				{
					depth: 1,
					id: "2.1",
					is_group: false,
					name: "asar",
					optional: false,
					parent_id: "2",
				},
			],
			started_at: 0,
			subtitle: "windows-x64",
			title: "artisan build",
			type: "configure",
		});

		assert.equal(tui.state().nodes.length, 3);
		assert.equal(tui.state().title, "artisan build");

		tui.dispatch({ at: 10, node_id: "1", status: "running", type: "status" });
		/** The renderer follows the deepest running task without being told to. */
		assert.equal(tui.state().selected_id, "1");
		await harness.flush();
		const panel_header = harness
			.captureCharFrame()
			.split("\n")
			.find((line) => line.includes("Steps"));
		assert.ok(panel_header, "expected the steps panel to render");
		assert.match(panel_header, /Steps.*native build · running/u);

		tui.dispatch({
			line: `${escape_character}[32mcompiling${escape_character}[0m artisan-core`,
			node_id: "1",
			type: "log",
		});
		const logged = tui.state().nodes[0]?.log_lines[0];
		assert.equal(logged?.text, "compiling artisan-core");
		assert.equal(logged?.chunks[0]?.style.foreground, "#22c55e");

		tui.dispatch({ at: 50, node_id: "1", status: "passed", type: "status" });
		assert.equal(tui.state().nodes[0]?.status, "passed");
		assert.equal(tui.state().nodes[0]?.ended_at, 50);

		tui.dispatch({ done: 34, node_id: "2.1", total: 61, type: "progress" });
		assert.deepEqual(tui.state().nodes[2]?.progress, { done: 34, total: 61 });

		tui.dispatch({ at: 90, outcome: "failed", type: "finish" });
		/** Everything still open when the run ends never got its turn. */
		assert.equal(tui.state().nodes[2]?.status, "cancelled");
		assert.equal(tui.state().outcome, "failed");

		/** Quit only reads `name`; the rest of a real KeyEvent is irrelevant here. */
		harness.renderer.keyInput.emit("keypress", { name: "q" } as unknown as KeyEvent);
		assert.equal(quit_requests, 1);
	} finally {
		tui.destroy();
	}
});
