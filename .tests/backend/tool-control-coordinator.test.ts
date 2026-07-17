import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import {
	Clock,
	Deferred,
	Duration,
	Effect,
	Fiber,
	FileSystem,
	Layer,
	ManagedRuntime,
	Queue,
} from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ToolControlCoordinator,
	ToolControlCoordinatorLive,
} from "../../modules/backend/src/tool-control/tool-control-coordinator";
import { ToolControlRepositoryLive } from "../../modules/backend/src/tool-control/tool-control-repository";
import { ToolExecutionRepositoryLive } from "../../modules/backend/src/tool-control/tool-execution-repository";
import { type EffectToolAdapterError } from "../../modules/backend/src/tool-control/internal/effect-tool-adapter";
import {
	type ToolRegistration,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalNotifier,
	JournalNotifierLive,
} from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	OrchestrationGroups,
	OrchestrationRuns,
	Projects,
	Threads,
	ToolInvocations,
	ToolThreadDispatchState,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const clock = { value: "2026-07-16T12:00:00.000Z" };

interface ToolState {
	calls: number;
	entered?: ReadonlyMap<string, Deferred.Deferred<void>>;
	eligibility_calls?: number;
	fail: boolean;
	gates?: ReadonlyMap<string, Deferred.Deferred<void>>;
	query_entered?: ReadonlyMap<string, Deferred.Deferred<void>>;
	query_gates?: ReadonlyMap<string, Deferred.Deferred<void>>;
}

interface InitialNowGate {
	readonly entered: Deferred.Deferred<void>;
	readonly release: Deferred.Deferred<void>;
}

interface ControlledSleep {
	readonly duration_millis: number;
	readonly release: Deferred.Deferred<void>;
}

const MakeControlledClock = Effect.gen(function* () {
	const sleeps = yield* Queue.unbounded<ControlledSleep>();
	const clock: Clock.Clock = {
		currentTimeMillis: Effect.succeed(0),
		currentTimeMillisUnsafe: () => 0,
		currentTimeNanos: Effect.succeed(0n),
		currentTimeNanosUnsafe: () => 0n,
		sleep: (duration) =>
			Effect.gen(function* () {
				const release = yield* Deferred.make<void>();

				yield* Queue.offer(sleeps, {
					duration_millis: Duration.toMillis(duration),
					release,
				});
				yield* Deferred.await(release);
			}),
	};

	return { clock, sleeps };
});

const descriptor = (tool_id: string, approval_policy: "automatic" | "required") => ({
	approval_policy,
	effect: "read" as const,
	input_schema: {
		properties: { query: { type: "string" } },
		required: ["query"],
		type: "object",
	},
	label: tool_id,
	revision: 1,
	source: "artisan" as const,
	summary: "Runs a bounded test tool",
	tool_id,
});

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-tool-control-coordinator-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function metadata_layer(instance_id: string, initial_now_gate?: InitialNowGate) {
	let identifier = 0;
	let initial_now_pending = initial_now_gate !== undefined;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++identifier}`),
		Now: Effect.suspend(() => {
			if (!initial_now_pending || initial_now_gate === undefined) {
				return Effect.sync(() => clock.value);
			}

			initial_now_pending = false;

			return Deferred.succeed(initial_now_gate.entered, undefined).pipe(
				Effect.andThen(Deferred.await(initial_now_gate.release)),
				Effect.as(clock.value),
			);
		}),
	});
}

function registration(
	tool_id: string,
	approval_policy: "automatic" | "required",
	state: ToolState,
	recovery_policy: ToolRegistration["recovery_policy"],
): ToolRegistration {
	const current_descriptor = descriptor(tool_id, approval_policy);

	return {
		adapter: {
			input_schema: current_descriptor.input_schema,
			Invoke: (context, arguments_) =>
				Effect.gen(function* () {
					state.calls += 1;
					const entered = state.entered?.get(context.thread_id);
					const gate = state.gates?.get(context.thread_id);
					const query =
						typeof arguments_ === "object" &&
						arguments_ !== null &&
						"query" in arguments_ &&
						typeof arguments_.query === "string"
							? arguments_.query
							: undefined;
					const query_entered =
						query === undefined ? undefined : state.query_entered?.get(query);
					const query_gate =
						query === undefined ? undefined : state.query_gates?.get(query);

					if (entered) {
						yield* Deferred.succeed(entered, undefined);
					}
					if (query_entered) {
						yield* Deferred.succeed(query_entered, undefined);
					}

					if (gate) {
						yield* Deferred.await(gate);
					}
					if (query_gate) {
						yield* Deferred.await(query_gate);
					}

					if (state.fail) {
						return yield* Effect.fail({
							reason_code: "execution_failed",
						} as EffectToolAdapterError);
					}

					return { call: state.calls, private_result: "secret" };
				}),
		},
		descriptor: current_descriptor,
		IsEligible: () =>
			Effect.sync(() => {
				state.eligibility_calls = (state.eligibility_calls ?? 0) + 1;
			}),
		recovery_policy,
	};
}

function make_runtime(
	database_path: string,
	instance_id: string,
	state: ToolState,
	notifier = JournalNotifierLive,
	initial_now_gate?: InitialNowGate,
	controlled_clock?: Clock.Clock,
) {
	const registry = make_tool_registry_layer([
		registration("tool.read", "automatic", state, "retry"),
		registration("tool.approval", "required", state, "outcome_unknown"),
	]);
	const infrastructure_base = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(instance_id, initial_now_gate),
		notifier,
		registry,
		NodeCrypto.layer,
	);
	const infrastructure =
		controlled_clock === undefined
			? infrastructure_base
			: Layer.merge(infrastructure_base, Layer.succeed(Clock.Clock, controlled_clock));
	const repositories = Layer.merge(ToolControlRepositoryLive, ToolExecutionRepositoryLive);
	const services = ToolControlCoordinatorLive.pipe(Layer.provideMerge(repositories));

	return ManagedRuntime.make(services.pipe(Layer.provideMerge(infrastructure)));
}

const InstallToolThreadDispatchState = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.run(`
		CREATE TABLE IF NOT EXISTS tool_thread_dispatch_state (
			thread_id text PRIMARY KEY REFERENCES threads(thread_id) ON DELETE CASCADE,
			admission_version integer NOT NULL DEFAULT 0,
			quiesced_at text,
			CONSTRAINT tool_thread_dispatch_state_admission_version_check
				CHECK (admission_version >= 0),
			CONSTRAINT tool_thread_dispatch_state_quiesced_at_check
				CHECK (
					quiesced_at IS NULL OR (
						strftime('%Y-%m-%dT%H:%M:%fZ', quiesced_at) IS quiesced_at
						AND substr(quiesced_at, 12, 2) BETWEEN '00' AND '23'
					)
				)
		)
	`);
});

const SeedThreadWithOwnership = (thread_id: string, run_id: string, agent_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* InstallToolThreadDispatchState;
		yield* database.client.run(`
		INSERT INTO threads (thread_id, title, title_source, created_at, updated_at)
		VALUES ('${thread_id}', 'Tool control', 'initial', '${clock.value}', '${clock.value}')
	`);
		yield* database.client.run(`
		INSERT INTO orchestration_runs
		(run_id, thread_id, agent_id, engine_id, status, working_directory, created_at, updated_at)
		VALUES ('${run_id}', '${thread_id}', '${agent_id}', 'test', 'running', 'C:/artisan', '${clock.value}', '${clock.value}')
	`);
	});

const SeedThread = SeedThreadWithOwnership("thread_1", "run_1", "agent_1");

const SeedEligibilityOwnership = Effect.gen(function* () {
	const database = yield* Database;

	yield* SeedThread;
	yield* database.client.insert(Threads).values({
		created_at: clock.value,
		thread_id: "thread_2",
		title: "Other thread",
		title_source: "initial",
		updated_at: clock.value,
	});
	yield* database.client.insert(Projects).values({
		canonical_root: "C:/artisan",
		display_name: "Artisan",
		project_id: "project_1",
		registered_at: clock.value,
		updated_at: clock.value,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_inactive",
		created_at: clock.value,
		engine_id: "test",
		run_id: "run_inactive",
		status: "completed",
		thread_id: "thread_1",
		updated_at: clock.value,
		working_directory: "C:/artisan",
	});
	yield* database.client.insert(OrchestrationGroups).values({
		coordinator_agent_id: "coordinator_1",
		created_at: clock.value,
		group_id: "group_1",
		journal_sequence: 1,
		max_concurrency: 1,
		state: "running",
		thread_id: "thread_1",
		updated_at: clock.value,
		version: 1,
	});
	yield* database.client.insert(AgentInstances).values({
		agent_id: "graph_agent_1",
		created_at: clock.value,
		display_name: "Graph agent",
		group_id: "group_1",
		role: "worker",
		updated_at: clock.value,
	});
	yield* database.client.insert(Assignments).values({
		active_run_id: "graph_run_1",
		agent_id: "graph_agent_1",
		assignment_id: "assignment_1",
		created_at: clock.value,
		current_attempt: 1,
		engine_id: "test",
		expected_result: "result",
		group_id: "group_1",
		instructions: "instructions",
		max_attempts: 1,
		parent_node_id: "node_1",
		permission_policy_json: JSON.stringify({
			approval: "on_request",
			network_access: false,
			write_access: true,
		}),
		profile: "default",
		role: "worker",
		scope_json: JSON.stringify({ kind: "files", value: "src", write_access: true }),
		state: "running",
		summary_contract: "summary",
		updated_at: clock.value,
		workspace_json: JSON.stringify({
			isolation: "shared",
			working_directory: "C:/artisan",
			workspace_id: "workspace_1",
		}),
	});
	yield* database.client.insert(AgentRuns).values({
		agent_id: "graph_agent_1",
		assignment_id: "assignment_1",
		attempt: 1,
		created_at: clock.value,
		dispatch_status: "active",
		engine_id: "test",
		group_id: "group_1",
		last_observation_sequence: 0,
		profile: "default",
		run_id: "graph_run_1",
		state: "running",
		updated_at: clock.value,
	});
});

function request(request_id: string, tool_id = "tool.read", query = "private-input") {
	return {
		arguments: { query },
		context: { agent_id: "agent_1", run_id: "run_1", thread_id: "thread_1" },
		request_id,
		tool: { revision: 1, tool_id },
	};
}

function request_for_thread(
	thread_id: string,
	run_id: string,
	agent_id: string,
	request_id: string,
) {
	return {
		arguments: { query: "private-input" },
		context: { agent_id, run_id, thread_id },
		request_id,
		tool: { revision: 1, tool_id: "tool.read" },
	};
}

const await_idle = ToolControlCoordinator.pipe(
	Effect.flatMap((coordinator) => coordinator.AwaitIdle),
);

const query = (invocation_id: string) =>
	ToolControlCoordinator.pipe(
		Effect.flatMap((coordinator) =>
			coordinator.QueryInvocation({ invocation_id, thread_id: "thread_1" }),
		),
	);

afterEach(async () => {
	clock.value = "2026-07-16T12:00:00.000Z";

	for (const directory of directories.splice(0)) {
		await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		);
	}
});

describe("ToolControlCoordinator", () => {
	it("authorizes exact active ordinary and graph ownership before registry enumeration", async () => {
		const state = { calls: 0, eligibility_calls: 0, fail: false };
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"eligibility",
			state,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedEligibilityOwnership;

					const coordinator = yield* ToolControlCoordinator;
					const ordinary = yield* coordinator.ListEligible({
						context: {
							agent_id: "agent_1",
							run_id: "run_1",
							thread_id: "thread_1",
							workspace_id: "workspace_1",
						},
					});
					const graph = yield* coordinator.ListEligible({
						context: {
							agent_id: "graph_agent_1",
							run_id: "graph_run_1",
							thread_id: "thread_1",
							workspace_id: "workspace_1",
						},
					});
					const rejected = yield* Effect.forEach(
						[
							{
								agent_id: "agent_inactive",
								run_id: "run_inactive",
								thread_id: "thread_1",
							},
							{
								agent_id: "agent_1",
								run_id: "run_unknown",
								thread_id: "thread_1",
							},
							{
								agent_id: "agent_1",
								run_id: "run_1",
								thread_id: "thread_2",
							},
							{
								agent_id: "agent_wrong",
								run_id: "run_1",
								thread_id: "thread_1",
							},
							{
								agent_id: "agent_1",
								run_id: "run_1",
								thread_id: "thread_1",
								workspace_id: "workspace_wrong",
							},
							{
								agent_id: "graph_agent_1",
								run_id: "graph_run_1",
								thread_id: "thread_1",
								workspace_id: "workspace_wrong",
							},
						],
						(context) => coordinator.ListEligible({ context }).pipe(Effect.flip),
					);

					return { graph, ordinary, rejected };
				}),
			);

			expect(result.ordinary.tools).toHaveLength(2);
			expect(result.graph.tools).toHaveLength(2);
			expect(result.rejected).toMatchObject([
				{ _tag: "ToolControlUnavailable", reason: "run_inactive" },
				{ _tag: "ToolControlUnavailable", reason: "ownership" },
				{ _tag: "ToolControlUnavailable", reason: "ownership" },
				{ _tag: "ToolControlUnavailable", reason: "ownership" },
				{ _tag: "ToolControlUnavailable", reason: "workspace_mismatch" },
				{ _tag: "ToolControlUnavailable", reason: "workspace_mismatch" },
			]);
			expect(state.eligibility_calls).toBe(4);
		} finally {
			await runtime.dispose();
		}
	});

	it("automatically settles private results and exact-replays them", async () => {
		const state = { calls: 0, fail: false };
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath), "automatic", state);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const coordinator = yield* ToolControlCoordinator;
					const first = yield* coordinator.Invoke(request("automatic"));

					yield* coordinator.AwaitIdle;
					expect(state.calls).toBe(1);
					yield* coordinator.QuiesceThread("thread_1");

					return { first, replay: yield* coordinator.Invoke(request("automatic")) };
				}),
			);

			expect(result.first.outcome).toBe("pending");
			expect(result.replay).toMatchObject({
				outcome: "completed",
				result: { call: 1, private_result: "secret" },
			});
			expect(state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("waits for the initial durable dispatch scan before reporting idle", async () => {
		const scan_entered = await Effect.runPromise(Deferred.make<void>());
		const scan_release = await Effect.runPromise(Deferred.make<void>());
		const idle_started = await Effect.runPromise(Deferred.make<void>());
		const controlled_clock = await Effect.runPromise(MakeControlledClock);
		const state = { calls: 0, fail: false };
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"startup_scan",
			state,
			JournalNotifierLive,
			{ entered: scan_entered, release: scan_release },
			controlled_clock.clock,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const coordinator = yield* ToolControlCoordinator;

					yield* Deferred.await(scan_entered);

					const invoked = yield* coordinator.Invoke(request("startup_scan_pending"));
					const idle_fiber = yield* Deferred.succeed(idle_started, undefined).pipe(
						Effect.andThen(coordinator.AwaitIdle),
						Effect.forkChild({ startImmediately: true }),
					);

					yield* Deferred.await(idle_started);
					const first_wait = yield* Queue.take(controlled_clock.sleeps);

					yield* Deferred.succeed(first_wait.release, undefined);

					const second_observation = yield* Effect.raceFirst(
						Queue.take(controlled_clock.sleeps).pipe(
							Effect.map((wait) => ({ _tag: "Waiting" as const, wait })),
						),
						Fiber.await(idle_fiber).pipe(Effect.as({ _tag: "Completed" as const })),
					);
					const completed_before_release = second_observation._tag === "Completed";
					const calls_before_release = state.calls;

					yield* Deferred.succeed(scan_release, undefined);

					if (second_observation._tag === "Waiting") {
						yield* Deferred.succeed(second_observation.wait.release, undefined);
					}

					const sleep_driver = yield* Effect.forever(
						Queue.take(controlled_clock.sleeps).pipe(
							Effect.flatMap((wait) =>
								wait.duration_millis <= 25
									? Deferred.succeed(wait.release, undefined).pipe(Effect.asVoid)
									: Effect.void,
							),
						),
					).pipe(Effect.forkChild({ startImmediately: true }));

					yield* Fiber.join(idle_fiber);
					yield* Fiber.interrupt(sleep_driver);

					return {
						calls_before_release,
						completed_before_release,
						first_wait_duration: first_wait.duration_millis,
						invoked,
						second_observation: second_observation._tag,
						second_wait_duration:
							second_observation._tag === "Waiting"
								? second_observation.wait.duration_millis
								: undefined,
						terminal: yield* coordinator.QueryInvocation({
							invocation_id: invoked.invocation.invocation_id,
							thread_id: "thread_1",
						}),
					};
				}),
			);

			expect(result.invoked.outcome).toBe("pending");
			expect(result.completed_before_release).toBe(false);
			expect(result.calls_before_release).toBe(0);
			expect(result.first_wait_duration).toBe(1);
			expect(result.second_observation).toBe("Waiting");
			expect(result.second_wait_duration).toBe(1);
			expect(result.terminal.invocation?.state).toBe("completed");
			expect(state.calls).toBe(1);
		} finally {
			await Effect.runPromise(Deferred.succeed(scan_release, undefined));
			await runtime.dispose();
		}
	});

	it("waits for approval, settles registry failures, and keeps notifier defects non-fatal", async () => {
		const state = { calls: 0, fail: true };
		const notifier = Layer.succeed(JournalNotifier, {
			Publish: () => Effect.die("notifier defect"),
			Subscribe: Effect.die("unused"),
		});
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"approval",
			state,
			notifier,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const coordinator = yield* ToolControlCoordinator;
					const invoked = yield* coordinator.Invoke(request("approved", "tool.approval"));
					const approval_id =
						invoked.invocation.state === "approval_required"
							? invoked.invocation.approval_id
							: "";
					const decided = yield* coordinator.Decide({
						approval_id,
						decision: "approved",
						decision_id: "decision_1",
						thread_id: "thread_1",
					});

					yield* coordinator.AwaitIdle;

					return { decided, terminal: yield* query(invoked.invocation.invocation_id) };
				}),
			);

			expect(result.decided.approval.state).toBe("approved");
			expect(result.terminal.invocation?.state).toBe("failed");
			expect(state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("allows two runtimes to admit and execute one exact request once", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const state = { calls: 0, fail: false };
		const first = make_runtime(database_path, "race_first", state);
		const second = make_runtime(database_path, "race_second", state);

		try {
			await first.runPromise(SeedThread);
			await Promise.all([
				first.runPromise(
					ToolControlCoordinator.pipe(
						Effect.flatMap((coordinator) => coordinator.Invoke(request("race"))),
					),
				),
				second.runPromise(
					ToolControlCoordinator.pipe(
						Effect.flatMap((coordinator) => coordinator.Invoke(request("race"))),
					),
				),
			]);
			await first.runPromise(await_idle);
			await second.runPromise(await_idle);

			expect(state.calls).toBe(1);
		} finally {
			await first.dispose();
			await second.dispose();
		}
	});

	it("serializes distinct same-thread invocations across two runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_entered = await Effect.runPromise(Deferred.make<void>());
		const first_release = await Effect.runPromise(Deferred.make<void>());
		const second_entered = await Effect.runPromise(Deferred.make<void>());
		const state = {
			calls: 0,
			fail: false,
			query_entered: new Map([
				["first-invocation", first_entered],
				["second-invocation", second_entered],
			]),
			query_gates: new Map([["first-invocation", first_release]]),
		};
		const first = make_runtime(database_path, "lane_first", state);
		const second = make_runtime(database_path, "lane_second", state);

		try {
			await first.runPromise(SeedThread);
			const first_invocation = await first.runPromise(
				ToolControlCoordinator.pipe(
					Effect.flatMap((coordinator) =>
						coordinator.Invoke(
							request("lane_request_first", "tool.read", "first-invocation"),
						),
					),
				),
			);

			await Effect.runPromise(
				Deferred.await(first_entered).pipe(Effect.timeout("3 seconds")),
			);

			const second_attempt = await second.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ToolControlCoordinator;
					const database = yield* Database;
					const invocation = yield* coordinator.Invoke(
						request("lane_request_second", "tool.read", "second-invocation"),
					);

					for (let attempt = 0; attempt < 300; attempt += 1) {
						const [dispatch_state] = yield* database.client
							.select()
							.from(ToolThreadDispatchState);

						if ((dispatch_state?.admission_version ?? 0) >= 2) {
							break;
						}

						if (attempt === 299) {
							return yield* Effect.die(
								"Second runtime did not attempt same-thread admission",
							);
						}

						yield* Effect.sleep("10 millis");
					}

					return {
						entered: yield* Deferred.isDone(second_entered),
						invocation,
					};
				}),
			);

			expect(first_invocation.outcome).toBe("pending");
			expect(second_attempt.invocation.outcome).toBe("pending");
			expect(second_attempt.entered).toBe(false);
			expect(state.calls).toBe(1);

			await Effect.runPromise(Deferred.succeed(first_release, undefined));
			await Effect.runPromise(
				Deferred.await(second_entered).pipe(Effect.timeout("3 seconds")),
			);
			await Promise.all([first.runPromise(await_idle), second.runPromise(await_idle)]);

			const invocations = await first.runPromise(
				Database.pipe(
					Effect.flatMap((database) => database.client.select().from(ToolInvocations)),
				),
			);

			expect(state.calls).toBe(2);
			expect(invocations.map(({ state: invocation_state }) => invocation_state)).toEqual([
				"completed",
				"completed",
			]);
		} finally {
			await Effect.runPromise(Deferred.succeed(first_release, undefined));
			await first.dispose();
			await second.dispose();
		}
	});

	it("dispatches an unrelated thread before a gated target invocation releases", async () => {
		const target_entered = await Effect.runPromise(Deferred.make<void>());
		const target_release = await Effect.runPromise(Deferred.make<void>());
		const unrelated_completed = await Effect.runPromise(Deferred.make<void>());
		const state = {
			calls: 0,
			entered: new Map([
				["thread_1", target_entered],
				["thread_2", unrelated_completed],
			]),
			fail: false,
			gates: new Map([["thread_1", target_release]]),
		};
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"dispatch_isolation",
			state,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					yield* SeedThreadWithOwnership("thread_2", "run_2", "agent_2");

					const coordinator = yield* ToolControlCoordinator;
					const target = yield* coordinator.Invoke(request("target"));

					yield* Deferred.await(target_entered).pipe(
						Effect.timeout("3 seconds"),
						Effect.mapError(
							(cause) => new Error("Target dispatch did not enter", { cause }),
						),
					);

					const unrelated = yield* coordinator.Invoke(
						request_for_thread("thread_2", "run_2", "agent_2", "unrelated"),
					);

					yield* Deferred.await(unrelated_completed).pipe(
						Effect.timeout("3 seconds"),
						Effect.mapError(
							(cause) => new Error("Unrelated dispatch did not complete", { cause }),
						),
					);

					return { target, unrelated };
				}),
			);

			expect(result.target.outcome).toBe("pending");
			expect(result.unrelated.outcome).toBe("pending");
			expect(state.calls).toBe(2);
		} finally {
			await Effect.runPromise(Deferred.succeed(target_release, undefined));
			await runtime.dispose();
		}
	});

	it("recovers pre-launch and pure-read launched claims once across runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const gate = await Effect.runPromise(Deferred.make<void>());
		const first_state = {
			calls: 0,
			fail: false,
			gates: new Map([["thread_1", gate]]),
		};
		const first = make_runtime(database_path, "first", first_state);

		try {
			await first.runPromise(SeedThread);
			const invoked = await first.runPromise(
				ToolControlCoordinator.pipe(
					Effect.flatMap((coordinator) => coordinator.Invoke(request("restart"))),
				),
			);

			await first.runPromise(
				Effect.gen(function* () {
					for (let attempt = 0; attempt < 100; attempt += 1) {
						if (first_state.calls === 1) {
							return;
						}

						yield* Effect.yieldNow;
					}

					return yield* Effect.die(
						"Tool execution did not reach its launched crash window",
					);
				}),
			);

			expect(invoked.outcome).toBe("pending");
		} finally {
			await first.dispose();
		}

		clock.value = "2026-07-16T12:00:31.000Z";
		const second_state = { calls: 0, fail: false };
		const second = make_runtime(database_path, "second", second_state);

		try {
			await second.runPromise(await_idle);
			const replay = await second.runPromise(
				ToolControlCoordinator.pipe(
					Effect.flatMap((coordinator) => coordinator.Invoke(request("restart"))),
				),
			);

			expect(replay.outcome).toBe("completed");
			expect(first_state.calls).toBe(1);
			expect(second_state.calls).toBe(1);
		} finally {
			await Effect.runPromise(Deferred.succeed(gate, undefined));
			await second.dispose();
		}
	});

	it("quarantines expired launched outcome-unknown claims without a second invocation", async () => {
		const gate = await Effect.runPromise(Deferred.make<void>());
		const state = {
			calls: 0,
			fail: false,
			gates: new Map([["thread_1", gate]]),
		};
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"quarantine",
			state,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const coordinator = yield* ToolControlCoordinator;
					const invoked = yield* coordinator.Invoke(
						request("quarantine", "tool.approval"),
					);
					const approval_id =
						invoked.invocation.state === "approval_required"
							? invoked.invocation.approval_id
							: "";

					yield* coordinator.Decide({
						approval_id,
						decision: "approved",
						decision_id: "decision_quarantine",
						thread_id: "thread_1",
					});
					for (let attempt = 0; attempt < 100; attempt += 1) {
						if (state.calls === 1) {
							break;
						}

						yield* Effect.yieldNow;
					}

					clock.value = "2026-07-16T12:00:31.000Z";
					yield* coordinator.Recover;
					yield* coordinator.AwaitIdle;

					return yield* query(invoked.invocation.invocation_id);
				}),
			);

			expect(result.invocation?.state).toBe("outcome_unknown");
			expect(state.calls).toBe(1);
		} finally {
			await Effect.runPromise(Deferred.succeed(gate, undefined));
			await runtime.dispose();
		}
	});
});
