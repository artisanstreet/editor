import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	Threads,
	WorkspaceGitMutationApprovals,
	WorkspaceGitMutationArtifacts,
	WorkspaceGitMutationClaims,
	WorkspaceGitOperations,
	WorkspaceGitSessions,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import {
	WorkspaceGitMutationRepository,
	WorkspaceGitMutationRepositoryLive,
	type RequestWorkspaceGitMutation,
} from "../../modules/backend/src/git/workspace-git-mutation-repository";
import {
	WorkspaceGitSessionRepository,
	WorkspaceGitSessionRepositoryLive,
	type ProjectObservation,
} from "../../modules/backend/src/git/workspace-git-session-repository";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const created_at = "2026-07-14T19:00:00.000Z";
const deleted_at = "2026-07-20T19:00:00.000Z";
const cutoff = "2026-07-19T19:00:00.000Z";
const digest = "a".repeat(64);
const source_head = "b".repeat(40);
const result_head = "c".repeat(40);

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-erasure-git-mutation-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "thread_erasure_git_mutation_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed(created_at),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		make_metadata_layer(),
		JournalNotifierLive,
		Layer.succeed(ThreadResourceQuiescer, { Quiesce: () => Effect.void }),
	);
	const repositories = Layer.mergeAll(
		WorkspaceGitMutationRepositoryLive,
		WorkspaceGitSessionRepositoryLive,
	).pipe(Layer.provideMerge(infrastructure));
	const erasure = ThreadErasureLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(Layer.merge(repositories, erasure));
}

function seed_threads(
	thread_ids: ReadonlyArray<string>,
	pinned_thread_ids: ReadonlyArray<string> = [],
) {
	return Effect.flatMap(Database, (database) =>
		database.client.insert(Threads).values(
			thread_ids.map((thread_id) => ({
				created_at,
				last_activity_at: created_at,
				pinned: pinned_thread_ids.includes(thread_id),
				thread_id,
				title: thread_id,
				title_source: "initial" as const,
				updated_at: created_at,
			})),
		),
	);
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

function session_observation(
	thread_id: string,
	workspace_id: string,
	operation_id: string,
): ProjectObservation {
	return {
		kind: "recovery",
		observed_at: created_at,
		operation_id,
		repository_root: `C:/${workspace_id}`,
		request_fingerprint: digest,
		selected_worktree_path: `C:/${workspace_id}`,
		session: {
			blockers: [],
			branch: "main",
			changed_files: [],
			diff_stats: { additions: 0, deletions: 0, files: 0 },
			has_diff: false,
			head: source_head,
			state: "ready",
		},
		thread_id,
		workspace_id,
		worktrees: [
			{
				adapter_path: `C:/${workspace_id}`,
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

function mutation_request(
	thread_id: string,
	workspace_id: string,
	approval_id: string,
	operation_id: string,
): RequestWorkspaceGitMutation {
	const operation = { message: "PRIVATE_COMMIT_MESSAGE", type: "commit" as const };

	return {
		approval_id,
		expected_session_version: 1,
		operation,
		plan: {
			binding: digest,
			message: operation.message,
			source: source_proof(),
			type: "commit",
		},
		request_fingerprint: digest,
		source_command: { message_id: `${operation_id}_request`, sent_at: created_at },
		thread_id,
		workspace_id,
	};
}

function merge_request(thread_id: string, workspace_id: string, approval_id: string) {
	return {
		...mutation_request(thread_id, workspace_id, approval_id, `${approval_id}_operation`),
		operation: { action: "start" as const, target_branch: "feature", type: "merge" as const },
		plan: {
			action: "start" as const,
			binding: digest,
			source: source_proof(),
			target_branch: "feature",
			target_head: result_head,
			type: "merge" as const,
		},
	};
}

function attempt() {
	return {
		binding: digest,
		exit_code: 0,
		operation_head: result_head,
		output_complete: true,
		output_identity: digest,
		phase: "mutation" as const,
		plan_binding: digest,
		result: { ...source_proof(), head: result_head },
		type: "attempt" as const,
	};
}

function merge_conflict_reconciliation() {
	return {
		action: "merge_conflict" as const,
		anchor: {
			branch: "main",
			identity: digest,
			original_head: source_head,
			plan_binding: digest,
			state: "merge" as const,
			target_head: result_head,
			type: "merge" as const,
		},
		type: "action_required" as const,
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ThreadErasure generic Git mutation state", () => {
	it.each(["requested", "approved", "executing", "action_required"] as const)(
		"fences %s mutation approval state from automatic erasure",
		async (state) => {
			const runtime = make_runtime(await make_database_path());

			try {
				await runtime.runPromise(
					Effect.gen(function* () {
						const repository = yield* WorkspaceGitMutationRepository;
						const sessions = yield* WorkspaceGitSessionRepository;
						const thread_id = `thread_${state}`;
						const workspace_id = `workspace_${state}`;
						const approval_id = `approval_${state}`;

						yield* seed_threads([thread_id]);
						yield* sessions.Project(
							session_observation(thread_id, workspace_id, `session_${state}`),
						);
						yield* repository.Request(
							state === "action_required"
								? merge_request(thread_id, workspace_id, approval_id)
								: mutation_request(
										thread_id,
										workspace_id,
										approval_id,
										`operation_${state}`,
									),
						);

						if (state === "requested") {
							return;
						}

						yield* repository.Decide({
							approval_id,
							approved: true,
							decision_command: {
								message_id: `${approval_id}_decision`,
								sent_at: created_at,
							},
							thread_id,
						});

						if (state === "approved") {
							return;
						}

						yield* repository.MarkExecuting(approval_id);

						if (state === "action_required") {
							const execution = yield* repository.ReadExecution(approval_id);
							yield* repository.RecordAttempt(
								{ approval_id, claim_token: execution.claim_token },
								attempt(),
							);
							yield* repository.RecordReconciliation(
								{ approval_id, claim_token: execution.claim_token },
								merge_conflict_reconciliation(),
							);
							yield* repository.Settle({
								action: "merge_conflict",
								approval_id,
								claim_token: execution.claim_token,
								type: "action_required",
							});
						}

						return;
					}),
				);
				const erased = await runtime.runPromise(
					Effect.gen(function* () {
						const erasure = yield* ThreadErasure;
						const database = yield* Database;

						const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

						return {
							erased,
							threads: yield* database.client.select().from(Threads),
							approvals: yield* database.client
								.select()
								.from(WorkspaceGitMutationApprovals),
						};
					}),
				);

				expect(erased.erased).toEqual([]);
				expect(erased.threads.map((thread) => thread.thread_id)).toEqual([
					`thread_${state}`,
				]);
				expect(erased.approvals[0]?.state).toBe(
					state === "action_required" ? "action_required" : state,
				);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("erases applied mutation evidence without deleting a retained thread's current session", async () => {
		const runtime = make_runtime(await make_database_path());
		const erased_thread_id = "thread_erased";
		const retained_thread_id = "thread_retained";
		const workspace_id = "workspace_shared";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const repository = yield* WorkspaceGitMutationRepository;
					const sessions = yield* WorkspaceGitSessionRepository;
					const approval_id = "approval_applied";

					yield* seed_threads(
						[erased_thread_id, retained_thread_id],
						[retained_thread_id],
					);
					yield* sessions.Project(
						session_observation(erased_thread_id, workspace_id, "session_erased"),
					);
					yield* repository.Request(
						mutation_request(
							erased_thread_id,
							workspace_id,
							approval_id,
							"operation_erased",
						),
					);
					yield* repository.Decide({
						approval_id,
						approved: true,
						decision_command: { message_id: "decision_applied", sent_at: created_at },
						thread_id: erased_thread_id,
					});
					yield* repository.MarkExecuting(approval_id);
					const execution = yield* repository.ReadExecution(approval_id);
					yield* repository.RecordAttempt(
						{ approval_id, claim_token: execution.claim_token },
						attempt(),
					);
					yield* repository.RecordReconciliation(
						{ approval_id, claim_token: execution.claim_token },
						{ branch: "main", head: result_head, type: "applied" },
					);
					yield* repository.Settle({
						approval_id,
						branch: "main",
						claim_token: execution.claim_token,
						head: result_head,
						type: "applied",
					});
					yield* database.client.insert(WorkspaceGitMutationClaims).values({
						approval_id,
						claimed_at: created_at,
						claim_token: "terminal_claim",
						thread_id: erased_thread_id,
						workspace_id,
					});
					yield* sessions.Project(
						session_observation(retained_thread_id, workspace_id, "session_retained"),
					);

					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return {
						approvals: yield* database.client
							.select()
							.from(WorkspaceGitMutationApprovals),
						artifacts: yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts),
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						erased,
						operations: yield* database.client.select().from(WorkspaceGitOperations),
						sessions: yield* database.client.select().from(WorkspaceGitSessions),
					};
				}),
			);

			expect(result.erased).toEqual([erased_thread_id]);
			expect(result.approvals).toEqual([]);
			expect(result.artifacts).toEqual([]);
			expect(result.claims).toEqual([]);
			expect(result.operations.map((operation) => operation.thread_id)).toEqual([
				retained_thread_id,
			]);
			expect(result.sessions.map((session) => session.workspace_id)).toEqual([workspace_id]);
		} finally {
			await runtime.dispose();
		}
	});
});
