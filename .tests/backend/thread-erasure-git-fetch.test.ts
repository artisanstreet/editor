import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	WorkspaceGitFetchRepository,
	WorkspaceGitFetchRepositoryLive,
} from "../../modules/backend/src/git/workspace-git-fetch-repository";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	Projects,
	Threads,
	WorkspaceGitFetchOperations,
	WorkspaceGitFetchStates,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const runtimes: Array<ManagedRuntime.ManagedRuntime<any, any>> = [];
const digest = "a".repeat(64);
const created_at = "2026-07-14T12:00:00.000Z";
const cutoff = "2026-07-21T12:00:00.000Z";
const deleted_at = "2026-07-22T12:00:00.000Z";

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-git-fetch-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	let next_id = 0;
	const quiesced_threads: Array<string> = [];
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		Layer.succeed(RuntimeMetadata, {
			instance_id: "thread_erasure_git_fetch_test",
			MakeId: (prefix) => Effect.sync(() => `${prefix}_fetch_${++next_id}`),
			Now: Effect.succeed(created_at),
		}),
		JournalNotifierLive,
		Layer.succeed(ThreadResourceQuiescer, {
			Quiesce: (thread_id) => Effect.sync(() => quiesced_threads.push(thread_id)),
		}),
	);
	const fetches = WorkspaceGitFetchRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const erasure = ThreadErasureLive.pipe(Layer.provideMerge(infrastructure));
	const runtime = ManagedRuntime.make(Layer.merge(fetches, erasure));

	runtimes.push(runtime);

	return { quiesced_threads, runtime };
}

function SeedThread() {
	return Effect.gen(function* () {
		const database = yield* Database;
		const project = {
			display_name: "Artisan",
			project_id: "project_1",
			root_path: "C:/artisan",
		};

		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			linked_projects_json: "[]",
			primary_project_id: project.project_id,
			primary_project_json: JSON.stringify(project),
			thread_id: "thread_1",
			title: "Fetch",
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(Projects).values({
			canonical_root: project.root_path,
			display_name: project.display_name,
			project_id: project.project_id,
			registered_at: created_at,
			updated_at: created_at,
			workspace_id: "workspace_1",
		});
		yield* database.client
			.insert(EventStreams)
			.values({ last_sequence: 0, stream_id: "thread:thread_1" });
	});
}

function PrepareManual() {
	return Effect.flatMap(WorkspaceGitFetchRepository, (repository) =>
		repository.PrepareManual({
			attempt_id: "attempt_1",
			message_id: "manual_1",
			request_fingerprint: digest,
			sent_at: created_at,
			thread_id: "thread_1",
			workspace_id: "workspace_1",
		}),
	);
}

afterEach(async () => {
	const directories = temporary_directories.splice(0);
	const active_runtimes = runtimes.splice(0);

	await Promise.all(active_runtimes.map((runtime) => runtime.dispose()));
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

describe("ThreadErasure local Git fetch state", () => {
	it("keeps a thread while its manual fetch is pending", async () => {
		const { quiesced_threads, runtime } = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
		);

		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const erasure = yield* ThreadErasure;

				yield* SeedThread();
				yield* PrepareManual();

				return {
					erased: yield* erasure.CleanupExpired(cutoff, deleted_at),
					threads: yield* database.client.select().from(Threads),
				};
			}),
		);

		expect(result.erased).toEqual([]);
		expect(result.threads.map(({ thread_id }) => thread_id)).toEqual(["thread_1"]);
		expect(quiesced_threads).toEqual([]);
	});

	it("erases terminal thread-owned fetch intent while retaining global policy and state", async () => {
		const { quiesced_threads, runtime } = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
		);

		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const erasure = yield* ThreadErasure;
				const fetches = yield* WorkspaceGitFetchRepository;

				yield* SeedThread();
				yield* fetches.UpdatePolicy({
					enabled: true,
					message_id: "policy_1",
					request_fingerprint: digest,
					sent_at: created_at,
				});
				yield* PrepareManual();
				yield* fetches.ClaimManual({
					lease_expires_at: "2026-07-14T12:04:00.000Z",
					lease_owner: "owner_1",
					message_id: "manual_1",
					now: created_at,
				});
				yield* fetches.CompleteClaim({
					attempt_id: "attempt_1",
					attempted_at: "2026-07-14T12:01:00.000Z",
					lease_owner: "owner_1",
					result: "succeeded",
					workspace_id: "workspace_1",
				});

				const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

				return {
					commands: yield* database.client.select().from(JournalCommands),
					erased,
					operations: yield* database.client.select().from(WorkspaceGitFetchOperations),
					projection: yield* fetches.Query,
					states: yield* database.client.select().from(WorkspaceGitFetchStates),
				};
			}),
		);

		expect(result.erased).toEqual(["thread_1"]);
		expect(quiesced_threads).toEqual(["thread_1"]);
		expect(result.operations.map(({ message_id }) => message_id)).toEqual(["policy_1"]);
		expect(result.commands.map(({ message_id }) => message_id)).toEqual(["policy_1"]);
		expect(result.states).toHaveLength(1);
		expect(result.projection).toEqual({
			enabled: true,
			workspaces: [
				{
					last_attempt: {
						attempted_at: "2026-07-14T12:01:00.000Z",
						result: "succeeded",
					},
					workspace_id: "workspace_1",
				},
			],
		});
	});
});
