import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceGitChangedFiles,
	WorkspaceGitOperations,
	WorkspaceGitSessions,
	WorkspaceGitWorktrees,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceGitSessionConflict,
	WorkspaceGitSessionRepository,
	WorkspaceGitSessionRepositoryLive,
	WorkspaceGitSessionUnavailable,
	type ProjectObservation,
} from "../../modules/backend/src/git/workspace-git-session-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-13T14:00:00.000Z";
const next = "2026-07-13T14:01:00.000Z";

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-git-session-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_git_session_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_git_session_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const service = WorkspaceGitSessionRepositoryLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(service);
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

const SeedThread = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: "thread_1",
		title: "Git session repository",
		title_source: "initial",
		updated_at: now,
	});
});

function observation(overrides: Partial<ProjectObservation> = {}): ProjectObservation {
	return {
		kind: "refresh",
		observed_at: now,
		operation_id: "git_observation_1",
		repository_root: "C:/workspace",
		request_fingerprint: "a".repeat(64),
		selected_worktree_path: "C:/workspace",
		session: {
			blockers: [],
			branch: "main",
			changed_files: [
				{
					conflicted: false,
					path: "src/main.ts",
					staged: false,
					status: "modified",
					untracked: false,
					unstaged: true,
				},
			],
			diff_stats: { additions: 2, deletions: 1, files: 1 },
			has_diff: true,
			head: "a".repeat(40),
			state: "ready",
		},
		source_command: {
			agent_id: "agent_1",
			message_id: "refresh_command_1",
			raw_origin: { provider: "codex", reference: "tool_1" },
			run_id: "run_1",
			sent_at: now,
		},
		thread_id: "thread_1",
		workspace_id: "workspace_1",
		worktrees: [
			{
				adapter_path: "C:/workspace",
				worktree: {
					bare: false,
					branch: "main",
					detached: false,
					head: "a".repeat(40),
					locked: false,
					location: "selected",
					prunable: false,
				},
			},
			{
				adapter_path: "C:/external",
				worktree: {
					bare: false,
					branch: "release",
					detached: false,
					head: "b".repeat(40),
					locked: false,
					location: "external",
					prunable: false,
				},
			},
		],
		...overrides,
	};
}

afterEach(async () => {
	const directories = temporary_directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(
			directories,
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("WorkspaceGitSessionRepository", () => {
	it("atomically replaces projections and preserves exact duplicate identity", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceGitSessionRepository;

					yield* SeedThread;
					const first = yield* repository.Project(observation());
					const duplicate = yield* repository.Project(observation());
					const second = yield* repository.Project(
						observation({
							observed_at: next,
							operation_id: "git_observation_2",
							request_fingerprint: "b".repeat(64),
							session: {
								blockers: [],
								branch: "release",
								changed_files: [],
								diff_stats: { additions: 0, deletions: 0, files: 0 },
								has_diff: false,
								head: "b".repeat(40),
								state: "ready",
							},
							source_command: {
								message_id: "refresh_command_2",
								sent_at: next,
							},
							worktrees: [
								{
									adapter_path: "C:/workspace",
									worktree: {
										bare: false,
										branch: "release",
										detached: false,
										head: "b".repeat(40),
										locked: false,
										location: "selected",
										prunable: false,
									},
								},
							],
						}),
					);
					const query = yield* repository.Query({ workspace_id: "workspace_1" });

					return {
						changed: yield* database.client.select().from(WorkspaceGitChangedFiles),
						commands: yield* database.client.select().from(JournalCommands),
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						first,
						query,
						second,
						sessions: yield* database.client.select().from(WorkspaceGitSessions),
						worktrees: yield* database.client.select().from(WorkspaceGitWorktrees),
					};
				}),
			);

			expect(result.duplicate).toEqual({ ...result.first, status: "duplicate" });
			expect(result.first.event.journal_sequence).toBe(result.first.session.journal_sequence);
			expect(result.second.event.journal_sequence).toBe(
				result.second.session.journal_sequence,
			);
			expect(result.query).toMatchObject({
				journal_sequence: 2,
				session: { branch: "release", changed_files: [], version: 2, worktrees: [{}] },
			});
			expect(result.sessions).toHaveLength(1);
			expect(result.worktrees).toHaveLength(1);
			expect(result.changed).toEqual([]);
			expect(result.commands).toHaveLength(2);
			expect(result.events).toHaveLength(2);
			expect(result.events[0]!.payload_json).not.toContain("C:/workspace");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects operation and source-command conflicts without changing projections", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceGitSessionRepository;

					yield* SeedThread;
					yield* repository.Project(observation());

					const operation_conflict = yield* repository
						.Project(observation({ request_fingerprint: "c".repeat(64) }))
						.pipe(Effect.exit);
					const source_conflict = yield* repository
						.Project(
							observation({
								operation_id: "git_observation_conflict",
								request_fingerprint: "d".repeat(64),
							}),
						)
						.pipe(Effect.exit);

					return {
						events: yield* database.client.select().from(JournalEvents),
						operation_conflict,
						source_conflict,
					};
				}),
			);

			expect(failure_from(result.operation_conflict)).toEqual(
				new WorkspaceGitSessionConflict({ reason: "operation_conflict" }),
			);
			expect(failure_from(result.source_conflict)).toEqual(
				new WorkspaceGitSessionConflict({ reason: "source_command_conflict" }),
			);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns an absent query and resumes exact pending evidence after restart", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);

		try {
			const absent = await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceGitSessionRepository;

					yield* SeedThread;

					return yield* repository.Query({ workspace_id: "missing_workspace" });
				}),
			);

			expect(absent).toEqual({ journal_sequence: 0 });
			await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitSessionRepository, (repository) =>
					repository.Project(observation()),
				),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceGitSessionRepository;
					const pending = yield* repository.ListPendingEvidence;
					const settled = yield* repository.MarkEvidenceRecorded("git_observation_1");
					const duplicate = yield* repository.MarkEvidenceRecorded("git_observation_1");
					const replay = yield* repository.Project(observation());

					return {
						duplicate,
						operations: yield* database.client.select().from(WorkspaceGitOperations),
						pending,
						replay,
						settled,
					};
				}),
			);

			expect(result.pending).toEqual([
				{
					agent_id: "agent_1",
					branch: "main",
					changed_file_count: 1,
					has_diff: true,
					operation_id: "git_observation_1",
					raw_origin: { provider: "codex", reference: "tool_1" },
					root_path: "C:/workspace",
					run_id: "run_1",
					thread_id: "thread_1",
					worktree_path: "C:/workspace",
				},
			]);
			expect(result.settled.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.replay.status).toBe("duplicate");
			expect(result.operations[0]).toMatchObject({
				evidence_recorded: true,
				evidence_root_path: null,
				evidence_worktree_path: null,
			});
		} finally {
			await second_runtime.dispose();
		}
	});

	it("settles unavailable evidence immediately and fences erased threads", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceGitSessionRepository;

					yield* SeedThread;
					yield* repository.Project(
						observation({
							repository_root: undefined,
							selected_worktree_path: undefined,
							session: {
								blockers: ["not_repository"],
								changed_files: [],
								diff_stats: { additions: 0, deletions: 0, files: 0 },
								has_diff: false,
								state: "unavailable",
							},
							worktrees: [],
						}),
					);
					const pending = yield* repository.ListPendingEvidence;

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: next,
						thread_id: "thread_1",
					});
					const claimed = yield* repository
						.Project(
							observation({
								operation_id: "claimed_operation",
								request_fingerprint: "e".repeat(64),
								source_command: { message_id: "claimed_command", sent_at: next },
							}),
						)
						.pipe(Effect.exit);

					yield* database.client.delete(ThreadErasureClaims);
					yield* database.client.delete(Threads);
					yield* database.client.insert(ThreadTombstones).values({
						deleted_at: next,
						thread_id: "thread_1",
					});
					const tombstoned = yield* repository
						.MarkEvidenceRecorded("git_observation_1")
						.pipe(Effect.exit);

					return { claimed, pending, tombstoned };
				}),
			);

			expect(result.pending).toEqual([]);
			expect(failure_from(result.claimed)).toEqual(
				new WorkspaceGitSessionUnavailable({ reason: "erased" }),
			);
			expect(failure_from(result.tombstoned)).toEqual(
				new WorkspaceGitSessionUnavailable({ reason: "erased" }),
			);
		} finally {
			await runtime.dispose();
		}
	});
});
