import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Exit, FileSystem, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { AgentRuns, OrchestrationRuns } from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const external_wait_migration = "20260714180447_external_wait_continuations";
const temporary_directories: Array<string> = [];
const timestamp = "2026-07-14T18:00:00.000Z";

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-external-wait-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < external_wait_migration);

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

describe("external wait continuation migration", () => {
	it("preserves legacy runs, applies defaults, and widens continuation uniqueness", async () => {
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
						INSERT INTO threads (thread_id, title, created_at, updated_at)
						VALUES ('thread_external_wait', 'External wait migration', '${timestamp}', '${timestamp}')
					`);
					yield* database.client.run(`
						INSERT INTO orchestration_groups (
							group_id, thread_id, coordinator_agent_id, state, max_concurrency,
							version, journal_sequence, created_at, updated_at
						)
						VALUES (
							'group_external_wait', 'thread_external_wait', 'agent_coordinator', 'running',
							1, 4, 8, '${timestamp}', '${timestamp}'
						)
					`);
					yield* database.client.run(`
						INSERT INTO agent_instances (
							agent_id, group_id, display_name, role, created_at, updated_at
						)
						VALUES (
							'agent_coordinator', 'group_external_wait', 'Coordinator', 'coordinator',
							'${timestamp}', '${timestamp}'
						)
					`);
					yield* database.client.run(`
						INSERT INTO assignments (
							assignment_id, group_id, agent_id, role, scope_json, engine_id, profile,
							workspace_json, permission_policy_json, summary_contract, parent_node_id,
							expected_result, instructions, state, current_attempt, max_attempts,
							active_run_id, heartbeat_json, created_at, updated_at
						)
						VALUES (
							'assignment_external_wait', 'group_external_wait', 'agent_worker', 'worker',
							'{"scope":"workspace"}', 'engine_test', 'profile_test',
							'{"root":"C:/workspace"}', '{"read":true}', 'return a result',
							'node_root', 'a completed result', 'continue the work', 'complete', 1, 3,
							'agent_run_legacy', '{"sequence":7}', '${timestamp}', '${timestamp}'
						)
					`);
					yield* database.client.run(`
						INSERT INTO orchestration_runs (
							run_id, thread_id, agent_id, engine_id, working_directory, status,
							native_thread_id, native_resume_json, created_at, updated_at
						)
						VALUES (
							'orchestration_run_legacy', 'thread_external_wait', 'agent_coordinator',
							'engine_test', 'C:/workspace', 'running', 'native-thread-legacy',
							'{"native_thread_id":"native-thread-legacy"}', '${timestamp}', '${timestamp}'
						)
					`);
					yield* database.client.run(`
						INSERT INTO agent_runs (
							run_id, group_id, assignment_id, agent_id, attempt, engine_id, profile,
							state, dispatch_status, owner_instance_id, native_thread_id,
							native_resume_json, native_identity_json, raw_origin_json,
							last_observation_sequence, created_at, updated_at, completed_at
						)
						VALUES (
							'agent_run_legacy', 'group_external_wait', 'assignment_external_wait',
							'agent_worker', 1, 'engine_test', 'profile_test', 'complete', 'terminal',
							'instance-legacy', 'native-thread-agent',
							'{"native_thread_id":"native-thread-agent"}',
							'{"thread_id":"native-thread-agent"}',
							'{"provider":"engine_test","reference":"native-thread-agent"}', 7,
							'${timestamp}', '${timestamp}', '${timestamp}'
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
					const orchestration_runs = yield* database.client
						.select()
						.from(OrchestrationRuns);
					const agent_runs = yield* database.client.select().from(AgentRuns);

					yield* database.client.run(`
						INSERT INTO agent_runs (
							run_id, group_id, assignment_id, agent_id, attempt, continuation_index,
							continuation_text, engine_id, open_mode, profile, state, dispatch_status,
							owner_instance_id, native_thread_id, native_resume_json, native_identity_json,
							raw_origin_json, last_observation_sequence, created_at, updated_at, completed_at
						)
						SELECT
							'agent_run_continuation', group_id, assignment_id, agent_id, attempt, 1,
							'continue from the external wait', engine_id, 'resume', profile, state,
							dispatch_status, owner_instance_id, native_thread_id, native_resume_json,
							native_identity_json, raw_origin_json, last_observation_sequence,
							created_at, updated_at, completed_at
						FROM agent_runs
						WHERE run_id = 'agent_run_legacy'
					`);
					const duplicate_continuation = yield* database.client
						.run(`
							INSERT INTO agent_runs (
								run_id, group_id, assignment_id, agent_id, attempt, continuation_index,
								engine_id, open_mode, profile, state, dispatch_status,
								last_observation_sequence, created_at, updated_at
							)
							SELECT
								'agent_run_duplicate', group_id, assignment_id, agent_id, attempt, 0,
								engine_id, 'start', profile, state, dispatch_status,
								last_observation_sequence, created_at, updated_at
							FROM agent_runs
							WHERE run_id = 'agent_run_legacy'
						`)
						.pipe(Effect.exit);
					const migrated_agent_runs = yield* database.client.select().from(AgentRuns);

					return {
						agent_runs,
						duplicate_continuation,
						migrated_agent_runs,
						orchestration_runs,
					};
				}),
			);

			expect(result.orchestration_runs).toEqual([
				{
					run_id: "orchestration_run_legacy",
					thread_id: "thread_external_wait",
					agent_id: "agent_coordinator",
					engine_id: "engine_test",
					working_directory: "C:/workspace",
					status: "running",
					open_mode: "start",
					native_thread_id: "native-thread-legacy",
					native_resume_json: '{"native_thread_id":"native-thread-legacy"}',
					created_at: timestamp,
					updated_at: timestamp,
				},
			]);
			expect(result.agent_runs).toEqual([
				{
					run_id: "agent_run_legacy",
					group_id: "group_external_wait",
					assignment_id: "assignment_external_wait",
					agent_id: "agent_worker",
					attempt: 1,
					continuation_index: 0,
					continuation_text: null,
					engine_id: "engine_test",
					open_mode: "start",
					profile: "profile_test",
					state: "complete",
					dispatch_status: "terminal",
					owner_instance_id: "instance-legacy",
					native_thread_id: "native-thread-agent",
					native_resume_json: '{"native_thread_id":"native-thread-agent"}',
					native_identity_json: '{"thread_id":"native-thread-agent"}',
					raw_origin_json: '{"provider":"engine_test","reference":"native-thread-agent"}',
					last_observation_sequence: 7,
					created_at: timestamp,
					updated_at: timestamp,
					completed_at: timestamp,
				},
			]);
			expect(result.migrated_agent_runs).toHaveLength(2);
			expect(
				result.migrated_agent_runs.find(
					({ run_id }) => run_id === "agent_run_continuation",
				),
			).toMatchObject({
				assignment_id: "assignment_external_wait",
				attempt: 1,
				continuation_index: 1,
				continuation_text: "continue from the external wait",
				open_mode: "resume",
			});
			expect(Exit.isFailure(result.duplicate_continuation)).toBe(true);
		} finally {
			await current_runtime.dispose();
		}
	});
});
