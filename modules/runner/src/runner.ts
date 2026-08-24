import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, Stream, type Scope } from "effect";
import { FetchHttpClient, HttpClient } from "effect/unstable/http";
import { ChildProcessSpawner } from "effect/unstable/process";
import strip_ansi from "strip-ansi";

import { NormalizeConfiguration } from "./configuration.ts";
import {
	DashboardError,
	ProcessError,
	ProcessExitedError,
	ReadinessError,
	type RunnerError,
} from "./error.ts";
import type {
	Configuration,
	LaneStatus,
	NormalizedProcess,
	Options,
	Process,
	Readiness as ReadinessModel,
	RoutedOutput,
} from "./model.ts";
import { DashboardFactory, type Dashboard } from "./platform.ts";
import {
	AwaitHttpReadiness,
	AwaitOutputReadiness,
	DecodeOutput,
	MatchesOutputReadiness,
	type OutputLine,
} from "./readiness.ts";
import { NodeDashboardLive } from "./tui/dashboard.ts";

export { ConfigurationError, ProcessError, ProcessExitedError, ReadinessError } from "./error.ts";
export type Error = RunnerError;
export type {
	DashboardMode,
	Endpoint,
	Lane,
	LaneStatus,
	Options,
	OutputRouter,
	Process,
	ProcessOutput,
	RoutedOutput,
} from "./model.ts";
export type Readiness = ReadinessModel;

type RunnerEnvironment =
	| ChildProcessSpawner.ChildProcessSpawner
	| DashboardFactory
	| HttpClient.HttpClient;

const PlainDashboard: Dashboard = {
	AwaitQuit: Effect.never,
	Log: (lane_id, line) =>
		Effect.sync(() => console.log(`[${lane_id}] ${strip_ansi(line).replaceAll("\r", "")}`)),
	SetStatus: (lane_id, status) => Effect.sync(() => console.log(`[${lane_id}] ${status}`)),
};

const SetProcessStatus = (
	dashboard: Dashboard,
	process: NormalizedProcess,
	status: LaneStatus,
): Effect.Effect<void> =>
	Effect.forEach(process.lane_ids, (lane_id) => dashboard.SetStatus(lane_id, status), {
		discard: true,
	});

const RouteOutput = (
	dashboard: Dashboard,
	process: NormalizedProcess,
	output: OutputLine,
): Effect.Effect<void> =>
	Effect.gen(function* () {
		const routed: ReadonlyArray<RoutedOutput> = yield* process.route_output === undefined
			? Effect.succeed(process.lane_ids.map((lane_id) => ({ lane_id, line: output.line })))
			: process.route_output({
					line: output.line,
					process_id: process.id,
					stream: output.stream,
				});
		for (const event of routed) {
			yield* dashboard.Log(event.lane_id, event.line);
			if (event.status !== undefined) yield* dashboard.SetStatus(event.lane_id, event.status);
		}
	});

const ObserveOutput = (
	dashboard: Dashboard,
	process: NormalizedProcess,
	ready: Deferred.Deferred<void, ReadinessError>,
	output: Stream.Stream<OutputLine, ProcessError>,
) =>
	output.pipe(
		Stream.runForEach((line) =>
			RouteOutput(dashboard, process, line).pipe(
				Effect.andThen(
					process.readiness._tag === "Output" &&
						MatchesOutputReadiness(process.readiness, line)
						? Deferred.succeed(ready, undefined).pipe(Effect.asVoid)
						: Effect.void,
				),
			),
		),
	);

const AwaitReadiness = (
	process: NormalizedProcess,
	ready: Deferred.Deferred<void, ReadinessError>,
): Effect.Effect<void, RunnerError, HttpClient.HttpClient> => {
	const readiness: ReadinessModel = process.readiness;
	if (readiness._tag === "Manual") return Effect.never;
	if (readiness._tag === "Immediate") return Effect.void;
	if (readiness._tag === "Output") return AwaitOutputReadiness(process.id, readiness, ready);
	return AwaitHttpReadiness(process.id, readiness);
};

const RunProcess = (
	dashboard: Dashboard,
	process: NormalizedProcess,
): Effect.Effect<
	never,
	RunnerError,
	ChildProcessSpawner.ChildProcessSpawner | HttpClient.HttpClient | Scope.Scope
> =>
	Effect.gen(function* () {
		yield* SetProcessStatus(dashboard, process, "starting");
		const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
		const handle = yield* spawner
			.spawn(process.command)
			.pipe(
				Effect.mapError(
					(cause) =>
						new ProcessError({ cause, operation: "spawn", process_id: process.id }),
				),
			);
		yield* SetProcessStatus(dashboard, process, "running");

		const ready = yield* Deferred.make<void, ReadinessError>();
		if (process.readiness._tag === "Immediate") yield* Deferred.succeed(ready, undefined);

		const stdout = ObserveOutput(
			dashboard,
			process,
			ready,
			DecodeOutput("stdout", handle.stdout).pipe(
				Stream.mapError(
					(cause) =>
						new ProcessError({ cause, operation: "output", process_id: process.id }),
				),
			),
		);
		const stderr = ObserveOutput(
			dashboard,
			process,
			ready,
			DecodeOutput("stderr", handle.stderr).pipe(
				Stream.mapError(
					(cause) =>
						new ProcessError({ cause, operation: "output", process_id: process.id }),
				),
			),
		);
		const readiness = AwaitReadiness(process, ready).pipe(
			Effect.andThen(SetProcessStatus(dashboard, process, "ready")),
		);
		const terminated = Effect.all([stdout, stderr, handle.exitCode], {
			concurrency: "unbounded",
		}).pipe(
			Effect.flatMap(([, , exit_code]) =>
				SetProcessStatus(dashboard, process, "failed").pipe(
					Effect.andThen(
						Effect.fail(new ProcessExitedError({ exit_code, process_id: process.id })),
					),
				),
			),
			Effect.mapError((cause) =>
				cause instanceof ProcessExitedError || cause instanceof ProcessError
					? cause
					: new ProcessError({ cause, operation: "output", process_id: process.id }),
			),
		);

		yield* Effect.all([readiness, terminated], {
			concurrency: "unbounded",
			discard: true,
		});
		return yield* Effect.never;
	});

const MakeDashboard = (
	configuration: Configuration,
): Effect.Effect<
	Dashboard,
	never,
	DashboardFactory | ChildProcessSpawner.ChildProcessSpawner | Scope.Scope
> =>
	Effect.gen(function* () {
		if (configuration.dashboard === "never") return PlainDashboard;
		const factory = yield* DashboardFactory;
		return yield* factory
			.Make(configuration)
			.pipe(
				Effect.catchTag("DashboardError", (error: DashboardError) =>
					Effect.sync(() =>
						console.warn(`[runner] dashboard unavailable: ${String(error.cause)}`),
					).pipe(Effect.as(PlainDashboard)),
				),
			);
	});

/** Injectable production seam. Layer launch keeps the process scope alive until failure or interruption. */
export const RunnerLive = (
	processes: ReadonlyArray<Process>,
	options?: Options,
): Layer.Layer<never, RunnerError, RunnerEnvironment> =>
	Layer.effectDiscard(
		NormalizeConfiguration(processes, options).pipe(
			Effect.flatMap((configuration) =>
				Effect.gen(function* () {
					const dashboard = yield* MakeDashboard(configuration);
					yield* Effect.raceFirst(
						Effect.all(
							configuration.processes.map((process) =>
								RunProcess(dashboard, process),
							),
							{ concurrency: "unbounded", discard: true },
						),
						dashboard.AwaitQuit,
					);
				}),
			),
		),
	);

const NodeRunnerPlatformLive = Layer.mergeAll(
	FetchHttpClient.layer,
	NodeDashboardLive,
	NodeChildProcessSpawner.layer.pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
	),
);

/** Batteries-included Node program; only NodeRuntime.runMain interprets the returned Effect. */
export const make = (
	processes: ReadonlyArray<Process>,
	options?: Options,
): Effect.Effect<never, RunnerError> =>
	RunnerLive(processes, options).pipe(Layer.provide(NodeRunnerPlatformLive), Layer.launch);

export const Readiness = {
	manual: (): Extract<ReadinessModel, { _tag: "Manual" }> => ({ _tag: "Manual" }),
	http: (
		url: string,
		options: Partial<Omit<Extract<ReadinessModel, { _tag: "Http" }>, "_tag" | "url">> = {},
	) => ({
		_tag: "Http" as const,
		interval: options.interval ?? "250 millis",
		timeout: options.timeout ?? "30 seconds",
		url,
	}),
	immediate: (): Extract<ReadinessModel, { _tag: "Immediate" }> => ({ _tag: "Immediate" }),
	output: (
		pattern: RegExp,
		options: Partial<
			Omit<Extract<ReadinessModel, { _tag: "Output" }>, "_tag" | "pattern">
		> = {},
	) => ({
		_tag: "Output" as const,
		pattern,
		stream: options.stream ?? "either",
		timeout: options.timeout ?? "30 seconds",
	}),
};
