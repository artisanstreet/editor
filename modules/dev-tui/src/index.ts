import {
	BoxRenderable,
	ScrollBoxRenderable,
	StyledText,
	TextRenderable,
	bold,
	brightBlack,
	brightBlue,
	brightCyan,
	brightGreen,
	brightRed,
	createCliRenderer,
	createTextAttributes,
	dim,
	parseColor,
	type CliRenderer,
	type KeyEvent,
	type StylableInput,
	type TextChunk,
	yellow,
} from "@opentui/core";

import {
	apply_dev_tui_event,
	create_dev_tui_state,
	select_dev_tui_lane,
	select_relative_dev_tui_lane,
	type DevLaneStatus,
	type DevLogChunk,
	type DevLogLine,
	type DevTuiEvent,
	type DevTuiState,
} from "./model";

export type { DevLogChunk, DevLogLine, DevLogStyle, DevTuiLane, DevTuiState } from "./model";
export {
	DevEndpoint,
	DevLaneDefinition,
	DevLaneId,
	DevLaneStatus,
	DevTuiEvent,
	is_dev_tui_event,
	sanitize_dev_log_line,
} from "./model";

export interface DevTui {
	readonly destroy: () => void;
	readonly dispatch: (event: DevTuiEvent) => void;
	readonly state: () => DevTuiState;
}

export interface DevTuiOptions {
	readonly on_quit?: () => void;
	readonly renderer?: CliRenderer;
}

const status_labels: Readonly<Record<DevLaneStatus, string>> = {
	failed: "× failed",
	ready: "✓ ready",
	running: "● running",
	starting: "◌ starting",
	stopped: "■ stopped",
	waiting: "○ waiting",
};

const status_styles: Readonly<Record<DevLaneStatus, (input: StylableInput) => TextChunk>> = {
	failed: brightRed,
	ready: brightGreen,
	running: brightBlue,
	starting: yellow,
	stopped: brightBlack,
	waiting: dim,
};

const status_border_colors: Readonly<Record<DevLaneStatus, string>> = {
	failed: "#ef4444",
	ready: "#22c55e",
	running: "#3b82f6",
	starting: "#eab308",
	stopped: "#475569",
	waiting: "#475569",
};

const format_sidebar = (state: DevTuiState): StyledText => {
	const chunks: TextChunk[] = [];

	state.lanes.forEach((lane, index) => {
		const selected = lane.id === state.selected_lane_id;
		const marker = selected ? "›" : " ";
		const label = `${marker} ${index + 1}  ${lane.label}`;

		chunks.push(selected ? brightCyan(bold(label)) : dim(label));
		chunks.push(status_styles[lane.status](`\n     ${status_labels[lane.status]}`));
		if (index < state.lanes.length - 1) chunks.push(dim("\n\n"));
	});

	return new StyledText(chunks);
};

const format_header = (state: DevTuiState): StyledText => {
	const endpoints = state.endpoints.map((endpoint) => endpoint.label).join("   ");
	const chunks = [brightCyan(bold(state.title))];

	if (endpoints.length > 0) chunks.push(dim(`\n${endpoints}`));

	return new StyledText(chunks);
};

const to_text_chunk = (chunk: DevLogChunk): TextChunk => {
	const text_chunk: TextChunk = { __isChunk: true, text: chunk.text };
	const attributes = createTextAttributes({
		bold: chunk.style.bold ?? false,
		dim: chunk.style.dim ?? false,
		inverse: chunk.style.inverse ?? false,
		italic: chunk.style.italic ?? false,
		strikethrough: chunk.style.strikethrough ?? false,
		underline: chunk.style.underline ?? false,
	});

	if (attributes !== 0) text_chunk.attributes = attributes;
	if (chunk.style.foreground !== undefined) text_chunk.fg = parseColor(chunk.style.foreground);
	if (chunk.style.background !== undefined) text_chunk.bg = parseColor(chunk.style.background);

	return text_chunk;
};

const format_log = (lines: ReadonlyArray<DevLogLine>): StyledText => {
	const chunks: TextChunk[] = [];

	lines.forEach((line, index) => {
		if (index > 0) chunks.push({ __isChunk: true, text: "\n" });
		for (const chunk of line.chunks) chunks.push(to_text_chunk(chunk));
	});

	return new StyledText(chunks);
};

const selected_lane = (state: DevTuiState) =>
	state.lanes.find((lane) => lane.id === state.selected_lane_id) ?? state.lanes[0];

/** Creates the interactive development-process terminal dashboard. */
export const create_dev_tui = async (options: DevTuiOptions = {}): Promise<DevTui> => {
	const renderer =
		options.renderer ??
		(await createCliRenderer({
			autoFocus: false,
			consoleMode: "disabled",
			enableMouseMovement: false,
			exitOnCtrlC: false,
			exitSignals: [],
			targetFps: 30,
			useKittyKeyboard: null,
			useMouse: false,
		}));
	const header = new TextRenderable(renderer, {
		content: new StyledText([brightCyan(bold("Development"))]),
		height: 2,
		id: "header",
	});
	const sidebar_text = new TextRenderable(renderer, {
		content: "",
		flexGrow: 1,
		id: "sidebar-text",
	});
	const sidebar = new BoxRenderable(renderer, {
		border: true,
		borderColor: "#3b4252",
		borderStyle: "rounded",
		flexDirection: "column",
		height: "100%",
		id: "sidebar",
		padding: 1,
		title: " Processes ",
		width: 29,
	});
	const log_text = new TextRenderable(renderer, {
		content: "Waiting for output…",
		fg: "#d8dee9",
		height: "auto",
		id: "log-text",
		minHeight: 1,
		wrapMode: "word",
	});
	const log_scroll = new ScrollBoxRenderable(renderer, {
		flexGrow: 1,
		height: "100%",
		id: "log-scroll",
		scrollY: true,
		stickyScroll: true,
		stickyStart: "bottom",
		viewportCulling: true,
		width: "100%",
	});
	const content = new BoxRenderable(renderer, {
		border: true,
		borderColor: "#3b4252",
		borderStyle: "rounded",
		flexDirection: "column",
		flexGrow: 1,
		height: "100%",
		id: "content",
		padding: 1,
		title: " Logs ",
	});
	const body = new BoxRenderable(renderer, {
		flexDirection: "row",
		flexGrow: 1,
		gap: 1,
		id: "body",
		width: "100%",
	});
	const footer = new TextRenderable(renderer, {
		content: new StyledText([
			dim("↑/↓ or j/k select   number jump   PgUp/PgDn scroll   q quit"),
		]),
		height: 1,
		id: "footer",
	});
	const app = new BoxRenderable(renderer, {
		flexDirection: "column",
		flexGrow: 1,
		gap: 1,
		height: "100%",
		id: "app",
		padding: 1,
		width: "100%",
	});
	let current_state = create_dev_tui_state();
	let destroyed = false;

	sidebar.add(sidebar_text);
	log_scroll.add(log_text);
	content.add(log_scroll);
	body.add(sidebar);
	body.add(content);
	app.add(header);
	app.add(body);
	app.add(footer);
	renderer.root.add(app);

	const render = () => {
		const lane = selected_lane(current_state);
		const lines = lane?.log_lines ?? [];

		header.content = format_header(current_state);
		sidebar_text.content = format_sidebar(current_state);
		content.title = lane === undefined ? " Logs " : ` ${lane.label} · ${lane.status} `;
		content.borderColor = lane === undefined ? "#475569" : status_border_colors[lane.status];
		log_text.content = lines.length === 0 ? "Waiting for output…" : format_log(lines);
		renderer.requestRender();
	};

	const handle_keypress = (key: KeyEvent) => {
		if (key.name === "q" || (key.ctrl && key.name === "c")) {
			options.on_quit?.();
			return;
		}

		if (key.name === "up" || key.name === "k") {
			current_state = select_relative_dev_tui_lane(current_state, -1);
			render();
			return;
		}

		if (key.name === "down" || key.name === "j") {
			current_state = select_relative_dev_tui_lane(current_state, 1);
			render();
			return;
		}

		const shortcut = Number(key.name);
		const lane = Number.isInteger(shortcut) ? current_state.lanes[shortcut - 1] : undefined;

		if (lane !== undefined) {
			current_state = select_dev_tui_lane(current_state, lane.id);
			render();
			return;
		}

		log_scroll.handleKeyPress(key);
	};

	renderer.keyInput.on("keypress", handle_keypress);
	render();

	return {
		destroy: () => {
			if (destroyed) return;
			destroyed = true;
			renderer.keyInput.off("keypress", handle_keypress);
			renderer.destroy();
		},
		dispatch: (event) => {
			if (destroyed || event.type === "shutdown") return;
			current_state = apply_dev_tui_event(current_state, event);
			render();
		},
		state: () => current_state,
	};
};
