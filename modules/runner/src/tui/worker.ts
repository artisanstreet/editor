import { createReadStream, createWriteStream } from "node:fs";
import { createInterface } from "node:readline";

import { NodeRuntime } from "@effect/platform-node-shared";
import { createCliRenderer } from "@opentui/core";
import { Effect, Option, Queue, Stream } from "effect";

import type { Configuration } from "../model.ts";
import type { Dashboard } from "../platform.ts";
import { MakeOpenTuiDashboard } from "./dashboard.ts";
import { DecodeDashboardConfiguration, DecodeDashboardEvent } from "./transport.ts";

const event_input = createReadStream("runner-dashboard-events", { autoClose: true, fd: 3 });
const command_output = createWriteStream("runner-dashboard-commands", { autoClose: true, fd: 4 });

const SendCommand = (type: "ready" | "shutdown") =>
	Effect.sync(() => {
		command_output.write(`${JSON.stringify({ type })}\n`);
	});

const ConsumeEvents = (
	control_events: Queue.Queue<ReturnType<typeof DecodeDashboardEvent>>,
	log_events: Queue.Queue<ReturnType<typeof DecodeDashboardEvent>>,
) =>
	Effect.callback<void>((resume) => {
		const lines = createInterface({ input: event_input });
		let settled = false;
		const finish = (effect: Effect.Effect<void>) => {
			if (settled) return;
			settled = true;
			resume(effect);
		};

		lines.on("line", (line) => {
			try {
				const event = DecodeDashboardEvent(line);
				if (event.type === "shutdown") {
					finish(Effect.void);
					return;
				}
				/** Node's readline callback cannot yield an Effect. This is the sole
				 * unsafe bridge; one scoped stream processor below owns all rendering. */
				Queue.offerUnsafe(event.type === "log" ? log_events : control_events, event);
			} catch {
				/** Invalid external dashboard messages never own the worker lifecycle. */
			}
		});
		lines.once("close", () => finish(Effect.void));
		return Effect.sync(() => lines.close());
	});

const TakeNextEvent = (
	control_events: Queue.Queue<ReturnType<typeof DecodeDashboardEvent>>,
	log_events: Queue.Queue<ReturnType<typeof DecodeDashboardEvent>>,
) =>
	Queue.poll(control_events).pipe(
		Effect.flatMap(
			Option.match({
				onNone: () => Effect.raceFirst(Queue.take(control_events), Queue.take(log_events)),
				onSome: Effect.succeed,
			}),
		),
	);

const RenderEvents = (
	dashboard: Dashboard,
	control_events: Queue.Queue<ReturnType<typeof DecodeDashboardEvent>>,
	log_events: Queue.Queue<ReturnType<typeof DecodeDashboardEvent>>,
) =>
	Stream.fromEffectRepeat(TakeNextEvent(control_events, log_events)).pipe(
		Stream.runForEach((event) =>
			event.type === "log"
				? dashboard.Log(event.lane_id, event.line)
				: event.type === "status"
					? dashboard.SetStatus(event.lane_id, event.status)
					: Effect.void,
		),
	);

const Main = Effect.scoped(
	Effect.gen(function* () {
		const configuration = DecodeDashboardConfiguration(process.argv[2] ?? "");
		const renderer = yield* Effect.tryPromise({
			catch: (cause) => (cause instanceof Error ? cause : new Error(String(cause))),
			try: () =>
				createCliRenderer({
					consoleMode: "disabled",
					exitOnCtrlC: false,
					exitSignals: [],
					targetFps: 30,
				}),
		});
		const dashboard_configuration: Configuration = {
			dashboard: "always",
			endpoints: configuration.endpoints,
			lanes: configuration.lanes.map((lane) =>
				lane.status === undefined
					? { id: lane.id, name: lane.name }
					: { id: lane.id, name: lane.name, status: lane.status },
			),
			max_log_lines: configuration.max_log_lines,
			processes: [],
			title: configuration.title,
		};
		const dashboard = yield* MakeOpenTuiDashboard(dashboard_configuration, renderer);
		const control_events = yield* Queue.unbounded<ReturnType<typeof DecodeDashboardEvent>>();
		const log_events = yield* Queue.sliding<ReturnType<typeof DecodeDashboardEvent>>(1_000);
		yield* Effect.addFinalizer(() =>
			Effect.all([Queue.shutdown(control_events), Queue.shutdown(log_events)], {
				discard: true,
			}),
		);
		yield* Effect.forkScoped(RenderEvents(dashboard, control_events, log_events));
		yield* SendCommand("ready");
		yield* Effect.exit(
			Effect.raceFirst(ConsumeEvents(control_events, log_events), dashboard.AwaitQuit),
		);
		yield* SendCommand("shutdown");
	}),
);

Main.pipe(
	Effect.ensuring(
		Effect.sync(() => {
			event_input.destroy();
			command_output.end();
		}),
	),
	NodeRuntime.runMain,
);
