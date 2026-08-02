import { describe, expect, it } from "vitest";

import {
	apply_dev_tui_event,
	create_dev_tui_state,
	is_dev_tui_event,
	sanitize_dev_log_line,
	select_relative_dev_tui_lane,
} from "@artisan/dev-tui/model";

const lanes = [
	{ id: "runner", label: "Overview", status: "ready" },
	{ id: "database", label: "Database", status: "waiting" },
	{ id: "web", label: "Web", status: "starting" },
] as const;

describe("development dashboard state", () => {
	it("accepts generic lanes and endpoint metadata", () => {
		expect(
			is_dev_tui_event({
				endpoints: [{ label: "Database", url: "postgres://127.0.0.1/example" }],
				lanes,
				title: "Example development",
				type: "configure",
			}),
		).toBe(true);
		expect(is_dev_tui_event({ lane_id: "database", line: "ready", type: "log" })).toBe(true);
		expect(is_dev_tui_event({ lane_id: "", line: "invalid", type: "log" })).toBe(false);
	});

	it("keeps a bounded log tail for each process", () => {
		let state = create_dev_tui_state(lanes, 2);

		for (const line of ["first", "second", "third"]) {
			state = apply_dev_tui_event(state, {
				lane_id: "database",
				line,
				type: "log",
			});
		}

		expect(state.lanes.find((lane) => lane.id === "database")?.log_lines).toEqual([
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
			lane_id: "database",
			line: "listening",
			type: "log",
		});
		const ready = apply_dev_tui_event(with_log, {
			lane_id: "database",
			status: "ready",
			type: "status",
		});
		const database = ready.lanes.find((lane) => lane.id === "database");

		expect(database?.status).toBe("ready");
		expect(database?.log_lines).toEqual(["listening"]);
	});

	it("strips terminal controls before retaining log lines", () => {
		expect(sanitize_dev_log_line("\u001B[32mReady\u001B[0m\r")).toBe("Ready");
	});
});
