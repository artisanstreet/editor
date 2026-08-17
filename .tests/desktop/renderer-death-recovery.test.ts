import { readFileSync } from "node:fs";

import { Deferred, Effect } from "effect";
import { describe, expect, it } from "vitest";

import { make_renderer_death_recovery } from "../../modules/desktop/src/renderer-death-recovery";

const root = new URL("../..", import.meta.url);
const Flush = () => new Promise<void>((resolve) => setImmediate(resolve));

describe("renderer death recovery", () => {
	it("defers synchronous success and clears its latch for a later request", async () => {
		const scheduled: Array<() => void> = [];
		let reconnects = 0;
		const recovery = make_renderer_death_recovery({
			IsAvailable: () => true,
			Reconnect: () => Effect.sync(() => (reconnects += 1)),
			ReportFailure: () => undefined,
			Schedule: (task) => scheduled.push(task),
		});

		recovery.Request();
		expect(reconnects).toBe(0);
		expect(scheduled).toHaveLength(1);
		scheduled[0]!();
		await Flush();
		expect(reconnects).toBe(1);
		recovery.Request();
		expect(scheduled).toHaveLength(2);
	});

	it("coalesces duplicate loss notifications until the reconnect settles", async () => {
		const scheduled: Array<() => void> = [];
		const reconnect_gate = await Effect.runPromise(Deferred.make<void>());
		let reconnects = 0;
		const recovery = make_renderer_death_recovery({
			IsAvailable: () => true,
			Reconnect: () =>
				Effect.sync(() => (reconnects += 1)).pipe(
					Effect.andThen(Deferred.await(reconnect_gate)),
				),
			ReportFailure: () => undefined,
			Schedule: (task) => scheduled.push(task),
		});

		recovery.Request();
		recovery.Request();
		expect(scheduled).toHaveLength(1);
		scheduled[0]!();
		await Flush();
		expect(reconnects).toBe(1);
		recovery.Request();
		expect(scheduled).toHaveLength(1);
		await Effect.runPromise(Deferred.succeed(reconnect_gate, undefined));
		await Flush();
		recovery.Request();
		expect(scheduled).toHaveLength(2);
	});

	it("cancels a deferred recovery once shutdown begins", async () => {
		const scheduled: Array<() => void> = [];
		let reconnects = 0;
		const recovery = make_renderer_death_recovery({
			IsAvailable: () => true,
			Reconnect: () => Effect.sync(() => (reconnects += 1)),
			ReportFailure: () => undefined,
			Schedule: (task) => scheduled.push(task),
		});

		recovery.Request();
		await Effect.runPromise(recovery.Close());
		scheduled[0]!();
		expect(reconnects).toBe(0);
	});

	it("interrupts a started recovery during shutdown and rejects later requests", async () => {
		const scheduled: Array<() => void> = [];
		const started = await Effect.runPromise(Deferred.make<void>());
		const finalized = await Effect.runPromise(Deferred.make<void>());
		let reconnects = 0;
		const recovery = make_renderer_death_recovery({
			IsAvailable: () => true,
			Reconnect: () =>
				Effect.sync(() => (reconnects += 1)).pipe(
					Effect.andThen(Deferred.succeed(started, undefined)),
					Effect.andThen(Effect.never),
					Effect.ensuring(Deferred.succeed(finalized, undefined)),
				),
			ReportFailure: () => undefined,
			Schedule: (task) => scheduled.push(task),
		});

		recovery.Request();
		scheduled[0]!();
		await Effect.runPromise(Deferred.await(started));
		expect(reconnects).toBe(1);
		await Effect.runPromise(recovery.Close());
		const finalized_exit = await Effect.runPromise(Deferred.poll(finalized));
		expect(finalized_exit._tag).toBe("Some");
		recovery.Request();
		expect(scheduled).toHaveLength(1);
	});

	it("reports typed failures and defects, then permits a later independent request", async () => {
		const scheduled: Array<() => void> = [];
		let reconnects = 0;
		let failures = 0;
		const recovery = make_renderer_death_recovery({
			IsAvailable: () => true,
			Reconnect: () => {
				reconnects += 1;
				return reconnects === 1
					? Effect.fail("handoff failed")
					: reconnects === 2
						? Effect.die("handoff defect")
						: Effect.void;
			},
			ReportFailure: () => (failures += 1),
			Schedule: (task) => scheduled.push(task),
		});

		recovery.Request();
		scheduled[0]!();
		await Flush();
		expect(failures).toBe(1);
		recovery.Request();
		expect(scheduled).toHaveLength(2);
		scheduled[1]!();
		await Flush();
		expect(failures).toBe(2);
		recovery.Request();
		expect(scheduled).toHaveLength(3);
		scheduled[2]!();
		await Flush();
		expect(reconnects).toBe(3);
	});

	it("wires only renderer-process loss to the deferred recovery controller", () => {
		const main = readFileSync(new URL("modules/desktop/src/main.ts", root), "utf8");

		expect(main).toContain("make_renderer_death_recovery");
		expect(main).toContain('editor_window.webContents.on("render-process-gone", () =>');
		expect(main).toContain("renderer_death_recovery.Request()");
		expect(main).toContain("renderer_death_recovery.Close()");
		expect(main).not.toContain(
			'editor_window.webContents.on("unresponsive", () => renderer_death_recovery',
		);
	});
});
