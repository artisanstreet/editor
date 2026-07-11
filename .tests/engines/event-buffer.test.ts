import { describe, expect, it } from "vitest";
import { Effect, Stream } from "effect";

import { MakeEngineEventBuffer } from "../../modules/engines/src/process/event-buffer";

const observation = (id: string) => ({
	_tag: "process_diagnostic" as const,
	artisan_run_id: "run",
	level: "info" as const,
	message: id,
	observation_id: id,
	raw: { engine_id: "test", frame: id, transport: "test" },
	sequence: 0,
});

describe("shared engine event buffer", () => {
	it("fails full-buffer delivery without deadlocking and closes outside the lock", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					let closes = 0;
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						capacity: 1,
						CloseResource: Effect.sync(() => {
							closes += 1;
						}),
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: "terminal",
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					yield* buffer.Emit(observation("first"));
					const second = yield* Effect.exit(buffer.Emit(observation("second")));
					const events = yield* buffer.Events.pipe(Stream.runCollect);
					return { closes, second, events };
				}),
			),
		);
		expect(result.closes).toBe(1);
		expect(result.events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
	});

	it("preserves exact terminal closure across twenty emit/finish races", async () => {
		for (let attempt = 0; attempt < 20; attempt += 1) {
			const events = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const buffer = yield* MakeEngineEventBuffer({
							artisan_run_id: `run-${attempt}`,
							capacity: 8,
							CloseResource: Effect.void,
							make_terminal_observation: (state, sequence) => ({
								_tag: "run_terminal",
								artisan_run_id: `run-${attempt}`,
								observation_id: "terminal",
								raw: { engine_id: "test", frame: state, transport: "test" },
								sequence,
								state,
							}),
						});
						yield* Effect.all(
							[buffer.Emit(observation("a")), buffer.Finish("closed")],
							{ concurrency: "unbounded" },
						).pipe(Effect.ignore);
						return yield* buffer.Events.pipe(Stream.runCollect);
					}),
				),
			);
			expect(events.filter((event) => event._tag === "run_terminal")).toHaveLength(1);
			expect(events.at(-1)?._tag).toBe("run_terminal");
		}
	});
});
