import {
	BoxRenderable,
	ScrollBoxRenderable,
	StyledText,
	TextRenderable,
	bold,
	brightBlack,
	brightCyan,
	brightGreen,
	brightRed,
	blue,
	createCliRenderer,
	createTextAttributes,
	dim,
	parseColor,
	type CliRenderer,
	type KeyEvent,
	type StylableInput,
	type TextChunk,
} from "@opentui/core";

import {
	apply_checklist_event,
	create_checklist_state,
	format_duration,
	format_progress,
	select_relative_checklist_node,
	type ChecklistEvent,
	type ChecklistNode,
	type ChecklistState,
	type LogChunk,
	type LogLine,
	type StepStatus,
} from "./model.ts";

/**
 * The dashboard. Output is collapsed to a following tail by default and only
 * expands on request or failure: the checklist is the signal, the output is the
 * detail behind it.
 */

export interface ChecklistTui {
	readonly destroy: () => void;
	readonly dispatch: (event: ChecklistEvent) => void;
	readonly state: () => ChecklistState;
}

export interface ChecklistTuiOptions {
	readonly on_quit?: () => void;
	readonly renderer?: CliRenderer;
}

const spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] as const;

const status_glyphs: Readonly<Record<StepStatus, string>> = {
	cancelled: "·",
	failed: "×",
	passed: "✓",
	pending: "○",
	running: "●",
	skipped: "-",
};

const status_styles: Readonly<Record<StepStatus, (input: StylableInput) => TextChunk>> = {
	cancelled: brightBlack,
	failed: brightRed,
	passed: brightGreen,
	pending: dim,
	running: blue,
	skipped: brightBlack,
};

const status_border_colors: Readonly<Record<StepStatus, string>> = {
	cancelled: "#475569",
	failed: "#ef4444",
	passed: "#22c55e",
	pending: "#475569",
	running: "#3b82f6",
	skipped: "#475569",
};

const name_column = 34;

const pad_to = (text: string, width: number): string =>
	text.length >= width ? `${text.slice(0, width - 1)} ` : text.padEnd(width, " ");

const node_annotation = (node: ChecklistNode, now: number): string => {
	if (node.status === "failed" && node.failure !== undefined) return node.failure;
	if (node.progress !== undefined && node.status === "running") {
		return `▸ ${format_progress(node.progress)}`;
	}
	if (node.detail !== undefined) return node.detail;
	if (node.status === "skipped") return "skipped";
	if (node.status === "cancelled") return "not reached";
	if (node.status === "running" && node.started_at !== undefined) {
		return format_duration(now - node.started_at);
	}
	if (node.started_at !== undefined && node.ended_at !== undefined) {
		return format_duration(node.ended_at - node.started_at);
	}

	return "";
};

const format_rows = (state: ChecklistState, frame: number, now: number): StyledText => {
	const chunks: TextChunk[] = [];

	state.nodes.forEach((node, index) => {
		const selected = node.id === state.selected_id;
		const glyph =
			node.status === "running" && !node.is_group
				? (spinner_frames[frame % spinner_frames.length] ?? "●")
				: status_glyphs[node.status];
		const indent = "  ".repeat(node.depth);
		const marker = selected ? "›" : " ";
		const label = `${marker} ${indent}${glyph}  ${node.name}`;

		chunks.push(status_styles[node.status](`${" "}`));
		chunks.push(
			node.is_group
				? bold(status_styles[node.status](pad_to(label, name_column)))
				: selected
					? brightCyan(bold(pad_to(label, name_column)))
					: status_styles[node.status](pad_to(label, name_column)),
		);

		const annotation = node_annotation(node, now);

		if (annotation.length > 0) {
			chunks.push(node.status === "failed" ? brightRed(annotation) : dim(annotation));
		}

		if (index < state.nodes.length - 1) chunks.push({ __isChunk: true, text: "\n" });
	});

	return new StyledText(chunks);
};

const format_header = (state: ChecklistState, now: number): StyledText => {
	const chunks = [brightCyan(bold(state.title))];

	if (state.subtitle !== undefined) chunks.push(dim(`  ${state.subtitle}`));
	if (state.started_at !== undefined) {
		chunks.push(dim(`  ${format_duration((state.finished_at ?? now) - state.started_at)}`));
	}
	if (state.outcome !== undefined) {
		chunks.push(state.outcome === "passed" ? brightGreen("  PASSED") : brightRed("  FAILED"));
	}

	return new StyledText(chunks);
};

const to_text_chunk = (chunk: LogChunk): TextChunk => {
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

const format_output = (lines: ReadonlyArray<LogLine>): StyledText => {
	const chunks: TextChunk[] = [];

	lines.forEach((line, index) => {
		if (index > 0) chunks.push({ __isChunk: true, text: "\n" });
		for (const chunk of line.chunks) chunks.push(to_text_chunk(chunk));
	});

	return new StyledText(chunks);
};

const selected_node = (state: ChecklistState): ChecklistNode | undefined =>
	state.nodes.find((node) => node.id === state.selected_id);

export const create_checklist_tui = async (
	options: ChecklistTuiOptions = {},
): Promise<ChecklistTui> => {
	const renderer =
		options.renderer ??
		(await createCliRenderer({
			consoleMode: "disabled",
			exitOnCtrlC: false,
			exitSignals: [],
			targetFps: 30,
		}));
	const header = new TextRenderable(renderer, {
		content: new StyledText([brightCyan(bold("Checklist"))]),
		height: 1,
		id: "header",
	});
	const rows_text = new TextRenderable(renderer, {
		content: "",
		height: "auto",
		id: "rows-text",
		minHeight: 1,
	});
	const rows_scroll = new ScrollBoxRenderable(renderer, {
		flexGrow: 1,
		id: "rows-scroll",
		scrollY: true,
		viewportCulling: true,
		width: "100%",
	});
	const steps = new BoxRenderable(renderer, {
		border: true,
		borderColor: "#3b4252",
		borderStyle: "rounded",
		flexDirection: "column",
		flexGrow: 2,
		id: "steps",
		padding: 1,
		title: " Steps ",
	});
	const output_text = new TextRenderable(renderer, {
		content: "Waiting for output…",
		fg: "#d8dee9",
		height: "auto",
		id: "output-text",
		minHeight: 1,
		wrapMode: "word",
	});
	const output_scroll = new ScrollBoxRenderable(renderer, {
		flexGrow: 1,
		height: "100%",
		id: "output-scroll",
		scrollY: true,
		stickyScroll: true,
		stickyStart: "bottom",
		viewportCulling: true,
		width: "100%",
	});
	const output = new BoxRenderable(renderer, {
		border: true,
		borderColor: "#3b4252",
		borderStyle: "rounded",
		flexDirection: "column",
		flexGrow: 3,
		id: "output",
		padding: 1,
		title: " Output ",
	});
	const footer = new TextRenderable(renderer, {
		content: new StyledText([
			dim("↑/↓ or j/k select   o expand output   PgUp/PgDn scroll   q quit"),
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

	let current_state = create_checklist_state();
	let destroyed = false;
	let expanded = false;
	let frame = 0;

	steps.add(rows_scroll);
	rows_scroll.add(rows_text);
	output_scroll.add(output_text);
	output.add(output_scroll);
	app.add(header);
	app.add(steps);
	app.add(output);
	app.add(footer);
	renderer.root.add(app);

	const render = () => {
		const now = Date.now();
		const node = selected_node(current_state);
		const lines = node?.log_lines ?? [];

		header.content = format_header(current_state, now);
		rows_text.content = format_rows(current_state, frame, now);
		output.title = node === undefined ? " Output " : ` ${node.name} · ${node.status} `;
		output.borderColor = node === undefined ? "#475569" : status_border_colors[node.status];
		output_text.content = lines.length === 0 ? "Waiting for output…" : format_output(lines);
		/** Expanding hides the checklist so a long failure gets the whole frame. */
		steps.visible = !expanded;
		renderer.requestRender();
	};

	const handle_keypress = (key: KeyEvent) => {
		if (key.name === "q" || (key.ctrl && key.name === "c")) {
			options.on_quit?.();
			return;
		}

		if (key.name === "o") {
			expanded = !expanded;
			render();
			return;
		}

		if (key.name === "up" || key.name === "k") {
			current_state = select_relative_checklist_node(current_state, -1);
			render();
			return;
		}

		if (key.name === "down" || key.name === "j") {
			current_state = select_relative_checklist_node(current_state, 1);
			render();
			return;
		}

		output_scroll.handleKeyPress(key);
	};

	/** Drives the spinner and the live elapsed clocks; nothing else animates. */
	const ticker = setInterval(() => {
		if (destroyed) return;
		frame += 1;
		render();
	}, 120);

	renderer.keyInput.on("keypress", handle_keypress);
	render();

	return {
		destroy: () => {
			if (destroyed) return;
			destroyed = true;
			clearInterval(ticker);
			renderer.keyInput.off("keypress", handle_keypress);
			renderer.destroy();
		},
		dispatch: (event) => {
			if (destroyed || event.type === "shutdown") return;
			current_state = apply_checklist_event(current_state, event);
			/** A failure is the one thing worth taking over the frame unasked. */
			if (event.type === "status" && event.status === "failed") expanded = true;
			render();
		},
		state: () => current_state,
	};
};
