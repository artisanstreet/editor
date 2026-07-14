import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Exit, FileSystem, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	WorkspaceGitMutationApprovals,
	WorkspaceGitMutationClaims,
	WorkspaceGitOperations,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const git_mutation_migration = "20260714004706_early_mandroid";
const execution_lease_migration = "20260714025111_curly_bruce_banner";
const temporary_directories: Array<string> = [];
const timestamp = "2026-07-14T12:00:00.000Z";
const fingerprint = "a".repeat(64);
const source_head = "b".repeat(40);

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-git-mutation-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const lease_prior_migrations_path = join(directory, "lease-prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < git_mutation_migration);
	const lease_prior_entries = entries.filter((entry) => entry < execution_lease_migration);

	yield* Effect.sync(() => temporary_directories.push(directory));
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* file_system.makeDirectory(lease_prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded" },
	);
	yield* Effect.forEach(
		lease_prior_entries,
		(entry) =>
			file_system.copy(
				join(migrations_path, entry),
				join(lease_prior_migrations_path, entry),
			),
		{ concurrency: "unbounded" },
	);

	return { database_path, lease_prior_migrations_path, prior_migrations_path };
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

describe("generic Git mutation migration", () => {
	it("preserves existing Git operations and enforces the new mutation lease schema", async () => {
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
						INSERT INTO workspace_git_operations (
							operation_id,
							request_fingerprint,
							kind,
							thread_id,
							workspace_id,
							session_version,
							journal_sequence,
							evidence_recorded,
							evidence_root_path,
							evidence_worktree_path,
							evidence_branch,
							evidence_changed_file_count,
							evidence_has_diff,
							sent_at,
							created_at,
							updated_at
						)
						VALUES (
							'legacy_git_operation',
							'${fingerprint}',
							'recovery',
							'thread_legacy',
							'workspace_legacy',
							1,
							1,
							0,
							'C:/legacy/repository',
							'C:/legacy/repository',
							'main',
							3,
							1,
							'${timestamp}',
							'${timestamp}',
							'${timestamp}'
						)
					`);
					yield* database.client.run(`
						INSERT INTO journal_commands (
							message_id,
							thread_id,
							origin,
							sent_at,
							accepted_at,
							payload_type,
							payload_json,
							schema_version,
							status
						)
						VALUES (
							'legacy_mutation_request',
							'thread_legacy',
							'frontend',
							'${timestamp}',
							'${timestamp}',
							'workspace.git.mutation.request',
							'{"expected_session_version":1,"operation":{"type":"commit"},"request_fingerprint":"${fingerprint}","type":"workspace.git.mutation.request","workspace_id":"workspace_legacy"}',
							1,
							'accepted'
						)
					`);
				}),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const lease_prior_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path: paths.lease_prior_migrations_path,
			}),
		);

		try {
			await lease_prior_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(WorkspaceGitMutationApprovals).values({
						approval_id: "legacy_claim_approval",
						approved: true,
						created_at: timestamp,
						decided_at: timestamp,
						decision_message_id: "legacy_claim_decision",
						execution_started_at: timestamp,
						expected_session_version: 1,
						operation_summary_json: '{"type":"clean"}',
						request_fingerprint: fingerprint,
						source_command_id: "legacy_claim_command",
						source_head,
						state: "executing",
						thread_id: "thread_legacy_claim",
						updated_at: timestamp,
						workspace_id: "workspace_legacy_claim",
					});
					yield* database.client.run(`
						INSERT INTO workspace_git_mutation_claims (
							workspace_id,
							approval_id,
							thread_id,
							claim_token,
							claimed_at
						)
						VALUES (
							'workspace_legacy_claim',
							'legacy_claim_approval',
							'thread_legacy_claim',
							'legacy_claim_token',
							'${timestamp}'
						)
					`);
				}),
			);
		} finally {
			await lease_prior_runtime.dispose();
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
					const legacy = yield* database.client.select().from(WorkspaceGitOperations);

					yield* database.client.insert(WorkspaceGitOperations).values({
						created_at: timestamp,
						evidence_recorded: true,
						journal_sequence: 2,
						kind: "mutation",
						operation_id: "generic_git_operation",
						request_fingerprint: fingerprint,
						sent_at: timestamp,
						session_version: 2,
						thread_id: "thread_generic",
						updated_at: timestamp,
						workspace_id: "workspace_generic",
					});
					yield* database.client.insert(WorkspaceGitMutationApprovals).values([
						{
							approval_id: "approval_1",
							created_at: timestamp,
							expected_session_version: 1,
							operation_summary_json: '{"type":"clean"}',
							request_fingerprint: fingerprint,
							source_command_id: "command_1",
							source_head,
							state: "requested",
							thread_id: "thread_1",
							updated_at: timestamp,
							workspace_id: "workspace_1",
						},
						{
							approval_id: "approval_2",
							created_at: timestamp,
							expected_session_version: 1,
							operation_summary_json: '{"type":"clean"}',
							request_fingerprint: fingerprint,
							source_command_id: "command_2",
							source_head,
							state: "requested",
							thread_id: "thread_2",
							updated_at: timestamp,
							workspace_id: "workspace_2",
						},
					]);
					yield* database.client.insert(WorkspaceGitMutationClaims).values({
						approval_id: "approval_1",
						claimed_at: timestamp,
						claim_token: "claim_unique",
						thread_id: "thread_1",
						workspace_id: "workspace_1",
					});
					const duplicate_token = yield* database.client
						.insert(WorkspaceGitMutationClaims)
						.values({
							approval_id: "approval_2",
							claimed_at: timestamp,
							claim_token: "claim_unique",
							thread_id: "thread_2",
							workspace_id: "workspace_2",
						})
						.pipe(Effect.exit);

					return {
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						commands: yield* database.client.select().from(JournalCommands),
						duplicate_token,
						operations: yield* database.client.select().from(WorkspaceGitOperations),
						legacy,
					};
				}),
			);

			expect(result.legacy).toHaveLength(1);
			expect(result.legacy[0]).toMatchObject({
				evidence_branch: "main",
				evidence_changed_file_count: 3,
				evidence_has_diff: true,
				evidence_recorded: false,
				evidence_root_path: "C:/legacy/repository",
				evidence_worktree_path: "C:/legacy/repository",
				kind: "recovery",
				operation_id: "legacy_git_operation",
				workspace_id: "workspace_legacy",
			});
			expect(result.operations.map((operation) => operation.kind).sort()).toEqual([
				"mutation",
				"recovery",
			]);
			expect(Exit.isFailure(result.duplicate_token)).toBe(true);
			expect(result.claims).toHaveLength(2);
			expect(
				result.claims.find(({ approval_id }) => approval_id === "legacy_claim_approval"),
			).toMatchObject({
				execution_completed_at: null,
				execution_started_at: timestamp,
				lease_expires_at: "1970-01-01T00:00:00.000Z",
				owner_instance_id: "legacy_expired",
			});
			expect(
				result.claims.find(({ approval_id }) => approval_id === "approval_1"),
			).toMatchObject({
				execution_completed_at: null,
				execution_started_at: null,
			});
			expect(result.commands).toMatchObject([
				{
					message_id: "legacy_mutation_request",
					payload_json:
						'{"expected_session_version":1,"operation":{"type":"commit"},"type":"workspace.git.mutation.request","workspace_id":"workspace_legacy"}',
				},
			]);
			expect(result.commands[0]?.payload_json).not.toContain("request_fingerprint");
		} finally {
			await current_runtime.dispose();
		}
	});
});
