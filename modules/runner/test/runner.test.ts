import { ChildProcess } from "effect/unstable/process";
import {
	ChildProcessSpawner,
	ExitCode,
	ProcessId,
	make as MakeChildProcessSpawner,
	makeHandle,
} from "effect/unstable/process/ChildProcessSpawner";
import { FetchHttpClient } from "effect/unstable/http";
import { Deferred, Effect, Exit, Fiber, Layer, Ref, Sink, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { Runner } from "../src/index.ts";
import { DashboardFactory, type Dashboard } from "../src/platform.ts";
import { RunnerLive, make } from "../src/runner.ts";

const Encoder = new TextEncoder();

const MakeDashboard = (events: Ref.Ref<ReadonlyArray<string>>): Dashboard => ({
	AwaitQuit: Effect.never,
	Log: (lane_id, line) => Ref.update(events, (current) => [...current, `log:${lane_id}:${line}`]),
	SetStatus: (lane_id, status) =>
		Ref.update(events, (current) => [...current, `status:${lane_id}:${status}`]),
});

describe("RunnerLive", () => {
	it("routes decoded output, fails on child exit, and releases the child scope", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const exit_code = yield* Deferred.make<ReturnType<typeof ExitCode>>();
					const released = yield* Ref.make(false);
					const events = yield* Ref.make<ReadonlyArray<string>>([]);
					const spawner = MakeChildProcessSpawner(() =>
						Effect.gen(function* () {
							yield* Effect.addFinalizer(() => Ref.set(released, true));
							return makeHandle({
								all: Stream.empty,
								exitCode: Deferred.await(exit_code),
								getInputFd: () => Sink.drain,
								getOutputFd: () => Stream.empty,
								isRunning: Effect.succeed(true),
								kill: () => Effect.void,
								pid: ProcessId(1),
								stderr: Stream.empty,
								stdin: Sink.drain,
								stdout: Stream.make(Encoder.encode("ready\n")),
								unref: Effect.succeed(Effect.void),
							});
						}),
					);
					const dependencies = Layer.mergeAll(
						Layer.succeed(ChildProcessSpawner, spawner),
						Layer.succeed(DashboardFactory, {
							Make: () => Effect.succeed(MakeDashboard(events)),
						}),
					);
					const program = Layer.launch(
						RunnerLive(
							[
								{
									command: ChildProcess.make`ignored`,
									id: "forge",
									lane_ids: ["logs", "web"],
									name: "Forge",
									readiness: {
										_tag: "Output",
										pattern: /ready/u,
										stream: "stdout",
										timeout: "1 second",
									},
								},
							],
							{
								dashboard: "auto",
								lanes: [
									{ id: "logs", name: "Logs" },
									{ id: "web", name: "Web" },
								],
							},
						).pipe(Layer.provide(dependencies)),
					);
					const fiber = yield* program.pipe(Effect.forkScoped());
					yield* Effect.sleep("10 millis");
					yield* Deferred.succeed(exit_code, ExitCode(1));
					const exit = yield* Fiber.join(fiber).pipe(Effect.exit);
					return {
						events: yield* Ref.get(events),
						exit,
						released: yield* Ref.get(released),
					};
				}),
			).pipe(Effect.provide(FetchHttpClient.layer)),
		);

		expect(Exit.isFailure(result.exit)).toBe(true);
		expect(result.events).toContain("log:logs:ready");
		expect(result.events).toContain("log:web:ready");
		expect(result.events).toContain("status:logs:ready");
		expect(result.events).toContain("status:web:ready");
		expect(result.released).toBe(true);
	});

	it("has the closed public make type", () => {
		const program: Effect.Effect<never, Runner.Error> = make([
			{ command: ChildProcess.make`ignored`, name: "Forge" },
		]);
		expect(program).toBeDefined();
		expect(Runner.Readiness.manual()).toEqual({ _tag: "Manual" });
	});

	it("leaves multiplexed lanes manual until route_output marks each one ready", async () => {
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const exit_code = yield* Deferred.make<ReturnType<typeof ExitCode>>();
					const dashboard_events = yield* Ref.make<ReadonlyArray<string>>([]);
					const spawner = MakeChildProcessSpawner(() =>
						Effect.succeed(
							makeHandle({
								all: Stream.empty,
								exitCode: Deferred.await(exit_code),
								getInputFd: () => Sink.drain,
								getOutputFd: () => Stream.empty,
								isRunning: Effect.succeed(true),
								kill: () => Effect.void,
								pid: ProcessId(2),
								stderr: Stream.empty,
								stdin: Sink.drain,
								stdout: Stream.make(Encoder.encode("forge ready\n")),
								unref: Effect.succeed(Effect.void),
							}),
						),
					);
					const dependencies = Layer.mergeAll(
						Layer.succeed(ChildProcessSpawner, spawner),
						Layer.succeed(DashboardFactory, {
							Make: () => Effect.succeed(MakeDashboard(dashboard_events)),
						}),
					);
					const fiber = yield* Layer.launch(
						RunnerLive(
							[
								{
									command: ChildProcess.make`ignored`,
									lane_ids: ["forge", "web"],
									name: "VSX",
									readiness: { _tag: "Manual" },
									route_output: (output) =>
										Effect.succeed([
											{
												lane_id: "forge",
												line: output.line,
												status: "ready",
											},
										]),
								},
							],
							{
								dashboard: "auto",
								lanes: [
									{ id: "forge", name: "Forge" },
									{ id: "web", name: "Web" },
								],
							},
						).pipe(Layer.provide(dependencies)),
					).pipe(Effect.forkScoped());

					yield* Effect.sleep("10 millis");
					const before_exit = yield* Ref.get(dashboard_events);
					yield* Deferred.succeed(exit_code, ExitCode(1));
					yield* Fiber.join(fiber).pipe(Effect.exit);
					return before_exit;
				}),
			).pipe(Effect.provide(FetchHttpClient.layer)),
		);

		expect(events).toContain("status:forge:ready");
		expect(events).not.toContain("status:web:ready");
	});
});
