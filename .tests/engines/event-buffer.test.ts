import { describe, expect, it } from "vitest";
import { Deferred, Effect, Exit, Fiber, Ref, Stream } from "effect";

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
	it("delivers a 600-observation burst to a deliberately slow consumer without loss", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						backpressure_timeout_ms: 1_000,
						capacity: 4,
						CloseResource: Effect.void,
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: "terminal",
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					const events_fiber = yield* buffer.Events.pipe(
						Stream.mapEffect((event) => Effect.sleep(1).pipe(Effect.as(event))),
						Stream.runCollect,
						Effect.forkChild,
					);
					yield* Effect.forEach(
						Array.from({ length: 600 }, (_, index) => observation(`event-${index}`)),
						(event) => buffer.Emit(event),
					);
					yield* buffer.Finish("completed");
					return yield* Fiber.join(events_fiber);
				}),
			),
		);
		expect(result).toHaveLength(601);
		expect(result.map((event) => event.sequence)).toEqual(
			Array.from({ length: 601 }, (_, index) => index + 1),
		);
		expect(result.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("wakes every concurrent producer as a single slot is released", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						backpressure_timeout_ms: 5_000,
						capacity: 1,
						CloseResource: Effect.void,
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: "terminal",
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					const events_fiber = yield* buffer.Events.pipe(
						Stream.mapEffect((event) => Effect.sleep(1).pipe(Effect.as(event))),
						Stream.runCollect,
						Effect.forkChild,
					);
					yield* Effect.forEach(
						Array.from({ length: 120 }, (_, index) =>
							observation(`concurrent-${index}`),
						),
						(event) => buffer.Emit(event),
						{ concurrency: "unbounded" },
					);
					yield* buffer.Finish("completed");

					return yield* Fiber.join(events_fiber);
				}),
			),
		);
		expect(result).toHaveLength(121);
		expect(result.map((event) => event.sequence)).toEqual(
			Array.from({ length: 121 }, (_, index) => index + 1),
		);
		expect(result.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("does not count a delayed enqueue hook as queued backpressure", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const first_hook_started = yield* Deferred.make<void>();
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						backpressure_timeout_ms: 10,
						BeforePrepare: (event) =>
							event.observation_id.startsWith("first")
								? Deferred.succeed(first_hook_started, undefined).pipe(
										Effect.andThen(Effect.sleep(30)),
									)
								: Effect.void,
						capacity: 1,
						CloseResource: Effect.void,
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: "terminal",
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					const events_fiber = yield* buffer.Events.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);
					const first = yield* buffer.Emit(observation("first")).pipe(Effect.forkChild);
					yield* Deferred.await(first_hook_started);
					const second = yield* buffer.Emit(observation("second")).pipe(Effect.forkChild);
					yield* Effect.all([Fiber.join(first), Fiber.join(second)], {
						concurrency: "unbounded",
					});
					yield* buffer.Finish("completed");
					return yield* Fiber.join(events_fiber);
				}),
			),
		);

		expect(result.map((event) => event.sequence)).toEqual([1, 2, 3]);
		expect(result.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("commits the canonical observation before a racing terminal", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const hook_started = yield* Deferred.make<void>();
					const allow_hook = yield* Deferred.make<void>();
					const seen = yield* Ref.make<
						{ readonly observation_id: string; readonly sequence: number } | undefined
					>(undefined);
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						BeforeEnqueue: (event) =>
							Ref.set(seen, {
								observation_id: event.observation_id,
								sequence: event.sequence,
							}).pipe(
								Effect.andThen(Deferred.succeed(hook_started, undefined)),
								Effect.andThen(Deferred.await(allow_hook)),
							),
						capacity: 1,
						CloseResource: Effect.void,
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: "terminal",
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					const events_fiber = yield* buffer.Events.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);
					const emit = yield* buffer
						.Emit(observation("canonical"))
						.pipe(Effect.forkChild);
					yield* Deferred.await(hook_started);
					const finish = yield* buffer.Finish("completed").pipe(Effect.forkChild);
					yield* Deferred.succeed(allow_hook, undefined);
					yield* Effect.all([Fiber.join(emit), Fiber.join(finish)], {
						concurrency: "unbounded",
					});
					return {
						events: yield* Fiber.join(events_fiber),
						seen: yield* Ref.get(seen),
					};
				}),
			),
		);

		expect(result.seen).toMatchObject({
			observation_id: "canonical:sequence:1",
			sequence: 1,
		});
		expect(result.events.map((event) => event.sequence)).toEqual([1, 2]);
		expect(result.events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("finalizes a blocked producer when the run closes without a consumer", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						backpressure_timeout_ms: 1_000,
						capacity: 1,
						CloseResource: Effect.void,
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
					const blocked = yield* buffer
						.Emit(observation("second"))
						.pipe(Effect.exit, Effect.forkChild);
					yield* Effect.sleep(10);
					yield* buffer.Finish("cancelled");
					return {
						blocked: yield* Fiber.join(blocked),
						events: yield* buffer.Events.pipe(Stream.runCollect),
					};
				}),
			),
		);
		expect(Exit.isFailure(result.blocked)).toBe(true);
		expect(result.events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "cancelled" });
	});

	it("persists a causal diagnostic before failing sustained overload", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						backpressure_timeout_ms: 10,
						capacity: 1,
						CloseResource: Effect.void,
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
					const overflow = yield* buffer.Emit(observation("second")).pipe(Effect.exit);

					return { events: yield* buffer.Events.pipe(Stream.runCollect), overflow };
				}),
			),
		);
		expect(Exit.isFailure(result.overflow)).toBe(true);
		expect(result.events.slice(-2)).toMatchObject([
			{
				_tag: "process_diagnostic",
				message: "Engine event buffer remained full and the run was stopped.",
			},
			{
				_tag: "run_terminal",
				error_ref: {
					artisan_code: "AE-RUN-301",
					detail: "The engine could not deliver observations fast enough to continue safely.",
				},
				state: "failed",
			},
		]);
	});

	it("attaches a generic safe explanation to every otherwise-bare failed finish", async () => {
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const buffer = yield* MakeEngineEventBuffer({
						artisan_run_id: "run",
						capacity: 1,
						CloseResource: Effect.void,
						make_terminal_observation: (state, sequence) => ({
							_tag: "run_terminal",
							artisan_run_id: "run",
							observation_id: "terminal",
							raw: { engine_id: "test", frame: state, transport: "test" },
							sequence,
							state,
						}),
					});
					yield* buffer.Finish("failed");
					return yield* buffer.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect(events).toMatchObject([
			{
				_tag: "run_terminal",
				error_ref: {
					artisan_code: "AE-RUN-301",
					detail: "The engine stopped before the run could finish.",
				},
				state: "failed",
			},
		]);
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
