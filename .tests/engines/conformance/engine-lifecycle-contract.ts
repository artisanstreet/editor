import { expect } from "vitest";
import { Cause, Effect, Exit, Fiber, Stream } from "effect";

import type { Engine, EngineObservation, EngineOpenInput } from "@artisan/engines";

function error_from(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function terminal_observations(events: ReadonlyArray<EngineObservation>) {
	return events.filter((event) => event._tag === "run_terminal");
}

/** Exercises provider-independent run invariants against any compatible Engine adapter. */
export async function assert_engine_lifecycle_contract(
	engine: Engine,
	open_input: EngineOpenInput,
) {
	const result = await Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const probe = yield* engine.Probe({
					client_name: "artisan-conformance",
					client_version: "0.3.0",
				});
				const run = yield* engine.Open(open_input);
				const events_fiber = yield* run.Events.pipe(Stream.runCollect, Effect.forkChild);
				const steer = {
					_tag: "steer" as const,
					command_id: "shared-steer",
					text: "Keep the shared intent",
				};
				const first = yield* run.Send(steer).pipe(Effect.exit);
				const duplicate = yield* run.Send(steer).pipe(Effect.exit);
				const conflict = yield* run
					.Send({ ...steer, text: "Change the shared intent" })
					.pipe(Effect.exit);

				yield* run.Send({ _tag: "close", command_id: "shared-close" });
				yield* run.Send({ _tag: "close", command_id: "shared-close-again" });

				const terminal = yield* run.Closed;
				const events = yield* Fiber.join(events_fiber);
				const after_close = yield* run
					.Send({
						_tag: "steer",
						command_id: "shared-after-close",
						text: "Too late",
					})
					.pipe(Effect.exit);

				return {
					after_close,
					conflict,
					duplicate,
					events: [...events],
					first,
					native_thread_id: run.native_thread_id,
					probe,
					resume_token: run.resume_token,
					terminal,
				};
			}),
		),
	);
	const sequences = result.events.map((event) => event.sequence);
	const terminals = terminal_observations(result.events);

	expect(result.probe.ready).toBe(true);
	expect(result.probe.authentication.state).toBe("authenticated");
	expect(result.probe.capabilities.events.state).toBe("supported");
	expect(result.native_thread_id).toBe(
		open_input._tag === "resume"
			? open_input.resume_token.native_thread_id
			: `native:${open_input.artisan_run_id}`,
	);
	expect(Exit.isSuccess(result.first)).toBe(true);
	expect(Exit.isSuccess(result.duplicate)).toBe(true);
	expect(error_from(result.conflict)).toMatchObject({
		_tag: "EngineCommandIdConflictError",
		command_id: "shared-steer",
	});
	expect(error_from(result.after_close)).toMatchObject({
		_tag: "EngineRunClosedError",
		command_id: "shared-after-close",
	});
	expect(result.terminal).toBe("closed");
	expect(terminals).toEqual([expect.objectContaining({ state: "closed" })]);
	expect(sequences).toEqual(Array.from({ length: sequences.length }, (_, index) => index + 1));
	expect(new Set(result.events.map((event) => event.observation_id)).size).toBe(
		result.events.length,
	);
	expect(result.events.every((event) => event.raw.engine_id === engine.Descriptor.id)).toBe(true);

	if (open_input._tag === "resume" && open_input.resume_token.opaque_checkpoint) {
		expect(result.resume_token.opaque_checkpoint).toBe(
			open_input.resume_token.opaque_checkpoint,
		);
	}
}
