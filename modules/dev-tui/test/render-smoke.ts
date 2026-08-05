import { strict as assert } from "node:assert";

import { parseColor } from "@opentui/core";
import { createTestRenderer } from "@opentui/core/testing";

import { create_dev_tui } from "../src/index";

interface BunTestModule {
	readonly test: (name: string, run_test: () => Promise<void> | void) => void;
}

const bun_test_specifier: string = "bun:test";
const { test } = (await import(bun_test_specifier)) as BunTestModule;

test("renders generic lanes, routes logs, and handles quit", async () => {
	const harness = await createTestRenderer({ height: 30, width: 110 });
	let quit_requests = 0;
	const tui = await create_dev_tui({
		on_quit: () => {
			quit_requests += 1;
		},
		renderer: harness.renderer,
	});

	try {
		tui.dispatch({
			endpoints: [
				{ label: "API", url: "http://127.0.0.1:4848" },
				{ label: "Web", url: "http://127.0.0.1:4849" },
			],
			lanes: [
				{ id: "runner", label: "Overview", status: "ready" },
				{ id: "api", label: "API", status: "running" },
				{ id: "web", label: "Web", status: "running" },
			],
			title: "Example development",
			type: "configure",
		});
		tui.dispatch({
			lane_id: "api",
			line: "\u001B[32mAPI is listening\u001B[0m\r",
			type: "log",
		});
		await harness.flush();

		assert.match(harness.captureCharFrame(), /Overview/u);
		assert.match(harness.captureCharFrame(), /API/u);

		await harness.mockInput.pressArrow("down");
		await harness.flush();

		const selected = harness.captureCharFrame();

		assert.match(selected, /API · running/u);
		assert.match(selected, /API is listening/u);
		assert.doesNotMatch(selected, /\[32m/u);

		const listening_span = harness
			.captureSpans()
			.lines.flatMap((line) => line.spans)
			.find((span) => span.text.includes("API is listening"));

		assert.ok(listening_span, "expected the log line to render as one span");
		assert.ok(
			listening_span.fg.equals(parseColor("#22c55e")),
			`expected ANSI green, got ${listening_span.fg.toString()}`,
		);

		await harness.mockInput.pressKey("q");
		await harness.flush();

		assert.equal(quit_requests, 1);
	} finally {
		tui.destroy();
	}
});
