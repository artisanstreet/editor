import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { tool_json_maximum_bytes } from "@artisan/protocol";
import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Exit, FileSystem, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	Threads,
	ToolControlCommands,
	ToolExecutionClaims,
	ToolInvocationPrivate,
	ToolInvocations,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const tool_control_migration = "20260716124049_complex_longshot";
const temporary_directories: Array<string> = [];
const digest = "a".repeat(64);
const now = "2026-07-16T12:00:00.000Z";
const decided_at = "2026-07-16T12:00:01.000Z";
const started_at = "2026-07-16T12:00:02.000Z";
const suspended_at = "2026-07-16T12:00:03.000Z";
const settled_at = "2026-07-16T12:00:04.000Z";
const oversized_json = `{"value":"${"\u00e9".repeat(tool_json_maximum_bytes / 2)}"}`;

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-tool-control-schema-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return join(directory, "artisan.db");
}).pipe(Effect.provide(NodeFileSystem.layer));

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-tool-control-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry !== tool_control_migration);

	yield* Effect.sync(() => temporary_directories.push(directory));
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded", discard: true },
	);

	return { database_path, prior_migrations_path };
}).pipe(Effect.provide(NodeFileSystem.layer));

type ToolInvocationRow = typeof ToolInvocations.$inferInsert;

function tool_invocation(overrides: Partial<ToolInvocationRow> = {}): ToolInvocationRow {
	return {
		agent_id: "agent_1",
		approval_id: null,
		approval_policy: "automatic",
		created_at: now,
		current_journal_sequence: 1,
		decided_at: null,
		decision: null,
		decision_id: null,
		descriptor_fingerprint: digest,
		effect: "read",
		input_schema_json: "{}",
		invocation_id: "invocation_automatic",
		label: "Read workspace",
		owner_kind: "ordinary_run",
		recovery_policy: "retry",
		request_id: "request_automatic",
		revision: 1,
		run_id: "run_1",
		settled_at: null,
		source: "artisan",
		started_at: null,
		state: "pending",
		summary: "Reads a bounded workspace projection.",
		suspended_at: null,
		thread_id: "thread_1",
		tool_id: "workspace.read",
		updated_at: now,
		workspace_id: null,
		...overrides,
	};
}

function required_invocation(
	state:
		| "approval_required"
		| "pending"
		| "running"
		| "completed"
		| "failed"
		| "denied"
		| "outcome_unknown"
		| "suspended",
	overrides: Partial<ToolInvocationRow> = {},
): ToolInvocationRow {
	const approved = !["approval_required", "denied"].includes(state);
	const started = ["running", "completed", "failed", "outcome_unknown", "suspended"].includes(
		state,
	);
	const settled = ["completed", "failed", "denied", "outcome_unknown"].includes(state);
	const updated_at = settled
		? settled_at
		: state === "suspended"
			? suspended_at
			: started
				? started_at
				: state === "pending"
					? decided_at
					: now;

	return tool_invocation({
		approval_id: `approval_${state}`,
		approval_policy: "required",
		decided_at: state === "approval_required" ? null : decided_at,
		decision: state === "approval_required" ? null : approved ? "approved" : "denied",
		decision_id: state === "approval_required" ? null : `decision_${state}`,
		invocation_id: `invocation_${state}`,
		recovery_policy: state === "suspended" ? "outcome_unknown" : "retry",
		request_id: `request_${state}`,
		settled_at: settled ? settled_at : null,
		started_at: started ? started_at : null,
		state,
		suspended_at: state === "suspended" ? suspended_at : null,
		updated_at,
		...overrides,
	});
}

function make_runtime(database_path: string, migration_path = migrations_path) {
	return ManagedRuntime.make(
		make_database_layer({ database_path, migrations_path: migration_path }),
	);
}

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

async function expect_constraint(effect: Effect.Effect<unknown, unknown, Database>) {
	const runtime = make_runtime(await make_database_path());

	try {
		await expect(runtime.runPromise(effect)).rejects.toBeDefined();
	} finally {
		await runtime.dispose();
	}
}

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

describe("tool control schema", () => {
	it("preserves a populated prior database while adding usable tool tables", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = make_runtime(paths.database_path, paths.prior_migrations_path);
		const legacy_thread = {
			affinity_version: 3,
			archived_at: null,
			created_at: "2026-07-15T10:00:00.000Z",
			current_goal: "Preserve durable state",
			last_activity_at: now,
			linked_projects_json: '[{"project_id":"project_legacy"}]',
			live_status: "Working",
			metadata_version: 4,
			pinned: true,
			primary_project_id: "project_legacy",
			primary_project_json: '{"project_id":"project_legacy"}',
			project_affinity_scores_json: '[{"project_id":"project_legacy","score":3}]',
			project_locked: true,
			rehome_suggestion_json: null,
			rename_suggestion: "Legacy thread",
			thread_id: "thread_legacy",
			title: "Legacy durable thread",
			title_locked: true,
			title_source: "user",
			updated_at: now,
		};
		let prior_row: typeof Threads.$inferSelect | undefined;

		try {
			prior_row = await prior_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(Threads).values(legacy_thread);

					const [row] = yield* database.client.select().from(Threads);

					return row;
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
					const [thread] = yield* database.client.select().from(Threads);

					yield* database.client
						.insert(ToolInvocations)
						.values(tool_invocation({ thread_id: legacy_thread.thread_id }));
					yield* database.client.insert(ToolInvocationPrivate).values({
						arguments_digest: digest,
						arguments_json: "{}",
						invocation_id: "invocation_automatic",
						request_fingerprint: digest,
						result_digest: null,
						result_json: null,
					});
					yield* database.client.insert(ToolControlCommands).values({
						accepted_at: now,
						approval_id: null,
						command_id: "command_migrated",
						decision: null,
						invocation_id: "invocation_automatic",
						kind: "invoke",
						request_fingerprint: digest,
					});
					yield* database.client.insert(ToolExecutionClaims).values({
						claim_token: "claim_migrated",
						claimed_at: now,
						invocation_id: "invocation_automatic",
						launch_started_at: null,
						lease_expires_at: now,
						owner_instance_id: "backend_1",
					});

					return {
						claims: yield* database.client.select().from(ToolExecutionClaims),
						commands: yield* database.client.select().from(ToolControlCommands),
						private_rows: yield* database.client.select().from(ToolInvocationPrivate),
						thread,
					};
				}),
			);

			expect(prior_row).toBeDefined();
			expect(result.thread).toEqual(prior_row);
			expect(result.commands).toHaveLength(1);
			expect(result.private_rows).toHaveLength(1);
			expect(result.claims).toHaveLength(1);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("accepts every required-approval lifecycle combination", async () => {
		const runtime = make_runtime(await make_database_path());
		const states = [
			"approval_required",
			"pending",
			"running",
			"completed",
			"failed",
			"denied",
			"outcome_unknown",
			"suspended",
		] as const;

		try {
			const rows = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(ToolInvocations).values(tool_invocation());
					yield* database.client
						.insert(ToolInvocations)
						.values(states.map((state) => required_invocation(state)));

					return yield* database.client.select().from(ToolInvocations);
				}),
			);

			expect(rows.map(({ state }) => state)).toEqual(["pending", ...states]);
			expect(rows.at(-1)).toMatchObject({
				effect: "read",
				recovery_policy: "outcome_unknown",
				state: "suspended",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects inconsistent approval lifecycle and recovery-policy states", async () => {
		const runtime = make_runtime(await make_database_path());
		const invalid_rows = [
			tool_invocation({
				approval_id: "approval_automatic",
				invocation_id: "invalid_automatic_approval",
				request_id: "invalid_automatic_approval",
			}),
			tool_invocation({
				invocation_id: "invalid_automatic_denied",
				request_id: "invalid_automatic_denied",
				settled_at,
				state: "denied",
			}),
			tool_invocation({
				decided_at,
				decision: "approved",
				decision_id: "decision_automatic",
				invocation_id: "invalid_automatic_decision",
				request_id: "invalid_automatic_decision",
			}),
			tool_invocation({
				approval_policy: "sometimes",
				invocation_id: "invalid_approval_policy",
				request_id: "invalid_approval_policy",
			}),
			tool_invocation({
				invocation_id: "invalid_state",
				request_id: "invalid_state",
				state: "unknown_state",
			}),
			required_invocation("approval_required", {
				decided_at,
				decision: "approved",
				decision_id: "invalid_early_decision",
				invocation_id: "invalid_early_decision",
				request_id: "invalid_early_decision",
			}),
			required_invocation("denied", {
				decision: "approved",
				invocation_id: "invalid_denied_decision",
				request_id: "invalid_denied_decision",
			}),
			required_invocation("denied", {
				invocation_id: "invalid_denied_started",
				request_id: "invalid_denied_started",
				started_at,
			}),
			required_invocation("pending", {
				decided_at: null,
				decision: null,
				decision_id: null,
				invocation_id: "invalid_pending_undecided",
				request_id: "invalid_pending_undecided",
			}),
			required_invocation("running", {
				invocation_id: "invalid_running_unstarted",
				request_id: "invalid_running_unstarted",
				started_at: null,
			}),
			required_invocation("completed", {
				invocation_id: "invalid_completed_unsettled",
				request_id: "invalid_completed_unsettled",
				settled_at: null,
			}),
			required_invocation("suspended", {
				invocation_id: "invalid_suspended_timestamp",
				request_id: "invalid_suspended_timestamp",
				suspended_at: null,
			}),
			tool_invocation({
				invocation_id: "invalid_recovery_policy",
				recovery_policy: "derive_from_effect",
				request_id: "invalid_recovery_policy",
			}),
			tool_invocation({
				created_at: "not-a-timestamp",
				invocation_id: "invalid_timestamp_format",
				request_id: "invalid_timestamp_format",
			}),
			tool_invocation({
				created_at: "2026-07-16T24:00:00.000Z",
				invocation_id: "invalid_timestamp_hour",
				request_id: "invalid_timestamp_hour",
				updated_at: "2026-07-16T24:00:00.000Z",
			}),
			required_invocation("completed", {
				invocation_id: "invalid_timestamp_order",
				request_id: "invalid_timestamp_order",
				settled_at: started_at,
				started_at: settled_at,
				updated_at: settled_at,
			}),
		];

		try {
			for (const row of invalid_rows) {
				await expect(
					runtime.runPromise(
						Effect.gen(function* () {
							const database = yield* Database;

							yield* database.client.insert(ToolInvocations).values(row);
						}),
					),
				).rejects.toBeDefined();
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("binds decision commands to the invocation's exact approval", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(ToolInvocations).values(tool_invocation());
					yield* database.client
						.insert(ToolInvocations)
						.values(required_invocation("approval_required"));
					yield* database.client.insert(ToolControlCommands).values([
						{
							accepted_at: now,
							approval_id: null,
							command_id: "command_invoke_1",
							decision: null,
							invocation_id: "invocation_automatic",
							kind: "invoke",
							request_fingerprint: digest,
						},
						{
							accepted_at: now,
							approval_id: null,
							command_id: "command_invoke_2",
							decision: null,
							invocation_id: "invocation_automatic",
							kind: "invoke",
							request_fingerprint: digest,
						},
						{
							accepted_at: now,
							approval_id: "approval_approval_required",
							command_id: "command_decision",
							decision: "approved",
							invocation_id: "invocation_approval_required",
							kind: "decision",
							request_fingerprint: digest,
						},
					]);
				}),
			);

			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* database.client.insert(ToolControlCommands).values({
							accepted_at: now,
							approval_id: "approval_other",
							command_id: "command_mismatched_approval",
							decision: "denied",
							invocation_id: "invocation_approval_required",
							kind: "decision",
							request_fingerprint: digest,
						});
					}),
				),
			).rejects.toBeDefined();
			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* database.client.insert(ToolControlCommands).values({
							accepted_at: now,
							approval_id: "approval_approval_required",
							command_id: "command_invalid_invoke",
							decision: "approved",
							invocation_id: "invocation_approval_required",
							kind: "invoke",
							request_fingerprint: digest,
						});
					}),
				),
			).rejects.toBeDefined();

			const commands = await runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(ToolControlCommands),
				),
			);

			expect(commands).toHaveLength(3);
			expect(new Set(commands.map(({ request_fingerprint }) => request_fingerprint))).toEqual(
				new Set([digest]),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps ownership exclusively on the invocation aggregate", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(ToolInvocations).values(tool_invocation());
					yield* database.client.insert(ToolControlCommands).values({
						accepted_at: now,
						approval_id: null,
						command_id: "command_owned_by_invocation",
						decision: null,
						invocation_id: "invocation_automatic",
						kind: "invoke",
						request_fingerprint: digest,
					});
					yield* database.client.insert(ToolExecutionClaims).values({
						claim_token: "claim_owned_by_invocation",
						claimed_at: now,
						invocation_id: "invocation_automatic",
						launch_started_at: null,
						lease_expires_at: now,
						owner_instance_id: "backend_1",
					});

					const command_with_owner = yield* database.client
						.run(`
							INSERT INTO tool_control_commands (
								command_id, kind, invocation_id, thread_id,
								request_fingerprint, accepted_at
							)
							VALUES (
								'command_conflicting_owner', 'invoke', 'invocation_automatic',
								'thread_other', '${digest}', '${now}'
							)
						`)
						.pipe(Effect.exit);
					const claim_with_owner = yield* database.client
						.run(`
							INSERT INTO tool_execution_claims (
								invocation_id, thread_id, claim_token, owner_instance_id,
								claimed_at, lease_expires_at
							)
							VALUES (
								'invocation_automatic', 'thread_other', 'claim_conflicting_owner',
								'backend_2', '${now}', '${now}'
							)
						`)
						.pipe(Effect.exit);

					return {
						claim: yield* database.client.select().from(ToolExecutionClaims),
						claim_with_owner,
						command: yield* database.client.select().from(ToolControlCommands),
						command_with_owner,
					};
				}),
			);

			expect(result.command).toEqual([
				{
					accepted_at: now,
					approval_id: null,
					command_id: "command_owned_by_invocation",
					decision: null,
					invocation_id: "invocation_automatic",
					kind: "invoke",
					request_fingerprint: digest,
				},
			]);
			expect(result.claim).toEqual([
				{
					claim_token: "claim_owned_by_invocation",
					claimed_at: now,
					invocation_id: "invocation_automatic",
					launch_started_at: null,
					lease_expires_at: now,
					owner_instance_id: "backend_1",
				},
			]);
			expect(Exit.isFailure(result.command_with_owner)).toBe(true);
			expect(Exit.isFailure(result.claim_with_owner)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("bounds private JSON by UTF-8 bytes and requires result pairs", async () => {
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client
					.insert(ToolInvocations)
					.values(tool_invocation({ input_schema_json: "{" }));
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolInvocationPrivate).values({
					arguments_digest: digest,
					arguments_json: "{",
					invocation_id: "invocation_automatic",
					request_fingerprint: digest,
					result_digest: null,
					result_json: null,
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolInvocationPrivate).values({
					arguments_digest: digest,
					arguments_json: "{}",
					invocation_id: "invocation_automatic",
					request_fingerprint: digest,
					result_digest: digest,
					result_json: "{",
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolInvocationPrivate).values({
					arguments_digest: digest,
					arguments_json: oversized_json,
					invocation_id: "invocation_automatic",
					request_fingerprint: digest,
					result_digest: null,
					result_json: null,
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolControlCommands).values({
					accepted_at: "2026-07-16T24:00:00.000Z",
					approval_id: null,
					command_id: "command_invalid_hour",
					decision: null,
					invocation_id: "invocation_automatic",
					kind: "invoke",
					request_fingerprint: digest,
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolInvocationPrivate).values({
					arguments_digest: digest,
					arguments_json: "{}",
					invocation_id: "invocation_automatic",
					request_fingerprint: digest,
					result_digest: digest,
					result_json: oversized_json,
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolInvocationPrivate).values({
					arguments_digest: digest,
					arguments_json: "{}",
					invocation_id: "invocation_automatic",
					request_fingerprint: digest,
					result_digest: null,
					result_json: "{}",
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolInvocationPrivate).values({
					arguments_digest: "invalid",
					arguments_json: "{}",
					invocation_id: "invocation_automatic",
					request_fingerprint: digest,
					result_digest: null,
					result_json: null,
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolControlCommands).values({
					accepted_at: "not-a-timestamp",
					approval_id: null,
					command_id: "command_invalid_timestamp",
					decision: null,
					invocation_id: "invocation_automatic",
					kind: "invoke",
					request_fingerprint: digest,
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolExecutionClaims).values({
					claim_token: "claim_invalid_timestamp",
					claimed_at: "not-a-timestamp",
					invocation_id: "invocation_automatic",
					launch_started_at: null,
					lease_expires_at: settled_at,
					owner_instance_id: "backend_1",
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolExecutionClaims).values({
					claim_token: "claim_invalid_hour",
					claimed_at: "2026-07-16T24:00:00.000Z",
					invocation_id: "invocation_automatic",
					launch_started_at: null,
					lease_expires_at: "2026-07-16T24:00:00.000Z",
					owner_instance_id: "backend_1",
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolExecutionClaims).values({
					claim_token: "claim_late_launch",
					claimed_at: now,
					invocation_id: "invocation_automatic",
					launch_started_at: settled_at,
					lease_expires_at: started_at,
					owner_instance_id: "backend_1",
				});
			}),
		);
	});

	it("enforces invocation, approval, decision, and claim uniqueness", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(ToolInvocations).values([
						required_invocation("approval_required"),
						required_invocation("pending"),
						tool_invocation({
							invocation_id: "invocation_claim_1",
							request_id: "request_claim_1",
						}),
						tool_invocation({
							invocation_id: "invocation_claim_2",
							request_id: "request_claim_2",
						}),
					]);
					yield* database.client.insert(ToolExecutionClaims).values({
						claim_token: "claim_unique",
						claimed_at: now,
						invocation_id: "invocation_claim_1",
						launch_started_at: null,
						lease_expires_at: now,
						owner_instance_id: "backend_1",
					});
				}),
			);

			const duplicates = [
				tool_invocation({
					invocation_id: "invocation_duplicate_request",
					request_id: "request_approval_required",
				}),
				required_invocation("approval_required", {
					invocation_id: "invocation_duplicate_approval",
					request_id: "request_duplicate_approval",
				}),
				required_invocation("pending", {
					approval_id: "approval_duplicate_decision",
					invocation_id: "invocation_duplicate_decision",
					request_id: "request_duplicate_decision",
				}),
			];

			for (const duplicate of duplicates) {
				await expect(
					runtime.runPromise(
						Effect.gen(function* () {
							const database = yield* Database;

							yield* database.client.insert(ToolInvocations).values(duplicate);
						}),
					),
				).rejects.toBeDefined();
			}

			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;

						yield* database.client.insert(ToolExecutionClaims).values({
							claim_token: "claim_unique",
							claimed_at: now,
							invocation_id: "invocation_claim_2",
							launch_started_at: null,
							lease_expires_at: now,
							owner_instance_id: "backend_2",
						});
					}),
				),
			).rejects.toBeDefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects invalid revisions, journal sequences, descriptors, and claim times", async () => {
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client
					.insert(ToolInvocations)
					.values(tool_invocation({ revision: 0 }));
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client
					.insert(ToolInvocations)
					.values(tool_invocation({ current_journal_sequence: 0 }));
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client
					.insert(ToolInvocations)
					.values(tool_invocation({ descriptor_fingerprint: "invalid" }));
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client
					.insert(ToolInvocations)
					.values(tool_invocation({ input_schema_json: oversized_json }));
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolExecutionClaims).values({
					claim_token: "claim_expired",
					claimed_at: started_at,
					invocation_id: "invocation_automatic",
					launch_started_at: null,
					lease_expires_at: now,
					owner_instance_id: "backend_1",
				});
			}),
		);
		await expect_constraint(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ToolInvocations).values(tool_invocation());
				yield* database.client.insert(ToolExecutionClaims).values({
					claim_token: "claim_early_launch",
					claimed_at: started_at,
					invocation_id: "invocation_automatic",
					launch_started_at: now,
					lease_expires_at: settled_at,
					owner_instance_id: "backend_1",
				});
			}),
		);
	});

	it("cascades invocation deletion to every child table", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const invocation = required_invocation("pending", {
						approval_id: "approval_cascade",
						decision_id: "decision_cascade",
						invocation_id: "invocation_cascade",
						request_id: "request_cascade",
					});

					yield* database.client.insert(ToolInvocations).values(invocation);
					yield* database.client.insert(ToolInvocationPrivate).values({
						arguments_digest: digest,
						arguments_json: "{}",
						invocation_id: invocation.invocation_id,
						request_fingerprint: digest,
						result_digest: null,
						result_json: null,
					});
					yield* database.client.insert(ToolControlCommands).values([
						{
							accepted_at: now,
							approval_id: null,
							command_id: "command_cascade_invoke",
							decision: null,
							invocation_id: invocation.invocation_id,
							kind: "invoke",
							request_fingerprint: digest,
						},
						{
							accepted_at: decided_at,
							approval_id: invocation.approval_id,
							command_id: "command_cascade_decision",
							decision: "approved",
							invocation_id: invocation.invocation_id,
							kind: "decision",
							request_fingerprint: digest,
						},
					]);
					yield* database.client.insert(ToolExecutionClaims).values({
						claim_token: "claim_cascade",
						claimed_at: now,
						invocation_id: invocation.invocation_id,
						launch_started_at: null,
						lease_expires_at: now,
						owner_instance_id: "backend_1",
					});
					yield* database.client.delete(ToolInvocations);

					return {
						claims: yield* database.client.select().from(ToolExecutionClaims),
						commands: yield* database.client.select().from(ToolControlCommands),
						private_rows: yield* database.client.select().from(ToolInvocationPrivate),
					};
				}),
			);

			expect(result).toEqual({ claims: [], commands: [], private_rows: [] });
		} finally {
			await runtime.dispose();
		}
	});
});
