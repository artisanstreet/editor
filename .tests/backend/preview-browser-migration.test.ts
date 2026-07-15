import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalStore,
	JournalStoreLive,
} from "../../modules/backend/src/persistence/journal-store";
import {
	EventStreams,
	JournalEvents,
	PreviewTargetRemovalClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const target_migration = "20260715185941_strong_frank_castle";
const temporary_directories: Array<string> = [];
const occurred_at = "2026-07-15T18:00:00.000Z";

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "preview_browser_migration_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_migration_${++next_id}`),
		Now: Effect.succeed(occurred_at),
	});
}

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-preview-browser-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < target_migration);

	yield* Effect.sync(() => temporary_directories.push(directory));
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded" },
	);

	return { database_path, prior_migrations_path };
}).pipe(Effect.provide(NodeFileSystem.layer));

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

describe("preview browser migration", () => {
	it("preserves legacy removal claims and supports generation-bound claims", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path: paths.prior_migrations_path,
			}),
		);

		try {
			await prior_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.run(`
						INSERT INTO preview_target_removal_claims (
							project_id,
							workspace_id,
							target_id,
							claim_token,
							owner_instance_id,
							lease_expires_at_ms,
							created_at_ms,
							updated_at_ms
						)
						VALUES (
							'project_legacy',
							'workspace_legacy',
							'target_legacy',
							'claim_token_legacy',
							'instance_legacy',
							1700000000000,
							1600000000000,
							1650000000000
						)
					`);
				}),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const current_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path,
			}),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const legacy_before_new_insert = yield* database.client
						.select()
						.from(PreviewTargetRemovalClaims);

					yield* database.client.insert(PreviewTargetRemovalClaims).values({
						claim_token: "claim_token_generation_bound",
						created_at_ms: 1800000000000,
						lease_expires_at_ms: 1900000000000,
						owner_instance_id: "instance_generation_bound",
						project_id: "project_generation_bound",
						target_generation_id: "generation_bound",
						target_id: "target_generation_bound",
						updated_at_ms: 1850000000000,
						workspace_id: "workspace_generation_bound",
					});

					return {
						legacy_before_new_insert,
						claims: yield* database.client.select().from(PreviewTargetRemovalClaims),
					};
				}),
			);

			expect(result.legacy_before_new_insert).toEqual([
				{
					claim_token: "claim_token_legacy",
					created_at_ms: 1600000000000,
					lease_expires_at_ms: 1700000000000,
					owner_instance_id: "instance_legacy",
					project_id: "project_legacy",
					target_generation_id: null,
					target_id: "target_legacy",
					updated_at_ms: 1650000000000,
					workspace_id: "workspace_legacy",
				},
			]);
			expect(result.claims).toEqual([
				...result.legacy_before_new_insert,
				{
					claim_token: "claim_token_generation_bound",
					created_at_ms: 1800000000000,
					lease_expires_at_ms: 1900000000000,
					owner_instance_id: "instance_generation_bound",
					project_id: "project_generation_bound",
					target_generation_id: "generation_bound",
					target_id: "target_generation_bound",
					updated_at_ms: 1850000000000,
					workspace_id: "workspace_generation_bound",
				},
			]);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("replays removal events persisted before target generations were published", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path: paths.prior_migrations_path,
			}),
		);

		try {
			await prior_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(Threads).values({
						created_at: occurred_at,
						thread_id: "thread_legacy_removal",
						title: "Legacy preview removal",
						updated_at: occurred_at,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 1,
						stream_id: "thread:thread_legacy_removal",
					});
					yield* database.client.insert(JournalEvents).values({
						causation_id: "remove_legacy_target",
						correlation_id: "remove_legacy_target",
						event_id: "event_legacy_target_removed",
						event_type: "preview.target.updated",
						idempotency_key: "remove_legacy_target",
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify({
							action: "removed",
							target: {
								created_at_ms: 1,
								project_id: "project_legacy",
								state: "removed",
								target_id: "target_legacy",
								updated_at_ms: 2,
								url: "http://localhost:5173/legacy",
								workspace_id: "workspace_legacy",
							},
							type: "preview.target.updated",
						}),
						schema_version: 1,
						stream_id: "thread:thread_legacy_removal",
						stream_sequence: 1,
						thread_id: "thread_legacy_removal",
					});
				}),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const infrastructure = Layer.mergeAll(
			make_database_layer({ database_path: paths.database_path, migrations_path }),
			JournalNotifierLive,
			make_metadata_layer(),
		);
		const current_runtime = ManagedRuntime.make(
			JournalStoreLive.pipe(Layer.provideMerge(infrastructure)),
		);

		try {
			const replay = await current_runtime.runPromise(
				Effect.flatMap(JournalStore, (journal) =>
					journal.ReadReplay({ after_journal_sequence: 0 }),
				),
			);

			expect(replay).toMatchObject([
				{
					payload: {
						action: "removed",
						target: {
							state: "removed",
							target_id: "target_legacy",
						},
						type: "preview.target.updated",
					},
				},
			]);
			expect(replay[0]?.payload).toMatchObject({
				action: "removed",
				target: expect.not.objectContaining({ generation_id: expect.anything() }),
			});
		} finally {
			await current_runtime.dispose();
		}
	});
});
