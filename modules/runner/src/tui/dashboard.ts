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
	createTextAttributes,
	dim,
	parseColor,
	type CliRenderer,
	type KeyEvent,
	type StylableInput,
	type TextChunk,
	yellow,
} from "@opentui/core";
import { createRequire } from "node:module";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
	Deferred,
	Effect,
	Layer,
	Option,
	Queue,
	Ref,
	Stream,
	type Duration,
	type Scope,
} from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import strip_ansi from "strip-ansi";

import { DashboardError } from "../error.ts";
import type { Configuration, LaneStatus } from "../model.ts";
import { DashboardFactory, type Dashboard } from "../platform.ts";
import {
	AppendDashboardLog,
	CreateDashboardState,
	SelectDashboardLane,
	SelectRelativeDashboardLane,
	SetDashboardStatus,
	type DashboardState,
	type LogChunk,
} from "./model.ts";
import { DecodeDashboardCommand, type DashboardEvent } from "./transport.ts";

const status_labels: Readonly<Record<LaneStatus, string>> = {
	failed: "× failed",
	ready: "✓ ready",
	running: "● running",
	starting: "◌ starting",
	stopped: "■ stopped",
	waiting: "○ waiting",
};

const status_styles: Readonly<Record<LaneStatus, (input: StylableInput) => TextChunk>> = {
	failed: brightRed,
	ready: brightGreen,
	running: brightBlue,
	starting: yellow,
	stopped: brightBlack,
	waiting: dim,
};

const status_border_colors: Readonly<Record<LaneStatus, string>> = {
	failed: "#ef4444",
	ready: "#22c55e",
	running: "#3b82f6",
	starting: "#eab308",
	stopped: "#475569",
	waiting: "#475569",
};

const format_sidebar = (state: DashboardState): StyledText => {
	const chunks: TextChunk[] = [];
	state.lanes.forEach((lane, index) => {
		const selected = lane.id === state.selected_lane_id;
		const label = `${selected ? "›" : " "} ${index + 1}  ${lane.name}`;
		chunks.push(selected ? brightCyan(bold(label)) : dim(label));
		chunks.push(status_styles[lane.status](`\n     ${status_labels[lane.status]}`));
		if (index < state.lanes.length - 1) chunks.push(dim("\n\n"));
	});
	return new StyledText(chunks);
};

const format_header = (state: DashboardState): StyledText => {
	const endpoints = state.endpoints.map((endpoint) => endpoint.label).join("   ");
	const chunks = [brightCyan(bold(state.title))];
	if (endpoints.length > 0) chunks.push(dim(`\n${endpoints}`));
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

const format_log = (state: DashboardState): StyledText | string => {
	const lane = state.lanes.find((candidate) => candidate.id === state.selected_lane_id);
	if (lane === undefined || lane.log_lines.length === 0) return "Waiting for output…";
	const chunks: TextChunk[] = [];
	lane.log_lines.forEach((line, index) => {
		if (index > 0) chunks.push({ __isChunk: true, text: "\n" });
		for (const chunk of line.chunks) chunks.push(to_text_chunk(chunk));
	});
	return new StyledText(chunks);
};

const is_interactive_terminal = (): boolean =>
	process.stdin.isTTY === true &&
	process.stdout.isTTY === true &&
	process.env.CI === undefined &&
	process.env.TERM !== "dumb";

export const ShouldUseNodeDashboard = (configuration: Configuration): boolean =>
	configuration.dashboard === "always" ||
	(configuration.dashboard === "auto" && is_interactive_terminal());

const MakeConsoleDashboard = (): Dashboard => ({
	AwaitQuit: Effect.never,
	Log: (lane_id, line) =>
		Effect.sync(() => console.log(`[${lane_id}] ${strip_ansi(line).replaceAll("\r", "")}`)),
	SetStatus: () => Effect.void,
});

const dependency_require = createRequire(import.meta.url);
const text_encoder = new TextEncoder();
const max_pending_events = 1_000;

export const ResolveBunExecutable = (): string => {
	const package_path = dependency_require.resolve("bun/package.json");
	const manifest = JSON.parse(readFileSync(package_path, "utf8")) as {
		readonly bin: { readonly bun: string };
	};
	return join(dirname(package_path), manifest.bin.bun);
};

export const ResolveDashboardWorker = (): string => {
	const source_worker = fileURLToPath(new URL("./worker.ts", import.meta.url));
	if (existsSync(source_worker)) return source_worker;
	return fileURLToPath(new URL("./tui/worker.mjs", import.meta.url));
};

export const MakeOpenTuiDashboard = (
	configuration: Configuration,
	renderer: CliRenderer,
): Effect.Effect<Dashboard, never, Scope.Scope> =>
	Effect.gen(function* () {
		const quit = yield* Deferred.make<void>();
		const header = new TextRenderable(renderer, {
			content: new StyledText([brightCyan(bold(configuration.title))]),
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
		let state = CreateDashboardState(
			configuration.lanes,
			configuration.max_log_lines,
			configuration.title,
			configuration.endpoints,
		);

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
			const lane = state.lanes.find((candidate) => candidate.id === state.selected_lane_id);
			header.content = format_header(state);
			sidebar_text.content = format_sidebar(state);
			content.title = lane === undefined ? " Logs " : ` ${lane.name} · ${lane.status} `;
			content.borderColor =
				lane === undefined ? "#475569" : status_border_colors[lane.status];
			log_text.content = format_log(state);
			renderer.requestRender();
		};

		const handle_keypress = (key: KeyEvent) => {
			if (key.name === "q" || (key.ctrl && key.name === "c")) {
				/** OpenTUI owns this synchronous callback boundary; the Deferred keeps
				 * the resulting interruption inside the scoped Effect program. */
				Deferred.doneUnsafe(quit, Effect.void);
				return;
			}
			if (key.name === "up" || key.name === "k")
				state = SelectRelativeDashboardLane(state, -1);
			else if (key.name === "down" || key.name === "j")
				state = SelectRelativeDashboardLane(state, 1);
			else {
				const shortcut = Number(key.name);
				const lane = Number.isInteger(shortcut) ? state.lanes[shortcut - 1] : undefined;
				if (lane !== undefined) state = SelectDashboardLane(state, lane.id);
				else {
					log_scroll.handleKeyPress(key);
					return;
				}
			}
			render();
		};

		renderer.keyInput.on("keypress", handle_keypress);
		yield* Effect.addFinalizer(() =>
			Effect.sync(() => {
				renderer.keyInput.off("keypress", handle_keypress);
				renderer.destroy();
			}),
		);
		render();

		return {
			AwaitQuit: Deferred.await(quit).pipe(Effect.andThen(Effect.interrupt)),
			Log: (lane_id, line) =>
				Effect.sync(() => {
					state = AppendDashboardLog(state, lane_id, line);
					render();
				}),
			SetStatus: (lane_id, status) =>
				Effect.sync(() => {
					state = SetDashboardStatus(state, lane_id, status);
					render();
				}),
		};
	});

const ToWorkerConfiguration = (configuration: Configuration) =>
	JSON.stringify({
		endpoints: configuration.endpoints,
		lanes: configuration.lanes,
		max_log_lines: configuration.max_log_lines,
		title: configuration.title,
	});

const HandleCommand = (
	line: string,
	ready: Deferred.Deferred<void>,
	quit: Deferred.Deferred<void>,
	explicit_shutdown: Ref.Ref<boolean>,
): Effect.Effect<void> =>
	Effect.sync(() => {
		try {
			return DecodeDashboardCommand(line);
		} catch {
			return undefined;
		}
	}).pipe(
		Effect.flatMap((command) => {
			if (command === undefined) return Effect.void;
			if (command.type === "ready")
				return Deferred.succeed(ready, undefined).pipe(Effect.asVoid);
			return Ref.set(explicit_shutdown, true).pipe(
				Effect.andThen(Deferred.interrupt(quit)),
				Effect.asVoid,
			);
		}),
	);

export const TakeNextDashboardEvent = (
	control_events: Queue.Queue<DashboardEvent>,
	log_events: Queue.Queue<DashboardEvent>,
): Effect.Effect<DashboardEvent> =>
	Queue.poll(control_events).pipe(
		Effect.flatMap(
			Option.match({
				onNone: () => Effect.raceFirst(Queue.take(control_events), Queue.take(log_events)),
				onSome: Effect.succeed,
			}),
		),
	);

const ConsoleFallback = (event: DashboardEvent): Effect.Effect<void> =>
	Effect.sync(() => {
		if (event.type === "log")
			console.log(`[${event.lane_id}] ${strip_ansi(event.line).replaceAll("\r", "")}`);
		else if (event.type === "status") console.log(`[${event.lane_id}] ${event.status}`);
	});

export const MakeBunDashboard = (
	configuration: Configuration,
	worker: {
		readonly bun_executable: string;
		readonly startup_timeout?: Duration.Input;
		readonly worker_path: string;
	},
): Effect.Effect<
	Dashboard,
	DashboardError,
	ChildProcessSpawner.ChildProcessSpawner | Scope.Scope
> =>
	Effect.gen(function* () {
		const handle = yield* ChildProcess.make(
			worker.bun_executable,
			[worker.worker_path, ToWorkerConfiguration(configuration)],
			{
				additionalFds: { fd3: { type: "input" }, fd4: { type: "output" } },
				stderr: "inherit",
				stdin: "inherit",
				stdout: "inherit",
			},
		).pipe(Effect.mapError((cause) => new DashboardError({ cause, operation: "create" })));
		const ready = yield* Deferred.make<void>();
		const quit = yield* Deferred.make<void>();
		const active = yield* Ref.make(true);
		const explicit_shutdown = yield* Ref.make(false);
		const control_events = yield* Queue.unbounded<DashboardEvent>();
		const log_events = yield* Queue.sliding<DashboardEvent>(max_pending_events);
		yield* Effect.addFinalizer(() =>
			Effect.all([Queue.shutdown(control_events), Queue.shutdown(log_events)], {
				discard: true,
			}),
		);
		yield* Effect.forkScoped(
			Stream.fromEffectRepeat(TakeNextDashboardEvent(control_events, log_events)).pipe(
				Stream.map((event) => text_encoder.encode(`${JSON.stringify(event)}\n`)),
				Stream.run(handle.getInputFd(3)),
				Effect.catch(() => Ref.set(active, false)),
			),
		);
		yield* Effect.forkScoped(
			handle.getOutputFd(4).pipe(
				Stream.decodeText(),
				Stream.splitLines,
				Stream.runForEach((line) => HandleCommand(line, ready, quit, explicit_shutdown)),
				Effect.ensuring(
					Ref.get(explicit_shutdown).pipe(
						Effect.flatMap((requested) =>
							requested ? Effect.void : Ref.set(active, false),
						),
					),
				),
				Effect.catch(() => Ref.set(active, false)),
			),
		);
		yield* Effect.raceFirst(
			Deferred.await(ready),
			handle.exitCode.pipe(
				Effect.flatMap((exit_code) =>
					Effect.fail(
						new DashboardError({
							cause: new Error(`dashboard worker exited before ready (${exit_code})`),
							operation: "create",
						}),
					),
				),
				Effect.mapError((cause) => new DashboardError({ cause, operation: "create" })),
			),
		).pipe(
			Effect.timeoutOrElse({
				duration: worker.startup_timeout ?? "10 seconds",
				orElse: () =>
					Effect.fail(
						new DashboardError({
							cause: new Error("dashboard worker did not become ready"),
							operation: "create",
						}),
					),
			}),
		);
		yield* Effect.forkScoped(
			handle.exitCode.pipe(
				Effect.andThen(Ref.set(active, false)),
				Effect.catch(() => Ref.set(active, false)),
			),
		);
		const OfferEvent = (event: DashboardEvent): Effect.Effect<void> =>
			Ref.get(active).pipe(
				Effect.flatMap((is_active) => {
					if (!is_active) return ConsoleFallback(event);
					return event.type === "log"
						? Queue.offer(log_events, event).pipe(Effect.asVoid)
						: Queue.offer(control_events, event).pipe(Effect.asVoid);
				}),
			);
		return {
			AwaitQuit: Deferred.await(quit).pipe(Effect.andThen(Effect.never)),
			Log: (lane_id, line) => OfferEvent({ lane_id, line, type: "log" }),
			SetStatus: (lane_id, status) => OfferEvent({ lane_id, status, type: "status" }),
		};
	});

export const MakeNodeDashboard = (
	configuration: Configuration,
): Effect.Effect<
	Dashboard,
	DashboardError,
	ChildProcessSpawner.ChildProcessSpawner | Scope.Scope
> => {
	if (!ShouldUseNodeDashboard(configuration)) return Effect.succeed(MakeConsoleDashboard());
	return Effect.try({
		catch: (cause) => new DashboardError({ cause, operation: "create" }),
		try: () => ({
			bun_executable: ResolveBunExecutable(),
			worker_path: ResolveDashboardWorker(),
		}),
	}).pipe(Effect.flatMap((worker) => MakeBunDashboard(configuration, worker)));
};

export const NodeDashboardLive = Layer.succeed(DashboardFactory, { Make: MakeNodeDashboard });
