import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	Threads,
	ToolControlCommands,
	ToolExecutionClaims,
	ToolInvocationPrivate,
	ToolInvocations,
	ToolThreadDispatchState,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const previous_migration = "20260717115855_supreme_ghost_rider";
const temporary_directories: Array<string> = [];
const timestamp = "2026-07-17T12:00:00.000Z";
const digest = "a".repeat(64);

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-tool-control-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry <= previous_migration);

	temporary_directories.push(directory);
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded", discard: true },
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
					file_system.remove(directory, { force: true, recursive: true }),
				),
			{ concurrency: "unbounded", discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

function make_runtime(database_path: string, migration_path = migrations_path) {
	return ManagedRuntime.make(
		make_database_layer({ database_path, migrations_path: migration_path }),
	);
}

type LegacyRows = {
	claims: Array<typeof ToolExecutionClaims.$inferSelect>;
	commands: Array<typeof ToolControlCommands.$inferSelect>;
	private_rows: Array<typeof ToolInvocationPrivate.$inferSelect>;
	threads: Array<typeof Threads.$inferSelect>;
	invocations: Array<typeof ToolInvocations.$inferSelect>;
};

describe("ToolThreadDispatchState migration", () => {
	it("preserves durable tool-control data and adds constrained dispatch state", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = make_runtime(paths.database_path, paths.prior_migrations_path);
		const legacy_thread = {
			affinity_version: 3,
			archived_at: null,
			created_at: "2026-07-17T11:00:00.000Z",
			current_goal: "Preserve tool control",
			last_activity_at: timestamp,
			linked_projects_json: '[{"project_id":"project_legacy"}]',
			live_status: "Working",
			metadata_version: 4,
			pinned: true,
			primary_project_id: "project_legacy",
			primary_project_json: '{"project_id":"project_legacy"}',
			project_affinity_scores_json: '[{"project_id":"project_legacy","score":3}]',
			project_locked: true,
			rehome_suggestion_json: null,
			rename_suggestion: "Legacy tool thread",
			thread_id: "thread_tool_migration",
			title: "Legacy tool-control thread",
			title_locked: true,
			title_source: "user",
			updated_at: timestamp,
		};
		const legacy_invocation = {
			agent_id: "agent_legacy",
			approval_id: null,
			approval_policy: "automatic",
			created_at: timestamp,
			current_journal_sequence: 1,
			decided_at: null,
			decision: null,
			decision_id: null,
			descriptor_fingerprint: digest,
			effect: "read",
			input_schema_json: "{}",
			invocation_id: "invocation_tool_migration",
			label: "Read workspace",
			owner_kind: "ordinary_run",
			recovery_policy: "retry",
			request_id: "request_tool_migration",
			revision: 1,
			run_id: "run_tool_migration",
			settled_at: null,
			source: "artisan",
			started_at: null,
			state: "pending",
			summary: "Reads a bounded workspace projection.",
			suspended_at: null,
			thread_id: legacy_thread.thread_id,
			tool_id: "workspace.read",
			updated_at: timestamp,
			workspace_id: null,
		};

		let legacy_rows: LegacyRows | undefined;

		try {
			legacy_rows = await prior_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(Threads).values(legacy_thread);
					yield* database.client.insert(ToolInvocations).values(legacy_invocation);
					yield* database.client.insert(ToolInvocationPrivate).values({
						arguments_digest: digest,
						arguments_json: "{}",
						invocation_id: legacy_invocation.invocation_id,
						request_fingerprint: digest,
						result_digest: null,
						result_json: null,
					});
					yield* database.client.insert(ToolControlCommands).values({
						accepted_at: timestamp,
						approval_id: null,
						command_id: "command_tool_migration",
						decision: null,
						invocation_id: legacy_invocation.invocation_id,
						kind: "invoke",
						request_fingerprint: digest,
					});
					yield* database.client.insert(ToolExecutionClaims).values({
						claim_token: "claim_tool_migration",
						claimed_at: timestamp,
						invocation_id: legacy_invocation.invocation_id,
						launch_started_at: null,
						lease_expires_at: timestamp,
						owner_instance_id: "backend_legacy",
					});

					return {
						claims: yield* database.client.select().from(ToolExecutionClaims),
						commands: yield* database.client.select().from(ToolControlCommands),
						private_rows: yield* database.client.select().from(ToolInvocationPrivate),
						threads: yield* database.client.select().from(Threads),
						invocations: yield* database.client.select().from(ToolInvocations),
					};
				}),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const current_runtime = make_runtime(paths.database_path);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						claims: yield* database.client.select().from(ToolExecutionClaims),
						commands: yield* database.client.select().from(ToolControlCommands),
						private_rows: yield* database.client.select().from(ToolInvocationPrivate),
						threads: yield* database.client.select().from(Threads),
						invocations: yield* database.client.select().from(ToolInvocations),
						dispatch_state: yield* database.client
							.select()
							.from(ToolThreadDispatchState),
					};
				}),
			);

			const prior_rows = legacy_rows;

			expect(prior_rows).toBeDefined();
			if (!prior_rows) {
				throw new Error("Legacy rows were not captured");
			}

			expect(result.claims).toEqual(prior_rows.claims);
			expect(result.commands).toEqual(prior_rows.commands);
			expect(result.private_rows).toEqual(prior_rows.private_rows);
			expect(result.threads).toEqual(prior_rows.threads);
			expect(result.invocations).toEqual(prior_rows.invocations);
			expect(result.dispatch_state).toEqual([]);

			await expect(
				current_runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* database.client.insert(ToolThreadDispatchState).values({
							thread_id: "missing_thread",
						});
					}),
				),
			).rejects.toBeDefined();

			await expect(
				current_runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* database.client.insert(ToolThreadDispatchState).values({
							admission_version: -1,
							thread_id: legacy_thread.thread_id,
						});
					}),
				),
			).rejects.toBeDefined();

			await expect(
				current_runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* database.client.insert(ToolThreadDispatchState).values({
							quiesced_at: "2026-07-17T12:00:00Z",
							thread_id: legacy_thread.thread_id,
						});
					}),
				),
			).rejects.toBeDefined();

			await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(ToolThreadDispatchState).values({
						admission_version: 4,
						quiesced_at: timestamp,
						thread_id: legacy_thread.thread_id,
					});

					const [dispatch_state] = yield* database.client
						.select()
						.from(ToolThreadDispatchState);

					expect(dispatch_state).toEqual({
						admission_version: 4,
						quiesced_at: timestamp,
						thread_id: legacy_thread.thread_id,
					});

					yield* database.client.run(
						`DELETE FROM threads WHERE thread_id = '${legacy_thread.thread_id}'`,
					);
				}),
			);

			const remaining_dispatch_state = await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return yield* database.client.select().from(ToolThreadDispatchState);
				}),
			);

			expect(remaining_dispatch_state).toEqual([]);
		} finally {
			await current_runtime.dispose();
		}
	});
});
