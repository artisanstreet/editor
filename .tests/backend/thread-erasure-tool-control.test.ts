import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	AgentRuns,
	JournalEvents,
	OrchestrationGroups,
	OrchestrationRuns,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	ToolControlCommands,
	ToolExecutionClaims,
	ToolInvocationPrivate,
	ToolInvocations,
} from "../../modules/backend/src/persistence/schema";
import {
	ThreadErasure,
	ThreadErasureFailure,
	ThreadErasureLive,
} from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const created_at = "2026-07-16T12:00:00.000Z";
const cutoff = "2026-07-16T12:01:00.000Z";
const deleted_at = "2026-07-16T12:02:00.000Z";
const digest = "a".repeat(64);

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-tool-control-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(
	database_path: string,
	quiesce: (thread_id: string) => Effect.Effect<void, never, Database> = () => Effect.void,
) {
	const persistence = make_database_layer({ database_path, migrations_path });
	const resource_quiescer = Layer.effect(
		ThreadResourceQuiescer,
		Effect.gen(function* () {
			const database = yield* Database;

			return {
				Quiesce: (thread_id: string) =>
					quiesce(thread_id).pipe(Effect.provideService(Database, database)),
			};
		}),
	).pipe(Layer.provide(persistence));
	const infrastructure = Layer.mergeAll(persistence, JournalNotifierLive, resource_quiescer);

	return ManagedRuntime.make(ThreadErasureLive.pipe(Layer.provideMerge(infrastructure)));
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected failure");
}

const SeedThread = (thread_id: string, pinned = false) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			pinned,
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(EventStreams).values({
			last_sequence: 0,
			stream_id: `thread:${thread_id}`,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: `agent_${thread_id}`,
			created_at,
			engine_id: "codex",
			run_id: `run_${thread_id}`,
			status: "completed",
			thread_id,
			updated_at: created_at,
			working_directory: "C:/artisan",
		});
	});

const SeedOrdinaryRun = (thread_id: string, run_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: `agent_${thread_id}_${run_id}`,
			created_at,
			engine_id: "codex",
			run_id,
			status: "completed",
			thread_id,
			updated_at: created_at,
			working_directory: "C:/artisan",
		});
	});

const SeedGraphRun = (thread_id: string, run_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const group_id = `group_${thread_id}_${run_id}`;

		yield* database.client.insert(OrchestrationGroups).values({
			coordinator_agent_id: `coordinator_${thread_id}_${run_id}`,
			created_at,
			group_id,
			journal_sequence: 1,
			max_concurrency: 1,
			state: "completed",
			thread_id,
			updated_at: created_at,
			version: 1,
		});
		yield* database.client.insert(AgentRuns).values({
			agent_id: `agent_${thread_id}_${run_id}`,
			assignment_id: `assignment_${thread_id}_${run_id}`,
			attempt: 1,
			completed_at: created_at,
			created_at,
			dispatch_status: "completed",
			engine_id: "codex",
			group_id,
			last_observation_sequence: 0,
			profile: "default",
			run_id,
			state: "completed",
			updated_at: created_at,
		});
	});

const SeedInvocation = (
	thread_id: string,
	state: "approval_required" | "pending" | "running" | "suspended" | "completed",
	options: {
		readonly owner_kind?: "ordinary_run" | "graph_run";
		readonly private_values?: boolean;
		readonly retained_claim?: boolean;
		readonly run_id?: string;
	} = {},
) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const invocation_id = `invocation_${thread_id}_${state}`;
		const requires_approval = state === "approval_required";
		const started_at = state === "pending" || state === "approval_required" ? null : created_at;
		const suspended_at = state === "suspended" ? created_at : null;
		const settled_at = state === "completed" ? created_at : null;

		yield* database.client.insert(ToolInvocations).values({
			agent_id: `agent_${thread_id}`,
			approval_id: requires_approval ? `approval_${thread_id}_${state}` : null,
			approval_policy: requires_approval ? "required" : "automatic",
			created_at,
			current_journal_sequence: 1,
			decided_at: null,
			decision: null,
			decision_id: null,
			descriptor_fingerprint: digest,
			effect: "read",
			input_schema_json: '{"type":"object"}',
			invocation_id,
			label: "Read workspace",
			owner_kind: options.owner_kind ?? "ordinary_run",
			recovery_policy: "retry",
			request_id: `request_${thread_id}_${state}`,
			revision: 1,
			run_id: options.run_id ?? `run_${thread_id}`,
			settled_at,
			source: "artisan",
			started_at,
			state,
			summary: "Reads a bounded workspace view",
			suspended_at,
			thread_id,
			tool_id: "workspace.read",
			updated_at: created_at,
			workspace_id: "workspace_1",
		});

		if (options.private_values) {
			yield* database.client.insert(ToolInvocationPrivate).values({
				arguments_digest: digest,
				arguments_json: '{"token":"private-argument"}',
				invocation_id,
				request_fingerprint: digest,
				result_digest: digest,
				result_json: '{"token":"private-result"}',
			});
			yield* database.client.insert(ToolControlCommands).values({
				accepted_at: created_at,
				approval_id: null,
				command_id: `command_${thread_id}`,
				decision: null,
				invocation_id,
				kind: "invoke",
				request_fingerprint: digest,
			});
		}

		if (options.retained_claim) {
			yield* database.client.insert(ToolExecutionClaims).values({
				claim_token: `claim_${thread_id}`,
				claimed_at: created_at,
				invocation_id,
				lease_expires_at: deleted_at,
				launch_started_at: created_at,
				owner_instance_id: "backend_1",
			});
		}
	});

const SeedErasureClaim = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(ThreadErasureClaims).values({
			claimed_at: deleted_at,
			thread_id,
		});
	});

const ReadState = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		return yield* Effect.all({
			claims: database.client
				.select()
				.from(ThreadErasureClaims)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			commands: database.client
				.select()
				.from(ToolControlCommands)
				.pipe(
					Effect.map((rows) => rows.filter((row) => row.command_id.includes(thread_id))),
				),
			execution_claims: database.client
				.select()
				.from(ToolExecutionClaims)
				.pipe(
					Effect.map((rows) => rows.filter((row) => row.claim_token.includes(thread_id))),
				),
			invocations: database.client
				.select()
				.from(ToolInvocations)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			private_rows: database.client
				.select()
				.from(ToolInvocationPrivate)
				.pipe(
					Effect.map((rows) =>
						rows.filter((row) => row.invocation_id.includes(thread_id)),
					),
				),
			threads: database.client
				.select()
				.from(Threads)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
			tombstones: database.client
				.select()
				.from(ThreadTombstones)
				.pipe(Effect.map((rows) => rows.filter((row) => row.thread_id === thread_id))),
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

describe("ThreadErasure tool control", () => {
	it.each([
		["ordinary_run", "declared_target"],
		["graph_run", "declared_target"],
		["ordinary_run", "referenced_target"],
		["graph_run", "referenced_target"],
	] as const)(
		"fails closed for a %s invocation with %s ownership despite an overlapping run id",
		async (owner_kind, direction) => {
			const database_path = await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
				MakeDatabasePath,
			);
			const runtime = make_runtime(database_path);
			const target_thread_id = `thread_target_${owner_kind}_${direction}`;
			const unrelated_thread_id = `thread_unrelated_${owner_kind}_${direction}`;
			const invocation_thread_id =
				direction === "declared_target" ? target_thread_id : unrelated_thread_id;
			const owner_thread_id =
				direction === "declared_target" ? unrelated_thread_id : target_thread_id;
			const overlap_thread_id = invocation_thread_id;
			const shared_run_id = `run_shared_${owner_kind}_${direction}`;

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedThread(target_thread_id);
						yield* SeedThread(unrelated_thread_id, true);

						if (owner_kind === "ordinary_run") {
							yield* SeedOrdinaryRun(owner_thread_id, shared_run_id);
							yield* SeedGraphRun(overlap_thread_id, shared_run_id);
						} else {
							yield* SeedGraphRun(owner_thread_id, shared_run_id);
							yield* SeedOrdinaryRun(overlap_thread_id, shared_run_id);
						}

						yield* SeedInvocation(invocation_thread_id, "completed", {
							owner_kind,
							run_id: shared_run_id,
						});
						yield* SeedErasureClaim(target_thread_id);

						const outcome = yield* (yield* ThreadErasure)
							.ResumeClaimed(deleted_at)
							.pipe(Effect.exit);

						return {
							outcome,
							target: yield* ReadState(target_thread_id),
							unrelated: yield* ReadState(unrelated_thread_id),
						};
					}),
				);

				expect(failure_from(result.outcome)).toBeInstanceOf(ThreadErasureFailure);
				expect(result.target.claims).toHaveLength(1);
				expect(result.target.threads).toHaveLength(1);
				expect(result.target.tombstones).toEqual([]);
				expect(result.unrelated.threads).toHaveLength(1);
				expect(result.unrelated.tombstones).toEqual([]);
				expect([
					...result.target.invocations,
					...result.unrelated.invocations,
				]).toHaveLength(1);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it.each(["approval_required", "pending", "running", "suspended"] as const)(
		"excludes a thread with a %s tool invocation from retention claims",
		async (state) => {
			const runtime = make_runtime(
				await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
			);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedThread(`thread_${state}`);
						yield* SeedInvocation(`thread_${state}`, state);

						const erased = yield* (yield* ThreadErasure).CleanupExpired(
							cutoff,
							deleted_at,
						);
						const durable = yield* ReadState(`thread_${state}`);

						return { durable, erased };
					}),
				);

				expect(result.erased).toEqual([]);
				expect(result.durable.claims).toEqual([]);
				expect(result.durable.threads).toHaveLength(1);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("releases a claim when durable pending work appears after live quiescence", async () => {
		const thread_id = "thread_race";
		const runtime = make_runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
			(received_thread_id) =>
				received_thread_id === thread_id
					? SeedInvocation(thread_id, "pending").pipe(Effect.orDie)
					: Effect.void,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(thread_id);
					yield* SeedInvocation(thread_id, "completed");

					const erased = yield* (yield* ThreadErasure).CleanupExpired(cutoff, deleted_at);
					const durable = yield* ReadState(thread_id);

					return { durable, erased };
				}),
			);

			expect(result.erased).toEqual([]);
			expect(result.durable.claims).toEqual([]);
			expect(result.durable.invocations.map((row) => row.state)).toEqual([
				"completed",
				"pending",
			]);
			expect(result.durable.threads).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("deeply erases terminal tool state without preserving private values or unrelated threads", async () => {
		const target_thread_id = "thread_target";
		const unrelated_thread_id = "thread_unrelated";
		const runtime = make_runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread(target_thread_id);
					yield* SeedThread(unrelated_thread_id, true);
					yield* SeedInvocation(target_thread_id, "completed", {
						private_values: true,
						retained_claim: true,
					});
					yield* SeedInvocation(unrelated_thread_id, "completed", {
						private_values: true,
						retained_claim: true,
					});

					const erased = yield* (yield* ThreadErasure).CleanupExpired(cutoff, deleted_at);
					const database = yield* Database;
					const events = yield* database.client.select().from(JournalEvents);

					return {
						erased,
						events,
						target: yield* ReadState(target_thread_id),
						unrelated: yield* ReadState(unrelated_thread_id),
					};
				}),
			);

			expect(result.erased).toContain(target_thread_id);
			expect(result.target).toMatchObject({
				commands: [],
				execution_claims: [],
				invocations: [],
				private_rows: [],
				threads: [],
				tombstones: [{ thread_id: target_thread_id }],
			});
			expect(result.unrelated).toMatchObject({
				commands: [{ command_id: `command_${unrelated_thread_id}` }],
				execution_claims: [{ claim_token: `claim_${unrelated_thread_id}` }],
				invocations: [{ thread_id: unrelated_thread_id }],
				private_rows: [{ invocation_id: `invocation_${unrelated_thread_id}_completed` }],
				threads: [{ thread_id: unrelated_thread_id }],
			});
			expect(JSON.stringify(result.events)).not.toContain("private-argument");
			expect(JSON.stringify(result.events)).not.toContain("private-result");
		} finally {
			await runtime.dispose();
		}
	});
});
