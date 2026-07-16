import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ToolControlRepository,
	ToolControlRepositoryLive,
} from "../../modules/backend/src/tool-control/tool-control-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalNotifier,
	JournalNotifierLive,
} from "../../modules/backend/src/persistence/journal-notifier";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	EventStreams,
	JournalEvents,
	OrchestrationGroups,
	OrchestrationRuns,
	Projects,
	ThreadErasureClaims,
	Threads,
	ToolInvocationPrivate,
	ToolControlCommands,
	ToolInvocations,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const now = "2026-07-16T12:00:00.000Z";

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-tool-control-" });

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function metadata_layer() {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "tool_control_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_test_${++identifier}`),
		Now: Effect.succeed(now),
	});
}

function runtime(database_path: string, notifier = JournalNotifierLive) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(),
		notifier,
		NodeCrypto.layer,
	);

	return ManagedRuntime.make(ToolControlRepositoryLive.pipe(Layer.provideMerge(infrastructure)));
}

const automatic_descriptor = {
	approval_policy: "automatic" as const,
	effect: "read" as const,
	input_schema: {
		properties: { query: { type: "string" } },
		required: ["query"],
		type: "object",
	},
	label: "Read workspace",
	revision: 1,
	source: "artisan" as const,
	summary: "Reads a bounded workspace view",
	tool_id: "workspace.read",
};

const required_descriptor = {
	...automatic_descriptor,
	approval_policy: "required" as const,
	tool_id: "workspace.replace",
};

function request(
	overrides: Partial<{
		readonly request_id: string;
		readonly thread_id: string;
		readonly run_id: string;
		readonly agent_id: string;
		readonly workspace_id: string;
	}> = {},
) {
	return {
		arguments: { query: "private-token" },
		context: {
			agent_id: overrides.agent_id ?? "agent_1",
			run_id: overrides.run_id ?? "run_1",
			thread_id: overrides.thread_id ?? "thread_1",
			workspace_id: overrides.workspace_id ?? "workspace_1",
		},
		request_id: overrides.request_id ?? "request_1",
		tool: { revision: 1, tool_id: "workspace.read" },
	};
}

const SeedOrdinary = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: "thread_1",
		title: "Tool control",
		title_source: "initial",
		updated_at: now,
	});
	yield* database.client.insert(Projects).values({
		canonical_root: "C:/artisan",
		display_name: "Artisan",
		project_id: "project_1",
		registered_at: now,
		updated_at: now,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_1",
		created_at: now,
		engine_id: "codex",
		run_id: "run_1",
		status: "running",
		thread_id: "thread_1",
		updated_at: now,
		working_directory: "C:/artisan",
	});
});

const SeedGraph = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(OrchestrationGroups).values({
		coordinator_agent_id: "coordinator_1",
		created_at: now,
		group_id: "group_1",
		journal_sequence: 1,
		max_concurrency: 1,
		state: "running",
		thread_id: "thread_1",
		updated_at: now,
		version: 1,
	});
	yield* database.client.insert(AgentInstances).values({
		agent_id: "graph_agent_1",
		created_at: now,
		display_name: "Graph agent",
		group_id: "group_1",
		role: "worker",
		updated_at: now,
	});
	yield* database.client.insert(Assignments).values({
		active_run_id: "graph_run_1",
		agent_id: "graph_agent_1",
		assignment_id: "assignment_1",
		created_at: now,
		current_attempt: 1,
		engine_id: "codex",
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
		updated_at: now,
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
		created_at: now,
		dispatch_status: "active",
		engine_id: "codex",
		group_id: "group_1",
		last_observation_sequence: 0,
		profile: "default",
		run_id: "graph_run_1",
		state: "running",
		updated_at: now,
	});
});

afterEach(async () => {
	for (const directory of directories.splice(0)) {
		await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		);
	}
});

describe("ToolControlRepository", () => {
	it("persists source-safe automatic events and exact replay without exposing private arguments", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const repository = yield* ToolControlRepository;
					const accepted = yield* repository.Prepare({
						descriptor: automatic_descriptor,
						recovery_policy: "retry",
						request: request(),
					});
					const replay = yield* repository.Prepare({
						descriptor: automatic_descriptor,
						recovery_policy: "retry",
						request: request(),
					});
					const database = yield* Database;
					const events = yield* database.client.select().from(JournalEvents);
					const private_rows = yield* database.client
						.select()
						.from(ToolInvocationPrivate);

					return { accepted, events, private_rows, replay };
				}),
			);

			expect(result.accepted).toMatchObject({
				invocation: { state: "pending" },
				status: "accepted",
			});
			expect(result.replay).toMatchObject({
				invocation: { invocation_id: result.accepted.invocation.invocation_id },
				status: "duplicate",
			});
			expect(result.events.map((event) => event.event_type)).toEqual([
				"capability.invocation.updated",
				"tool.invocation.updated",
			]);
			expect(JSON.stringify(result.events)).not.toContain("private-token");
			expect(result.private_rows).toHaveLength(1);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("authorizes graph ownership and makes required approval decisions durable", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					yield* SeedGraph;
					const repository = yield* ToolControlRepository;
					const graph_request = {
						...request({
							agent_id: "graph_agent_1",
							run_id: "graph_run_1",
							request_id: "request_graph",
						}),
						tool: { revision: 1, tool_id: "workspace.replace" },
					};
					const prepared = yield* repository.Prepare({
						descriptor: required_descriptor,
						recovery_policy: "outcome_unknown",
						request: graph_request,
					});
					const decided = yield* repository.Decide({
						approval_id: prepared.approval!.approval_id,
						decision: "approved",
						decision_id: "decision_1",
						thread_id: "thread_1",
					});
					const database = yield* Database;
					const events = yield* database.client.select().from(JournalEvents);

					return { decided, events };
				}),
			);

			expect(result.decided).toMatchObject({
				approval: { state: "approved" },
				invocation: { state: "pending" },
				status: "accepted",
			});
			expect(result.events.map((event) => event.event_type)).toEqual([
				"tool.approval.updated",
				"capability.invocation.updated",
				"tool.invocation.updated",
				"tool.approval.updated",
				"capability.invocation.updated",
				"tool.invocation.updated",
			]);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("rejects changed intent, hides cross-thread rows, and fences erased threads", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const repository = yield* ToolControlRepository;
					const prepared = yield* repository.Prepare({
						descriptor: automatic_descriptor,
						recovery_policy: "retry",
						request: request(),
					});
					const hidden = yield* repository.QueryInvocation({
						invocation_id: prepared.invocation.invocation_id,
						thread_id: "thread_other",
					});
					const changed = yield* Effect.exit(
						repository.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: { ...request(), arguments: { query: "changed" } },
						}),
					);
					const database = yield* Database;
					yield* database.client
						.insert(ThreadErasureClaims)
						.values({ claimed_at: now, thread_id: "thread_1" });
					const erased = yield* Effect.exit(
						repository.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: request({ request_id: "request_erased" }),
						}),
					);

					return { changed, erased, hidden };
				}),
			);

			expect(result.hidden).toEqual({});
			expect(result.changed._tag).toBe("Failure");
			expect(result.erased._tag).toBe("Failure");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("fences exact retries and public queries once thread erasure begins", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const repository = yield* ToolControlRepository;
					const approval_request = {
						...request({ request_id: "request_erasure_replay" }),
						tool: { revision: 1, tool_id: "workspace.replace" },
					};
					const prepared = yield* repository.Prepare({
						descriptor: required_descriptor,
						recovery_policy: "outcome_unknown",
						request: approval_request,
					});
					const decision = {
						approval_id: prepared.approval!.approval_id,
						decision: "approved" as const,
						decision_id: "decision_erasure_replay",
						thread_id: "thread_1",
					};

					yield* repository.Decide(decision);

					const database = yield* Database;

					yield* database.client
						.insert(ThreadErasureClaims)
						.values({ claimed_at: now, thread_id: "thread_1" });

					return {
						approval_query: yield* Effect.exit(
							repository.QueryApproval({
								approval_id: prepared.approval!.approval_id,
								thread_id: "thread_1",
							}),
						),
						decision_replay: yield* Effect.exit(repository.Decide(decision)),
						invocation_query: yield* Effect.exit(
							repository.QueryInvocation({
								invocation_id: prepared.invocation.invocation_id,
								thread_id: "thread_1",
							}),
						),
						prepare_replay: yield* Effect.exit(
							repository.Prepare({
								descriptor: required_descriptor,
								recovery_policy: "outcome_unknown",
								request: approval_request,
							}),
						),
					};
				}),
			);

			expect(result.prepare_replay._tag).toBe("Failure");
			expect(result.decision_replay._tag).toBe("Failure");
			expect(result.invocation_query._tag).toBe("Failure");
			expect(result.approval_query._tag).toBe("Failure");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("rejects stale graph ownership without writing tool state", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					yield* SeedGraph;
					const database = yield* Database;
					const repository = yield* ToolControlRepository;
					const graph_request = request({
						agent_id: "graph_agent_1",
						run_id: "graph_run_1",
						request_id: "request_stale_group",
					});

					yield* database.client.run(
						"UPDATE orchestration_groups SET state = 'stopped' WHERE group_id = 'group_1'",
					);
					const stopped_group = yield* Effect.exit(
						repository.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: graph_request,
						}),
					);

					yield* database.client.run(
						"UPDATE orchestration_groups SET state = 'running' WHERE group_id = 'group_1'",
					);
					yield* database.client.run(
						"UPDATE assignments SET state = 'stopped' WHERE assignment_id = 'assignment_1'",
					);
					const stopped_assignment = yield* Effect.exit(
						repository.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: { ...graph_request, request_id: "request_stale_assignment" },
						}),
					);

					return {
						events: yield* database.client.select().from(JournalEvents),
						invocations: yield* database.client.select().from(ToolInvocations),
						stopped_assignment,
						stopped_group,
					};
				}),
			);

			expect(result.stopped_group._tag).toBe("Failure");
			expect(result.stopped_assignment._tag).toBe("Failure");
			expect(result.invocations).toEqual([]);
			expect(result.events).toEqual([]);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("keeps exact admission receipts stable but rejects first approval after a run stops", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const repository = yield* ToolControlRepository;
					const approval_request = {
						...request({ request_id: "request_stale_approval" }),
						tool: { revision: 1, tool_id: "workspace.replace" },
					};
					const prepared = yield* repository.Prepare({
						descriptor: required_descriptor,
						recovery_policy: "outcome_unknown",
						request: approval_request,
					});
					const database = yield* Database;

					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'completed' WHERE run_id = 'run_1'",
					);

					const replay = yield* repository.Prepare({
						descriptor: required_descriptor,
						recovery_policy: "outcome_unknown",
						request: approval_request,
					});
					const decision = yield* Effect.exit(
						repository.Decide({
							approval_id: prepared.approval!.approval_id,
							decision: "approved",
							decision_id: "decision_stale_approval",
							thread_id: "thread_1",
						}),
					);

					return {
						decision,
						events: yield* database.client.select().from(JournalEvents),
						replay,
					};
				}),
			);

			expect(result.replay.status).toBe("duplicate");
			expect(result.decision._tag).toBe("Failure");
			expect(result.events).toHaveLength(3);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("converges concurrent exact prepares from separate runtimes without duplicating admission state", async () => {
		const database_path = await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			MakeDatabasePath,
		);
		const first_runtime = runtime(database_path);
		const second_runtime = runtime(database_path);

		try {
			await first_runtime.runPromise(SeedOrdinary);

			const [first, second] = await Promise.all([
				first_runtime.runPromise(
					ToolControlRepository.pipe(
						Effect.flatMap((repository) =>
							repository.Prepare({
								descriptor: automatic_descriptor,
								recovery_policy: "retry",
								request: request({ request_id: "request_concurrent_prepare" }),
							}),
						),
					),
				),
				second_runtime.runPromise(
					ToolControlRepository.pipe(
						Effect.flatMap((repository) =>
							repository.Prepare({
								descriptor: automatic_descriptor,
								recovery_policy: "retry",
								request: request({ request_id: "request_concurrent_prepare" }),
							}),
						),
					),
				),
			]);
			const durable = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						commands: yield* database.client.select().from(ToolControlCommands),
						events: yield* database.client.select().from(JournalEvents),
						invocations: yield* database.client.select().from(ToolInvocations),
						private_rows: yield* database.client.select().from(ToolInvocationPrivate),
					};
				}),
			);

			expect([first.status, second.status].sort()).toEqual(["accepted", "duplicate"]);
			expect(first.invocation.invocation_id).toBe(second.invocation.invocation_id);
			expect(durable.invocations).toHaveLength(1);
			expect(durable.private_rows).toHaveLength(1);
			expect(durable.commands).toHaveLength(1);
			expect(durable.events.map((event) => event.event_type)).toEqual([
				"capability.invocation.updated",
				"tool.invocation.updated",
			]);
		} finally {
			await first_runtime.dispose();
			await second_runtime.dispose();
		}
	});

	it("accepts one of two conflicting concurrent first decisions without writing a partial loser event set", async () => {
		const database_path = await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			MakeDatabasePath,
		);
		const first_runtime = runtime(database_path);
		const second_runtime = runtime(database_path);

		try {
			const approval_id = await first_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const repository = yield* ToolControlRepository;
					const prepared = yield* repository.Prepare({
						descriptor: required_descriptor,
						recovery_policy: "outcome_unknown",
						request: {
							...request({ request_id: "request_concurrent_decision" }),
							tool: { revision: 1, tool_id: "workspace.replace" },
						},
					});

					return prepared.approval!.approval_id;
				}),
			);
			const [approved, denied] = await Promise.all([
				first_runtime.runPromise(
					ToolControlRepository.pipe(
						Effect.flatMap((repository) =>
							repository
								.Decide({
									approval_id,
									decision: "approved",
									decision_id: "decision_concurrent_approved",
									thread_id: "thread_1",
								})
								.pipe(Effect.exit),
						),
					),
				),
				second_runtime.runPromise(
					ToolControlRepository.pipe(
						Effect.flatMap((repository) =>
							repository
								.Decide({
									approval_id,
									decision: "denied",
									decision_id: "decision_concurrent_denied",
									thread_id: "thread_1",
								})
								.pipe(Effect.exit),
						),
					),
				),
			]);
			const durable = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						commands: yield* database.client.select().from(ToolControlCommands),
						events: yield* database.client.select().from(JournalEvents),
						invocations: yield* database.client.select().from(ToolInvocations),
					};
				}),
			);
			const exits = [approved, denied];
			const accepted = exits.find((exit) => exit._tag === "Success");
			const rejected = exits.find((exit) => exit._tag === "Failure");

			expect(accepted?._tag).toBe("Success");
			expect(rejected?._tag).toBe("Failure");
			expect(JSON.stringify(rejected)).toContain("ToolControlConflict");
			expect(JSON.stringify(rejected)).toContain("changed_intent");
			expect(durable.invocations).toHaveLength(1);
			expect(durable.commands).toHaveLength(2);
			expect(durable.events).toHaveLength(6);
			expect(
				durable.events.filter(
					(event) =>
						event.causation_id === "decision_concurrent_approved" ||
						event.causation_id === "decision_concurrent_denied",
				),
			).toHaveLength(3);
		} finally {
			await first_runtime.dispose();
			await second_runtime.dispose();
		}
	});

	it.each([
		["arguments digest", "arguments_digest", "a".repeat(64)],
		["arguments JSON", "arguments_json", JSON.stringify({ query: "private-token-tampered" })],
		["request fingerprint", "request_fingerprint", "b".repeat(64)],
	] as const)(
		"fails closed when a private %s is tampered without serializing private arguments",
		async (_, column, value) => {
			const current_runtime = runtime(
				await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
			);

			try {
				const result = await current_runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedOrdinary;
						const repository = yield* ToolControlRepository;
						const prepared = yield* repository.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: request({ request_id: `request_tampered_${column}` }),
						});
						const database = yield* Database;

						yield* database.client.run(
							`UPDATE tool_invocation_private SET ${column} = '${value}' WHERE invocation_id = '${prepared.invocation.invocation_id}'`,
						);

						return yield* repository
							.Prepare({
								descriptor: automatic_descriptor,
								recovery_policy: "retry",
								request: request({ request_id: `request_tampered_${column}` }),
							})
							.pipe(Effect.exit);
					}),
				);

				expect(result._tag).toBe("Failure");
				expect(JSON.stringify(result)).not.toContain("private-token");
			} finally {
				await current_runtime.dispose();
			}
		},
	);

	it("rolls back a prepare whose journal idempotency key collides without changing thread activity or its stream", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const database = yield* Database;

					yield* database.client.insert(JournalEvents).values({
						agent_id: "agent_1",
						causation_id: "collision",
						correlation_id: "collision",
						event_id: "event_collision",
						event_type: "collision",
						idempotency_key: "tool_invocation:invocation_test_1:started",
						occurred_at: now,
						origin: "backend",
						payload_json: JSON.stringify({ type: "collision" }),
						raw_origin_json: null,
						run_id: "run_1",
						schema_version: 1,
						stream_id: "collision_stream",
						stream_sequence: 1,
						thread_id: "thread_1",
					});
					const before = {
						events: yield* database.client.select().from(JournalEvents),
						streams: yield* database.client.select().from(EventStreams),
						threads: yield* database.client.select().from(Threads),
					};
					const repository = yield* ToolControlRepository;
					const failed = yield* repository
						.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: request({ request_id: "request_journal_collision" }),
						})
						.pipe(Effect.exit);

					return {
						after: {
							commands: yield* database.client.select().from(ToolControlCommands),
							events: yield* database.client.select().from(JournalEvents),
							invocations: yield* database.client.select().from(ToolInvocations),
							private_rows: yield* database.client
								.select()
								.from(ToolInvocationPrivate),
							streams: yield* database.client.select().from(EventStreams),
							threads: yield* database.client.select().from(Threads),
						},
						before,
						failed,
					};
				}),
			);

			expect(result.failed._tag).toBe("Failure");
			expect(result.after.invocations).toEqual([]);
			expect(result.after.private_rows).toEqual([]);
			expect(result.after.commands).toEqual([]);
			expect(result.after.events).toEqual(result.before.events);
			expect(result.after.streams).toEqual(result.before.streams);
			expect(result.after.threads).toEqual(result.before.threads);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("keeps a committed prepare durable when its notifier defects, then returns its duplicate receipt after restart", async () => {
		const database_path = await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			MakeDatabasePath,
		);
		const defecting_runtime = runtime(
			database_path,
			Layer.succeed(JournalNotifier, {
				Publish: () => Effect.die("notifier defect"),
				Subscribe: Effect.die("unused"),
			}),
		);

		try {
			const defect = await defecting_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedOrdinary;
					const repository = yield* ToolControlRepository;

					return yield* repository
						.Prepare({
							descriptor: automatic_descriptor,
							recovery_policy: "retry",
							request: request({ request_id: "request_notifier_restart" }),
						})
						.pipe(Effect.exit);
				}),
			);

			expect(defect._tag).toBe("Failure");
		} finally {
			await defecting_runtime.dispose();
		}

		const restarted_runtime = runtime(database_path);

		try {
			const result = await restarted_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ToolControlRepository;
					const invocation = yield* repository.QueryInvocation({
						invocation_id: "invocation_test_1",
						thread_id: "thread_1",
					});
					const replay = yield* repository.Prepare({
						descriptor: automatic_descriptor,
						recovery_policy: "retry",
						request: request({ request_id: "request_notifier_restart" }),
					});

					return { invocation, replay };
				}),
			);

			expect(result.invocation.invocation?.state).toBe("pending");
			expect(result.replay).toMatchObject({
				invocation: { invocation_id: "invocation_test_1" },
				status: "duplicate",
			});
		} finally {
			await restarted_runtime.dispose();
		}
	});
});
