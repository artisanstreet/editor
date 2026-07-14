import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Deferred, Effect, Exit, Queue, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	Engine,
	EngineCommand,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
	EngineRunTerminalState,
} from "@artisan/engines";
import type {
	CommandEnvelope,
	HelloEnvelope,
	OrchestrationGraph,
	OrchestrationGraphQueryEnvelope,
	SubscribeEnvelope,
} from "@artisan/protocol";
import {
	AgentGraphOrchestrator,
	AgentGraphRepository,
	ExternalWaitRepository,
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	AgentRuns,
	Assignments,
	ExternalWaits,
	OrchestrationRawObservations,
	ProjectHostedOrigins,
	Projects,
	Threads,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

interface ControlledRun {
	readonly closed: Deferred.Deferred<EngineRunTerminalState>;
	readonly input: EngineOpenInput;
	readonly queue: Queue.Queue<EngineObservation, Cause.Done<void>>;
	sequence: number;
}

function make_controlled_engine() {
	const runs: Array<ControlledRun> = [];
	let scopes_closed = 0;
	const capability_names = [
		"approval",
		"auth",
		"cancel",
		"close",
		"events",
		"global_guidance",
		"harness_context",
		"model_selection",
		"native_tools",
		"probe",
		"question",
		"raw_frames",
		"resume",
		"start",
		"steer",
		"subagents",
	] as const;
	const capabilities = Object.fromEntries(
		capability_names.map((name) => [name, { state: "supported" as const }]),
	) as Engine["Descriptor"]["capabilities"];

	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			const queue = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
			const closed = yield* Deferred.make<EngineRunTerminalState>();
			const controlled = { closed, input, queue, sequence: 0 } satisfies ControlledRun;

			runs.push(controlled);
			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
					scopes_closed += 1;
					yield* Queue.end(queue);
					yield* Deferred.succeed(closed, "closed");
				}),
			);

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Deferred.await(closed),
				Events: Stream.fromQueue(queue),
				native_thread_id: `native:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
				Send: (_command: EngineCommand) => Effect.void,
			} satisfies EngineRun;
		});

	const FindRun = (assignment_id: string, occurrence = 0) => {
		const matches = runs.filter((run) =>
			"initial_text" in run.input
				? run.input.initial_text.includes(`Work on ${assignment_id}`)
				: false,
		);

		return matches[occurrence];
	};

	const Emit = (
		run: ControlledRun,
		observation:
			| {
					readonly _tag: "agent_message_completed";
					readonly message: string;
					readonly turn_id: string;
			  }
			| { readonly _tag: "run_state"; readonly state: "opening" | "running" | "waiting" }
			| { readonly _tag: "run_terminal"; readonly state: EngineRunTerminalState },
	) =>
		Effect.gen(function* () {
			run.sequence += 1;
			yield* Queue.offer(run.queue, {
				...observation,
				artisan_run_id: run.input.artisan_run_id,
				observation_id: `observation:${run.input.artisan_run_id}:${run.sequence}`,
				raw: {
					engine_id: "controlled",
					frame: observation,
					native_id: `native-observation:${run.sequence}`,
					transport: "test",
				},
				sequence: run.sequence,
			} as EngineObservation);
		});

	const Finish = (run: ControlledRun, state: EngineRunTerminalState) =>
		Effect.gen(function* () {
			yield* Emit(run, { _tag: "run_terminal", state });
			yield* Queue.end(run.queue);
			yield* Deferred.succeed(run.closed, state);
		});

	return {
		Emit,
		FindRun,
		Finish,
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Controlled graph engine",
				id: "controlled",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by graph tests"),
		} satisfies Engine,
		runs,
		scopes_closed: () => scopes_closed,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-agent-graph-lifecycle-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function command(
	message_id: string,
	payload: CommandEnvelope["payload"],
	thread_id = "thread_graph",
): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id,
	};
}

function assignment(assignment_id: string, max_attempts = 1) {
	return {
		assignment_id,
		engine_id: "controlled",
		expected_result: `${assignment_id} result`,
		instructions: `Work on ${assignment_id}`,
		max_attempts,
		parent_node_id: "group_graph",
		permission_policy: {
			approval: "never" as const,
			network_access: false,
			write_access: false,
		},
		profile: "default",
		role: "tester",
		scope: { kind: "test" as const, value: assignment_id, write_access: false },
		summary_contract: "Return a concise result",
		workspace: {
			isolation: "isolated" as const,
			workspace_id: `workspace_${assignment_id}`,
			working_directory: tmpdir(),
		},
	};
}

function start_with_join(strategy: "first_success" | "require_all") {
	return command(`start_${strategy}`, {
		assignments: [assignment("assignment_a"), assignment("assignment_b")],
		group_id: "group_graph",
		joins: [
			{
				join_id: "join_results",
				strategy,
				upstream_assignment_ids: ["assignment_a", "assignment_b"],
			},
		],
		type: "orchestration.group.start",
	});
}

function start_retry_group() {
	return command("start_retry", {
		assignments: [assignment("assignment_a", 2), assignment("assignment_b")],
		group_id: "group_graph",
		type: "orchestration.group.start",
	});
}

function start_downstream_group(strategy: "review" | "synthesize") {
	return command(`start_${strategy}`, {
		assignments: [
			assignment("assignment_a"),
			assignment("assignment_b"),
			{ ...assignment("assignment_c"), parent_node_id: "join_results" },
		],
		group_id: "group_graph",
		joins: [
			{
				downstream_assignment_id: "assignment_c",
				join_id: "join_results",
				strategy,
				upstream_assignment_ids: ["assignment_a", "assignment_b"],
			},
		],
		type: "orchestration.group.start",
	});
}

function route(runtime: ReturnType<typeof make_backend_runtime>, input: CommandEnvelope) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(input);
		}),
	);
}

function get_graph(runtime: ReturnType<typeof make_backend_runtime>) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const service = yield* AgentGraphOrchestrator;

			return yield* service.GetGraph("group_graph");
		}),
	);
}

async function wait_for_graph(
	runtime: ReturnType<typeof make_backend_runtime>,
	predicate: (graph: OrchestrationGraph) => boolean,
) {
	for (let attempt = 0; attempt < 200; attempt += 1) {
		const graph = await get_graph(runtime);

		if (predicate(graph)) {
			return graph;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Graph did not reach the expected state");
}

async function create_thread(runtime: ReturnType<typeof make_backend_runtime>) {
	await route(
		runtime,
		command("create_graph_thread", { title: "Agent graph", type: "thread.create" }),
	);
}

function hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_graph",
		origin: "frontend",
		payload: { event_cursors: [], last_journal_sequence: 0, supported_protocol_versions: [1] },
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function take(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("multi-agent graph lifecycle", () => {
	it("rejects malformed provider resume state before graph activation", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			await create_thread(runtime);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* AgentGraphRepository;

					yield* repository.StartGroup(
						command("start_resume_boundary", {
							assignments: [assignment("assignment_resume")],
							group_id: "group_graph",
							type: "orchestration.group.start",
						}),
					);

					const [run] = yield* database.client.select().from(AgentRuns).limit(1);

					if (!run) {
						return yield* Effect.die("Expected the graph run to exist");
					}

					const claimed = yield* repository.ClaimRun(run.run_id, "instance_resume_test");
					const malformed = yield* repository
						.ActivateRun(run.run_id, "instance_resume_test", "native_thread_1", {
							native_thread_id: "native_thread_1",
							provider_state: "invented",
						})
						.pipe(Effect.exit);
					const mismatched = yield* repository
						.ActivateRun(run.run_id, "instance_resume_test", "native_thread_1", {
							native_thread_id: "native_thread_2",
						})
						.pipe(Effect.exit);
					const [rejected_run] = yield* database.client.select().from(AgentRuns).limit(1);
					const activated = yield* repository.ActivateRun(
						run.run_id,
						"instance_resume_test",
						"native_thread_1",
						{
							native_thread_id: "native_thread_1",
							opaque_checkpoint: "provider-owned",
						},
					);
					const [persisted_run] = yield* database.client
						.select()
						.from(AgentRuns)
						.limit(1);

					return {
						activated,
						claimed,
						malformed,
						mismatched,
						persisted_run,
						rejected_run,
					};
				}),
			);

			expect(result.claimed).toBe(true);
			expect(Exit.isFailure(result.malformed)).toBe(true);
			expect(Exit.isFailure(result.mismatched)).toBe(true);
			expect(result.rejected_run).toMatchObject({
				dispatch_status: "dispatching",
				native_resume_json: null,
				native_thread_id: null,
				state: "queued",
			});
			expect(result.activated.run.native_thread_id).toBe("native_thread_1");
			expect(result.persisted_run).toMatchObject({
				dispatch_status: "active",
				native_resume_json:
					'{"native_thread_id":"native_thread_1","opaque_checkpoint":"provider-owned"}',
				native_thread_id: "native_thread_1",
				state: "running",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("marks an external wait source closed only after the native run closes", async () => {
		const database_path = await make_database_path();
		const controlled = make_controlled_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			const external_assignment = assignment("assignment_external");
			const companion_assignment = assignment("assignment_companion");

			await create_thread(runtime);
			await route(
				runtime,
				command("start_external_wait", {
					assignments: [external_assignment, companion_assignment],
					group_id: "group_graph",
					type: "orchestration.group.start",
				}),
			);
			await wait_for_graph(runtime, (graph) => graph.assignments[0]?.state === "running");
			const run = controlled.FindRun("assignment_external");

			if (!run) {
				throw new Error("Expected one controlled engine run");
			}

			const before_close = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const external_waits = yield* ExternalWaitRepository;
					const agent_run = (yield* database.client.select().from(AgentRuns)).find(
						(candidate) => candidate.run_id === run.input.artisan_run_id,
					);
					const assignment_row = (yield* database.client.select().from(Assignments)).find(
						(candidate) => candidate.assignment_id === "assignment_external",
					);

					if (!agent_run || !assignment_row) {
						return yield* Effect.die(new Error("Expected one active graph run"));
					}

					const workspace = external_assignment.workspace;
					const target = {
						branch: "main",
						expected_head_commit: "b".repeat(40),
						pull_request_number: 7,
						pull_request_origin: {
							native_id: "pr_7",
							provider_id: "github",
							resource_kind: "pull_request" as const,
						},
						repository: {
							host: "github.com",
							name: "editor",
							owner: "artisan",
							provider_id: "github",
						},
					};

					yield* database.client.insert(Projects).values({
						canonical_root: workspace.working_directory,
						display_name: "Artisan",
						project_id: "project_external",
						registered_at: "2026-07-10T08:00:00.000Z",
						updated_at: "2026-07-10T08:00:00.000Z",
						workspace_id: workspace.workspace_id,
					});
					yield* database.client.insert(ProjectHostedOrigins).values({
						canonical_host: "github.com",
						clone_url: "https://github.com/artisan/editor.git",
						fetch_url: "https://github.com/artisan/editor.git",
						name: "editor",
						native_id: "repository_1",
						owner: "artisan",
						project_id: "project_external",
						provider_id: "github",
						push_url: "https://github.com/artisan/editor.git",
						remote_name: "origin",
						selected_account_login: "sander",
						web_url: "https://github.com/artisan/editor",
					});
					yield* database.client.update(Threads).set({
						primary_project_id: "project_external",
						primary_project_json: JSON.stringify({
							display_name: "Artisan",
							project_id: "project_external",
							root_path: workspace.working_directory,
						}),
					});
					yield* external_waits.Register({
						baseline: {
							branch: target.branch,
							checks: [],
							expected_head_commit: target.expected_head_commit,
							gates: [{ _tag: "required_checks_terminal" }],
							pull_request_native_id: target.pull_request_origin.native_id,
							pull_request_number: target.pull_request_number,
							pull_request_origin: target.pull_request_origin,
							repository: target.repository,
							review_decision: "review_required",
							reviews: [],
							review_threads: [],
						},
						owner: {
							_tag: "assignment_run",
							agent_id: agent_run.agent_id,
							assignment_id: agent_run.assignment_id,
							engine_id: agent_run.engine_id,
							group_id: agent_run.group_id,
							run_id: agent_run.run_id,
						},
						project_id: "project_external",
						request: {
							expected_head_commit: target.expected_head_commit,
							gates: [{ _tag: "required_checks_terminal" }],
							pull_request_number: target.pull_request_number,
							source_run_id: agent_run.run_id,
							workspace_id: workspace.workspace_id,
						},
						request_fingerprint: "c".repeat(64),
						source_command: {
							message_id: "external_wait_request_1",
							sent_at: "2026-07-10T08:01:00.000Z",
						},
						target,
						thread_id: "thread_graph",
						wait_id: "wait_source_closure",
					});

					return yield* database.client.select().from(ExternalWaits).limit(1);
				}),
			);

			expect(before_close[0]?.source_closed_at).toBeNull();
			await runtime.runPromise(controlled.Finish(run, "closed"));

			let closed_at: string | null = null;

			for (let attempt = 0; attempt < 100 && closed_at === null; attempt += 1) {
				closed_at = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const [wait] = yield* database.client.select().from(ExternalWaits).limit(1);

						return wait?.source_closed_at ?? null;
					}),
				);

				if (closed_at === null) {
					await new Promise<void>((resolve) => setTimeout(resolve, 10));
				}
			}

			expect(closed_at).not.toBeNull();
			const assignment_state = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const [assignment_row] = yield* database.client
						.select()
						.from(Assignments)
						.limit(1);

					return assignment_row?.state;
				}),
			);

			expect(assignment_state).toBe("waiting_external");
		} finally {
			await runtime.dispose();
		}
	});

	it("resolves require_all and first_success joins with mixed terminal outcomes", async () => {
		for (const strategy of ["require_all", "first_success"] as const) {
			const database_path = await make_database_path();
			const controlled = make_controlled_engine();
			const runtime = make_backend_runtime({
				database_path,
				engines: [controlled.engine],
				migrations_path,
			});

			try {
				await create_thread(runtime);
				await route(runtime, start_with_join(strategy));
				await wait_for_graph(runtime, (graph) =>
					graph.agent_runs.every(({ state }) => state === "running"),
				);

				const run_a = controlled.FindRun("assignment_a")!;
				const run_b = controlled.FindRun("assignment_b")!;

				await Effect.runPromise(
					controlled.Emit(run_a, {
						_tag: "agent_message_completed",
						message: "Durable result A",
						turn_id: "turn_a",
					}),
				);
				const summarized = await wait_for_graph(
					runtime,
					(graph) => graph.assignments[0]!.state === "summarized",
				);

				expect(summarized.artifacts).toMatchObject([
					{
						assignment_id: "assignment_a",
						content: "Durable result A",
						kind: "summary",
						raw_origin: {
							provider: "controlled",
							reference: "native-observation:1",
						},
					},
				]);

				await Effect.runPromise(
					controlled.Emit(run_a, { _tag: "run_state", state: "waiting" }),
				);
				const after_provider_state = await wait_for_graph(
					runtime,
					(graph) =>
						graph.agent_runs.find(({ run_id }) => run_id === run_a.input.artisan_run_id)
							?.last_observation_sequence === 2,
				);

				expect(after_provider_state.assignments[0]!.state).toBe("summarized");
				expect(
					after_provider_state.agent_runs.find(
						({ run_id }) => run_id === run_a.input.artisan_run_id,
					)?.state,
				).toBe("summarized");

				await Effect.runPromise(controlled.Finish(run_a, "completed"));
				const after_first = await wait_for_graph(
					runtime,
					(graph) => graph.assignments[0]!.state === "complete",
				);

				expect(after_first.joins[0]!.state).toBe(
					strategy === "first_success" ? "complete" : "joining",
				);

				await Effect.runPromise(
					controlled.Finish(run_b, strategy === "first_success" ? "failed" : "completed"),
				);

				const terminal = await wait_for_graph(runtime, (graph) =>
					["complete", "failed"].includes(graph.group.state),
				);

				expect(terminal.joins[0]).toMatchObject(
					strategy === "first_success"
						? { selected_assignment_id: "assignment_a", state: "complete" }
						: { state: "complete" },
				);
				expect(terminal.group.state).toBe("complete");
			} finally {
				await runtime.dispose();
			}
		}
	});

	it("creates monotonic retry attempts and ignores late observations from an old run", async () => {
		const database_path = await make_database_path();
		const controlled = make_controlled_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(runtime, start_retry_group());
			await wait_for_graph(runtime, (graph) =>
				graph.agent_runs.every(({ state }) => state === "running"),
			);

			const first = controlled.FindRun("assignment_a")!;

			await Effect.runPromise(controlled.Finish(first, "failed"));

			const retried = await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_a")
						?.current_attempt === 2 &&
					graph.agent_runs.some(
						({ attempt, state }) => attempt === 2 && state === "running",
					),
			);
			const current = retried.assignments.find(
				({ assignment_id }) => assignment_id === "assignment_a",
			)!;
			const old_run = retried.agent_runs.find(
				({ assignment_id, attempt }) => assignment_id === "assignment_a" && attempt === 1,
			)!;

			const late_observation = {
				_tag: "run_terminal" as const,
				artisan_run_id: old_run.run_id,
				observation_id: "late_old_completion",
				raw: {
					engine_id: "controlled",
					frame: { kind: "late", state: "completed" },
					native_id: "native-late-99",
					native_method: "thread.completed",
					protocol_version: "2.1",
					raw_frame_base64: "AQIDBA==",
					transport: "stdio-jsonl",
				},
				sequence: 99,
				state: "completed" as const,
			} satisfies EngineObservation;
			const ingestion = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* AgentGraphRepository;
					const first_ingestion = yield* repository.RecordObservation(late_observation);
					const duplicate_ingestion =
						yield* repository.RecordObservation(late_observation);

					return { duplicate_ingestion, first_ingestion };
				}),
			);
			const persisted_observations = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return yield* database.client.select().from(OrchestrationRawObservations);
				}),
			);

			const after_late = await get_graph(runtime);
			const persisted_late = persisted_observations.filter(
				({ observation_id }) => observation_id === late_observation.observation_id,
			);

			expect(ingestion).toEqual({ duplicate_ingestion: [], first_ingestion: [] });
			expect(persisted_late).toEqual([
				expect.objectContaining({
					engine_id: "controlled",
					frame_json: '{"kind":"late","state":"completed"}',
					native_id: "native-late-99",
					native_method: "thread.completed",
					protocol_version: "2.1",
					raw_frame_base64: "AQIDBA==",
					run_id: old_run.run_id,
					sequence: 99,
					transport: "stdio-jsonl",
				}),
			]);
			expect(
				after_late.assignments.find(
					({ assignment_id }) => assignment_id === "assignment_a",
				),
			).toMatchObject({
				active_run_id: current.active_run_id,
				current_attempt: 2,
				state: "running",
			});
			expect(after_late.agent_runs.find(({ run_id }) => run_id === old_run.run_id)).toEqual(
				old_run,
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an explicit retry command without creating another attempt", async () => {
		const database_path = await make_database_path();
		const controlled = make_controlled_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(runtime, start_retry_group());
			await wait_for_graph(runtime, (graph) =>
				graph.agent_runs.every(({ state }) => state === "running"),
			);

			await Effect.runPromise(
				controlled.Finish(controlled.FindRun("assignment_a")!, "cancelled"),
			);
			await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_a")
						?.state === "stopped",
			);

			const retry = command("retry_assignment_a", {
				assignment_id: "assignment_a",
				group_id: "group_graph",
				type: "assignment.retry",
			});
			const accepted = await route(runtime, retry);
			const duplicate = await route(runtime, retry);
			const graph = await wait_for_graph(
				runtime,
				(current) =>
					current.assignments.find(
						({ assignment_id }) => assignment_id === "assignment_a",
					)?.current_attempt === 2 &&
					current.agent_runs.some(
						({ assignment_id, attempt, state }) =>
							assignment_id === "assignment_a" &&
							attempt === 2 &&
							state === "running",
					),
			);
			const attempts = graph.agent_runs.filter(
				({ assignment_id }) => assignment_id === "assignment_a",
			);

			expect(accepted[0]).toMatchObject({ payload: { status: "accepted" } });
			expect(duplicate[0]).toMatchObject({ payload: { status: "duplicate" } });
			expect(duplicate.slice(1)).toEqual(accepted.slice(1));
			expect(attempts.map(({ attempt, state }) => ({ attempt, state }))).toEqual([
				{ attempt: 1, state: "stopped" },
				{ attempt: 2, state: "running" },
			]);
			expect(controlled.FindRun("assignment_a", 1)).toBeDefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("releases synthesize and review work as explicit downstream assignments", async () => {
		for (const strategy of ["synthesize", "review"] as const) {
			const database_path = await make_database_path();
			const controlled = make_controlled_engine();
			const runtime = make_backend_runtime({
				database_path,
				engines: [controlled.engine],
				migrations_path,
			});

			try {
				await create_thread(runtime);
				await route(runtime, start_downstream_group(strategy));
				const initial = await wait_for_graph(
					runtime,
					(graph) =>
						graph.agent_runs.filter(({ state }) => state === "running").length === 2,
				);
				const downstream = initial.assignments.find(
					({ assignment_id }) => assignment_id === "assignment_c",
				)!;

				expect(downstream).toMatchObject({
					parent_node_id: "join_results",
					state: "joining",
				});
				expect(controlled.runs).toHaveLength(2);

				await Effect.runPromise(
					controlled.Finish(controlled.FindRun("assignment_a")!, "completed"),
				);
				await Effect.runPromise(
					controlled.Finish(controlled.FindRun("assignment_b")!, "completed"),
				);

				const released = await wait_for_graph(
					runtime,
					(graph) =>
						graph.assignments.find(
							({ assignment_id }) => assignment_id === "assignment_c",
						)?.state === "running",
				);
				const downstream_run = controlled.FindRun("assignment_c")!;

				expect(released.joins[0]).toMatchObject({ state: "complete", strategy });
				expect(released.edges).toEqual(
					expect.arrayContaining([
						expect.objectContaining({
							from_node_id: "assignment_a",
							kind: "result",
							to_node_id: "join_results",
						}),
						expect.objectContaining({
							from_node_id: "join_results",
							kind: "dependency",
							to_node_id: "assignment_c",
						}),
					]),
				);

				await Effect.runPromise(controlled.Finish(downstream_run, "completed"));
				expect(
					(await wait_for_graph(runtime, (graph) => graph.group.state === "complete"))
						.group.state,
				).toBe("complete");
			} finally {
				await runtime.dispose();
			}
		}
	});

	it("serves graph query, subscription snapshot, and ordered patches", async () => {
		const database_path = await make_database_path();
		const controlled = make_controlled_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(hello());
						yield* take(connection, 2);
						yield* connection.Receive(
							command("create_graph_thread", {
								title: "Agent graph",
								type: "thread.create",
							}),
						);
						yield* take(connection, 2);
						yield* connection.Receive(start_retry_group());
						yield* take(connection, 4);

						const query: OrchestrationGraphQueryEnvelope = {
							kind: "orchestration.graph.query",
							message_id: "query_graph",
							origin: "frontend",
							payload: { group_id: "group_graph" },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-07-10T08:01:00.000Z",
						};

						yield* connection.Receive(query);
						const query_result = yield* take(connection, 1);
						const subscribe: SubscribeEnvelope = {
							kind: "subscribe",
							message_id: "subscribe_graph",
							origin: "frontend",
							payload: { group_id: "group_graph", type: "orchestration.graph" },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-07-10T08:02:00.000Z",
							subscription_id: "graph_subscription",
						};

						yield* connection.Receive(subscribe);
						const subscription = yield* take(connection, 2);
						const snapshot = subscription.find(
							(envelope) => envelope.kind === "orchestration.graph.snapshot",
						);
						const worker =
							snapshot?.kind === "orchestration.graph.snapshot"
								? snapshot.payload.graph.agent_instances.find(
										({ role }) => role !== "coordinator",
									)
								: undefined;

						yield* connection.Receive(
							command("rename_for_patch", {
								agent_id: worker!.agent_id,
								display_name: "Patchwork",
								group_id: "group_graph",
								type: "agent_instance.rename",
							}),
						);
						const patched = yield* take(connection, 3);

						return { patched, query_result, subscription };
					}),
				),
			);

			expect(result.query_result).toMatchObject([
				{
					kind: "orchestration.graph.query.result",
					payload: { graph: { group: { group_id: "group_graph" } } },
				},
			]);
			expect(result.subscription.map(({ kind }) => kind)).toEqual([
				"subscription.started",
				"orchestration.graph.snapshot",
			]);
			expect(result.patched).toContainEqual(
				expect.objectContaining({
					kind: "orchestration.graph.patch",
					sequence: 1,
				}),
			);
		} finally {
			await runtime.dispose();
		}
	});
});
