import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";
import type { EventEnvelope } from "@artisan/protocol";

import {
	AgentGraphRepository,
	AgentGraphRepositoryLive,
} from "../../modules/backend/src/orchestration/agent-graph-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	OrchestrationRepository,
	OrchestrationRepositoryLive,
} from "../../modules/backend/src/persistence/orchestration-repository";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationGroups,
	OrchestrationRawObservations,
	OrchestrationRuns,
	RunUsageSamples,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-16T12:00:00.000Z";

type UsageKind = "graph" | "ordinary";
type UsageRuntime = ManagedRuntime.ManagedRuntime<any, any>;
type RetryProbe = {
	readonly continue_second_attempt: Deferred.Deferred<void>;
	readonly second_attempt_started: Deferred.Deferred<void>;
};

const ordinary_ids = {
	agent_id: "agent_usage_ordinary",
	engine_id: "engine_usage_ordinary",
	run_id: "run_usage_ordinary",
	thread_id: "thread_usage_ordinary",
} as const;
const graph_ids = {
	agent_id: "agent_usage_graph",
	assignment_id: "assignment_usage_graph",
	engine_id: "engine_usage_graph",
	group_id: "group_usage_graph",
	run_id: "run_usage_graph",
	thread_id: "thread_usage_graph",
} as const;

function identifiers(kind: UsageKind) {
	return kind === "ordinary" ? ordinary_ids : graph_ids;
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-orchestration-usage-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_test_database_layer(database_path: string, retry_probe?: RetryProbe) {
	const database_layer = make_database_layer({ database_path, migrations_path });

	if (!retry_probe) {
		return database_layer;
	}

	return Layer.effect(
		Database,
		Effect.gen(function* () {
			const database = yield* Database;
			let callback_count = 0;
			const Transaction: typeof database.client.transaction = (operation, config) =>
				database.client.transaction(
					(transaction) =>
						Effect.suspend(() => {
							callback_count += 1;

							if (callback_count !== 2) {
								return operation(transaction);
							}

							return Deferred.succeed(
								retry_probe.second_attempt_started,
								undefined,
							).pipe(
								Effect.andThen(Deferred.await(retry_probe.continue_second_attempt)),
								Effect.andThen(operation(transaction)),
							);
						}),
					config,
				);
			const client = new Proxy(database.client, {
				get: (target, property, receiver) =>
					property === "transaction"
						? Transaction
						: Reflect.get(target, property, receiver),
			});

			return { client };
		}),
	).pipe(Layer.provide(database_layer));
}

function make_infrastructure(database_path: string, retry_probe?: RetryProbe) {
	return Layer.mergeAll(
		make_test_database_layer(database_path, retry_probe),
		JournalNotifierLive,
		RuntimeMetadataLive,
		NodeCrypto.layer,
	);
}

function make_usage_runtime(
	kind: UsageKind,
	database_path: string,
	retry_probe?: RetryProbe,
): UsageRuntime {
	const infrastructure = make_infrastructure(database_path, retry_probe);

	return kind === "ordinary"
		? ManagedRuntime.make(OrchestrationRepositoryLive.pipe(Layer.provideMerge(infrastructure)))
		: ManagedRuntime.make(AgentGraphRepositoryLive.pipe(Layer.provideMerge(infrastructure)));
}

const SeedOrdinaryUsage = Effect.gen(function* () {
	const database = yield* Database;
	const ids = ordinary_ids;

	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: ids.thread_id,
		title: "Ordinary usage",
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationCoordinators).values({
		active_run_id: ids.run_id,
		agent_id: ids.agent_id,
		created_at: now,
		display_name: "Usage coordinator",
		engine_id: ids.engine_id,
		role: "coordinator",
		thread_id: ids.thread_id,
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: ids.agent_id,
		created_at: now,
		engine_id: ids.engine_id,
		run_id: ids.run_id,
		status: "running",
		thread_id: ids.thread_id,
		updated_at: now,
		working_directory: "C:/usage",
	});
});

const SeedGraphUsage = Effect.gen(function* () {
	const database = yield* Database;
	const ids = graph_ids;

	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: ids.thread_id,
		title: "Graph usage",
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationGroups).values({
		coordinator_agent_id: ids.agent_id,
		created_at: now,
		group_id: ids.group_id,
		journal_sequence: 0,
		max_concurrency: 1,
		state: "running",
		thread_id: ids.thread_id,
		updated_at: now,
		version: 1,
	});
	yield* database.client.insert(AgentInstances).values({
		agent_id: ids.agent_id,
		created_at: now,
		display_name: "Usage agent",
		group_id: ids.group_id,
		role: "tester",
		updated_at: now,
	});
	yield* database.client.insert(Assignments).values({
		active_run_id: ids.run_id,
		agent_id: ids.agent_id,
		assignment_id: ids.assignment_id,
		created_at: now,
		current_attempt: 1,
		engine_id: ids.engine_id,
		expected_result: "Usage result",
		group_id: ids.group_id,
		instructions: "Measure usage",
		max_attempts: 1,
		parent_node_id: ids.group_id,
		permission_policy_json: JSON.stringify({
			approval: "never",
			network_access: false,
			write_access: false,
		}),
		profile: "default",
		role: "tester",
		scope_json: JSON.stringify({ kind: "test", value: "usage", write_access: false }),
		state: "running",
		summary_contract: "Return usage",
		updated_at: now,
		workspace_json: JSON.stringify({
			isolation: "isolated",
			workspace_id: "workspace_usage_graph",
			working_directory: "C:/usage",
		}),
	});
	yield* database.client.insert(AgentRuns).values({
		agent_id: ids.agent_id,
		assignment_id: ids.assignment_id,
		attempt: 1,
		created_at: now,
		dispatch_status: "active",
		engine_id: ids.engine_id,
		group_id: ids.group_id,
		last_observation_sequence: 0,
		profile: "default",
		run_id: ids.run_id,
		state: "running",
		updated_at: now,
	});
});

const SetBusyTimeout = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.run("PRAGMA busy_timeout = 0");
});

function usage_observation(
	kind: UsageKind,
	observation_id: string,
	input_tokens: number | undefined,
	output_tokens: number | undefined,
	sample_scope: "run_total" | "turn_total",
	turn_id?: string,
): EngineObservation {
	const ids = identifiers(kind);

	return {
		_tag: "usage",
		artisan_run_id: ids.run_id,
		...(input_tokens === undefined ? {} : { input_tokens }),
		observation_id,
		...(output_tokens === undefined ? {} : { output_tokens }),
		raw: {
			engine_id: ids.engine_id,
			frame: { input_tokens, output_tokens, sample_scope, turn_id },
			native_id: `native:${observation_id}`,
			transport: "test",
		},
		sample_scope,
		sequence: 1,
		...(turn_id ? { turn_id } : {}),
	};
}

function record_usage(
	kind: UsageKind,
	runtime: UsageRuntime,
	observation: EngineObservation,
): Promise<ReadonlyArray<EventEnvelope>> {
	return kind === "ordinary"
		? runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;

					return yield* repository.RecordObservation(observation);
				}),
			)
		: runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* AgentGraphRepository;

					return yield* repository.RecordObservation(observation);
				}),
			);
}

async function seed_usage(kind: UsageKind, runtime: UsageRuntime) {
	await runtime.runPromise(kind === "ordinary" ? SeedOrdinaryUsage : SeedGraphUsage);
}

async function project_usage(kind: UsageKind, runtime: UsageRuntime) {
	if (kind === "ordinary") {
		const work = await runtime.runPromise(
			Effect.gen(function* () {
				const repository = yield* OrchestrationRepository;

				return yield* repository.GetWork(ordinary_ids.thread_id);
			}),
		);

		return work?.usage;
	}

	const graph = await runtime.runPromise(
		Effect.gen(function* () {
			const repository = yield* AgentGraphRepository;

			return yield* repository.GetGraph(graph_ids.group_id);
		}),
	);

	return graph.agent_runs.find((run) => run.run_id === graph_ids.run_id)?.usage;
}

async function read_usage_state(kind: UsageKind, runtime: UsageRuntime) {
	const ids = identifiers(kind);
	const state = await runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;

			return yield* Effect.all({
				events: database.client.select().from(JournalEvents),
				observations: database.client.select().from(OrchestrationRawObservations),
				samples: database.client.select().from(RunUsageSamples),
			});
		}),
	);

	return {
		events: state.events.filter(
			(event) => event.run_id === ids.run_id && event.event_type === "run.usage.updated",
		),
		observations: state.observations.filter((observation) => observation.run_id === ids.run_id),
		samples: state.samples.filter((sample) => sample.run_id === ids.run_id),
	};
}

function turn_aggregate(
	samples: ReadonlyArray<{
		readonly input_tokens: number;
		readonly output_tokens: number;
		readonly sample_scope: string;
	}>,
) {
	return samples
		.filter((sample) => sample.sample_scope === "turn_total")
		.reduce(
			(total, sample) => ({
				input_tokens: total.input_tokens + sample.input_tokens,
				output_tokens: total.output_tokens + sample.output_tokens,
			}),
			{ input_tokens: 0, output_tokens: 0 },
		);
}

async function make_retry_probe(): Promise<RetryProbe> {
	return {
		continue_second_attempt: await Effect.runPromise(Deferred.make<void>()),
		second_attempt_started: await Effect.runPromise(Deferred.make<void>()),
	};
}

function within_timeout<A>(promise: Promise<A>) {
	return Promise.race([
		promise,
		new Promise<never>((_resolve, reject) =>
			setTimeout(() => reject(new Error("Usage operation timed out")), 5_000),
		),
	]);
}

async function hold_sqlite_write_lock(runtime: UsageRuntime) {
	const acquired = await Effect.runPromise(Deferred.make<void>());
	const released = await Effect.runPromise(Deferred.make<void>());
	let lock_released = false;
	const held = runtime.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const database = yield* Database;
				let committed = false;

				yield* Effect.addFinalizer(() =>
					committed ? Effect.void : database.client.run("ROLLBACK").pipe(Effect.ignore),
				);
				yield* database.client.run("PRAGMA busy_timeout = 0");
				yield* database.client.run("BEGIN IMMEDIATE");
				yield* Deferred.succeed(acquired, undefined);
				yield* Deferred.await(released);
				yield* database.client.run("COMMIT");

				committed = true;
			}),
		),
	);

	await within_timeout(Effect.runPromise(Deferred.await(acquired)));

	return {
		Release: async () => {
			if (!lock_released) {
				lock_released = true;
				await Effect.runPromise(Deferred.succeed(released, undefined));
			}

			await held;
		},
	};
}

async function continue_second_attempt(retry_probe: RetryProbe) {
	await Effect.runPromise(Deferred.succeed(retry_probe.continue_second_attempt, undefined));
}

async function run_retry_race(
	kind: UsageKind,
	first_observation: EngineObservation,
	second_observation: EngineObservation,
) {
	const database_path = await make_database_path();
	const retry_probe = await make_retry_probe();
	const first_runtime = make_usage_runtime(kind, database_path);
	const second_runtime = make_usage_runtime(kind, database_path, retry_probe);
	let lock: Awaited<ReturnType<typeof hold_sqlite_write_lock>> | undefined;

	try {
		await seed_usage(kind, first_runtime);
		await Promise.all([
			first_runtime.runPromise(SetBusyTimeout),
			second_runtime.runPromise(SetBusyTimeout),
		]);
		lock = await hold_sqlite_write_lock(first_runtime);
		const second_write = record_usage(kind, second_runtime, second_observation).then(
			(value) => ({ status: "success" as const, value }),
			(error) => ({ error, status: "failure" as const }),
		);

		await within_timeout(Effect.runPromise(Deferred.await(retry_probe.second_attempt_started)));
		await lock.Release();
		lock = undefined;

		const first_events = await record_usage(kind, first_runtime, first_observation);

		await continue_second_attempt(retry_probe);

		return {
			first_events,
			projection: await project_usage(kind, first_runtime),
			second_write: await within_timeout(second_write),
			state: await read_usage_state(kind, first_runtime),
		};
	} finally {
		await lock?.Release();
		await continue_second_attempt(retry_probe);
		await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
	}
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

for (const kind of ["ordinary", "graph"] as const) {
	describe(`${kind} durable usage`, () => {
		it("retains incomplete observations without projecting false zero counters", async () => {
			const database_path = await make_database_path();
			const runtime = make_usage_runtime(kind, database_path);
			const ids = identifiers(kind);

			try {
				await seed_usage(kind, runtime);

				const input_only = await record_usage(
					kind,
					runtime,
					usage_observation(
						kind,
						`${kind}_input_only`,
						5,
						undefined,
						"turn_total",
						"partial_a",
					),
				);
				const output_only = await record_usage(
					kind,
					runtime,
					usage_observation(
						kind,
						`${kind}_output_only`,
						undefined,
						7,
						"turn_total",
						"partial_b",
					),
				);
				const partial_state = await read_usage_state(kind, runtime);

				expect(input_only).toEqual([]);
				expect(output_only).toEqual([]);
				expect(partial_state.samples).toEqual([]);
				expect(partial_state.events).toEqual([]);
				expect(partial_state.observations).toHaveLength(2);
				expect(await project_usage(kind, runtime)).toBeUndefined();

				const first = usage_observation(
					kind,
					`${kind}_turn_a_first`,
					5,
					4,
					"turn_total",
					"turn_a",
				);
				const first_events = await record_usage(kind, runtime, first);
				const duplicate = await record_usage(kind, runtime, first);
				await record_usage(
					kind,
					runtime,
					usage_observation(
						kind,
						`${kind}_turn_a_component_update`,
						3,
						9,
						"turn_total",
						"turn_a",
					),
				);
				const stale = await record_usage(
					kind,
					runtime,
					usage_observation(kind, `${kind}_turn_a_stale`, 4, 8, "turn_total", "turn_a"),
				);
				await record_usage(
					kind,
					runtime,
					usage_observation(kind, `${kind}_turn_b`, 7, 8, "turn_total", "turn_b"),
				);
				const overflow = await record_usage(
					kind,
					runtime,
					usage_observation(
						kind,
						`${kind}_turn_b_overflow`,
						Number.MAX_SAFE_INTEGER,
						8,
						"turn_total",
						"turn_b",
					),
				);
				await record_usage(
					kind,
					runtime,
					usage_observation(kind, `${kind}_run_total`, 100, 200, "run_total"),
				);

				const state = await read_usage_state(kind, runtime);

				expect(first_events[0]?.raw_origin).toMatchObject({
					provider: expect.stringMatching(/^engine:[0-9a-f]{64}$/),
					reference: expect.stringMatching(/^engine_observation:[0-9a-f]{64}$/),
				});
				expect(JSON.stringify(first_events[0]?.raw_origin)).not.toContain(
					`native:${kind}_turn_a_first`,
				);
				expect(duplicate).toEqual([]);
				expect(stale).toEqual([]);
				expect(overflow).toEqual([]);
				expect(state.samples).toHaveLength(3);
				expect(state.samples).toEqual(
					expect.arrayContaining([
						expect.objectContaining({
							input_tokens: 5,
							output_tokens: 9,
							sample_scope: "turn_total",
							scope_key: "turn:turn_a",
						}),
						expect.objectContaining({
							input_tokens: 7,
							output_tokens: 8,
							sample_scope: "turn_total",
							scope_key: "turn:turn_b",
						}),
						expect.objectContaining({
							input_tokens: 100,
							output_tokens: 200,
							sample_scope: "run_total",
							scope_key: "run",
						}),
					]),
				);
				expect(await project_usage(kind, runtime)).toEqual({
					input_tokens: 100,
					output_tokens: 200,
				});
				expect(state.events).toHaveLength(4);
				expect(
					state.observations.some(
						({ observation_id }) => observation_id === `${kind}_input_only`,
					),
				).toBe(true);
				expect(
					state.observations.some(
						({ observation_id }) => observation_id === `${kind}_output_only`,
					),
				).toBe(true);
				expect(state.observations.every(({ run_id }) => run_id === ids.run_id)).toBe(true);
			} finally {
				await runtime.dispose();
			}

			const restarted = make_usage_runtime(kind, database_path);

			try {
				expect(await project_usage(kind, restarted)).toEqual({
					input_tokens: 100,
					output_tokens: 200,
				});
			} finally {
				await restarted.dispose();
			}
		});

		it("retries a different-turn writer race across two runtimes", async () => {
			const result = await run_retry_race(
				kind,
				usage_observation(kind, `${kind}_race_turn_a`, 11, 13, "turn_total", "race_a"),
				usage_observation(kind, `${kind}_race_turn_b`, 17, 19, "turn_total", "race_b"),
			);

			expect(result.first_events).toHaveLength(1);
			expect(result.second_write).toMatchObject({ status: "success" });
			if (result.second_write.status === "success") {
				expect(result.second_write.value).toHaveLength(1);
			}
			expect(result.state.samples).toHaveLength(2);
			expect(turn_aggregate(result.state.samples)).toEqual({
				input_tokens: 28,
				output_tokens: 32,
			});
			expect(result.state.observations).toHaveLength(2);
			expect(result.state.events).toHaveLength(2);
			expect(result.projection).toEqual({ input_tokens: 28, output_tokens: 32 });
		});

		it("rejects an overflow race without corrupting the committed aggregate", async () => {
			const accepted_input = Number.MAX_SAFE_INTEGER - 5;
			const result = await run_retry_race(
				kind,
				usage_observation(
					kind,
					`${kind}_overflow_race_accepted`,
					accepted_input,
					2,
					"turn_total",
					"overflow_a",
				),
				usage_observation(
					kind,
					`${kind}_overflow_race_rejected`,
					10,
					3,
					"turn_total",
					"overflow_b",
				),
			);

			expect(result.first_events).toHaveLength(1);
			expect(result.second_write).toEqual({ status: "success", value: [] });
			expect(result.state.samples).toEqual([
				expect.objectContaining({
					input_tokens: accepted_input,
					output_tokens: 2,
					scope_key: "turn:overflow_a",
				}),
			]);
			expect(Number.isSafeInteger(turn_aggregate(result.state.samples).input_tokens)).toBe(
				true,
			);
			expect(result.state.observations).toHaveLength(2);
			expect(result.state.events).toHaveLength(1);
			expect(result.projection).toEqual({
				input_tokens: accepted_input,
				output_tokens: 2,
			});
		});
	});
}
