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
	WorkspaceChangeOperations,
	WorkspaceGitMutationArtifacts,
	WorkspaceGitMutationClaims,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceGitCheckoutRepository,
	WorkspaceGitCheckoutRepositoryLive,
	type RequestWorkspaceGitCheckout,
} from "../../modules/backend/src/git/workspace-git-checkout-repository";
import {
	WorkspaceGitMutationConflict,
	WorkspaceGitMutationInvariant,
	WorkspaceGitMutationRepository,
	WorkspaceGitMutationRepositoryLive,
	WorkspaceGitMutationUnavailable,
	type RequestWorkspaceGitMutation,
	type WorkspaceGitMutationDecision,
} from "../../modules/backend/src/git/workspace-git-mutation-repository";
import {
	WorkspaceGitSessionRepository,
	WorkspaceGitSessionRepositoryLive,
	type ProjectObservation,
} from "../../modules/backend/src/git/workspace-git-session-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-14T10:00:00.000Z";
const later = "2026-07-14T10:01:00.000Z";
const digest = "a".repeat(64);
const source_head = "b".repeat(40);
const result_head = "c".repeat(40);
const remote_endpoint = "https://example.com/repository.git";
let next_id = 0;
let next_time = Date.parse("2026-07-14T11:00:00.000Z");

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-git-mutation-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function make_metadata_layer() {
	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_git_mutation_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_git_mutation_${++next_id}`),
		Now: Effect.sync(() => new Date(next_time++).toISOString()),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const repositories = Layer.mergeAll(
		WorkspaceGitSessionRepositoryLive,
		WorkspaceGitCheckoutRepositoryLive,
		WorkspaceGitMutationRepositoryLive,
	).pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(repositories);
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

function expect_conflict(exit: Exit.Exit<unknown, unknown>, reason: string) {
	const failure = failure_from(exit);

	expect(failure).toBeInstanceOf(WorkspaceGitMutationConflict);
	expect(failure).toMatchObject({ reason });
}

const SeedThreads = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values([
		{
			created_at: now,
			thread_id: "thread_1",
			title: "Git mutation repository",
			title_source: "initial",
			updated_at: now,
		},
		{
			created_at: now,
			thread_id: "thread_2",
			title: "Git mutation conflict",
			title_source: "initial",
			updated_at: now,
		},
	]);
});

function session_observation(
	operation_id: string,
	overrides: { readonly workspace_id?: string } = {},
): ProjectObservation {
	return {
		kind: "recovery",
		observed_at: now,
		operation_id,
		repository_root: "C:/workspace",
		request_fingerprint: digest,
		selected_worktree_path: "C:/workspace",
		session: {
			blockers: [],
			branch: "main",
			changed_files: [],
			diff_stats: { additions: 0, deletions: 0, files: 0 },
			has_diff: false,
			head: source_head,
			state: "ready",
		},
		thread_id: "thread_1",
		workspace_id: overrides.workspace_id ?? "workspace_1",
		worktrees: [
			{
				adapter_path: "C:/workspace",
				worktree: {
					bare: false,
					branch: "main",
					detached: false,
					head: source_head,
					locked: false,
					location: "selected",
					prunable: false,
				},
			},
		],
	};
}

function source_proof() {
	return {
		branch: "main",
		configuration_identity: digest,
		head: source_head,
		index_identity: digest,
		repository_identity: digest,
		state: "none" as const,
		state_identity: digest,
		status_identity: digest,
		tracked_identity: digest,
		untracked_identity: digest,
		worktree_identity: digest,
	};
}

function mutation_request(
	overrides: Partial<RequestWorkspaceGitMutation> = {},
): RequestWorkspaceGitMutation {
	const source = source_proof();
	const operation = { message: "PRIVATE_COMMIT_MESSAGE", type: "commit" as const };

	return {
		approval_id: "mutation_approval_1",
		expected_session_version: 1,
		operation,
		plan: {
			binding: digest,
			message: operation.message,
			source,
			type: "commit",
		},
		request_fingerprint: digest,
		source_command: { message_id: "mutation_request_1", sent_at: now },
		thread_id: "thread_1",
		workspace_id: "workspace_1",
		...overrides,
	};
}

function mutation_decision(
	overrides: Partial<WorkspaceGitMutationDecision> = {},
): WorkspaceGitMutationDecision {
	return {
		approval_id: "mutation_approval_1",
		approved: true,
		decision_command: { message_id: "mutation_decision_1", sent_at: later },
		thread_id: "thread_1",
		...overrides,
	};
}

function push_mutation_request(set_upstream = false): RequestWorkspaceGitMutation {
	const operation = {
		remote: "origin",
		set_upstream,
		target_branch: "main",
		type: "push" as const,
	};

	return mutation_request({
		operation,
		plan: {
			binding: digest,
			remote: operation.remote,
			remote_endpoint,
			set_upstream: operation.set_upstream,
			source: source_proof(),
			source_branch: "main",
			source_head,
			target_branch: operation.target_branch,
			type: "push",
		},
	});
}

function checkout_request(): RequestWorkspaceGitCheckout {
	return {
		approval_id: "checkout_approval_1",
		expected_session_version: 1,
		request_fingerprint: digest,
		source_command: { message_id: "checkout_request_1", sent_at: now },
		target_branch: "release",
		target_head: result_head,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function attempt(plan_binding = digest) {
	return {
		binding: digest,
		exit_code: 0,
		operation_head: result_head,
		output_complete: true,
		output_identity: digest,
		phase: "mutation" as const,
		plan_binding,
		result: { ...source_proof(), head: result_head },
		type: "attempt" as const,
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

describe("WorkspaceGitMutationRepository", () => {
	it("redacts public records while replaying the original approval despite a newly prepared plan", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_request"));
					const requested = yield* repository.Request(mutation_request());
					const replay = yield* repository.Request(
						mutation_request({
							plan: { ...mutation_request().plan, binding: "d".repeat(64) },
						}),
					);
					const changed = yield* repository
						.Request(
							mutation_request({
								operation: { message: "CHANGED_PRIVATE_INTENT", type: "commit" },
								plan: {
									binding: digest,
									message: "CHANGED_PRIVATE_INTENT",
									source: source_proof(),
									type: "commit",
								},
							}),
						)
						.pipe(Effect.exit);

					return {
						artifacts: yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts),
						changed,
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						requested,
						replay,
					};
				}),
			);

			expect(result.replay).toEqual({ ...result.requested, status: "duplicate" });
			expect_conflict(result.changed, "request_conflict");
			expect(result.requested.approval.operation).toEqual({ type: "commit" });
			expect(JSON.stringify(result.commands)).not.toContain("PRIVATE_COMMIT_MESSAGE");
			expect(JSON.stringify(result.events)).not.toContain("PRIVATE_COMMIT_MESSAGE");
			expect(JSON.stringify(result.artifacts)).toContain("PRIVATE_COMMIT_MESSAGE");
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps decisions immutable and persists a leased execution through restart and atomic applied settlement", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);

		try {
			const result = await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceGitMutationRepository;
					const sessions = yield* WorkspaceGitSessionRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_execution"));
					yield* repository.Request(mutation_request());
					const decided = yield* repository.Decide(mutation_decision());
					const decision_replay = yield* repository.Decide(mutation_decision());
					const opposing = yield* repository
						.Decide(mutation_decision({ approved: false }))
						.pipe(Effect.exit);
					yield* repository.MarkExecuting("mutation_approval_1");
					const execution = yield* repository.ReadExecution("mutation_approval_1");

					return { decided, decision_replay, execution, opposing };
				}),
			);

			expect(result.decision_replay).toEqual({ ...result.decided, status: "duplicate" });
			expect_conflict(result.opposing, "decision_conflict");
			expect(result.execution.claim_token).toMatch(/^claim_git_mutation_/u);
		} finally {
			await first_runtime.dispose();
		}

		const restarted = make_runtime(database_path);

		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceGitMutationRepository;
					const execution = yield* repository.ReadExecution("mutation_approval_1");
					const identity = {
						approval_id: "mutation_approval_1",
						claim_token: execution.claim_token,
					};
					const stale_attempt = yield* repository
						.RecordAttempt(
							{ approval_id: "mutation_approval_1", claim_token: "stale_claim" },
							attempt(),
						)
						.pipe(Effect.exit);
					const stale_reconciliation = yield* repository
						.RecordReconciliation(
							{ approval_id: "mutation_approval_1", claim_token: "stale_claim" },
							{ type: "outcome_unknown" },
						)
						.pipe(Effect.exit);
					yield* repository.RecordAttempt(identity, attempt());
					yield* repository.RecordReconciliation(identity, {
						branch: "main",
						head: result_head,
						type: "applied",
					});
					const settled = yield* repository.Settle({
						approval_id: "mutation_approval_1",
						branch: "main",
						claim_token: execution.claim_token,
						head: result_head,
						type: "applied",
					});
					const replay = yield* repository.Settle({
						approval_id: "mutation_approval_1",
						branch: "main",
						claim_token: "stale_claim",
						head: result_head,
						type: "applied",
					});

					return {
						artifacts: yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts),
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						replay,
						settled,
						stale_attempt,
						stale_reconciliation,
					};
				}),
			);

			expect_conflict(result.stale_attempt, "lease_conflict");
			expect_conflict(result.stale_reconciliation, "lease_conflict");
			expect(result.settled.approval.state).toBe("applied");
			expect(result.replay).toEqual({ ...result.settled, status: "duplicate" });
			expect(result.claims).toEqual([]);
			expect(result.artifacts[0]?.attempt_json).toContain('"type":"attempt"');
			expect(result.artifacts[0]?.reconciliation_json).toContain('"type":"applied"');
		} finally {
			await restarted.dispose();
		}
	});

	it("settles a crash-recovery source reconciliation as interrupted without inventing an attempt", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_recovery"));
					yield* repository.Request(mutation_request());
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					const execution = yield* repository.ReadExecution("mutation_approval_1");
					yield* repository.RecordReconciliation(
						{
							approval_id: "mutation_approval_1",
							claim_token: execution.claim_token,
						},
						{ type: "source" },
					);
					const settled = yield* repository.Settle({
						approval_id: "mutation_approval_1",
						claim_token: execution.claim_token,
						reason: "interrupted",
						type: "outcome_unknown",
					});

					return {
						artifact: (yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts))[0],
						settled,
					};
				}),
			);

			expect(result.settled.approval).toMatchObject({
				reason: "interrupted",
				state: "outcome_unknown",
			});
			expect(result.artifact?.attempt_json).toBeNull();
			expect(result.artifact?.reconciliation_json).toBe('{"type":"source"}');
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when persisted reconciliation claims success without an attempt", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_corruption"));
					yield* repository.Request(mutation_request());
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					const execution = yield* repository.ReadExecution("mutation_approval_1");

					yield* database.client.update(WorkspaceGitMutationArtifacts).set({
						reconciled_at: later,
						reconciliation_json: JSON.stringify({
							branch: "main",
							head: result_head,
							type: "applied",
						}),
						updated_at: later,
					});
					const settlement = yield* repository
						.Settle({
							approval_id: "mutation_approval_1",
							branch: "main",
							claim_token: execution.claim_token,
							head: result_head,
							type: "applied",
						})
						.pipe(Effect.exit);

					return {
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						settlement,
					};
				}),
			);

			const failure = failure_from(result.settlement);

			expect(failure).toBeInstanceOf(WorkspaceGitMutationInvariant);
			expect(result.claims).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects applied reconciliation that contradicts its persisted attempt", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_failed_attempt"));
					yield* repository.Request(mutation_request());
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					const execution = yield* repository.ReadExecution("mutation_approval_1");
					const identity = {
						approval_id: "mutation_approval_1",
						claim_token: execution.claim_token,
					};

					yield* repository.RecordAttempt(identity, {
						...attempt(),
						exit_code: 1,
						operation_head: source_head,
						result: source_proof(),
					});
					const reconciliation = yield* repository
						.RecordReconciliation(identity, {
							branch: "main",
							head: result_head,
							type: "applied",
						})
						.pipe(Effect.exit);

					return {
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						reconciliation,
					};
				}),
			);

			expect_conflict(result.reconciliation, "artifact_conflict");
			expect(result.claims).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it.each([
		["the remote applied it before a nonzero process exit", 1, "mutation"],
		["its requested upstream settlement completed", 0, "settlement"],
	] as const)("accepts a reconciled push when %s", async (_scenario, exit_code, phase) => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_push_reconciliation"));
					yield* repository.Request(push_mutation_request(phase === "settlement"));
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					const execution = yield* repository.ReadExecution("mutation_approval_1");
					const identity = {
						approval_id: "mutation_approval_1",
						claim_token: execution.claim_token,
					};

					yield* repository.RecordAttempt(identity, {
						...attempt(),
						exit_code,
						operation_head: source_head,
						phase,
						result: source_proof(),
					});
					yield* repository.RecordReconciliation(identity, {
						branch: "main",
						head: source_head,
						remote: "origin",
						remote_endpoint,
						remote_head: source_head,
						target_branch: "main",
						type: "applied",
					});
					const settled = yield* repository.Settle({
						approval_id: "mutation_approval_1",
						branch: "main",
						claim_token: execution.claim_token,
						head: source_head,
						remote_head: source_head,
						type: "applied",
					});

					return {
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						settled,
					};
				}),
			);

			expect(result.settled.approval).toMatchObject({
				resulting_head: source_head,
				state: "applied",
			});
			expect(result.claims).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects push reconciliation that names a different approved remote ref", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_push_ref_binding"));
					yield* repository.Request(push_mutation_request());
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					const execution = yield* repository.ReadExecution("mutation_approval_1");
					const identity = {
						approval_id: "mutation_approval_1",
						claim_token: execution.claim_token,
					};
					const reconciliation = {
						branch: "main",
						head: source_head,
						remote: "origin",
						remote_endpoint,
						remote_head: source_head,
						target_branch: "main",
						type: "applied" as const,
					};

					yield* repository.RecordAttempt(identity, {
						...attempt(),
						exit_code: 1,
						operation_head: source_head,
						result: source_proof(),
					});
					const wrong_remote = yield* repository
						.RecordReconciliation(identity, {
							...reconciliation,
							remote: "upstream",
						})
						.pipe(Effect.exit);
					const wrong_endpoint = yield* repository
						.RecordReconciliation(identity, {
							...reconciliation,
							remote_endpoint: "https://example.com/other.git",
						})
						.pipe(Effect.exit);
					const wrong_branch = yield* repository
						.RecordReconciliation(identity, {
							...reconciliation,
							target_branch: "release",
						})
						.pipe(Effect.exit);

					return {
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						wrong_branch,
						wrong_endpoint,
						wrong_remote,
					};
				}),
			);

			expect_conflict(result.wrong_remote, "artifact_conflict");
			expect_conflict(result.wrong_endpoint, "artifact_conflict");
			expect_conflict(result.wrong_branch, "artifact_conflict");
			expect(result.claims).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it.each([
		["zero-exit precondition", 0, "precondition"],
		["nonzero precondition", 1, "precondition"],
		["nonzero settlement", 1, "settlement"],
	] as const)(
		"rejects a %s push attempt as applied evidence",
		async (_scenario, exit_code, phase) => {
			const runtime = make_runtime(await make_database_path());

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const sessions = yield* WorkspaceGitSessionRepository;
						const repository = yield* WorkspaceGitMutationRepository;

						yield* SeedThreads;
						yield* sessions.Project(session_observation(`session_push_${phase}`));
						yield* repository.Request(push_mutation_request(phase === "settlement"));
						yield* repository.Decide(mutation_decision());
						yield* repository.MarkExecuting("mutation_approval_1");
						const execution = yield* repository.ReadExecution("mutation_approval_1");
						const identity = {
							approval_id: "mutation_approval_1",
							claim_token: execution.claim_token,
						};

						yield* repository.RecordAttempt(identity, {
							...attempt(),
							exit_code,
							operation_head: source_head,
							phase,
							result: source_proof(),
						});
						const reconciliation = yield* repository
							.RecordReconciliation(identity, {
								branch: "main",
								head: source_head,
								remote: "origin",
								remote_endpoint,
								remote_head: source_head,
								target_branch: "main",
								type: "applied",
							})
							.pipe(Effect.exit);

						return {
							claims: yield* database.client
								.select()
								.from(WorkspaceGitMutationClaims),
							reconciliation,
						};
					}),
				);

				expect_conflict(result.reconciliation, "artifact_conflict");
				expect(result.claims).toHaveLength(1);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("allows one same-thread continuation from its conflict anchor and rejects mismatched or second live children", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;
					const parent_plan = {
						binding: digest,
						source: source_proof(),
						target_branch: "feature",
						target_head: result_head,
						type: "merge" as const,
						action: "start" as const,
					};

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_anchor"));
					yield* repository.Request(
						mutation_request({
							operation: { action: "start", target_branch: "feature", type: "merge" },
							plan: parent_plan,
						}),
					);
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					const parent = yield* repository.ReadExecution("mutation_approval_1");
					const parent_identity = {
						approval_id: "mutation_approval_1",
						claim_token: parent.claim_token,
					};
					const anchor = {
						branch: "main",
						identity: digest,
						original_head: source_head,
						plan_binding: digest,
						state: "merge" as const,
						target_head: result_head,
						type: "merge" as const,
					};
					yield* repository.RecordAttempt(parent_identity, attempt());
					yield* repository.RecordReconciliation(parent_identity, {
						action: "merge_conflict",
						anchor,
						type: "action_required",
					});
					yield* repository.Settle({
						action: "merge_conflict",
						approval_id: "mutation_approval_1",
						claim_token: parent.claim_token,
						type: "action_required",
					});
					const continuation = mutation_request({
						action_approval_id: "mutation_approval_1",
						approval_id: "mutation_approval_2",
						operation: { action: "continue", type: "merge" },
						plan: {
							action: "continue",
							anchor,
							binding: digest,
							source: source_proof(),
							type: "merge",
						},
						source_command: { message_id: "mutation_request_2", sent_at: later },
					});
					const accepted = yield* repository.Request(continuation);
					const second = yield* repository
						.Request({
							...continuation,
							approval_id: "mutation_approval_3",
							source_command: { message_id: "mutation_request_3", sent_at: later },
						})
						.pipe(Effect.exit);
					const mismatch = yield* repository
						.ReadActionAnchor({
							action_approval_id: "mutation_approval_1",
							operation: { action: "continue", type: "rebase" },
							thread_id: "thread_1",
							workspace_id: "workspace_1",
						})
						.pipe(Effect.exit);

					return { accepted, mismatch, second };
				}),
			);

			expect(result.accepted.approval.action_approval_id).toBe("mutation_approval_1");
			expect_conflict(result.mismatch, "action_conflict");
			expect_conflict(result.second, "action_conflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("cross-fences controlled writes and legacy checkout claims, then rejects access once its thread is erased", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const checkouts = yield* WorkspaceGitCheckoutRepository;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_fence"));
					yield* repository.Request(mutation_request());
					yield* repository.Decide(mutation_decision());
					yield* database.client.insert(WorkspaceChangeOperations).values({
						action: "replace",
						change_id: "change_active",
						created_at: now,
						lifecycle: "claimed",
						message_id: "workspace_change_active",
						request_fingerprint: digest,
						sent_at: now,
						thread_id: "thread_1",
						updated_at: now,
						workspace_id: "workspace_1",
					});
					yield* database.client.insert(WorkspaceMutationAuthorities).values({
						agent_id: "agent_1",
						authority_kind: "base_run",
						change_id: "change_active",
						created_at: now,
						message_id: "workspace_change_active",
						run_id: "run_1",
						thread_id: "thread_1",
						working_directory: "C:/workspace",
						workspace_id: "workspace_1",
					});
					const blocked = yield* repository
						.MarkExecuting("mutation_approval_1")
						.pipe(Effect.exit);
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "committed", updated_at: later });
					yield* checkouts.Request(checkout_request());
					yield* checkouts.Decide({
						approval_id: "checkout_approval_1",
						approved: true,
						decision_command: { message_id: "checkout_decision_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* checkouts.MarkExecuting("checkout_approval_1");
					const legacy_claim = yield* repository
						.MarkExecuting("mutation_approval_1")
						.pipe(Effect.exit);
					yield* database.client
						.insert(ThreadErasureClaims)
						.values({ claimed_at: later, thread_id: "thread_1" });
					const erased = yield* repository
						.Query({ approval_id: "mutation_approval_1", thread_id: "thread_1" })
						.pipe(Effect.exit);

					return { blocked, erased, legacy_claim };
				}),
			);

			expect_conflict(result.blocked, "workspace_mutation_active");
			expect_conflict(result.legacy_claim, "claim_conflict");
			const failure = failure_from(result.erased);
			expect(failure).toBeInstanceOf(WorkspaceGitMutationUnavailable);
			expect(failure).toMatchObject({ reason: "erased" });
		} finally {
			await runtime.dispose();
		}
	});

	it("prevents legacy checkout from claiming a workspace owned by a generic Git mutation", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const blocked = await runtime.runPromise(
				Effect.gen(function* () {
					const checkouts = yield* WorkspaceGitCheckoutRepository;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_generic_fence"));
					yield* repository.Request(mutation_request());
					yield* repository.Decide(mutation_decision());
					yield* repository.MarkExecuting("mutation_approval_1");
					yield* checkouts.Request(checkout_request());
					yield* checkouts.Decide({
						approval_id: "checkout_approval_1",
						approved: true,
						decision_command: { message_id: "checkout_decision_1", sent_at: later },
						thread_id: "thread_1",
					});

					return yield* checkouts.MarkExecuting("checkout_approval_1").pipe(Effect.exit);
				}),
			);

			const failure = failure_from(blocked);

			expect(failure).toMatchObject({ reason: "claim_conflict" });
		} finally {
			await runtime.dispose();
		}
	});
});
