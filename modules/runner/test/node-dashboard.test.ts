import { Effect, Queue, Sink, Stream } from "effect";
import { ChildProcessSpawner } from "effect/unstable/process";
import { describe, expect, it, vi } from "vitest";

import type { Configuration } from "../src/model.ts";
import { MakeBunDashboard, TakeNextDashboardEvent } from "../src/tui/dashboard.ts";
import type { DashboardEvent } from "../src/tui/transport.ts";

const configuration: Configuration = {
	dashboard: "always",
	endpoints: [{ label: "API", url: "http://127.0.0.1:4848" }],
	lanes: [
		{ id: "runner", name: "Runner", status: "ready" },
		{ id: "api", name: "API", status: "running" },
	],
	max_log_lines: 100,
	processes: [],
	title: "Example development",
};

describe("Bun dashboard parent transport", () => {
	it("fails dashboard startup when a live worker never becomes ready", async () => {
		const spawner = ChildProcessSpawner.make(() =>
			Effect.succeed(
				ChildProcessSpawner.makeHandle({
					all: Stream.empty,
					exitCode: Effect.never,
					getInputFd: () => Sink.drain,
					getOutputFd: () => Stream.never,
					isRunning: Effect.succeed(true),
					kill: () => Effect.void,
					pid: ChildProcessSpawner.ProcessId(0),
					stderr: Stream.empty,
					stdin: Sink.drain,
					stdout: Stream.empty,
					unref: Effect.succeed(Effect.void),
				}),
			),
		);
		const result = await Effect.runPromiseExit(
			Effect.scoped(
				MakeBunDashboard(configuration, {
					bun_executable: "bun",
					startup_timeout: 1,
					worker_path: "worker.js",
				}),
			).pipe(Effect.provideService(ChildProcessSpawner.ChildProcessSpawner, spawner)),
		);
		expect(result._tag).toBe("Failure");
	});

	it("prioritizes control events over a bounded sliding log tail", async () => {
		const next = await Effect.runPromise(
			Effect.gen(function* () {
				const control_events = yield* Queue.unbounded<DashboardEvent>();
				const log_events = yield* Queue.sliding<DashboardEvent>(1);
				yield* Queue.offer(log_events, { lane_id: "api", line: "old", type: "log" });
				yield* Queue.offer(log_events, { lane_id: "api", line: "new", type: "log" });
				yield* Queue.offer(control_events, {
					lane_id: "api",
					status: "ready",
					type: "status",
				});
				return yield* TakeNextDashboardEvent(control_events, log_events);
			}),
		);
		expect(next).toEqual({ lane_id: "api", status: "ready", type: "status" });
	});

	it("spawns a worker with inherited TUI streams and routes typed events through fd3", async () => {
		const writes: Uint8Array[] = [];
		let command: unknown;
		const spawner = ChildProcessSpawner.make((current_command) =>
			Effect.sync(() => {
				command = current_command;
				return ChildProcessSpawner.makeHandle({
					all: Stream.empty,
					exitCode: Effect.never,
					getInputFd: () =>
						Sink.forEach<Uint8Array, void, never, never>((chunk) =>
							Effect.sync(() => {
								writes.push(chunk);
							}),
						),
					getOutputFd: (fd) =>
						fd === 4
							? Stream.fromIterable([
									new TextEncoder().encode('{"type":"ready"}\n'),
								]).pipe(Stream.concat(Stream.never))
							: Stream.empty,
					isRunning: Effect.succeed(true),
					kill: () => Effect.void,
					pid: ChildProcessSpawner.ProcessId(1),
					stderr: Stream.empty,
					stdin: Sink.drain,
					stdout: Stream.empty,
					unref: Effect.succeed(Effect.void),
				});
			}),
		);

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const dashboard = yield* MakeBunDashboard(configuration, {
						bun_executable: "bun",
						worker_path: "worker.js",
					});
					yield* dashboard.Log("api", "\u001B[32mAPI is listening\u001B[0m");
					yield* dashboard.SetStatus("api", "ready");
					yield* Effect.yieldNow;
				}),
			).pipe(Effect.provideService(ChildProcessSpawner.ChildProcessSpawner, spawner)),
		);

		expect(command).toMatchObject({
			args: expect.arrayContaining(["worker.js"]),
			options: {
				additionalFds: { fd3: { type: "input" }, fd4: { type: "output" } },
				stderr: "inherit",
				stdin: "inherit",
				stdout: "inherit",
			},
		});
		const events = new TextDecoder().decode(Buffer.concat(writes));
		expect(events).toContain(
			'{"lane_id":"api","line":"\\u001b[32mAPI is listening\\u001b[0m","type":"log"}',
		);
		expect(events).toContain('{"lane_id":"api","status":"ready","type":"status"}');
	});

	it("keeps the runner alive and writes subsequent events to plain logs after a ready worker closes", async () => {
		const fallback = vi.spyOn(console, "log").mockImplementation(() => undefined);
		try {
			const spawner = ChildProcessSpawner.make(() =>
				Effect.succeed(
					ChildProcessSpawner.makeHandle({
						all: Stream.empty,
						exitCode: Effect.never,
						getInputFd: () => Sink.drain,
						getOutputFd: (fd) =>
							fd === 4
								? Stream.fromIterable([
										new TextEncoder().encode('{"type":"ready"}\n'),
									])
								: Stream.empty,
						isRunning: Effect.succeed(false),
						kill: () => Effect.void,
						pid: ChildProcessSpawner.ProcessId(2),
						stderr: Stream.empty,
						stdin: Sink.drain,
						stdout: Stream.empty,
						unref: Effect.succeed(Effect.void),
					}),
				),
			);
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const dashboard = yield* MakeBunDashboard(configuration, {
							bun_executable: "bun",
							worker_path: "worker.js",
						});
						yield* Effect.yieldNow;
						yield* dashboard.Log("api", "\u001B[31mworker unavailable\u001B[0m\r");
						yield* dashboard.SetStatus("api", "failed");
					}),
				).pipe(Effect.provideService(ChildProcessSpawner.ChildProcessSpawner, spawner)),
			);
			expect(fallback).toHaveBeenCalledWith("[api] worker unavailable");
			expect(fallback).toHaveBeenCalledWith("[api] failed");
		} finally {
			fallback.mockRestore();
		}
	});
});
