import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	HostedGitSnapshotOperations,
	HostedGitSnapshots,
	JournalCommands,
	JournalEvents,
	ProjectHostedOrigins,
	Projects,
	ThreadTombstones,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const created_at = "2026-07-01T12:00:00.000Z";
const cutoff = "2026-07-08T12:00:00.000Z";
const deleted_at = "2026-07-14T12:00:00.000Z";

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-hosted-git-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		Layer.succeed(RuntimeMetadata, {
			instance_id: "thread_erasure_hosted_git_test",
			MakeId: (prefix) => Effect.succeed(`${prefix}_erasure`),
			Now: Effect.succeed(deleted_at),
		}),
		Layer.succeed(ThreadResourceQuiescer, { Quiesce: () => Effect.void }),
		JournalNotifierLive,
	);
	const erasure = ThreadErasureLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(erasure);
}

const SeedState = Effect.gen(function* () {
	const database = yield* Database;
	const project = {
		display_name: "Artisan Editor",
		project_id: "project_1",
		root_path: "C:/projects/artisan-editor",
	};
	const lookup = {
		association: { _tag: "none" },
		branch: "main",
		expected_head_commit: "a".repeat(40),
		repository: {
			host: "github.com",
			name: "editor",
			owner: "artisan",
			provider_id: "github",
		},
	};

	yield* database.client.insert(Projects).values({
		canonical_root: project.root_path,
		display_name: project.display_name,
		project_id: project.project_id,
		registered_at: created_at,
		updated_at: created_at,
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
	yield* database.client.insert(Threads).values({
		created_at,
		last_activity_at: created_at,
		primary_project_id: project.project_id,
		primary_project_json: JSON.stringify(project),
		thread_id: "thread_1",
		title: "Hosted state",
		title_source: "initial",
		updated_at: created_at,
	});
	yield* database.client.insert(EventStreams).values({
		last_sequence: 1,
		stream_id: "thread:thread_1",
	});
	yield* database.client.insert(JournalCommands).values({
		accepted_at: created_at,
		message_id: "hosted_refresh_1",
		origin: "frontend",
		payload_json: JSON.stringify({ type: "hosted.git.snapshot.refresh" }),
		payload_type: "hosted.git.snapshot.refresh",
		schema_version: 1,
		sent_at: created_at,
		status: "accepted",
		thread_id: "thread_1",
	});
	const [event] = yield* database.client
		.insert(JournalEvents)
		.values({
			causation_id: "hosted_refresh_1",
			correlation_id: "hosted_refresh_1",
			event_id: "event_hosted_refresh_1",
			event_type: "hosted.git.snapshot.updated",
			idempotency_key: "hosted_git_snapshot:hosted_refresh_1",
			occurred_at: created_at,
			origin: "backend",
			payload_json: JSON.stringify({
				snapshot: {
					journal_sequence: 1,
					lookup,
					observed_at: created_at,
					project_id: project.project_id,
					version: 1,
					workspace_freshness: "unverified",
					workspace_id: "workspace_1",
				},
				type: "hosted.git.snapshot.updated",
			}),
			schema_version: 1,
			stream_id: "thread:thread_1",
			stream_sequence: 1,
			thread_id: "thread_1",
		})
		.returning({ sequence: JournalEvents.sequence });

	yield* database.client.insert(HostedGitSnapshots).values({
		journal_sequence: event!.sequence,
		lookup_json: JSON.stringify(lookup),
		observed_at: created_at,
		project_id: project.project_id,
		version: 1,
	});
	yield* database.client.insert(HostedGitSnapshotOperations).values({
		journal_sequence: event!.sequence,
		operation_id: "hosted_refresh_1",
		project_id: project.project_id,
		request_fingerprint: "a".repeat(64),
		sent_at: created_at,
		snapshot_version: 1,
		source_command_id: "hosted_refresh_1",
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	});
});

afterEach(async () => {
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(
			cleanup,
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("ThreadErasure hosted Git snapshot state", () => {
	it("erases thread-owned replay state while preserving the project-wide snapshot", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;

					yield* SeedState;
					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return {
						commands: yield* database.client.select().from(JournalCommands),
						erased,
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client
							.select()
							.from(HostedGitSnapshotOperations),
						projects: yield* database.client.select().from(Projects),
						snapshots: yield* database.client.select().from(HostedGitSnapshots),
						tombstones: yield* database.client.select().from(ThreadTombstones),
					};
				}),
			);

			expect(result.erased).toEqual(["thread_1"]);
			expect(result.operations).toEqual([]);
			expect(result.commands).toEqual([]);
			expect(result.projects).toHaveLength(1);
			expect(result.snapshots).toHaveLength(1);
			expect(result.tombstones).toHaveLength(1);
			expect(result.events).toHaveLength(2);
			expect(result.events[0]).toMatchObject({
				event_type: "thread.content_erased",
				payload_json: '{"type":"thread.content_erased"}',
			});
		} finally {
			await runtime.dispose();
		}
	});
});
