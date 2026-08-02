import { strict as assert } from "node:assert";

import { createTestRenderer } from "@opentui/core/testing";

import { create_dev_tui } from "../src/index";

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
		forge_origin: "http://127.0.0.1:4848",
		lanes: [
			{ id: "runner", label: "Overview", status: "ready" },
			{ id: "forge", label: "Forge", status: "running" },
			{ id: "web", label: "Web", status: "running" },
		],
		title: "Artisan dev",
		type: "configure",
		web_origin: "http://127.0.0.1:4849",
	});
	tui.dispatch({ lane_id: "forge", line: "Forge is listening", type: "log" });
	await harness.flush();

	assert.match(harness.captureCharFrame(), /Overview/u);
	assert.match(harness.captureCharFrame(), /Forge/u);

	await harness.mockInput.pressArrow("down");
	await harness.flush();

	const selected = harness.captureCharFrame();

	assert.match(selected, /Forge · running/u);
	assert.match(selected, /Forge is listening/u);

	await harness.mockInput.pressKey("q");
	await harness.flush();

	assert.equal(quit_requests, 1);
} finally {
	tui.destroy();
}

console.log("OpenTUI renderer smoke test passed.");
