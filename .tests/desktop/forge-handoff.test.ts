import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";

import { Deferred, Effect, Exit, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	DesktopForgeLifecycle,
	DesktopRenderer,
	ForgeHandoffProcess,
	handoff_cleanup_timeout,
	handoff_exit_drain_delay_ms,
	IsWindowsCommandScript,
	make_desktop_forge_lifecycle_layer,
	make_node_forge_handoff_process_layer_with,
	owned_stop_cleanup_timeout,
	OwnedForgeStopArguments,
} from "../../modules/desktop/src/forge-handoff";
import type { ForgeHandoff } from "../../modules/desktop/src/renderer-host";

const MakeLifecycle = (
	process: ForgeHandoffProcess["Service"],
	renderer: DesktopRenderer["Service"],
) =>
	Effect.runPromise(
		DesktopForgeLifecycle.pipe(
			Effect.provide(make_desktop_forge_lifecycle_layer("C:/Artisan/ae.exe")),
			Effect.provide(Layer.succeed(DesktopRenderer, renderer)),
			Effect.provide(Layer.succeed(ForgeHandoffProcess, process)),
		),
	);

describe("desktop Forge handoff lifecycle", () => {
	it("loads the connection loader before the background handoff navigates the same renderer", async () => {
		const events: Array<string> = [];
		let loader_finished = false;
		const gate = await Effect.runPromise(Deferred.make<ForgeHandoff>());
		const loader_gate = await Effect.runPromise(Deferred.make<void>());
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () =>
					Effect.sync(() => events.push("handoff")).pipe(
						Effect.andThen(Deferred.await(gate)),
					),
				StopOwned: () => Effect.void,
			}),
			DesktopRenderer.of({
				ClearCookies: () => Effect.sync(() => events.push("clear-cookies")),
				LoadUrl: (url) =>
					Effect.sync(() => events.push(url)).pipe(
						Effect.andThen(
							url === "artisan://app/"
								? Deferred.await(loader_gate).pipe(
										Effect.tap(() =>
											Effect.sync(() => (loader_finished = true)),
										),
									)
								: Effect.void,
						),
					),
			}),
		);

		const start = Effect.runPromise(lifecycle.Start());
		await new Promise<void>((resolve) => setImmediate(resolve));
		expect(loader_finished).toBe(false);
		expect(events).toEqual(["clear-cookies", "artisan://app/"]);
		await Effect.runPromise(Deferred.succeed(loader_gate, undefined));
		await start;
		expect(loader_finished).toBe(true);
		const pairing = Effect.runPromise(lifecycle.Reconnect());
		await new Promise<void>((resolve) => setImmediate(resolve));
		expect(events).toEqual(["clear-cookies", "artisan://app/", "handoff"]);

		await Effect.runPromise(
			Deferred.succeed(gate, {
				endpoint: "http://127.0.0.1:52985/",
				pair_code: "one-time",
				version: 1,
			}),
		);
		await new Promise<void>((resolve) => setImmediate(resolve));
		await pairing;
		expect(events).toEqual([
			"clear-cookies",
			"artisan://app/",
			"handoff",
			"artisan://app/?artisan-handoff=1#pair=one-time&forge=http%3A%2F%2F127.0.0.1%3A52985%2F",
		]);
	});

	it("uses a fresh document-navigation marker for every completed handoff", async () => {
		let requests = 0;
		const paired_urls: string[] = [];
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () =>
					Effect.sync(() => {
						requests += 1;
						return {
							endpoint: "http://127.0.0.1:52985/",
							pair_code: `one-time-${String(requests)}`,
							version: 1 as const,
						};
					}),
				StopOwned: () => Effect.void,
			}),
			DesktopRenderer.of({
				ClearCookies: () => Effect.void,
				LoadUrl: (url) => Effect.sync(() => paired_urls.push(url)),
			}),
		);

		await Effect.runPromise(lifecycle.Reconnect());
		await Effect.runPromise(lifecycle.Reconnect());

		expect(paired_urls).toEqual([
			"artisan://app/?artisan-handoff=1#pair=one-time-1&forge=http%3A%2F%2F127.0.0.1%3A52985%2F",
			"artisan://app/?artisan-handoff=2#pair=one-time-2&forge=http%3A%2F%2F127.0.0.1%3A52985%2F",
		]);
	});

	it("coalesces concurrent reconnects through one handoff and paired navigation", async () => {
		let requests = 0;
		const paired_urls: Array<string> = [];
		const pair_navigation = await Effect.runPromise(Deferred.make<void>());
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () =>
					Effect.sync(() => {
						requests += 1;
						return {
							endpoint: "http://127.0.0.1:52985/",
							pair_code: "one-time",
							version: 1 as const,
						};
					}),
				StopOwned: () => Effect.void,
			}),
			DesktopRenderer.of({
				ClearCookies: () => Effect.void,
				LoadUrl: (url) =>
					url === "artisan://app/"
						? Effect.void
						: Effect.sync(() => paired_urls.push(url)).pipe(
								Effect.andThen(Deferred.await(pair_navigation)),
							),
			}),
		);

		const first = Effect.runPromise(lifecycle.Reconnect());
		await new Promise<void>((resolve) => setImmediate(resolve));
		const second = Effect.runPromise(lifecycle.Reconnect());
		await new Promise<void>((resolve) => setImmediate(resolve));
		expect(requests).toBe(1);
		expect(paired_urls).toHaveLength(1);
		await Effect.runPromise(Deferred.succeed(pair_navigation, undefined));
		await Promise.all([first, second]);
	});

	it("clears an interrupted reconnect so waiters settle and a later reconnect can pair", async () => {
		let requests = 0;
		const first_started = await Effect.runPromise(Deferred.make<void>());
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () => {
					requests += 1;
					return requests === 1
						? Deferred.succeed(first_started, undefined).pipe(
								Effect.andThen(Effect.never),
							)
						: Effect.succeed({
								endpoint: "http://127.0.0.1:52985/",
								pair_code: "recovered",
								version: 1 as const,
							});
				},
				StopOwned: () => Effect.void,
			}),
			DesktopRenderer.of({ ClearCookies: () => Effect.void, LoadUrl: () => Effect.void }),
		);

		await Effect.runPromise(
			Effect.gen(function* () {
				const first = yield* lifecycle
					.Reconnect()
					.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Deferred.await(first_started);
				const waiting = yield* lifecycle
					.Reconnect()
					.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Fiber.interrupt(first);
				yield* Fiber.await(waiting);
				yield* lifecycle.Reconnect();
			}),
		);
		expect(requests).toBe(2);
	});

	it("shares an in-flight handoff with quit cleanup and stops only its exact owned instance", async () => {
		const gate = await Effect.runPromise(Deferred.make<ForgeHandoff>());
		const stops: Array<string> = [];
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () => Deferred.await(gate),
				StopOwned: (_path, instance_id) => Effect.sync(() => stops.push(instance_id)),
			}),
			DesktopRenderer.of({ ClearCookies: () => Effect.void, LoadUrl: () => Effect.void }),
		);

		await Effect.runPromise(lifecycle.Start());
		const pairing = Effect.runPromise(lifecycle.Reconnect());
		await new Promise<void>((resolve) => setImmediate(resolve));
		const cleanup = Effect.runPromise(lifecycle.Cleanup());
		await Effect.runPromise(
			Deferred.succeed(gate, {
				endpoint: "http://127.0.0.1:52985/",
				owned_instance_id: "forge_owner-1",
				pair_code: "one-time",
				version: 1,
			}),
		);
		await cleanup;
		await pairing;
		expect(stops).toEqual(["forge_owner-1"]);
	});

	it("retains editor ownership when a later pairing joins a pre-existing Forge", async () => {
		let requests = 0;
		const stops: Array<string> = [];
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () =>
					Effect.sync(() => {
						requests += 1;
						return {
							endpoint: "http://127.0.0.1:52985/",
							...(requests === 1 ? { owned_instance_id: "forge_owner-1" } : {}),
							pair_code: `one-time-${String(requests)}`,
							version: 1 as const,
						};
					}),
				StopOwned: (_path, instance_id) => Effect.sync(() => stops.push(instance_id)),
			}),
			DesktopRenderer.of({ ClearCookies: () => Effect.void, LoadUrl: () => Effect.void }),
		);

		await Effect.runPromise(lifecycle.Reconnect());
		await Effect.runPromise(lifecycle.Reconnect());
		await Effect.runPromise(lifecycle.Cleanup());
		expect(stops).toEqual(["forge_owner-1"]);
	});

	it("stops known ownership without waiting for paired navigation to settle", async () => {
		const navigation_started = await Effect.runPromise(Deferred.make<void>());
		const stops: Array<string> = [];
		const lifecycle = await MakeLifecycle(
			ForgeHandoffProcess.of({
				Request: () =>
					Effect.succeed({
						endpoint: "http://127.0.0.1:52985/",
						owned_instance_id: "forge_owner-1",
						pair_code: "one-time",
						version: 1,
					}),
				StopOwned: (_path, instance_id) => Effect.sync(() => stops.push(instance_id)),
			}),
			DesktopRenderer.of({
				ClearCookies: () => Effect.void,
				LoadUrl: () =>
					Deferred.succeed(navigation_started, undefined).pipe(
						Effect.andThen(Effect.never),
					),
			}),
		);

		await Effect.runPromise(
			Effect.gen(function* () {
				const reconnect = yield* lifecycle
					.Reconnect()
					.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Deferred.await(navigation_started);
				yield* lifecycle.Cleanup().pipe(Effect.timeout("1 second"));
				yield* Fiber.interrupt(reconnect);
			}),
		);
		expect(stops).toEqual(["forge_owner-1"]);
	});

	it("completes from CLI exit without waiting for the inherited stdout pipe to close", async () => {
		const stdout = new PassThrough();
		const child = Object.assign(new EventEmitter(), {
			kill: () => true,
			stdout,
		}) as unknown as ChildProcess;
		const layer = make_node_forge_handoff_process_layer_with(() => {
			queueMicrotask(() => {
				stdout.write(
					'{"endpoint":"http://127.0.0.1:52985/","owned_instance_id":"forge_owner-1","pair_code":"one-time","version":1}\n',
				);
				child.emit("exit", 0, null);
			});
			return child;
		});
		const process = await Effect.runPromise(ForgeHandoffProcess.pipe(Effect.provide(layer)));

		const handoff = await Effect.runPromise(
			process.Request("C:/Artisan/ae.exe").pipe(Effect.timeout("1 second")),
		);
		expect(handoff.owned_instance_id).toBe("forge_owner-1");
		expect(stdout.closed).toBe(false);
	});

	it("rejects stdout that crosses its bound during the post-exit drain", async () => {
		const stdout = new PassThrough();
		const child = Object.assign(new EventEmitter(), {
			kill: () => true,
			stdout,
		}) as unknown as ChildProcess;
		const layer = make_node_forge_handoff_process_layer_with(() => {
			queueMicrotask(() => {
				stdout.write(
					'{"endpoint":"http://127.0.0.1:52985/","owned_instance_id":"forge_owner-1","pair_code":"one-time","version":1}\n',
				);
				child.emit("exit", 0, null);
				setTimeout(() => stdout.write("x".repeat(64 * 1024)), 10);
			});
			return child;
		});
		const process = await Effect.runPromise(ForgeHandoffProcess.pipe(Effect.provide(layer)));

		const exit = await Effect.runPromiseExit(process.Request("C:/Artisan/ae.exe"));
		expect(Exit.isFailure(exit)).toBe(true);
	});

	it("uses a shell only for explicit Windows command-script compatibility", () => {
		expect(IsWindowsCommandScript("C:/Artisan/ae.exe")).toBe(false);
		expect(IsWindowsCommandScript("/usr/local/bin/ae")).toBe(false);
		expect(IsWindowsCommandScript("C:/Artisan/ae.cmd")).toBe(true);
		expect(IsWindowsCommandScript("C:/Artisan/ae.BAT")).toBe(true);
		expect(OwnedForgeStopArguments("forge_owner-1")).toEqual([
			"stop",
			"--instance-id",
			"forge_owner-1",
		]);
	});

	it("keeps cleanup budgets above the prior five-second quit cutoff", () => {
		expect(handoff_exit_drain_delay_ms).toBeLessThan(1_000);
		expect(handoff_cleanup_timeout).toBe("25 seconds");
		expect(owned_stop_cleanup_timeout).toBe("30 seconds");
	});
});
