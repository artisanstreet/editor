import { describe, expect, it } from "vitest";
import { Effect, Stream } from "effect";

import type { EngineObservation } from "@artisan/engines";

import { make_fake_engine } from "../harness/fake-engine";
import {
	EngineCommandScenarios,
	EngineOpenScenarios,
	EngineTerminalCommandScenarios,
} from "../scenarios/engine-scenarios";

function terminal_observations(events: ReadonlyArray<EngineObservation>) {
	return events.filter((event) => event._tag === "run_terminal");
}

describe("Engine conformance", () => {
	it("preserves scoped lifecycle, ordering, command semantics, and cleanup", async () => {
		let cleanup_count = 0;
		const engine = make_fake_engine({
			on_cleanup: () => {
				cleanup_count += 1;
			},
		});

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const probe = yield* engine.Probe({
						client_name: "conformance",
						client_version: "0.2.0",
					});
					const run = yield* engine.Open(EngineOpenScenarios.start);
					const initial_events = yield* run.Events.pipe(
						Stream.take(2),
						Stream.runCollect,
					);

					expect(probe.ready).toBe(true);
					expect(probe.authentication.state).toBe("authenticated");
					expect(probe.capabilities.events.state).toBe("supported");
					expect(run.artisan_run_id).toBe(EngineOpenScenarios.start.artisan_run_id);
					expect(run.native_thread_id).toBe("native:artisan-run-start");
					expect(initial_events.map((event) => event._tag)).toEqual([
						"run_state",
						"run_state",
					]);
					expect(initial_events.map((event) => event.observation_id)).toEqual([
						"artisan-run-start:1",
						"artisan-run-start:2",
					]);
					expect(initial_events.map((event) => event.sequence)).toEqual([1, 2]);

					for (const command of EngineCommandScenarios) {
						yield* run.Send(command);
					}

					const command_events = yield* run.Events.pipe(
						Stream.take(3),
						Stream.runCollect,
					);
					yield* run.Send(EngineCommandScenarios[0]!);

					const conflict = yield* Effect.match(
						run.Send({
							...EngineCommandScenarios[0]!,
							text: "Changed intent under the same id",
						}),
						{
							onFailure: (error) => error,
							onSuccess: () => undefined,
						},
					);

					expect(command_events.map((event) => event._tag)).toEqual([
						"agent_message_delta",
						"turn_state",
						"agent_message_completed",
					]);
					expect(conflict).toMatchObject({
						_tag: "EngineCommandIdConflictError",
						command_id: "command-steer",
					});

					yield* run.Send(EngineTerminalCommandScenarios.close);
					yield* run.Send(EngineTerminalCommandScenarios.second_close);

					const terminal_state = yield* run.Closed;
					const closed_events = yield* run.Events.pipe(Stream.runCollect);
					const closed_steer = yield* Effect.match(
						run.Send({
							_tag: "steer",
							command_id: "command-after-close",
							text: "Too late",
						}),
						{
							onFailure: (error) => error,
							onSuccess: () => undefined,
						},
					);

					expect(terminal_state).toBe("closed");
					expect(terminal_observations(closed_events)).toHaveLength(1);
					expect(terminal_observations(closed_events)[0]?.state).toBe("closed");
					expect(closed_steer).toMatchObject({
						_tag: "EngineRunClosedError",
						command_id: "command-after-close",
					});
				}),
			),
		);

		expect(cleanup_count).toBe(1);
	});

	it("supports resume, cancellation, unsupported commands, backpressure, and scope finalization", async () => {
		let cleanup_count = 0;
		const make_engine = (options?: Parameters<typeof make_fake_engine>[0]) =>
			make_fake_engine({
				...options,
				on_cleanup: () => {
					cleanup_count += 1;
				},
			});

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const resume_engine = make_engine();
					const resume_run = yield* resume_engine.Open(EngineOpenScenarios.resume);

					expect(resume_run.native_thread_id).toBe("native-thread-resume");

					yield* resume_run.Send(EngineTerminalCommandScenarios.cancel);

					const cancelled_events = yield* resume_run.Events.pipe(Stream.runCollect);

					expect(yield* resume_run.Closed).toBe("cancelled");
					expect(terminal_observations(cancelled_events)).toEqual([
						expect.objectContaining({ state: "cancelled" }),
					]);

					const unsupported_engine = make_engine({ unsupported_commands: ["steer"] });
					const unsupported_run = yield* unsupported_engine.Open(
						EngineOpenScenarios.start,
					);
					const unsupported = yield* Effect.match(
						unsupported_run.Send(EngineTerminalCommandScenarios.unsupported_steer),
						{
							onFailure: (error) => error,
							onSuccess: () => undefined,
						},
					);

					expect(unsupported).toMatchObject({
						_tag: "EngineUnsupportedCommandError",
						command: "steer",
					});

					yield* unsupported_run.Send(EngineTerminalCommandScenarios.close);
					yield* unsupported_run.Closed;

					const backpressure_engine = make_engine({ event_capacity: 2 });
					const backpressure_run = yield* backpressure_engine.Open(
						EngineOpenScenarios.start,
					);
					const backpressure = yield* Effect.match(
						backpressure_run.Send({
							_tag: "steer",
							command_id: "command-backpressure",
							text: "Overflow",
						}),
						{
							onFailure: (error) => error,
							onSuccess: () => undefined,
						},
					);

					expect(backpressure).toMatchObject({
						_tag: "EngineBackpressureError",
						capacity: 2,
					});

					const buffered_open_events = yield* backpressure_run.Events.pipe(
						Stream.take(2),
						Stream.runCollect,
					);

					expect(buffered_open_events.map((event) => event.sequence)).toEqual([1, 2]);

					yield* backpressure_run.Send({
						_tag: "steer",
						command_id: "command-backpressure",
						text: "Overflow",
					});

					const retried_event = yield* backpressure_run.Events.pipe(
						Stream.take(1),
						Stream.runCollect,
					);

					expect(retried_event).toMatchObject([
						{ _tag: "agent_message_delta", sequence: 3 },
					]);

					yield* backpressure_run.Send(EngineTerminalCommandScenarios.close);
					const backpressure_events = yield* backpressure_run.Events.pipe(
						Stream.runCollect,
					);

					expect(terminal_observations(backpressure_events)).toHaveLength(1);

					const scoped_engine = make_engine();

					yield* scoped_engine.Open(EngineOpenScenarios.start);
				}),
			),
		);

		expect(cleanup_count).toBe(4);
	});
});
