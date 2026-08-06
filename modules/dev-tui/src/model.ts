import { Schema } from "effect";

import strip_ansi from "strip-ansi";

export const DevLaneId = Schema.NonEmptyString;
export type DevLaneId = typeof DevLaneId.Type;

export const DevLaneStatus = Schema.Literals([
	"failed",
	"ready",
	"running",
	"starting",
	"stopped",
	"waiting",
]);
export type DevLaneStatus = typeof DevLaneStatus.Type;

export const DevEndpoint = Schema.Struct({
	label: Schema.NonEmptyString,
	url: Schema.NonEmptyString,
});
export type DevEndpoint = typeof DevEndpoint.Type;

export const DevLaneDefinition = Schema.Struct({
	id: DevLaneId,
	label: Schema.NonEmptyString,
	status: DevLaneStatus,
});
export type DevLaneDefinition = typeof DevLaneDefinition.Type;

export interface DevLogStyle {
	readonly background?: string;
	readonly bold?: boolean;
	readonly dim?: boolean;
	readonly foreground?: string;
	readonly inverse?: boolean;
	readonly italic?: boolean;
	readonly strikethrough?: boolean;
	readonly underline?: boolean;
}

export interface DevLogChunk {
	readonly style: DevLogStyle;
	readonly text: string;
}

export interface DevLogLine {
	readonly chunks: ReadonlyArray<DevLogChunk>;
	readonly text: string;
}

export interface DevTuiLane extends DevLaneDefinition {
	readonly log_carry_style: DevLogStyle;
	readonly log_lines: ReadonlyArray<DevLogLine>;
}

export interface DevTuiState {
	readonly endpoints: ReadonlyArray<DevEndpoint>;
	readonly lanes: ReadonlyArray<DevTuiLane>;
	readonly max_log_lines: number;
	readonly selected_lane_id: DevLaneId;
	readonly title: string;
}

export const DevTuiEvent = Schema.Union([
	Schema.Struct({
		endpoints: Schema.Array(DevEndpoint),
		lanes: Schema.Array(DevLaneDefinition),
		title: Schema.NonEmptyString,
		type: Schema.Literal("configure"),
	}),
	Schema.Struct({
		lane_id: DevLaneId,
		line: Schema.String,
		type: Schema.Literal("log"),
	}),
	Schema.Struct({
		lane_id: DevLaneId,
		status: DevLaneStatus,
		type: Schema.Literal("status"),
	}),
	Schema.Struct({ type: Schema.Literal("shutdown") }),
]).pipe(Schema.toTaggedUnion("type"));
export type DevTuiEvent = typeof DevTuiEvent.Type;

const default_lane: DevLaneDefinition = {
	id: "runner",
	label: "Overview",
	status: "starting",
};

const plain_style: DevLogStyle = {};

/**
 * The 16 ANSI colors mapped onto the dashboard's own chrome palette (Tailwind
 * 500s, 400s for bright) so child-process output blends with the TUI instead
 * of pulling in the host terminal's scheme. Black maps to slate because true
 * black is invisible on the dashboard background.
 */
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

/** The offset is caller-guaranteed in range; the fallback only satisfies indexing. */
const palette_color = (palette: ReadonlyArray<string>, offset: number): string =>
	palette[offset] ?? "#d8dee9";

const apply_sgr_parameters = (style: DevLogStyle, parameters: string): DevLogStyle => {
	const codes = parameters
		.split(";")
		.map((part) => (part.length === 0 ? 0 : Number.parseInt(part, 10)));
	let next: { -readonly [Key in keyof DevLogStyle]: DevLogStyle[Key] } = { ...style };

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
				/**
				 * An unrecognized extended-color mode invalidates this selector
				 * only; the codes after it are ordinary SGR parameters that still
				 * deserve to apply. Skipping just the mode keeps a malformed color
				 * from cancelling the rest of the line's styling.
				 */
				index += 1;
			}
		}
	}

	return next;
};

/**
 * Consumes SGR sequences (captured), every other CSI/OSC/Fe escape (dropped),
 * and carriage returns, leaving only printable text between matches.
 */
const terminal_sequence_pattern =
	// oxlint-disable-next-line no-control-regex -- terminal escapes are the entire point here
	/\u001B(?:\[([\d;]*)m|\[[0-?]*[ -/]*[@-~]|\][^\u0007\u001B]*(?:\u0007|\u001B\\)?|[@-_])|\r/gu;

export interface ParsedDevLogLine {
	readonly line: DevLogLine;
	readonly next_style: DevLogStyle;
}

/**
 * Splits one raw log line into styled chunks. `initial_style` carries an
 * unclosed style over from the previous line of the same stream: colorizers
 * wrap whole multi-line strings in a single open/reset pair, so the opening
 * sequence of a block often arrives lines before its reset.
 */
export const parse_dev_log_line = (
	line: string,
	initial_style: DevLogStyle = plain_style,
): ParsedDevLogLine => {
	const chunks: DevLogChunk[] = [];
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

export const create_dev_tui_state = (
	lanes: ReadonlyArray<DevLaneDefinition> = [default_lane],
	max_log_lines = 1_000,
): DevTuiState => {
	const configured_lanes = lanes.length === 0 ? [default_lane] : lanes;
	const bounded_log_lines = Number.isFinite(max_log_lines)
		? Math.max(1, Math.floor(max_log_lines))
		: 1_000;

	return {
		endpoints: [],
		lanes: configured_lanes.map((lane) => ({
			...lane,
			log_carry_style: plain_style,
			log_lines: [],
		})),
		max_log_lines: bounded_log_lines,
		selected_lane_id: configured_lanes[0]?.id ?? default_lane.id,
		title: "Development",
	};
};

const replace_lane = (
	state: DevTuiState,
	lane_id: DevLaneId,
	update: (lane: DevTuiLane) => DevTuiLane,
): DevTuiState => {
	const lane_exists = state.lanes.some((lane) => lane.id === lane_id);

	if (!lane_exists) return state;

	return {
		...state,
		lanes: state.lanes.map((lane) => (lane.id === lane_id ? update(lane) : lane)),
	};
};

export const apply_dev_tui_event = (state: DevTuiState, event: DevTuiEvent): DevTuiState => {
	if (event.type === "shutdown") return state;

	if (event.type === "configure") {
		const previous_lanes = new Map(state.lanes.map((lane) => [lane.id, lane]));
		const configured_lanes = event.lanes.length === 0 ? [default_lane] : event.lanes;
		const selected_lane_id = configured_lanes.some((lane) => lane.id === state.selected_lane_id)
			? state.selected_lane_id
			: (configured_lanes[0]?.id ?? default_lane.id);

		return {
			...state,
			endpoints: event.endpoints,
			lanes: configured_lanes.map((lane) => ({
				...lane,
				log_carry_style: previous_lanes.get(lane.id)?.log_carry_style ?? plain_style,
				log_lines: previous_lanes.get(lane.id)?.log_lines ?? [],
			})),
			selected_lane_id,
			title: event.title,
		};
	}

	if (event.type === "status") {
		return replace_lane(state, event.lane_id, (lane) => ({
			...lane,
			status: event.status,
		}));
	}

	return replace_lane(state, event.lane_id, (lane) => {
		const parsed = parse_dev_log_line(event.line, lane.log_carry_style);

		return {
			...lane,
			log_carry_style: parsed.next_style,
			log_lines: [...lane.log_lines, parsed.line].slice(-state.max_log_lines),
		};
	});
};

export const select_dev_tui_lane = (state: DevTuiState, lane_id: DevLaneId): DevTuiState => {
	const lane_exists = state.lanes.some((lane) => lane.id === lane_id);

	if (!lane_exists || lane_id === state.selected_lane_id) return state;

	return { ...state, selected_lane_id: lane_id };
};

export const select_relative_dev_tui_lane = (state: DevTuiState, delta: number): DevTuiState => {
	const current_index = state.lanes.findIndex((lane) => lane.id === state.selected_lane_id);
	const lane_count = state.lanes.length;

	if (lane_count === 0) return state;

	const next_index = (((current_index + delta) % lane_count) + lane_count) % lane_count;
	const next_lane = state.lanes[next_index];

	if (next_lane === undefined) return state;

	return select_dev_tui_lane(state, next_lane.id);
};

/** Removes terminal formatting that must not be retained in dashboard log state. */
export const sanitize_dev_log_line = (line: string): string =>
	strip_ansi(line).replaceAll("\r", "");

export const is_dev_tui_event = Schema.is(DevTuiEvent);
