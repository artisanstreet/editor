import { Schema } from "effect";

import strip_ansi from "strip-ansi";

/**
 * View state for a checklist run. Pure: every transition arrives as a
 * serializable event so the same reducer drives the in-process plain sink and
 * the out-of-process terminal renderer.
 */

export const StepStatus = Schema.Literals([
	"cancelled",
	"failed",
	"passed",
	"pending",
	"running",
	"skipped",
]);
export type StepStatus = typeof StepStatus.Type;

export const terminal_statuses: ReadonlySet<StepStatus> = new Set<StepStatus>([
	"cancelled",
	"failed",
	"passed",
	"skipped",
]);

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

const plain_style: LogStyle = {};

/**
 * The 16 ANSI colors mapped onto the checklist's own palette so tool output
 * blends with the frame instead of pulling in the host terminal's scheme.
 * Black maps to slate because true black is invisible on the background.
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
				/**
				 * An unrecognized extended-color mode invalidates this selector
				 * only; the codes after it are ordinary SGR parameters that still
				 * deserve to apply.
				 */
				index += 1;
			}
		}
	}

	return next;
};

const escape_character = String.fromCodePoint(0x1b);
const bell_character = String.fromCodePoint(0x07);

/**
 * Consumes SGR sequences (captured), every other CSI/OSC/Fe escape (dropped),
 * and carriage returns, leaving only printable text between matches. Assembled
 * from code points so the control characters never sit literally in the source.
 */
const terminal_sequence_pattern = new RegExp(
	`${escape_character}(?:\\[([\\d;]*)m` +
		`|\\[[0-?]*[ -/]*[@-~]` +
		`|\\][^${bell_character}${escape_character}]*(?:${bell_character}|${escape_character}\\\\)?` +
		`|[@-_])` +
		`|\\r`,
	"gu",
);

export interface ParsedLogLine {
	readonly line: LogLine;
	readonly next_style: LogStyle;
}

/**
 * Splits one raw output line into styled chunks. `initial_style` carries an
 * unclosed style over from the previous line of the same step: colorizers wrap
 * whole multi-line strings in a single open/reset pair, so the opening sequence
 * of a block often arrives lines before its reset.
 */
export const parse_log_line = (
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

/** Removes terminal formatting that must not be retained in checklist state. */
export const sanitize_log_line = (line: string): string => strip_ansi(line).replaceAll("\r", "");

export const NodeDefinition = Schema.Struct({
	depth: Schema.Number,
	id: Schema.NonEmptyString,
	is_group: Schema.Boolean,
	name: Schema.String,
	optional: Schema.Boolean,
	parent_id: Schema.NullOr(Schema.String),
});
export type NodeDefinition = typeof NodeDefinition.Type;

export const ChecklistEvent = Schema.Union([
	Schema.Struct({
		max_log_lines: Schema.Number,
		nodes: Schema.Array(NodeDefinition),
		started_at: Schema.Number,
		subtitle: Schema.NullOr(Schema.String),
		title: Schema.String,
		type: Schema.Literal("configure"),
	}),
	/** A dynamic group resolved its children; they are spliced in beneath it. */
	Schema.Struct({
		node_id: Schema.NonEmptyString,
		nodes: Schema.Array(NodeDefinition),
		type: Schema.Literal("expand"),
	}),
	Schema.Struct({
		at: Schema.Number,
		node_id: Schema.NonEmptyString,
		status: StepStatus,
		type: Schema.Literal("status"),
	}),
	Schema.Struct({
		detail: Schema.String,
		node_id: Schema.NonEmptyString,
		type: Schema.Literal("detail"),
	}),
	Schema.Struct({
		done: Schema.Number,
		node_id: Schema.NonEmptyString,
		total: Schema.NullOr(Schema.Number),
		type: Schema.Literal("progress"),
	}),
	Schema.Struct({
		line: Schema.String,
		node_id: Schema.NonEmptyString,
		type: Schema.Literal("log"),
	}),
	Schema.Struct({
		node_id: Schema.NonEmptyString,
		reason: Schema.String,
		type: Schema.Literal("failure"),
	}),
	Schema.Struct({
		at: Schema.Number,
		outcome: Schema.Literals(["failed", "passed"]),
		type: Schema.Literal("finish"),
	}),
	Schema.Struct({ type: Schema.Literal("shutdown") }),
]).pipe(Schema.toTaggedUnion("type"));
export type ChecklistEvent = typeof ChecklistEvent.Type;

export const is_checklist_event = Schema.is(ChecklistEvent);

export interface StepProgress {
	readonly done: number;
	readonly total: number | undefined;
}

export interface ChecklistNode {
	readonly depth: number;
	readonly detail: string | undefined;
	readonly ended_at: number | undefined;
	readonly failure: string | undefined;
	readonly id: string;
	readonly is_group: boolean;
	readonly log_carry_style: LogStyle;
	readonly log_lines: ReadonlyArray<LogLine>;
	readonly name: string;
	readonly optional: boolean;
	readonly parent_id: string | undefined;
	readonly progress: StepProgress | undefined;
	readonly started_at: number | undefined;
	readonly status: StepStatus;
}

export interface ChecklistState {
	readonly finished_at: number | undefined;
	readonly max_log_lines: number;
	readonly nodes: ReadonlyArray<ChecklistNode>;
	readonly outcome: "failed" | "passed" | undefined;
	readonly selected_id: string | undefined;
	readonly started_at: number | undefined;
	readonly subtitle: string | undefined;
	readonly title: string;
}

const to_node = (definition: NodeDefinition): ChecklistNode => ({
	depth: definition.depth,
	detail: undefined,
	ended_at: undefined,
	failure: undefined,
	id: definition.id,
	is_group: definition.is_group,
	log_carry_style: plain_style,
	log_lines: [],
	name: definition.name,
	optional: definition.optional,
	parent_id: definition.parent_id ?? undefined,
	progress: undefined,
	started_at: undefined,
	status: "pending",
});

export const create_checklist_state = (title = "Checklist"): ChecklistState => ({
	finished_at: undefined,
	max_log_lines: 500,
	nodes: [],
	outcome: undefined,
	selected_id: undefined,
	started_at: undefined,
	subtitle: undefined,
	title,
});

const replace_node = (
	state: ChecklistState,
	node_id: string,
	update: (node: ChecklistNode) => ChecklistNode,
): ChecklistState => {
	if (!state.nodes.some((node) => node.id === node_id)) return state;

	return {
		...state,
		nodes: state.nodes.map((node) => (node.id === node_id ? update(node) : node)),
	};
};

/**
 * The renderer follows the deepest running task so a long run reads as a moving
 * cursor rather than a wall. Groups never take the selection: their own output
 * is empty by construction.
 */
const follow_running = (state: ChecklistState): ChecklistState => {
	const running = state.nodes.find((node) => node.status === "running" && !node.is_group);

	if (running === undefined) return state;

	return { ...state, selected_id: running.id };
};

export const apply_checklist_event = (
	state: ChecklistState,
	event: ChecklistEvent,
): ChecklistState => {
	if (event.type === "shutdown") return state;

	if (event.type === "configure") {
		return {
			...state,
			max_log_lines: Math.max(1, Math.floor(event.max_log_lines)),
			nodes: event.nodes.map(to_node),
			started_at: event.started_at,
			subtitle: event.subtitle ?? undefined,
			title: event.title,
		};
	}

	if (event.type === "expand") {
		const parent_index = state.nodes.findIndex((node) => node.id === event.node_id);

		if (parent_index === -1) return state;

		return {
			...state,
			nodes: [
				...state.nodes.slice(0, parent_index + 1),
				...event.nodes.map(to_node),
				...state.nodes.slice(parent_index + 1),
			],
		};
	}

	if (event.type === "status") {
		return follow_running(
			replace_node(state, event.node_id, (node) => ({
				...node,
				ended_at: terminal_statuses.has(event.status) ? event.at : node.ended_at,
				started_at: event.status === "running" ? event.at : node.started_at,
				status: event.status,
			})),
		);
	}

	if (event.type === "detail") {
		return replace_node(state, event.node_id, (node) => ({ ...node, detail: event.detail }));
	}

	if (event.type === "progress") {
		return replace_node(state, event.node_id, (node) => ({
			...node,
			progress: { done: event.done, total: event.total ?? undefined },
		}));
	}

	if (event.type === "failure") {
		return replace_node(state, event.node_id, (node) => ({ ...node, failure: event.reason }));
	}

	if (event.type === "finish") {
		return {
			...state,
			finished_at: event.at,
			/** Anything still open when the run ends never got its turn. */
			nodes: state.nodes.map((node) =>
				node.status === "pending" || node.status === "running"
					? { ...node, ended_at: node.ended_at ?? event.at, status: "cancelled" }
					: node,
			),
			outcome: event.outcome,
			selected_id:
				state.nodes.find((node) => node.status === "failed")?.id ?? state.selected_id,
		};
	}

	return replace_node(state, event.node_id, (node) => {
		const parsed = parse_log_line(event.line, node.log_carry_style);

		return {
			...node,
			log_carry_style: parsed.next_style,
			log_lines: [...node.log_lines, parsed.line].slice(-state.max_log_lines),
		};
	});
};

export const select_checklist_node = (state: ChecklistState, node_id: string): ChecklistState =>
	state.nodes.some((node) => node.id === node_id) ? { ...state, selected_id: node_id } : state;

/** Group rows carry no output of their own, so selection skips them. */
export const select_relative_checklist_node = (
	state: ChecklistState,
	delta: number,
): ChecklistState => {
	const selectable = state.nodes.filter((node) => !node.is_group);

	if (selectable.length === 0) return state;

	const current = selectable.findIndex((node) => node.id === state.selected_id);
	const next_index =
		(((current + delta) % selectable.length) + selectable.length) % selectable.length;
	const next = selectable[next_index];

	return next === undefined ? state : { ...state, selected_id: next.id };
};

export interface ChecklistSummaryEntry {
	readonly duration_ms: number | undefined;
	readonly failure: string | undefined;
	readonly name: string;
	readonly optional: boolean;
	readonly path: string;
	readonly status: StepStatus;
}

export interface ChecklistSummary {
	readonly duration_ms: number | undefined;
	readonly outcome: "failed" | "passed" | undefined;
	readonly steps: ReadonlyArray<ChecklistSummaryEntry>;
	readonly title: string;
}

const node_path = (state: ChecklistState, node: ChecklistNode): string => {
	const segments = [node.name];
	let parent_id = node.parent_id;

	while (parent_id !== undefined) {
		const parent = state.nodes.find((candidate) => candidate.id === parent_id);

		if (parent === undefined) break;
		segments.unshift(parent.name);
		parent_id = parent.parent_id;
	}

	return segments.join(" › ");
};

export const summarize_checklist = (state: ChecklistState): ChecklistSummary => ({
	duration_ms:
		state.started_at === undefined || state.finished_at === undefined
			? undefined
			: state.finished_at - state.started_at,
	outcome: state.outcome,
	steps: state.nodes
		.filter((node) => !node.is_group)
		.map((node) => ({
			duration_ms:
				node.started_at === undefined || node.ended_at === undefined
					? undefined
					: node.ended_at - node.started_at,
			failure: node.failure,
			name: node.name,
			optional: node.optional,
			path: node_path(state, node),
			status: node.status,
		})),
	title: state.title,
});

export const format_duration = (milliseconds: number): string => {
	if (milliseconds < 1_000) return `${Math.round(milliseconds)}ms`;
	if (milliseconds < 60_000) return `${(milliseconds / 1_000).toFixed(1)}s`;

	const minutes = Math.floor(milliseconds / 60_000);
	const seconds = Math.round((milliseconds % 60_000) / 1_000);

	return `${minutes}m ${seconds}s`;
};

export const format_progress = (progress: StepProgress): string =>
	progress.total === undefined ? `${progress.done}` : `${progress.done}/${progress.total}`;
