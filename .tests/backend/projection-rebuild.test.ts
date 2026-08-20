import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	ProjectionRebuildService,
	ProjectionRebuildBarrier,
	ProjectionRebuildBarrierLive,
	ProjectionRebuildServiceLive,
} from "../../modules/backend/src/persistence/projection-rebuild-service";
import {
	EventStreams,
	GitWorkspaceProjections,
	GitMutationOperations,
	JournalEvents,
	JournalCommands,
	LegacyWorkspaceChangeProjections,
	OrchestrationCoordinators,
	OrchestrationRuns,
	ProjectionRebuildLocks,
	Threads,
	WorkspaceChangeDiffs,
	WorkspaceChangeOperations,
	WorkspaceChangeSnapshots,
	WorkspaceChanges,
	WorkspaceMutationAuthorities,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const timestamp = "2026-07-18T12:00:00.000Z";
const hash_a = "a".repeat(64);
const hash_b = "b".repeat(64);
const empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-projection-rebuild-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

function make_runtime(database_path: string, barrier = ProjectionRebuildBarrierLive) {
	const database = make_database_layer({ database_path, migrations_path });
	return ManagedRuntime.make(
		Layer.merge(
			database,
			ProjectionRebuildServiceLive.pipe(Layer.provideMerge(barrier), Layer.provide(database)),
		),
	);
}

const thread = {
	affinity_version: 0,
	created_at: timestamp,
	last_activity_at: timestamp,
	live_status: "Idle",
	metadata_version: 0,
	pinned: false,
	project_affinity_scores: [],
	project_locked: false,
	linked_projects: [],
	thread_id: "thread_1",
	title: "Thread one",
	title_locked: false,
	title_source: "initial" as const,
	updated_at: timestamp,
	activity_version: 0,
	current_goal: "Thread one",
};

const change = {
	after_identity: { algorithm: "sha256" as const, byte_count: 2, content_hash: hash_b },
	agent_id: "agent_1",
	before_identity: { algorithm: "sha256" as const, byte_count: 1, content_hash: hash_a },
	change_id: "change_1",
	created_at: timestamp,
	path: "src/example.ts",
	review_state: "needs_review" as const,
	rollback_state: "available" as const,
	run_id: "run_1",
	source_command_id: "replace_1",
	thread_id: "thread_1",
	updated_at: timestamp,
	version: 1,
	workspace_id: "workspace_1",
};

const workspace = {
	journal_sequence: 3,
	observed_at: timestamp,
	repository_state: "not_repository" as const,
	snapshot_id: hash_a,
	version: 1,
	workspace_id: "workspace_1",
};

function Seed(database: Database["Service"]) {
	return Effect.gen(function* () {
		yield* database.client.insert(EventStreams).values({
			last_sequence: 4,
			stream_id: "thread:thread_1",
		});
		yield* database.client.insert(JournalEvents).values([
			{
				causation_id: "create_1",
				correlation_id: "create_1",
				event_id: "event_1",
				event_type: "thread.created",
				occurred_at: timestamp,
				origin: "backend",
				payload_json: JSON.stringify({ type: "thread.created", title: thread.title }),
				raw_origin_json: null,
				schema_version: 1,
				stream_id: "thread:thread_1",
				stream_sequence: 1,
				thread_id: "thread_1",
			},
			{
				causation_id: "replace_1",
				correlation_id: "replace_1",
				event_id: "event_2",
				event_type: "workspace.change.updated",
				occurred_at: timestamp,
				origin: "backend",
				payload_json: JSON.stringify({
					action: "recorded",
					change,
					type: "workspace.change.updated",
				}),
				raw_origin_json: null,
				schema_version: 1,
				stream_id: "thread:thread_1",
				stream_sequence: 2,
				thread_id: "thread_1",
			},
			{
				causation_id: "git_1",
				correlation_id: "git_1",
				event_id: "event_3",
				event_type: "git.workspace.updated",
				occurred_at: timestamp,
				origin: "backend",
				payload_json: JSON.stringify({
					cause: "refresh",
					type: "git.workspace.updated",
					workspace,
				}),
				raw_origin_json: null,
				schema_version: 1,
				stream_id: "thread:thread_1",
				stream_sequence: 3,
				thread_id: "thread_1",
			},
			{
				causation_id: "run_1",
				correlation_id: "run_1",
				event_id: "event_4",
				event_type: "run.lifecycle",
				occurred_at: "2026-07-18T12:01:00.000Z",
				origin: "backend",
				payload_json: JSON.stringify({
					state: "running",
					summary_title: "Harness summary",
					type: "run.lifecycle",
					working_directory: "C:/workspace",
				}),
				raw_origin_json: null,
				schema_version: 1,
				stream_id: "thread:thread_1",
				stream_sequence: 4,
				thread_id: "thread_1",
			},
		]);
		yield* database.client.insert(JournalCommands).values({
			accepted_at: timestamp,
			causation_id: null,
			message_id: "dedupe_1",
			origin: "frontend",
			payload_json: '{"type":"thread.activity.record","activity_kind":"renamed"}',
			payload_type: "thread.activity.record",
			raw_origin_json: null,
			schema_version: 1,
			sent_at: timestamp,
			status: "accepted",
			thread_id: "thread_1",
		});
		yield* database.client.insert(Threads).values({ ...thread, title: "corrupt" });
		yield* database.client.insert(WorkspaceChanges).values({
			after_identity_json: JSON.stringify(change.after_identity),
			agent_id: change.agent_id,
			before_identity_json: JSON.stringify(change.before_identity),
			change_id: change.change_id,
			created_at: change.created_at,
			diff_state: "available",
			path: "corrupt.ts",
			raw_origin_json: null,
			review_state: change.review_state,
			reviewed_at: null,
			rollback_state: change.rollback_state,
			rolled_back_at: null,
			run_id: change.run_id,
			source_command_id: change.source_command_id,
			thread_id: change.thread_id,
			updated_at: change.updated_at,
			version: change.version,
			workspace_id: change.workspace_id,
		});
		yield* database.client.insert(WorkspaceChangeOperations).values({
			action: "replace",
			change_id: change.change_id,
			created_at: timestamp,
			lifecycle: "committed",
			message_id: change.source_command_id,
			request_fingerprint: hash_a,
			sent_at: timestamp,
			thread_id: change.thread_id,
			updated_at: timestamp,
		});
		yield* database.client.insert(WorkspaceChangeDiffs).values({
			added_line_count: 0,
			after_identity_json: JSON.stringify(change.after_identity),
			before_identity_json: JSON.stringify(change.before_identity),
			change_id: change.change_id,
			context_lines: 3,
			created_at: timestamp,
			format: "unified",
			format_version: 1,
			patch: Buffer.from(""),
			patch_byte_count: 0,
			patch_hash: empty_hash,
			path: change.path,
			removed_line_count: 0,
			source_command_id: change.source_command_id,
			thread_id: change.thread_id,
			workspace_id: change.workspace_id,
		});
		yield* database.client.insert(GitWorkspaceProjections).values({
			journal_sequence: 3,
			observed_at: timestamp,
			projection_json: JSON.stringify({ ...workspace, snapshot_id: hash_b }),
			snapshot_id: hash_b,
			updated_at: timestamp,
			version: 1,
			workspace_id: workspace.workspace_id,
		});
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ProjectionRebuildService", () => {
	it("rebuilds the supported projections while preserving immutable diff state", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					yield* database.client.delete(Threads);
					yield* database.client.delete(GitWorkspaceProjections);
					const before = yield* rebuild.Verify();
					const repaired = yield* rebuild.Rebuild();
					const after = yield* rebuild.Verify();
					const [stored_thread] = yield* database.client.select().from(Threads);
					const [stored_change] = yield* database.client.select().from(WorkspaceChanges);
					const diffs = yield* database.client.select().from(WorkspaceChangeDiffs);
					return { after, before, diffs, repaired, stored_change, stored_thread };
				}),
			);
			expect(result.before.equivalent).toBe(false);
			expect(result.repaired).toMatchObject({ equivalent: true, rebuilt: true });
			expect(result.after.equivalent).toBe(true);
			expect(result.stored_thread?.title).toBe(thread.title);
			expect(result.stored_thread).toMatchObject({
				activity_version: 1,
				last_activity_at: "2026-07-18T12:01:00.000Z",
				metadata_version: 1,
				summary_title: "Harness summary",
			});
			expect(result.stored_change?.path).toBe(change.path);
			expect(result.diffs).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("derives rebuilt thread status from root-run authority instead of stale events", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent_1",
						created_at: timestamp,
						engine_id: "engine_1",
						last_observation_sequence: 1,
						run_id: "run_authority",
						status: "completed",
						thread_id: thread.thread_id,
						updated_at: timestamp,
						working_directory: "C:/workspace",
					});
					yield* database.client.insert(OrchestrationCoordinators).values({
						active_run_id: "run_authority",
						agent_id: "agent_1",
						created_at: timestamp,
						display_name: "Primary coordinator",
						engine_id: "engine_1",
						role: "primary",
						thread_id: thread.thread_id,
						updated_at: timestamp,
					});
					yield* database.client.update(Threads).set({ live_status: "Working" });

					const repaired = yield* rebuild.Rebuild();
					const [stored] = yield* database.client.select().from(Threads);

					return { repaired, stored };
				}),
			);

			expect(result.repaired).toMatchObject({ equivalent: true, rebuilt: true });
			expect(result.stored).toMatchObject({ live_status: "Complete" });
		} finally {
			await runtime.dispose();
		}
	});

	it("refuses journal corruption without changing projections", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					const before = yield* database.client.select().from(Threads);
					yield* database.client.update(JournalEvents).set({ payload_json: "{}" });
					const outcome = yield* rebuild.Rebuild().pipe(Effect.exit);
					const after = yield* database.client.select().from(Threads);
					return { after, before, outcome };
				}),
			);
			expect(result.outcome._tag).toBe("Failure");
			expect(result.after).toEqual(result.before);
		} finally {
			await runtime.dispose();
		}
	});

	it("rolls back Git cursor corruption without changing projections or the writer lease", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					const before = yield* database.client.select().from(GitWorkspaceProjections);
					yield* database.client.run(
						'UPDATE journal_events SET payload_json = \'{"type":"git.workspace.updated","cause":"refresh","workspace":{"workspace_id":"workspace_1","snapshot_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":1,"journal_sequence":2,"observed_at":"2026-07-18T12:00:00.000Z","repository_state":"not_repository"}}\' WHERE event_id = \'event_3\'',
					);
					const outcome = yield* rebuild.Rebuild().pipe(Effect.exit);
					const after = yield* database.client.select().from(GitWorkspaceProjections);
					const locks = yield* database.client.select().from(ProjectionRebuildLocks);
					return { after, before, locks, outcome };
				}),
			);
			expect(result.outcome._tag).toBe("Failure");
			expect(result.after).toEqual(result.before);
			expect(result.locks).toEqual([{ generation: 0, lock_id: 1 }]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rebuilds deleted pre-diff projections from durable migration provenance", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					yield* database.client.delete(WorkspaceChangeDiffs);
					yield* database.client.run(
						"UPDATE workspace_changes SET diff_state = 'legacy_unavailable'",
					);
					yield* database.client.insert(LegacyWorkspaceChangeProjections).values({
						change_id: change.change_id,
						source_command_id: change.source_command_id,
						thread_id: change.thread_id,
					});
					yield* database.client.delete(WorkspaceChanges);
					const repaired = yield* rebuild.Rebuild();
					const restored = yield* database.client.select().from(WorkspaceChanges);
					return { repaired, restored };
				}),
			);
			expect(result.repaired).toMatchObject({ equivalent: true, rebuilt: true });
			expect(result.restored).toMatchObject([{ diff_state: "legacy_unavailable" }]);
		} finally {
			await runtime.dispose();
		}
	});

	it("removes stale workspace projections that own no private diff", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					yield* database.client.insert(WorkspaceChanges).values({
						after_identity_json: JSON.stringify(change.after_identity),
						agent_id: change.agent_id,
						before_identity_json: JSON.stringify(change.before_identity),
						change_id: "stale_change",
						created_at: timestamp,
						diff_state: "legacy_unavailable",
						path: "stale.ts",
						raw_origin_json: null,
						review_state: "needs_review",
						reviewed_at: null,
						rollback_state: "consumed",
						rolled_back_at: null,
						run_id: change.run_id,
						source_command_id: "stale_command",
						thread_id: change.thread_id,
						updated_at: timestamp,
						version: 1,
						workspace_id: change.workspace_id,
					});
					yield* rebuild.Rebuild();
					return yield* database.client.select().from(WorkspaceChanges);
				}),
			);
			expect(result.map((row) => row.change_id)).toEqual([change.change_id]);
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes overlapping two-runtime repairs without changing the ledger", async () => {
		const database_path = await make_database_path();
		const first_entered = await Effect.runPromise(Deferred.make<void>());
		const first_release = await Effect.runPromise(Deferred.make<void>());
		const second_before = await Effect.runPromise(Deferred.make<void>());
		const second_entered = await Effect.runPromise(Deferred.make<void>());
		const first = make_runtime(
			database_path,
			Layer.succeed(ProjectionRebuildBarrier, {
				BeforeWriterLock: Effect.void,
				AfterWriterLock: Deferred.succeed(first_entered, undefined).pipe(
					Effect.andThen(Deferred.await(first_release)),
				),
			}),
		);
		const second = make_runtime(
			database_path,
			Layer.succeed(ProjectionRebuildBarrier, {
				BeforeWriterLock: Deferred.succeed(second_before, undefined),
				AfterWriterLock: Deferred.succeed(second_entered, undefined),
			}),
		);
		try {
			const seeded = await first.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* Seed(database);
					const before_events = yield* database.client.select().from(JournalEvents);
					const before_cursors = yield* database.client.select().from(EventStreams);
					const before_private = {
						diffs: yield* database.client.select().from(WorkspaceChangeDiffs),
						operations: yield* database.client.select().from(WorkspaceChangeOperations),
					};
					return { before_cursors, before_events, before_private };
				}),
			);
			const first_rebuild = first.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const rebuild = yield* ProjectionRebuildService;
						const fiber = yield* Effect.forkScoped(rebuild.Rebuild());
						yield* Deferred.await(first_entered);
						yield* Deferred.await(first_release);
						return yield* Fiber.join(fiber);
					}),
				),
			);
			await Effect.runPromise(Deferred.await(first_entered));
			const second_rebuild = second.runPromise(
				Effect.gen(function* () {
					const rebuild = yield* ProjectionRebuildService;
					return yield* rebuild.Rebuild();
				}),
			);
			await Effect.runPromise(Deferred.await(second_before));
			await Effect.runPromise(Deferred.succeed(first_release, undefined));
			const [local, remote] = await Promise.all([first_rebuild, second_rebuild]);
			await Effect.runPromise(Deferred.await(second_entered));
			const persisted = await first.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						cursors: yield* database.client.select().from(EventStreams),
						events: yield* database.client.select().from(JournalEvents),
						private: {
							diffs: yield* database.client.select().from(WorkspaceChangeDiffs),
							operations: yield* database.client
								.select()
								.from(WorkspaceChangeOperations),
						},
					};
				}),
			);
			expect(local.equivalent).toBe(true);
			expect(remote.equivalent).toBe(true);
			expect(persisted.events).toEqual(seeded.before_events);
			expect(persisted.cursors).toEqual(seeded.before_cursors);
			expect(persisted.private).toEqual(seeded.before_private);
		} finally {
			await Promise.all([first.dispose(), second.dispose()]);
		}
	});

	it("preserves replay cursors and private operation artifacts across rebuild and restart", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);
		let before: unknown;
		try {
			before = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* Seed(database);
					yield* database.client.insert(WorkspaceMutationPayloads).values({
						created_at: timestamp,
						expected: Buffer.from("before"),
						expected_byte_count: 6,
						expected_hash: hash_a,
						message_id: change.source_command_id,
						replacement: Buffer.from("after"),
						replacement_byte_count: 5,
						replacement_hash: hash_b,
						state: "available",
						thread_id: change.thread_id,
						updated_at: timestamp,
					});
					yield* database.client.insert(WorkspaceChangeSnapshots).values({
						byte_count: 6,
						change_id: change.change_id,
						content: Buffer.from("before"),
						content_hash: hash_a,
						created_at: timestamp,
						state: "available",
						thread_id: change.thread_id,
						updated_at: timestamp,
					});
					yield* database.client.insert(WorkspaceMutationAuthorities).values({
						agent_id: change.agent_id,
						approval: null,
						assignment_id: null,
						authority_kind: "base_run",
						change_id: change.change_id,
						created_at: timestamp,
						group_id: null,
						message_id: change.source_command_id,
						run_id: change.run_id,
						scope_kind: null,
						scope_value: null,
						thread_id: change.thread_id,
						working_directory: "C:/workspace",
						workspace_id: change.workspace_id,
					});
					yield* database.client.insert(GitMutationOperations).values({
						approval_id: "git_approval_1",
						agent_id: change.agent_id,
						completed_at: timestamp,
						decision_at: timestamp,
						decision_message_id: "git_decision_1",
						expected_snapshot_id: hash_a,
						expected_workspace_version: 1,
						kind: "stage",
						lifecycle: "succeeded",
						mutation_id: "git_mutation_1",
						paths_json: '["src/example.ts"]',
						request_fingerprint: hash_b,
						requested_at: timestamp,
						result_snapshot_id: hash_b,
						result_workspace_version: 2,
						run_id: change.run_id,
						source_message_id: "git_source_1",
						thread_id: change.thread_id,
						updated_at: timestamp,
						workspace_id: change.workspace_id,
					});
					const snapshots = {
						cursors: yield* database.client.select().from(EventStreams),
						commands: yield* database.client.select().from(JournalCommands),
						diffs: yield* database.client.select().from(WorkspaceChangeDiffs),
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client.select().from(WorkspaceChangeOperations),
						payloads: yield* database.client.select().from(WorkspaceMutationPayloads),
						snapshots: yield* database.client.select().from(WorkspaceChangeSnapshots),
						authorities: yield* database.client
							.select()
							.from(WorkspaceMutationAuthorities),
						git_operations: yield* database.client.select().from(GitMutationOperations),
					};
					yield* rebuild.Rebuild();
					return snapshots;
				}),
			);
		} finally {
			await runtime.dispose();
		}
		const restarted = make_runtime(database_path);
		try {
			const after = await restarted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					return {
						cursors: yield* database.client.select().from(EventStreams),
						commands: yield* database.client.select().from(JournalCommands),
						diffs: yield* database.client.select().from(WorkspaceChangeDiffs),
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client.select().from(WorkspaceChangeOperations),
						payloads: yield* database.client.select().from(WorkspaceMutationPayloads),
						snapshots: yield* database.client.select().from(WorkspaceChangeSnapshots),
						authorities: yield* database.client
							.select()
							.from(WorkspaceMutationAuthorities),
						git_operations: yield* database.client.select().from(GitMutationOperations),
						verification: yield* rebuild.Verify(),
					};
				}),
			);
			expect(after.verification.equivalent).toBe(true);
			const { verification: _verification, ...snapshots } = after;
			expect(snapshots).toEqual(before);
		} finally {
			await restarted.dispose();
		}
	});

	it("rolls back an interrupted rebuild after acquiring the writer lease", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const barrier = Layer.succeed(ProjectionRebuildBarrier, {
			BeforeWriterLock: Effect.void,
			AfterWriterLock: Deferred.succeed(entered, undefined).pipe(
				Effect.andThen(Deferred.await(release)),
			),
		});
		const runtime = make_runtime(await make_database_path(), barrier);
		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const database = yield* Database;
						const rebuild = yield* ProjectionRebuildService;
						yield* Seed(database);
						const before_threads = yield* database.client.select().from(Threads);
						const fiber = yield* Effect.forkScoped(rebuild.Rebuild());
						yield* Deferred.await(entered);
						const interrupter = yield* Effect.forkScoped(Fiber.interrupt(fiber));
						yield* Effect.yieldNow;
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(interrupter);
						yield* Fiber.await(fiber);
						const interrupted_locks = yield* database.client
							.select()
							.from(ProjectionRebuildLocks);
						const interrupted_threads = yield* database.client.select().from(Threads);
						const recovered = yield* rebuild.Rebuild();
						return {
							before_threads,
							interrupted_locks,
							interrupted_threads,
							recovered,
						};
					}),
				),
			);
			expect(result.interrupted_locks).toEqual([{ generation: 0, lock_id: 1 }]);
			expect(result.interrupted_threads).toEqual(result.before_threads);
			expect(result.recovered.equivalent).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});
});
