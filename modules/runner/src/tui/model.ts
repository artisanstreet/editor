import type { Endpoint, Lane, LaneStatus } from "../model.ts";

import strip_ansi from "strip-ansi";

export interface LogStyle {
	readonly background?: string;
	readonly bold?: boolean;
	readonly dim?: boolean;
	readonly foreground?: string;
	readonly inverse?: boolean;
	readonly italic?: boolean;
	readonly strikethrough?: boolean;
	readonly underline?: boolean;
}

export interface LogChunk {
	readonly style: LogStyle;
	readonly text: string;
}

export interface LogLine {
	readonly chunks: ReadonlyArray<LogChunk>;
	readonly text: string;
}

export interface DashboardLane {
	readonly id: string;
	readonly log_carry_style: LogStyle;
	readonly log_lines: ReadonlyArray<LogLine>;
	readonly name: string;
	readonly status: LaneStatus;
}

export interface DashboardState {
	readonly endpoints: ReadonlyArray<Endpoint>;
	readonly lanes: ReadonlyArray<DashboardLane>;
	readonly max_log_lines: number;
	readonly selected_lane_id: string;
	readonly title: string;
}

const default_lane: Required<Lane> = {
	id: "runner",
	name: "Runner",
	status: "starting",
};

const plain_style: LogStyle = {};

/** The original runner palette, retained so child output matches the dashboard chrome. */
const base_colors = [
	"#475569",
	"#ef4444",
	"#22c55e",
	"#eab308",
	"#3b82f6",
	"#a855f7",
	"#06b6d4",
	"#d8dee9",
] as const;
const bright_colors = [
	"#64748b",
	"#f87171",
	"#4ade80",
	"#facc15",
	"#60a5fa",
	"#c084fc",
	"#22d3ee",
	"#f8fafc",
] as const;

const channel_hex = (value: number): string =>
	Math.max(0, Math.min(255, Math.round(value)))
		.toString(16)
		.padStart(2, "0");

const rgb_hex = (red: number, green: number, blue: number): string =>
	`#${channel_hex(red)}${channel_hex(green)}${channel_hex(blue)}`;

const xterm_256_color = (index: number): string | undefined => {
	if (!Number.isInteger(index) || index < 0 || index > 255) return undefined;
	if (index < 8) return base_colors[index];
	if (index < 16) return bright_colors[index - 8];
	if (index < 232) {
		const cube = index - 16;
		const level = (component: number): number => (component === 0 ? 0 : 55 + component * 40);
		return rgb_hex(
			level(Math.floor(cube / 36)),
			level(Math.floor(cube / 6) % 6),
			level(cube % 6),
		);
	}
	const gray = 8 + (index - 232) * 10;
	return rgb_hex(gray, gray, gray);
};

const palette_color = (palette: ReadonlyArray<string>, offset: number): string =>
	palette[offset] ?? "#d8dee9";

const apply_sgr_parameters = (style: LogStyle, parameters: string): LogStyle => {
	const codes = parameters
		.split(";")
		.map((part) => (part.length === 0 ? 0 : Number.parseInt(part, 10)));
	let next: { -readonly [Key in keyof LogStyle]: LogStyle[Key] } = { ...style };

	for (let index = 0; index < codes.length; index += 1) {
		const code = codes[index] ?? 0;

		if (code === 0) next = {};
		else if (code === 1) next.bold = true;
		else if (code === 2) next.dim = true;
		else if (code === 3) next.italic = true;
		else if (code === 4) next.underline = true;
		else if (code === 7) next.inverse = true;
		else if (code === 9) next.strikethrough = true;
		else if (code === 22) {
			delete next.bold;
			delete next.dim;
		} else if (code === 23) delete next.italic;
		else if (code === 24) delete next.underline;
		else if (code === 27) delete next.inverse;
		else if (code === 29) delete next.strikethrough;
		else if (code >= 30 && code <= 37) next.foreground = palette_color(base_colors, code - 30);
		else if (code === 39) delete next.foreground;
		else if (code >= 40 && code <= 47) next.background = palette_color(base_colors, code - 40);
		else if (code === 49) delete next.background;
		else if (code >= 90 && code <= 97)
			next.foreground = palette_color(bright_colors, code - 90);
		else if (code >= 100 && code <= 107)
			next.background = palette_color(bright_colors, code - 100);
		else if (code === 38 || code === 48) {
			const target = code === 38 ? "foreground" : "background";
			const mode = codes[index + 1];

			if (mode === 5) {
				const color = xterm_256_color(codes[index + 2] ?? -1);
				if (color !== undefined) next[target] = color;
				index += 2;
			} else if (mode === 2) {
				next[target] = rgb_hex(
					codes[index + 2] ?? 0,
					codes[index + 3] ?? 0,
					codes[index + 4] ?? 0,
				);
				index += 4;
			} else {
				index += 1;
			}
		}
	}

	return next;
};

/** Consumes terminal controls while retaining SGR styling for OpenTUI. */
const terminal_sequence_pattern =
	// oxlint-disable-next-line no-control-regex -- terminal escapes are the entire point here
	/\u001B(?:\[([\d;]*)m|\[[0-?]*[ -/]*[@-~]|\][^\u0007\u001B]*(?:\u0007|\u001B\\)?|[@-_])|\r/gu;

export interface ParsedLogLine {
	readonly line: LogLine;
	readonly next_style: LogStyle;
}

export const ParseLogLine = (
	line: string,
	initial_style: LogStyle = plain_style,
): ParsedLogLine => {
	const chunks: LogChunk[] = [];
	let text = "";
	let style = initial_style;
	let last_index = 0;

	const push_segment = (segment: string) => {
		if (segment.length === 0) return;
		text += segment;
		const previous = chunks.at(-1);
		if (previous !== undefined && previous.style === style) {
			chunks[chunks.length - 1] = { style, text: previous.text + segment };
		} else {
			chunks.push({ style, text: segment });
		}
	};

	for (const match of line.matchAll(terminal_sequence_pattern)) {
		push_segment(line.slice(last_index, match.index));
		if (match[1] !== undefined) style = apply_sgr_parameters(style, match[1]);
		last_index = match.index + match[0].length;
	}
	push_segment(line.slice(last_index));

	return { line: { chunks, text }, next_style: style };
};

export const CreateDashboardState = (
	lanes: ReadonlyArray<Lane>,
	max_log_lines: number,
	title: string,
	endpoints: ReadonlyArray<Endpoint>,
): DashboardState => {
	const configured_lanes = lanes.length === 0 ? [default_lane] : lanes;
	const bounded_log_lines = Number.isFinite(max_log_lines)
		? Math.max(1, Math.floor(max_log_lines))
		: 1_000;
	return {
		endpoints,
		lanes: configured_lanes.map((lane) => ({
			id: lane.id,
			log_carry_style: plain_style,
			log_lines: [],
			name: lane.name,
			status: lane.status ?? "waiting",
		})),
		max_log_lines: bounded_log_lines,
		selected_lane_id: configured_lanes[0]?.id ?? default_lane.id,
		title,
	};
};

const replace_lane = (
	state: DashboardState,
	lane_id: string,
	update: (lane: DashboardLane) => DashboardLane,
): DashboardState => {
	if (!state.lanes.some((lane) => lane.id === lane_id)) return state;
	return {
		...state,
		lanes: state.lanes.map((lane) => (lane.id === lane_id ? update(lane) : lane)),
	};
};

export const AppendDashboardLog = (
	state: DashboardState,
	lane_id: string,
	line: string,
): DashboardState =>
	replace_lane(state, lane_id, (lane) => {
		const parsed = ParseLogLine(line, lane.log_carry_style);
		return {
			...lane,
			log_carry_style: parsed.next_style,
			log_lines: [...lane.log_lines, parsed.line].slice(-state.max_log_lines),
		};
	});

export const SetDashboardStatus = (
	state: DashboardState,
	lane_id: string,
	status: LaneStatus,
): DashboardState => replace_lane(state, lane_id, (lane) => ({ ...lane, status }));

export const SelectDashboardLane = (state: DashboardState, lane_id: string): DashboardState => {
	if (!state.lanes.some((lane) => lane.id === lane_id) || lane_id === state.selected_lane_id)
		return state;
	return { ...state, selected_lane_id: lane_id };
};

export const SelectRelativeDashboardLane = (
	state: DashboardState,
	delta: number,
): DashboardState => {
	const current_index = state.lanes.findIndex((lane) => lane.id === state.selected_lane_id);
	const lane_count = state.lanes.length;
	if (lane_count === 0) return state;
	const next_index = (((current_index + delta) % lane_count) + lane_count) % lane_count;
	const next_lane = state.lanes[next_index];
	return next_lane === undefined ? state : SelectDashboardLane(state, next_lane.id);
};

export const SanitizeLogLine = (line: string): string => strip_ansi(line).replaceAll("\r", "");
