import { describe, expect, it } from "vitest";

import {
	apply_dev_tui_event,
	create_dev_tui_state,
	is_dev_tui_event,
	select_relative_dev_tui_lane,
} from "@artisan/dev-tui/model";

const lanes = [
	{ id: "runner", label: "Overview", status: "ready" },
	{ id: "forge", label: "Forge", status: "waiting" },
	{ id: "web", label: "Web", status: "starting" },
] as const;

describe("development dashboard state", () => {
	it("keeps a bounded log tail for each process", () => {
		let state = create_dev_tui_state(lanes, 2);

		for (const line of ["first", "second", "third"]) {
			state = apply_dev_tui_event(state, {
				lane_id: "forge",
				line,
				type: "log",
			});
		}

		expect(state.lanes.find((lane) => lane.id === "forge")?.log_lines).toEqual([
			"second",
			"third",
		]);
	});

	it("wraps process selection in both directions", () => {
		const state = create_dev_tui_state(lanes);
		const previous = select_relative_dev_tui_lane(state, -1);
		const next = select_relative_dev_tui_lane(previous, 1);

		expect(previous.selected_lane_id).toBe("web");
		expect(next.selected_lane_id).toBe("runner");
	});

	it("updates process status without dropping its logs", () => {
		const with_log = apply_dev_tui_event(create_dev_tui_state(lanes), {
			lane_id: "forge",
			line: "listening",
			type: "log",
		});
		const ready = apply_dev_tui_event(with_log, {
			lane_id: "forge",
			status: "ready",
			type: "status",
		});
		const forge = ready.lanes.find((lane) => lane.id === "forge");

		expect(forge?.status).toBe("ready");
		expect(forge?.log_lines).toEqual(["listening"]);
	});

	it("rejects malformed messages from the supervisor pipe", () => {
		expect(is_dev_tui_event({ lane_id: "forge", line: "ok", type: "log" })).toBe(true);
		expect(is_dev_tui_event({ lane_id: "database", line: "no", type: "log" })).toBe(false);
		expect(is_dev_tui_event({ type: "configure" })).toBe(false);
	});
});
