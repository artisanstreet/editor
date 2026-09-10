import { describe, expect, it } from "vitest";

import {
	AppendDashboardLog,
	CreateDashboardState,
	ParseLogLine,
	SanitizeLogLine,
	SelectRelativeDashboardLane,
	SetDashboardStatus,
} from "../src/tui/model.ts";

const lanes = [
	{ id: "runner", name: "Runner", status: "ready" },
	{ id: "database", name: "Database", status: "waiting" },
	{ id: "web", name: "Web", status: "starting" },
] as const;

const esc = String.fromCharCode(27);
const bel = String.fromCharCode(7);

describe("dashboard state", () => {
	it("keeps a bounded log tail per lane", () => {
		let state = CreateDashboardState(lanes, 2, "Example", []);
		for (const line of ["first", "second", "third"])
			state = AppendDashboardLog(state, "database", line);
		expect(
			state.lanes.find((lane) => lane.id === "database")?.log_lines.map((line) => line.text),
		).toEqual(["second", "third"]);
	});

	it("wraps selection and retains logs while updating status", () => {
		const state = CreateDashboardState(lanes, 10, "Example", []);
		const previous = SelectRelativeDashboardLane(state, -1);
		const logged = AppendDashboardLog(previous, "database", "listening");
		const ready = SetDashboardStatus(logged, "database", "ready");
		expect(previous.selected_lane_id).toBe("web");
		expect(ready.lanes.find((lane) => lane.id === "database")).toMatchObject({
			log_lines: [{ text: "listening" }],
			status: "ready",
		});
	});

	it("parses ANSI SGR, truecolor, and carries open styles across lines", () => {
		expect(
			ParseLogLine(`${esc}[32mready${esc}[39m in ${esc}[1m120${esc}[22m ms`).line.chunks,
		).toEqual([
			{ style: { foreground: "#22c55e" }, text: "ready" },
			{ style: {}, text: " in " },
			{ style: { bold: true }, text: "120" },
			{ style: {}, text: " ms" },
		]);
		expect(ParseLogLine(`${esc}[38;2;1;2;3mx`).line.chunks).toEqual([
			{ style: { foreground: "#010203" }, text: "x" },
		]);
		const first = ParseLogLine(`${esc}[36mfirst`);
		expect(ParseLogLine(`second${esc}[0m plain`, first.next_style).line.chunks).toEqual([
			{ style: { foreground: "#06b6d4" }, text: "second" },
			{ style: {}, text: " plain" },
		]);
	});

	it("drops non-SGR controls from retained plain logs", () => {
		const input = `${esc}]8;;https://example.com${bel}label${esc}]8;;${bel} ${esc}[2Jdone\r`;
		expect(ParseLogLine(input).line.text).toBe("label done");
		expect(SanitizeLogLine(`${esc}[32mReady${esc}[0m\r`)).toBe("Ready");
	});
});
