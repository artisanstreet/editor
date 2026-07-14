import { Deferred, Effect, Fiber, Layer, ManagedRuntime, Option } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import { type Engine, EngineRegistry, EngineRegistryError } from "@artisan/engines";

import {
	ExternalWaitDispatcher,
	ExternalWaitDispatcherLive,
	ExternalWaitDispatchScheduler,
	ExternalWaitDispatchSchedulerLive,
} from "../../modules/backend/src/external-wait/external-wait-dispatcher";
import {
	ExternalWaitRepository,
	ExternalWaitInvariant,
	type ExternalWaitRepositoryError,
	type ExternalWaitMaterialization,
	type ExternalWaitWakeClaim,
} from "../../modules/backend/src/external-wait/external-wait-repository";
import { AgentGraphOrchestrator } from "../../modules/backend/src/orchestration/agent-graph-orchestrator";
import { AgentOrchestrator } from "../../modules/backend/src/orchestration/agent-orchestrator";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

interface DispatcherState {
	active_schedules: number;
	claims: Map<string, ExternalWaitWakeClaim>;
	claim_inputs: Array<string>;
	discovered_wakes: ReadonlyArray<{ outbox_id: string; thread_id: string }>;
	graph_notifications: number;
	materialize?: (
		input: Parameters<(typeof ExternalWaitRepository.Service)["MaterializeWake"]>[0],
	) => Effect.Effect<ExternalWaitMaterialization, ExternalWaitRepositoryError>;
	materialize_inputs: Array<
		Parameters<(typeof ExternalWaitRepository.Service)["MaterializeWake"]>[0]
	>;
	notifications: number;
	released_outbox_ids: Array<string>;
	scheduled: number;
}

function make_claim(engine_id: string, outbox_id: string): ExternalWaitWakeClaim {
	return {
		outbox_id,
		owner: {
			_tag: "thread_run",
			agent_id: "agent_1",
			engine_id,
			run_id: "run_1",
		},
	} as ExternalWaitWakeClaim;
}

function make_discovery(outbox_id: string, thread_id: string) {
	return { outbox_id, thread_id };
}

function make_engine(engine_id: string, resume_supported: boolean): Engine {
	return {
		Descriptor: {
			capabilities: {
				resume: { state: resume_supported ? "supported" : "unsupported" },
			},
			id: engine_id,
		},
	} as Engine;
}

function make_runtime(state: DispatcherState, engines: ReadonlyArray<Engine> = []) {
	const repository = {
		ClaimWake: (input) =>
			Effect.sync(() => {
				state.claim_inputs.push(input.outbox_id);
				const claim = state.claims.get(input.outbox_id);
				state.claims.delete(input.outbox_id);

				return claim === undefined ? Option.none() : Option.some(claim);
			}),
		DiscoverWakes: () => Effect.succeed(state.discovered_wakes),
		MaterializeWake: (input) => {
			state.materialize_inputs.push(input);

			return (
				state.materialize?.(input) ??
				Effect.succeed({ status: "created" } as ExternalWaitMaterialization)
			);
		},
		ReleaseWake: (input) =>
			Effect.sync(() => {
				state.released_outbox_ids.push(input.outbox_id);

				return Option.none();
			}),
	} satisfies Pick<
		typeof ExternalWaitRepository.Service,
		"ClaimWake" | "DiscoverWakes" | "MaterializeWake" | "ReleaseWake"
	>;
	const engine_registry = Layer.succeed(EngineRegistry, {
		Get: (engine_id) => {
			const engine = engines.find(({ Descriptor }) => Descriptor.id === engine_id);

			return engine === undefined
				? Effect.fail(new EngineRegistryError({ engine_id, reason: "not_found" }))
				: Effect.succeed(engine);
		},
		List: Effect.succeed(engines),
	});
	const orchestrator = Layer.succeed(AgentOrchestrator, {
		Handle: () => Effect.die("unused"),
		NotifyWorkAvailable: Effect.sync(() => {
			state.notifications += 1;
		}),
		QuiesceThread: () => Effect.void,
		Recover: Effect.void,
	});
	const graph_orchestrator = Layer.succeed(AgentGraphOrchestrator, {
		GetGraph: () => Effect.die("unused"),
		Handle: () => Effect.die("unused"),
		NotifyWorkAvailable: Effect.sync(() => {
			state.graph_notifications += 1;
		}),
		QuiesceThread: () => Effect.void,
		Recover: Effect.void,
	});
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id: "external_wait_dispatcher_test",
		MakeId: () => Effect.succeed("unused"),
		Now: Effect.succeed("2026-07-14T20:00:00.000Z"),
	});
	const scheduler = Layer.succeed(ExternalWaitDispatchScheduler, {
		Schedule: () =>
			Effect.acquireRelease(
				Effect.sync(() => {
					state.active_schedules += 1;
					state.scheduled += 1;
				}),
				() =>
					Effect.sync(() => {
						state.active_schedules -= 1;
					}),
			).pipe(Effect.andThen(Effect.never)),
	});
	const layer = ExternalWaitDispatcherLive.pipe(
		Layer.provideMerge(
			Layer.succeed(
				ExternalWaitRepository,
				repository as unknown as typeof ExternalWaitRepository.Service,
			),
		),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(orchestrator),
		Layer.provideMerge(graph_orchestrator),
		Layer.provideMerge(metadata),
		Layer.provideMerge(scheduler),
	);

	return ManagedRuntime.make(layer);
}

function make_state(): DispatcherState {
	return {
		active_schedules: 0,
		claims: new Map(),
		claim_inputs: [],
		discovered_wakes: [],
		graph_notifications: 0,
		materialize_inputs: [],
		notifications: 0,
		released_outbox_ids: [],
		scheduled: 0,
	};
}

const RunOnce = Effect.flatMap(ExternalWaitDispatcher, (dispatcher) => dispatcher.RunOnce);
const QuiesceThread = (thread_id: string) =>
	Effect.flatMap(ExternalWaitDispatcher, (dispatcher) => dispatcher.QuiesceThread(thread_id));
const StartDispatcher = Effect.flatMap(ExternalWaitDispatcher, () => Effect.void);

describe("ExternalWaitDispatcher", () => {
	it("waits one interval before the first scheduled background cycle", async () => {
		let cycles = 0;
		const program = Effect.scoped(
			Effect.gen(function* () {
				const scheduler = yield* ExternalWaitDispatchScheduler;

				yield* Effect.forkScoped(
					scheduler.Schedule(
						Effect.sync(() => {
							cycles += 1;
						}),
					),
				);
				yield* Effect.yieldNow;

				expect(cycles).toBe(0);

				yield* TestClock.adjust("1 second");
				yield* Effect.yieldNow;

				expect(cycles).toBe(1);
			}),
		).pipe(
			Effect.provide(ExternalWaitDispatchSchedulerLive),
			Effect.provide(TestClock.layer()),
		);

		await Effect.runPromise(program);
	});

	it("materializes a claimed wake using its exact engine resume capability", async () => {
		const state = make_state();
		const runtime = make_runtime(state, [make_engine("engine_1", true)]);

		try {
			await runtime.runPromise(StartDispatcher);
			state.claims.set("outbox_1", make_claim("engine_1", "outbox_1"));
			state.discovered_wakes = [make_discovery("outbox_1", "thread_1")];

			const result = await runtime.runPromise(RunOnce);

			expect(result).toEqual({
				materialized_outbox_ids: ["outbox_1"],
				released_or_skipped_outbox_ids: [],
			});
			expect(state.materialize_inputs).toEqual([
				{
					lease_owner: "external_wait_dispatcher_test",
					native_resume_supported: true,
					now: "2026-07-14T20:00:00.000Z",
					outbox_id: "outbox_1",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("releases a claimed wake when its owner engine is unavailable", async () => {
		const state = make_state();
		const runtime = make_runtime(state);

		try {
			await runtime.runPromise(StartDispatcher);
			state.claims.set("outbox_1", make_claim("removed_engine", "outbox_1"));
			state.discovered_wakes = [make_discovery("outbox_1", "thread_1")];

			const result = await runtime.runPromise(RunOnce);

			expect(result.released_or_skipped_outbox_ids).toEqual(["outbox_1"]);
			expect(state.released_outbox_ids).toEqual(["outbox_1"]);
			expect(state.materialize_inputs).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("notifies both dispatchers for empty and failed cycles", async () => {
		const state = make_state();
		const runtime = make_runtime(state, [make_engine("engine_1", false)]);

		try {
			await runtime.runPromise(StartDispatcher);
			expect(state.notifications).toBe(1);
			expect(state.graph_notifications).toBe(1);

			state.claims.set("outbox_1", make_claim("engine_1", "outbox_1"));
			state.claims.set("outbox_2", make_claim("engine_1", "outbox_2"));
			state.discovered_wakes = [
				make_discovery("outbox_1", "thread_1"),
				make_discovery("outbox_2", "thread_1"),
			];
			state.materialize = (input) =>
				input.outbox_id === "outbox_1"
					? Effect.fail(new ExternalWaitInvariant({ message: "materialization failed" }))
					: Effect.succeed({ status: "created" } as ExternalWaitMaterialization);

			const exit = await runtime.runPromiseExit(RunOnce);

			expect(exit._tag).toBe("Failure");
			expect(state.notifications).toBe(2);
			expect(state.graph_notifications).toBe(2);
			expect(state.materialize_inputs.map((input) => input.outbox_id)).toEqual([
				"outbox_1",
				"outbox_2",
			]);
			expect(state.released_outbox_ids).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("starts through a corrupt wake and dispatches later valid work", async () => {
		const state = make_state();
		state.claims.set("corrupt_outbox", make_claim("engine_1", "corrupt_outbox"));
		state.discovered_wakes = [make_discovery("corrupt_outbox", "thread_1")];
		state.materialize = () =>
			Effect.fail(new ExternalWaitInvariant({ message: "materialization failed" }));
		const runtime = make_runtime(state, [make_engine("engine_1", true)]);

		try {
			await runtime.runPromise(StartDispatcher);

			expect(state.scheduled).toBe(1);
			expect(state.notifications).toBe(1);
			expect(state.graph_notifications).toBe(1);

			state.claims.set("valid_outbox", make_claim("engine_1", "valid_outbox"));
			state.discovered_wakes = [make_discovery("valid_outbox", "thread_1")];
			state.materialize = () =>
				Effect.succeed({ status: "created" } as ExternalWaitMaterialization);

			const result = await runtime.runPromise(RunOnce);

			expect(result.materialized_outbox_ids).toEqual(["valid_outbox"]);
			expect(state.materialize_inputs.map((input) => input.outbox_id)).toEqual([
				"corrupt_outbox",
				"valid_outbox",
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes overlapping manual cycles", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const state = make_state();
		let active_materializations = 0;
		let maximum_materializations = 0;
		const runtime = make_runtime(state, [make_engine("engine_1", false)]);

		try {
			await runtime.runPromise(StartDispatcher);
			state.claims.set("outbox_1", make_claim("engine_1", "outbox_1"));
			state.discovered_wakes = [make_discovery("outbox_1", "thread_1")];
			state.materialize = () =>
				Effect.sync(() => {
					active_materializations += 1;
					maximum_materializations = Math.max(
						maximum_materializations,
						active_materializations,
					);
				}).pipe(
					Effect.andThen(Deferred.succeed(started, undefined)),
					Effect.andThen(Deferred.await(release)),
					Effect.ensuring(
						Effect.sync(() => {
							active_materializations -= 1;
						}),
					),
					Effect.as({ status: "created" } as ExternalWaitMaterialization),
				);

			const first = runtime.runPromise(RunOnce);

			await Effect.runPromise(Deferred.await(started));
			state.claims.set("outbox_1", make_claim("engine_1", "outbox_1"));

			const second = runtime.runPromise(RunOnce);

			await Effect.runPromise(Deferred.succeed(release, undefined));
			await Promise.all([first, second]);

			expect(state.materialize_inputs).toHaveLength(2);
			expect(maximum_materializations).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("waits for an admitted materialization before quiescing its thread", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const state = make_state();
		const events: Array<string> = [];
		const runtime = make_runtime(state, [make_engine("engine_1", true)]);

		try {
			await runtime.runPromise(StartDispatcher);
			state.claims.set("outbox_1", make_claim("engine_1", "outbox_1"));
			state.discovered_wakes = [make_discovery("outbox_1", "thread_1")];
			state.materialize = () =>
				Effect.sync(() => {
					events.push("materialization_started");
				}).pipe(
					Effect.andThen(Deferred.succeed(started, undefined)),
					Effect.andThen(Deferred.await(release)),
					Effect.andThen(
						Effect.sync(() => {
							events.push("materialization_finished");
						}),
					),
					Effect.as({ status: "created" } as ExternalWaitMaterialization),
				);

			await runtime.runPromise(
				Effect.gen(function* () {
					const dispatch = yield* Effect.forkChild(RunOnce, { startImmediately: true });
					yield* Deferred.await(started);

					const quiesce = yield* Effect.forkChild(
						QuiesceThread("thread_1").pipe(
							Effect.andThen(
								Effect.sync(() => {
									events.push("quiesced");
								}),
							),
						),
						{ startImmediately: true },
					);
					yield* Effect.yieldNow;

					yield* Deferred.succeed(release, undefined);
					yield* Fiber.join(dispatch);
					yield* Fiber.join(quiesce);
				}),
			);

			expect(state.materialize_inputs).toHaveLength(1);
			expect(events).toEqual([
				"materialization_started",
				"materialization_finished",
				"quiesced",
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("skips a later discovered wake after its thread is quiesced", async () => {
		const state = make_state();
		const runtime = make_runtime(state, [make_engine("engine_1", true)]);

		try {
			await runtime.runPromise(StartDispatcher);
			await runtime.runPromise(QuiesceThread("thread_1"));

			state.claims.set("outbox_1", make_claim("engine_1", "outbox_1"));
			state.discovered_wakes = [make_discovery("outbox_1", "thread_1")];

			const result = await runtime.runPromise(RunOnce);

			expect(result).toEqual({
				materialized_outbox_ids: [],
				released_or_skipped_outbox_ids: ["outbox_1"],
			});
			expect(state.claim_inputs).toEqual([]);
			expect(state.materialize_inputs).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("dispatches a different thread while one thread is quiesced", async () => {
		const state = make_state();
		const runtime = make_runtime(state, [make_engine("engine_1", true)]);

		try {
			await runtime.runPromise(StartDispatcher);
			await runtime.runPromise(QuiesceThread("thread_1"));

			state.claims.set("outbox_2", make_claim("engine_1", "outbox_2"));
			state.discovered_wakes = [make_discovery("outbox_2", "thread_2")];

			const result = await runtime.runPromise(RunOnce);

			expect(result.materialized_outbox_ids).toEqual(["outbox_2"]);
			expect(state.claim_inputs).toEqual(["outbox_2"]);
			expect(state.materialize_inputs.map((input) => input.outbox_id)).toEqual(["outbox_2"]);
		} finally {
			await runtime.dispose();
		}
	});

	it("starts one scoped loop and releases it when the runtime closes", async () => {
		const state = make_state();
		const runtime = make_runtime(state);

		await runtime.runPromise(StartDispatcher);

		expect(state.scheduled).toBe(1);
		expect(state.active_schedules).toBe(1);

		await runtime.dispose();

		expect(state.active_schedules).toBe(0);
	});
});
