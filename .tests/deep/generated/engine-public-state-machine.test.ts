import { Cause, Effect, Exit, Stream } from "effect";
import { describe, expect, it } from "vitest";

import type { EngineCommand, EngineObservation } from "@artisan/engines";
import { MakeEngineEventBuffer } from "../../../modules/engines/src/process/event-buffer";

import { make_fake_engine } from "../../engines/harness/fake-engine";
import { EngineOpenScenarios } from "../../engines/scenarios/engine-scenarios";

/** Generates reproducible action choices without making a randomized failure irreproducible. */
function make_action_indexes(seed: number, count: number) {
	let state = seed >>> 0;

	return Array.from({ length: count }, () => {
		state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0;

		return state % 3;
	});
}

function error_from(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function command_for(index: number, variant: number): EngineCommand {
	const command_id = `command_${index}`;

	switch (variant) {
		case 0:
			return { _tag: "steer", command_id, text: `Steer ${index}` };
		case 1:
			return {
				_tag: "respond_approval",
				approval_id: `approval_${index}`,
				approved: index % 2 === 0,
				command_id,
			};
		default:
			return {
				_tag: "respond_question",
				answers: { [`question_${index}`]: [`Answer ${index}`] },
				command_id,
			};
	}
}

function expected_effect(event: EngineObservation) {
	switch (event._tag) {
		case "agent_message_delta":
		case "agent_message_completed":
		case "turn_state":
			return event.raw.frame;
		default:
			return undefined;
	}
}

function diagnostic(id: string): EngineObservation {
	return {
		_tag: "process_diagnostic",
		artisan_run_id: "generated-buffer-run",
		level: "info",
		message: id,
		observation_id: id,
		raw: { engine_id: "generated", frame: id, transport: "generated-test" },
		sequence: 0,
	};
}

describe("generated public engine state machine", () => {
	it("preserves idempotency, correlation, contiguous streams, terminality, and cleanup across fixed seeds", async () => {
		for (const seed of [1, 7, 19, 43, 97, 211, 503, 997]) {
			let cleanup_count = 0;
			const engine = make_fake_engine({
				event_capacity: 64,
				on_cleanup: () => {
					cleanup_count += 1;
				},
			});
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const run = yield* engine.Open({
							...EngineOpenScenarios.start,
							artisan_run_id: `generated-run-${seed}`,
						});
						const commands = make_action_indexes(seed, 9).map((variant, index) =>
							command_for(index, variant),
						);

						for (const command of commands) {
							yield* run.Send(command);
							yield* run.Send(command);
						}

						const conflict = yield* run
							.Send({
								_tag: "steer",
								command_id: commands[0]!.command_id,
								text: "Changed intent must not duplicate an effect",
							})
							.pipe(Effect.exit);
						const terminal =
							seed % 2 === 0
								? ({ _tag: "cancel", command_id: `terminal_${seed}` } as const)
								: ({ _tag: "close", command_id: `terminal_${seed}` } as const);

						yield* run.Send(terminal);
						yield* run.Send(terminal);

						return {
							conflict,
							events: yield* run.Events.pipe(Stream.runCollect),
						};
					}),
				),
			);
			const effect_command_ids = result.events
				.map(expected_effect)
				.filter(
					(frame): frame is { readonly command_id: string } =>
						typeof frame === "object" && frame !== null && "command_id" in frame,
				)
				.map((frame) => frame.command_id);
			const sequences = result.events.map((event) => event.sequence);
			const terminal_events = result.events.filter((event) => event._tag === "run_terminal");

			expect(error_from(result.conflict)).toMatchObject({
				_tag: "EngineCommandIdConflictError",
				command_id: `command_0`,
			});
			expect(effect_command_ids).toEqual(
				Array.from({ length: 9 }, (_, index) => `command_${index}`),
			);
			expect(sequences).toEqual(
				Array.from({ length: sequences.length }, (_, index) => index + 1),
			);
			expect(terminal_events).toEqual([
				expect.objectContaining({ state: seed % 2 === 0 ? "cancelled" : "closed" }),
			]);
			expect(cleanup_count).toBe(1);
		}
	});

	it("keeps one contiguous terminal stream and one cleanup across generated emit/finish interleavings", async () => {
		for (const seed of [3, 11, 29, 61, 127, 257, 521, 1_031]) {
			let cleanup_count = 0;
			const events = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const buffer = yield* MakeEngineEventBuffer({
							artisan_run_id: `generated-buffer-${seed}`,
							capacity: 64,
							CloseResource: Effect.sync(() => {
								cleanup_count += 1;
							}),
							make_terminal_observation: (state, sequence) => ({
								_tag: "run_terminal",
								artisan_run_id: `generated-buffer-${seed}`,
								observation_id: "terminal",
								raw: {
									engine_id: "generated",
									frame: state,
									transport: "generated-test",
								},
								sequence,
								state,
							}),
						});
						const operations = make_action_indexes(seed, 16).map((choice, index) =>
							choice === 0
								? buffer.Finish("closed")
								: buffer
										.Emit(diagnostic(`seed_${seed}_event_${index}`))
										.pipe(Effect.ignore),
						);

						yield* Effect.all(operations, { concurrency: "unbounded", discard: true });

						return yield* buffer.Events.pipe(Stream.runCollect);
					}),
				),
			);
			const sequences = events.map((event) => event.sequence);
			const terminals = events.filter((event) => event._tag === "run_terminal");

			expect(sequences).toEqual(
				Array.from({ length: sequences.length }, (_, index) => index + 1),
			);
			expect(terminals).toHaveLength(1);
			expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "closed" });
			expect(cleanup_count).toBe(1);
		}
	});
});
