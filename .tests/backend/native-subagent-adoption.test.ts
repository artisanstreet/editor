import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	EngineObservation,
	EngineSubagentObservation,
	EngineSubagentTranscriptObservation,
} from "@artisan/engines";
import { AgentGraphRepository, make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	ConversationItems,
	ConversationTurns,
	NativeSubagentBindings,
	NativeSubagentObservationInbox,
	NativeSubagentTranscriptInbox,
	OrchestrationGraphEdges,
	OrchestrationGroups,
	OrchestrationRuns,
	Threads,
} from "../../modules/backend/src/persistence/tables";
import {
	SessionDefaultsService,
	session_defaults_thread_id,
} from "../../modules/backend/src/settings/session-defaults-service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const timestamp = "2026-08-09T12:00:00.000Z";

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-native-subagents-"));

	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

const Native = (
	observation_id: string,
	sequence: number,
	agent_native_thread_id: string,
	parent_native_thread_id: string,
	state: EngineSubagentObservation["state"],
	activity?: string,
): EngineSubagentObservation => ({
	_tag: "subagent",
	agent_native_thread_id,
	artisan_run_id: "root-run",
	...(activity === undefined ? {} : { activity }),
	observation_id,
	parent_native_thread_id,
	raw: {
		engine_id: "codex",
		frame: { agent_native_thread_id, parent_native_thread_id, state },
		native_id: `native:${observation_id}`,
		transport: "test",
	},
	sequence,
	state,
	turn_id: `turn:${agent_native_thread_id}`,
});

const RootTerminal = (sequence: number): EngineObservation => ({
	_tag: "run_terminal",
	artisan_run_id: "root-run",
	observation_id: `root-terminal:${sequence}`,
	raw: {
		engine_id: "codex",
		frame: { state: "completed" },
		native_id: `native:root-terminal:${sequence}`,
		transport: "test",
	},
	sequence,
	state: "completed",
});

const Transcript = (
	observation_id: string,
	sequence: number,
): EngineSubagentTranscriptObservation => ({
	_tag: "subagent_transcript",
	agent_native_thread_id: "native-child",
	artisan_run_id: "root-run",
	content: {
		_tag: "agent_message_completed",
		item_id: "child-message",
		message: "Child prose",
		phase: "final",
	},
	observation_id,
	parent_native_thread_id: "native-root",
	raw: { engine_id: "codex", frame: {}, transport: "test" },
	sequence,
});

const InsertRootRun = (run_id: string, native_thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "root-agent",
			created_at: timestamp,
			engine_id: "codex",
			native_resume_json: null,
			native_thread_id,
			run_id,
			status: "running",
			thread_id: "thread-native",
			updated_at: timestamp,
			working_directory: "C:\\workspace",
		});
	});

const InsertRoot = InsertRootRun("root-run", "native-root");

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("provider-native subagent adoption", () => {
	it("replays pending child prose beneath its adopted child turn exactly once", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});
		try {
			await runtime.runPromise(InsertRoot);
			const transcript = Transcript("child-prose", 1);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					yield* repository.RecordObservation(transcript);
				}),
			);
			const pending = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return yield* database.client.select().from(NativeSubagentTranscriptInbox);
				}),
			);
			expect(pending[0]).toMatchObject({ observation_id: "child-prose", processed_at: null });

			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					const lifecycle = Native(
						"child-running",
						2,
						"native-child",
						"native-root",
						"running",
					);
					yield* repository.RecordObservation(lifecycle);
					yield* graph.RecordObservedSubagent(lifecycle);
					yield* graph.RecoverObservedSubagents;
					yield* graph.RecoverObservedSubagents;
				}),
			);
			const projected = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						items: yield* database.client.select().from(ConversationItems),
						turns: yield* database.client.select().from(ConversationTurns),
						inbox: yield* database.client.select().from(NativeSubagentTranscriptInbox),
					};
				}),
			);
			const binding = projected.bindings[0]!;
			const child_turn = projected.turns.find(
				(turn) => turn.turn_id === `run:${binding.run_id}`,
			)!;
			expect(JSON.parse(child_turn.entity_json)).toMatchObject({
				agent_id: binding.agent_id,
				parent_id: "run:root-run",
			});
			expect(projected.items.filter((item) => item.item_id === "child-message")).toHaveLength(
				1,
			);
			expect(
				JSON.parse(
					projected.items.find((item) => item.item_id === "child-message")!.entity_json,
				),
			).toMatchObject({
				agent_id: binding.agent_id,
				turn_id: `run:${binding.run_id}`,
				text: "Child prose",
			});
			const delegation_items = projected.items.filter((item) =>
				item.item_id.includes("activity:subagent:root-run"),
			);
			expect(delegation_items).toHaveLength(1);
			expect(JSON.parse(delegation_items[0]!.entity_json)).toMatchObject({
				subagent: { agent_id: binding.agent_id, display_name: expect.any(String) },
				turn_id: "run:root-run",
				type: "activity",
			});
			expect(projected.inbox[0]?.processed_at).not.toBeNull();
		} finally {
			await runtime.dispose();
		}
	});
	it("replays a canonical observation left pending across the graph handoff", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const observation = Native(
				"child-discovered",
				1,
				"native-child",
				"native-root",
				"running",
				"Reviewing persistence",
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					yield* repository.RecordObservation(observation);
				}),
			);

			const before = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						conversation_turns: yield* database.client.select().from(ConversationTurns),
						inbox: yield* database.client.select().from(NativeSubagentObservationInbox),
					};
				}),
			);
			expect(before.bindings).toEqual([]);
			expect(before.conversation_turns).toEqual([]);
			expect(before.inbox).toEqual([
				expect.objectContaining({ observation_id: "child-discovered", processed_at: null }),
			]);

			const recovered = await runtime.runPromise(
				Effect.gen(function* () {
					const graph = yield* AgentGraphRepository;
					yield* graph.RecoverObservedSubagents;
					const [group] = yield* graph.ListGroups("thread-native", false);
					if (!group) return undefined;
					return yield* graph.GetGraph(group.group_id);
				}),
			);
			expect(recovered).toBeDefined();
			expect(recovered?.agent_instances).toEqual(
				expect.arrayContaining([
					expect.objectContaining({ display_name: expect.any(String), role: "worker" }),
				]),
			);
			expect(recovered?.assignments[0]).toMatchObject({
				heartbeat: { current_action: "Reviewing persistence" },
				state: "running",
			});
			expect(recovered?.agent_runs[0]).toMatchObject({
				execution_origin: "provider_observed",
				last_observation_sequence: 1,
			});

			const after = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return yield* database.client.select().from(NativeSubagentObservationInbox);
				}),
			);
			expect(after[0]?.processed_at).not.toBeNull();
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps a randomly assigned native identity across a cold restart", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({
			database_path,
			engines: [],
			migrations_path,
		});
		let persisted_identity:
			| { readonly agent_id: string; readonly display_name: string }
			| undefined;

		try {
			await first_runtime.runPromise(InsertRoot);
			const observation = Native(
				"restart-child",
				1,
				"native-restart-child",
				"native-root",
				"running",
			);
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					yield* repository.RecordObservation(observation);
					yield* graph.RecordObservedSubagent(observation);
				}),
			);
			persisted_identity = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const agents = yield* database.client.select().from(AgentInstances);
					return agents.find((agent) => agent.role === "worker");
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		if (persisted_identity === undefined) {
			throw new Error("Expected a persisted native worker identity");
		}

		const restarted_runtime = make_backend_runtime({
			database_path,
			engines: [],
			migrations_path,
		});
		try {
			const replay = Native(
				"restart-child-replay",
				2,
				"native-restart-child",
				"native-root",
				"waiting",
			);
			const restarted = await restarted_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					yield* repository.RecordObservation(replay);
					yield* graph.RecordObservedSubagent(replay);
					const database = yield* Database;
					return {
						agents: yield* database.client.select().from(AgentInstances),
						bindings: yield* database.client.select().from(NativeSubagentBindings),
					};
				}),
			);
			const restarted_binding = restarted.bindings.find(
				(binding) => binding.agent_native_thread_id === "native-restart-child",
			);
			const restarted_identity = restarted.agents.find(
				(agent) => agent.agent_id === persisted_identity.agent_id,
			);

			expect(restarted_binding).toMatchObject({ agent_id: persisted_identity.agent_id });
			expect(restarted_identity).toMatchObject({
				agent_id: persisted_identity.agent_id,
				display_name: persisted_identity.display_name,
			});
			expect(restarted.agents.filter((agent) => agent.role === "worker")).toHaveLength(1);
		} finally {
			await restarted_runtime.dispose();
		}
	});

	it("allocates selected dataset names once across every native group in a thread", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const Record = (observation: EngineSubagentObservation) =>
				runtime.runPromise(
					Effect.gen(function* () {
						const repository = yield* OrchestrationRepository;
						const graph = yield* AgentGraphRepository;
						yield* repository.RecordObservation(observation);
						yield* graph.RecordObservedSubagent(observation);
					}),
				);

			await Record(Native("first-child", 1, "native-child-one", "native-root", "running"));
			await runtime.runPromise(InsertRootRun("root-run-two", "native-root-two"));
			await Record({
				...Native("second-child", 1, "native-child-two", "native-root-two", "running"),
				artisan_run_id: "root-run-two",
			});

			await runtime.runPromise(
				Effect.gen(function* () {
					const settings = yield* SessionDefaultsService;
					yield* settings.Update({
						kind: "command",
						message_id: "select-playful-agent-names",
						origin: "frontend",
						payload: {
							agent_name_dataset: "playful",
							type: "session.defaults.update",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: timestamp,
						thread_id: session_defaults_thread_id,
					});
				}),
			);
			await Record(Native("first-update", 2, "native-child-one", "native-root", "waiting"));
			await runtime.runPromise(InsertRootRun("root-run-three", "native-root-three"));
			await Record({
				...Native("third-child", 1, "native-child-three", "native-root-three", "running"),
				artisan_run_id: "root-run-three",
			});

			const persisted = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						agents: yield* database.client.select().from(AgentInstances),
						bindings: yield* database.client.select().from(NativeSubagentBindings),
					};
				}),
			);
			const names = new Map(
				persisted.agents.map((agent) => [agent.agent_id, agent.display_name]),
			);
			const child_name = (native_thread_id: string) => {
				const binding = persisted.bindings.find(
					(candidate) => candidate.agent_native_thread_id === native_thread_id,
				);
				return binding === undefined ? undefined : names.get(binding.agent_id);
			};
			const first_binding = persisted.bindings.find(
				(binding) => binding.agent_native_thread_id === "native-child-one",
			);
			if (first_binding === undefined) {
				throw new Error("Expected the first observed child binding");
			}
			const first_persisted_name = names.get(first_binding.agent_id);
			if (first_persisted_name === undefined) {
				throw new Error("Expected the first observed child name");
			}
			await Record(Native("first-replay", 3, "native-child-one", "native-root", "running"));
			const replayed_agents = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return yield* database.client.select().from(AgentInstances);
				}),
			);
			expect(
				replayed_agents.find((agent) => agent.agent_id === first_binding.agent_id),
			).toMatchObject({ display_name: first_persisted_name });

			const first_child_name = child_name("native-child-one");
			const second_child_name = child_name("native-child-two");
			const third_child_name = child_name("native-child-three");
			expect(first_child_name).toEqual(expect.any(String));
			expect(second_child_name).toEqual(expect.any(String));
			expect(first_child_name).not.toBe(second_child_name);
			expect(third_child_name).toBeOneOf([
				"Sprocket",
				"Biscuit",
				"Noodle",
				"Widget",
				"Marmalade",
				"Button",
				"Doodle",
				"Pip",
			]);
			const worker_names = persisted.agents
				.filter((agent) => agent.role === "worker")
				.map((agent) => agent.display_name);
			expect(new Set(worker_names).size).toBe(worker_names.length);
		} finally {
			await runtime.dispose();
		}
	});

	it("consumes a first late child and transcript after its root is terminal", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const lifecycle = Native(
				"late-first-child",
				2,
				"native-late-child",
				"native-root",
				"running",
			);
			const transcript = {
				...Transcript("late-first-child-prose", 3),
				agent_native_thread_id: "native-late-child",
			} satisfies EngineSubagentTranscriptObservation;
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					yield* repository.RecordObservation(RootTerminal(1));
					yield* repository.RecordObservation(lifecycle);
					yield* graph.RecordObservedSubagent(lifecycle);
					yield* repository.RecordObservation(transcript);
					/** Simulate a crash between canonical inbox persistence and graph handoff. */
					yield* graph.RecoverObservedSubagents;
				}),
			);

			const persisted = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						assignments: yield* database.client.select().from(Assignments),
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						groups: yield* database.client.select().from(OrchestrationGroups),
						lifecycle_inbox: yield* database.client
							.select()
							.from(NativeSubagentObservationInbox),
						transcript_inbox: yield* database.client
							.select()
							.from(NativeSubagentTranscriptInbox),
					};
				}),
			);
			expect(persisted.groups).toEqual([]);
			expect(persisted.bindings).toEqual([]);
			expect(persisted.assignments).toEqual([]);
			expect(persisted.lifecycle_inbox).toEqual([]);
			expect(persisted.transcript_inbox).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("reserves names across ordinary groups in one thread while allowing another thread to reuse them", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					for (const thread_id of ["thread-one", "thread-two"]) {
						yield* database.client.insert(Threads).values({
							created_at: timestamp,
							thread_id,
							title: thread_id,
							updated_at: timestamp,
						});
					}
					const graph = yield* AgentGraphRepository;
					const Start = (
						message_id: string,
						group_id: string,
						thread_id: string,
						display_name?: string,
					) =>
						graph.StartGroup({
							kind: "command",
							message_id,
							origin: "frontend",
							payload: {
								assignments: [
									{
										assignment_id: `${group_id}-assignment`,
										...(display_name === undefined ? {} : { display_name }),
										engine_id: "codex",
										expected_result: "Result",
										instructions: "Work",
										parent_node_id: group_id,
										permission_policy: {
											approval: "on_request",
											network_access: false,
											write_access: false,
										},
										profile: "default",
										role: "worker",
										scope: {
											kind: "custom",
											value: "Work",
											write_access: false,
										},
										summary_contract: "Result",
										workspace: {
											isolation: "shared",
											workspace_id: thread_id,
											working_directory: "C:\\workspace",
										},
									},
								],
								group_id,
								name_bank: ["Ada"],
								type: "orchestration.group.start",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: timestamp,
							thread_id,
						});
					yield* Start("start-one", "group-one", "thread-one");
					yield* Start("start-two", "group-two", "thread-one");
					yield* Start("start-three", "group-three", "thread-two");
					yield* Start("start-explicit", "group-explicit", "thread-one", "Pinned Agent");
				}),
			);

			const agents = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return yield* database.client.select().from(AgentInstances);
				}),
			);
			const worker_names = (group_id: string) =>
				agents.find((agent) => agent.group_id === group_id && agent.role === "worker")!
					.display_name;
			expect(worker_names("group-one")).toBe("Ada");
			expect(worker_names("group-two")).toBe("Ada 2");
			expect(worker_names("group-three")).toBe("Ada");
			expect(worker_names("group-explicit")).toBe("Pinned Agent");
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps one nested graph monotonic and settles it only after root and children", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const Record = (observation: EngineSubagentObservation) =>
				runtime.runPromise(
					Effect.gen(function* () {
						const repository = yield* OrchestrationRepository;
						const graph = yield* AgentGraphRepository;
						yield* repository.RecordObservation(observation);
						yield* graph.RecordObservedSubagent(observation);
						yield* graph.ReconcileObservedRoot(observation.artisan_run_id);
					}),
				);

			await Record(
				Native(
					"child-a-running",
					1,
					"native-child-a",
					"native-root",
					"running",
					"Reviewing code",
				),
			);
			await Record(
				Native(
					"child-b-running",
					2,
					"native-child-b",
					"native-child-a",
					"running",
					"Running tests",
				),
			);

			const nested = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						edges: yield* database.client.select().from(OrchestrationGraphEdges),
						groups: yield* database.client.select().from(OrchestrationGroups),
					};
				}),
			);
			expect(nested.groups).toHaveLength(1);
			expect(nested.bindings).toHaveLength(2);
			const parent = nested.bindings.find(
				(binding) => binding.agent_native_thread_id === "native-child-a",
			)!;
			const child = nested.bindings.find(
				(binding) => binding.agent_native_thread_id === "native-child-b",
			)!;
			expect(nested.edges).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						from_node_id: parent.assignment_id,
						kind: "delegation",
						to_node_id: child.assignment_id,
					}),
				]),
			);

			await Record(
				Native("child-a-complete", 3, "native-child-a", "native-root", "completed"),
			);
			await Record(Native("child-a-stale", 2, "native-child-a", "native-root", "running"));
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					yield* repository.RecordObservation(RootTerminal(4));
					/** Startup recovery must repair a persisted terminal root with no child terminal frame. */
					yield* graph.RecoverObservedSubagents;
				}),
			);

			const reconciled = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						assignments: yield* database.client.select().from(Assignments),
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						groups: yield* database.client.select().from(OrchestrationGroups),
						runs: yield* database.client.select().from(AgentRuns),
						turns: yield* database.client.select().from(ConversationTurns),
					};
				}),
			);
			expect(reconciled.groups[0]?.state).toBe("complete");
			expect(reconciled.runs.find((run) => run.run_id === parent.run_id)).toMatchObject({
				last_observation_sequence: 3,
				state: "complete",
			});
			expect(
				reconciled.bindings.find((binding) => binding.run_id === child.run_id),
			).toMatchObject({
				state: "stopped",
			});
			expect(reconciled.runs.find((run) => run.run_id === child.run_id)).toMatchObject({
				completed_at: expect.any(String),
				state: "stopped",
			});
			expect(
				reconciled.assignments.find(
					(assignment) => assignment.active_run_id === child.run_id,
				),
			).toMatchObject({ state: "stopped" });
			expect(
				JSON.parse(
					reconciled.turns.find((turn) => turn.turn_id === `run:${child.run_id}`)!
						.entity_json,
				),
			).toMatchObject({ lifecycle: "cancelled" });

			await Record(
				Native("child-b-complete", 5, "native-child-b", "native-child-a", "completed"),
			);
			const settled = await runtime.runPromise(
				Effect.gen(function* () {
					const graph = yield* AgentGraphRepository;
					return yield* graph.GetGraph(nested.groups[0]!.group_id);
				}),
			);
			expect(settled.group.state).toBe("complete");
			expect(settled.agent_runs.find((run) => run.run_id === child.run_id)).toMatchObject({
				state: "complete",
			});
			const settled_worker_names = settled.agent_instances
				.filter((agent) => agent.role === "worker")
				.map((agent) => agent.display_name);
			expect(
				settled.agent_instances.find((agent) => agent.role === "coordinator"),
			).toMatchObject({
				display_name: "Coordinator",
			});
			expect(new Set(settled_worker_names).size).toBe(2);

			await Record(Native("child-c-late", 6, "native-child-c", "native-root", "running"));
			const after_late_discovery = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						inbox: yield* database.client.select().from(NativeSubagentObservationInbox),
					};
				}),
			);
			expect(after_late_discovery.bindings).toHaveLength(2);
			expect(
				after_late_discovery.bindings.some(
					(binding) => binding.agent_native_thread_id === "native-child-c",
				),
			).toBe(false);
			expect(
				after_late_discovery.inbox.find(
					(observation) => observation.observation_id === "child-c-late",
				),
			).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("repairs a child edge when its native parent is observed later", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const Record = (observation: EngineSubagentObservation) =>
				runtime.runPromise(
					Effect.gen(function* () {
						const repository = yield* OrchestrationRepository;
						const graph = yield* AgentGraphRepository;
						yield* repository.RecordObservation(observation);
						yield* graph.RecordObservedSubagent(observation);
					}),
				);

			await Record(
				Native("nested-before-parent", 1, "native-nested", "native-parent", "running"),
			);
			await Record(Native("late-parent", 2, "native-parent", "native-root", "running"));

			const topology = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						bindings: yield* database.client.select().from(NativeSubagentBindings),
						edges: yield* database.client.select().from(OrchestrationGraphEdges),
					};
				}),
			);
			const parent = topology.bindings.find(
				(binding) => binding.agent_native_thread_id === "native-parent",
			)!;
			const child = topology.bindings.find(
				(binding) => binding.agent_native_thread_id === "native-nested",
			)!;
			expect(topology.edges).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						from_node_id: parent.assignment_id,
						kind: "delegation",
						to_node_id: child.assignment_id,
					}),
				]),
			);
		} finally {
			await runtime.dispose();
		}
	});
});
