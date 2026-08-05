import { describe, expect, it } from "vitest";

import {
	apply_dev_tui_event,
	create_dev_tui_state,
	is_dev_tui_event,
	parse_dev_log_line,
	sanitize_dev_log_line,
	select_relative_dev_tui_lane,
} from "@artisan/dev-tui/model";

const lanes = [
	{ id: "runner", label: "Overview", status: "ready" },
	{ id: "database", label: "Database", status: "waiting" },
	{ id: "web", label: "Web", status: "starting" },
] as const;

/** Built from char codes so no raw control bytes live in this source file. */
const esc = String.fromCharCode(27);
const bel = String.fromCharCode(7);

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

		const log_lines = state.lanes.find((lane) => lane.id === "database")?.log_lines;

		expect(log_lines?.map((line) => line.text)).toEqual(["second", "third"]);
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
		expect(database?.log_lines.map((line) => line.text)).toEqual(["listening"]);
	});

	it("strips terminal controls before retaining log lines", () => {
		expect(sanitize_dev_log_line("\u001B[32mReady\u001B[0m\r")).toBe("Ready");
	});

	it("parses SGR sequences into styled chunks", () => {
		const parsed = parse_dev_log_line(`${esc}[32mready${esc}[39m in ${esc}[1m120${esc}[22m ms`);

		expect(parsed.line.text).toBe("ready in 120 ms");
		expect(parsed.line.chunks).toEqual([
			{ style: { foreground: "#22c55e" }, text: "ready" },
			{ style: {}, text: " in " },
			{ style: { bold: true }, text: "120" },
			{ style: {}, text: " ms" },
		]);
	});

	it("resolves 256-color and truecolor sequences", () => {
		expect(parse_dev_log_line(`${esc}[38;5;208mx`).line.chunks).toEqual([
			{ style: { foreground: "#ff8700" }, text: "x" },
		]);
		expect(parse_dev_log_line(`${esc}[38;2;1;2;3mx`).line.chunks).toEqual([
			{ style: { foreground: "#010203" }, text: "x" },
		]);
	});

	it("carries an unclosed style into following lines of the same lane", () => {
		let state = create_dev_tui_state(lanes);

		for (const line of [`${esc}[36mfirst`, `second${esc}[0m plain`]) {
			state = apply_dev_tui_event(state, { lane_id: "database", line, type: "log" });
		}

		const database = state.lanes.find((lane) => lane.id === "database");

		expect(database?.log_lines[0]?.chunks).toEqual([
			{ style: { foreground: "#06b6d4" }, text: "first" },
		]);
		expect(database?.log_lines[1]?.chunks).toEqual([
			{ style: { foreground: "#06b6d4" }, text: "second" },
			{ style: {}, text: " plain" },
		]);
	});

	it("drops non-SGR terminal sequences and carriage returns", () => {
		const parsed = parse_dev_log_line(
			`${esc}]8;;https://example.com${bel}label${esc}]8;;${bel} ${esc}[2Jdone\r`,
		);

		expect(parsed.line.text).toBe("label done");
		expect(parsed.line.chunks).toEqual([{ style: {}, text: "label done" }]);
	});
});
