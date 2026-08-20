import { Deferred, Effect, Fiber, Stream } from "effect";
import { describe, expect, it } from "vitest";

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

const MakeBuffer = () =>
	MakeEngineEventBuffer({
		artisan_run_id: "run",
		CloseResource: Effect.void,
		make_terminal_observation: (state, sequence, error_ref) => ({
			_tag: "run_terminal",
			artisan_run_id: "run",
			...(error_ref === undefined ? {} : { error_ref }),
			observation_id: `terminal:${sequence}`,
			raw: { engine_id: "test", frame: state, transport: "test" },
			sequence,
			state,
		}),
	});

describe("shared engine event buffer", () => {
	it("retains a large burst in order until a delayed consumer drains it", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeBuffer();
					yield* Effect.forEach(
						Array.from({ length: 1_000 }, (_, index) => observation(`event-${index}`)),
						buffer.Emit,
					);
					yield* buffer.Finish("completed");
					return yield* buffer.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect(result).toHaveLength(1_001);
		expect(result.map((event) => event.sequence)).toEqual(
			Array.from({ length: 1_001 }, (_, index) => index + 1),
		);
		expect(result.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("serializes concurrent producers without dropping any observation", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeBuffer();
					yield* Effect.forEach(
						Array.from({ length: 400 }, (_, index) =>
							observation(`concurrent-${index}`),
						),
						buffer.Emit,
						{ concurrency: "unbounded", discard: true },
					);
					yield* buffer.Finish("completed");
					return yield* buffer.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect(result).toHaveLength(401);
		expect(new Set(result.map((event) => event.observation_id)).size).toBe(401);
		expect(result.map((event) => event.sequence)).toEqual(
			Array.from({ length: 401 }, (_, index) => index + 1),
		);
	});

	it("rejects no already-produced observation when closure races an emit", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						BeforeEnqueue: () =>
							Deferred.succeed(started, undefined).pipe(
								Effect.andThen(Deferred.await(release)),
							),
						CloseResource: Effect.void,
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: `terminal:${sequence}`,
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					const emitting = yield* buffer
						.Emit(observation("first"))
						.pipe(Effect.forkChild);
					yield* Deferred.await(started);
					const finishing = yield* buffer.Finish("completed").pipe(Effect.forkChild);
					yield* Deferred.succeed(release, undefined);
					yield* Fiber.join(emitting);
					yield* Fiber.join(finishing);
					return yield* buffer.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect(result.map((event) => event.sequence)).toEqual([1, 2]);
		expect(result.map((event) => event._tag)).toEqual(["process_diagnostic", "run_terminal"]);
	});
});
