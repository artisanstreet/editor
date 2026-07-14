import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	HostedGitSnapshotConflict,
	HostedGitSnapshotInvariant,
	HostedGitSnapshotRepository,
	HostedGitSnapshotRepositoryLive,
	HostedGitSnapshotUnavailable,
	type ProjectHostedGitSnapshot,
} from "../../modules/backend/src/git-provider/hosted-git-snapshot-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	HostedGitSnapshotOperations,
	HostedGitSnapshots,
	JournalCommands,
	JournalEvents,
	ProjectHostedOrigins,
	Projects,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-14T14:00:00.000Z";
const next = "2026-07-14T14:01:00.000Z";
const project = {
	display_name: "Artisan Editor",
	project_id: "project_1",
	root_path: "C:/projects/artisan-editor",
};

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-hosted-git-snapshot-",
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
		instance_id: "hosted_git_snapshot_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_hosted_git_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const service = HostedGitSnapshotRepositoryLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(service);
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

const SeedProject = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Projects).values({
		canonical_root: project.root_path,
		display_name: project.display_name,
		project_id: project.project_id,
		registered_at: now,
		updated_at: now,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(ProjectHostedOrigins).values({
		canonical_host: "github.com",
		clone_url: "https://github.com/artisan/editor.git",
		fetch_url: "https://github.com/artisan/editor.git",
		name: "editor",
		native_id: "repository_1",
		owner: "artisan",
		project_id: project.project_id,
		provider_id: "github",
		push_url: "https://github.com/artisan/editor.git",
		remote_name: "origin",
		selected_account_login: "alice",
		web_url: "https://github.com/artisan/editor",
	});
	yield* database.client.insert(Threads).values([
		{
			created_at: now,
			primary_project_id: project.project_id,
			primary_project_json: JSON.stringify(project),
			thread_id: "thread_1",
			title: "Hosted Git snapshot",
			title_source: "initial",
			updated_at: now,
		},
		{
			created_at: now,
			thread_id: "thread_unattached",
			title: "Unattached",
			title_source: "initial",
			updated_at: now,
		},
	]);
});

function observation(overrides: Partial<ProjectHostedGitSnapshot> = {}): ProjectHostedGitSnapshot {
	return {
		lookup: {
			association: { _tag: "none" },
			branch: "main",
			expected_head_commit: "a".repeat(40),
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
		},
		observed_at: now,
		operation_id: "hosted_snapshot_1",
		project_id: project.project_id,
		request_fingerprint: "a".repeat(64),
		source_command: { message_id: "hosted_snapshot_1", sent_at: now },
		thread_id: "thread_1",
		workspace_id: "workspace_1",
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

describe("HostedGitSnapshotRepository", () => {
	it("converges concurrent duplicate refreshes on one event and snapshot", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* HostedGitSnapshotRepository;

					yield* SeedProject;
					const accepted = yield* Effect.all(
						[repository.Project(observation()), repository.Project(observation())],
						{ concurrency: "unbounded" },
					);

					return {
						accepted,
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client
							.select()
							.from(HostedGitSnapshotOperations),
						snapshots: yield* database.client.select().from(HostedGitSnapshots),
					};
				}),
			);

			expect(result.accepted.map(({ status }) => status).toSorted()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(result.accepted[0]!.snapshot).toEqual(result.accepted[1]!.snapshot);
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(1);
			expect(result.operations).toHaveLength(1);
			expect(result.snapshots).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists one exact snapshot and replays its original event after replacement", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* HostedGitSnapshotRepository;

					yield* SeedProject;
					const first = yield* repository.Project(observation());
					const duplicate = yield* repository.Project(observation());
					const second = yield* repository.Project(
						observation({
							lookup: {
								association: { _tag: "none" },
								branch: "release",
								expected_head_commit: "b".repeat(40),
								repository: observation().lookup.repository,
							},
							observed_at: next,
							operation_id: "hosted_snapshot_2",
							request_fingerprint: "b".repeat(64),
							source_command: { message_id: "hosted_snapshot_2", sent_at: next },
						}),
					);
					const replay = yield* repository.Replay({
						operation_id: "hosted_snapshot_1",
						project_id: project.project_id,
						request_fingerprint: "a".repeat(64),
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: "workspace_1",
					});
					const query = yield* repository.Query({ workspace_id: "workspace_1" });

					return {
						commands: yield* database.client.select().from(JournalCommands),
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						first,
						operations: yield* database.client
							.select()
							.from(HostedGitSnapshotOperations),
						query,
						replay,
						second,
						snapshots: yield* database.client.select().from(HostedGitSnapshots),
					};
				}),
			);

			expect(result.duplicate).toEqual({ ...result.first, status: "duplicate" });
			expect(result.first.snapshot.version).toBe(1);
			expect(result.second.snapshot.version).toBe(2);
			expect(result.query.snapshot).toMatchObject({
				lookup: { branch: "release", expected_head_commit: "b".repeat(40) },
				version: 2,
				workspace_freshness: "unverified",
			});
			expect(result.replay).toMatchObject({
				value: {
					snapshot: {
						lookup: { branch: "main", expected_head_commit: "a".repeat(40) },
						version: 1,
					},
					status: "duplicate",
				},
			});
			expect(result.commands).toHaveLength(2);
			expect(result.events).toHaveLength(2);
			expect(result.operations).toHaveLength(2);
			expect(result.snapshots).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects changed intent and a thread that is not attached to the project", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* HostedGitSnapshotRepository;

					yield* SeedProject;
					yield* repository.Project(observation());

					return {
						changed_intent: yield* Effect.exit(
							repository.Project(
								observation({ request_fingerprint: "c".repeat(64) }),
							),
						),
						unattached: yield* Effect.exit(
							repository.Project(
								observation({
									operation_id: "hosted_snapshot_unattached",
									request_fingerprint: "d".repeat(64),
									source_command: {
										message_id: "hosted_snapshot_unattached",
										sent_at: now,
									},
									thread_id: "thread_unattached",
								}),
							),
						),
					};
				}),
			);

			expect(failure_from(result.changed_intent)).toBeInstanceOf(HostedGitSnapshotConflict);
			expect(failure_from(result.unattached)).toEqual(
				new HostedGitSnapshotUnavailable({ reason: "thread_not_attached" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when the durable provider projection is corrupt", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const exit = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* HostedGitSnapshotRepository;

					yield* SeedProject;
					yield* repository.Project(observation());
					yield* database.client
						.update(HostedGitSnapshots)
						.set({ lookup_json: "not-json" });

					return yield* Effect.exit(repository.Query({ workspace_id: "workspace_1" }));
				}),
			);

			expect(failure_from(exit)).toBeInstanceOf(HostedGitSnapshotInvariant);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects replay when the persisted source command payload was altered", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const exit = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* HostedGitSnapshotRepository;

					yield* SeedProject;
					yield* repository.Project(observation());
					yield* database.client
						.update(JournalCommands)
						.set({ payload_json: '{"type":"hosted.git.snapshot.refresh"}' });

					return yield* Effect.exit(
						repository.Replay({
							operation_id: "hosted_snapshot_1",
							project_id: project.project_id,
							request_fingerprint: "a".repeat(64),
							sent_at: now,
							thread_id: "thread_1",
							workspace_id: "workspace_1",
						}),
					);
				}),
			);

			expect(failure_from(exit)).toBeInstanceOf(HostedGitSnapshotConflict);
		} finally {
			await runtime.dispose();
		}
	});
});
