import { describe, expect, it } from "vitest";
import { Cause, Effect, Exit, Stream } from "effect";

import type { EngineObservation } from "@artisan/engines";

import { make_fake_engine } from "./harness/fake-engine";
import { EngineOpenScenarios } from "./scenarios/engine-scenarios";

function error_from(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function terminal_events(events: ReadonlyArray<EngineObservation>) {
	return events.filter((event) => event._tag === "run_terminal");
}

describe("Engine public state machine", () => {
	it("keeps retries idempotent, rejects changed intent, emits one terminal event, and cleans up", async () => {
		let cleanup_count = 0;
		const engine = make_fake_engine({
			on_cleanup: () => {
				cleanup_count += 1;
			},
		});
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(EngineOpenScenarios.start);

					yield* run.Send({
						_tag: "steer",
						command_id: "retry",
						text: "Keep this intent",
					});
					yield* run.Send({
						_tag: "steer",
						command_id: "retry",
						text: "Keep this intent",
					});

					const conflict = yield* run
						.Send({ _tag: "steer", command_id: "retry", text: "Changed intent" })
						.pipe(Effect.exit);

					yield* run.Send({ _tag: "close", command_id: "close" });
					yield* run.Send({ _tag: "close", command_id: "close-again" });

					const events = yield* run.Events.pipe(Stream.runCollect);

					return { conflict, events };
				}),
			),
		);
		const sequences = result.events.map((event) => event.sequence);

		expect(error_from(result.conflict)).toMatchObject({
			_tag: "EngineCommandIdConflictError",
			command_id: "retry",
		});
		expect(result.events.filter((event) => event._tag === "agent_message_delta")).toHaveLength(
			1,
		);
		expect(terminal_events(result.events)).toEqual([
			expect.objectContaining({ state: "closed" }),
		]);
		expect(sequences).toEqual(
			Array.from({ length: sequences.length }, (_, index) => index + 1),
		);
		expect(cleanup_count).toBe(1);
	});

	it("reaches a cancelled terminal state through a second deterministic sequence", async () => {
		const engine = make_fake_engine();
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(EngineOpenScenarios.resume);

					yield* run.Send({ _tag: "cancel", command_id: "cancel" });

					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);
		const sequences = events.map((event) => event.sequence);

		expect(terminal_events(events)).toEqual([expect.objectContaining({ state: "cancelled" })]);
		expect(sequences).toEqual(
			Array.from({ length: sequences.length }, (_, index) => index + 1),
		);
	});
});
