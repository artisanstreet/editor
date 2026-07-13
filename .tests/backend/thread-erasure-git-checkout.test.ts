import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	Threads,
	WorkspaceGitChangedFiles,
	WorkspaceGitCheckoutApprovals,
	WorkspaceGitCheckoutClaims,
	WorkspaceGitOperations,
	WorkspaceGitSessions,
	WorkspaceGitWorktrees,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceGitCheckoutRepository,
	WorkspaceGitCheckoutRepositoryLive,
	type RequestWorkspaceGitCheckout,
} from "../../modules/backend/src/git/workspace-git-checkout-repository";
import {
	WorkspaceGitSessionRepository,
	WorkspaceGitSessionRepositoryLive,
	type ProjectObservation,
} from "../../modules/backend/src/git/workspace-git-session-repository";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const created_at = "2026-07-13T19:00:00.000Z";
const deleted_at = "2026-07-20T19:00:00.000Z";
const cutoff = "2026-07-19T19:00:00.000Z";

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-erasure-git-checkout-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "thread_erasure_git_checkout_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed(created_at),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
		Layer.succeed(ThreadResourceQuiescer, { Quiesce: () => Effect.void }),
	);
	const repositories = Layer.mergeAll(
		WorkspaceGitCheckoutRepositoryLive,
		WorkspaceGitSessionRepositoryLive,
	).pipe(Layer.provideMerge(infrastructure));
	const erasure = ThreadErasureLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(Layer.merge(repositories, erasure));
}

function SeedThreads(
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

function observation(options: {
	readonly changed?: boolean;
	readonly operation_id: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}): ProjectObservation {
	const head = "a".repeat(40);
	const changed_files = options.changed
		? [
				{
					conflicted: false,
					path: "src/retained-cursor.ts",
					staged: false,
					status: "modified" as const,
					untracked: false,
					unstaged: true,
				},
			]
		: [];

	return {
		kind: "checkout",
		observed_at: created_at,
		operation_id: options.operation_id,
		repository_root: `C:/${options.workspace_id}`,
		request_fingerprint: "a".repeat(64),
		selected_worktree_path: `C:/${options.workspace_id}`,
		session: {
			blockers: [],
			branch: "main",
			changed_files,
			diff_stats: options.changed
				? { additions: 1, deletions: 0, files: 1 }
				: { additions: 0, deletions: 0, files: 0 },
			has_diff: options.changed ?? false,
			head,
			state: "ready",
		},
		thread_id: options.thread_id,
		workspace_id: options.workspace_id,
		worktrees: [
			{
				adapter_path: `C:/${options.workspace_id}`,
				worktree: {
					bare: false,
					branch: "main",
					detached: false,
					head,
					locked: false,
					location: "selected",
					prunable: false,
				},
			},
		],
	};
}

function checkout_request(
	thread_id: string,
	workspace_id: string,
	approval_id: string,
): RequestWorkspaceGitCheckout {
	return {
		approval_id,
		expected_session_version: 1,
		request_fingerprint: "b".repeat(64),
		source_command: {
			message_id: `${approval_id}_request`,
			sent_at: created_at,
		},
		target_branch: "release",
		target_head: "b".repeat(40),
		thread_id,
		workspace_id,
	};
}

function approval_state(state: "requested" | "approved" | "executing") {
	return Effect.gen(function* () {
		const checkouts = yield* WorkspaceGitCheckoutRepository;
		const sessions = yield* WorkspaceGitSessionRepository;
		const thread_id = `thread_${state}`;
		const workspace_id = `workspace_${state}`;
		const approval_id = `approval_${state}`;

		yield* SeedThreads([thread_id]);
		yield* sessions.Project(
			observation({ operation_id: `operation_${state}`, thread_id, workspace_id }),
		);
		yield* checkouts.Request(checkout_request(thread_id, workspace_id, approval_id));

		if (state === "requested") {
			return thread_id;
		}

		yield* checkouts.Decide({
			approval_id,
			approved: true,
			decision_command: { message_id: `${approval_id}_decision`, sent_at: created_at },
			thread_id,
		});

		if (state === "approved") {
			return thread_id;
		}

		yield* checkouts.MarkExecuting(approval_id);

		return thread_id;
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ThreadErasure Git checkout state", () => {
	it.each(["requested", "approved", "executing"] as const)(
		"fences %s checkout approval state from automatic erasure",
		async (state) => {
			const runtime = make_runtime(await make_database_path());

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;
						const thread_id = yield* approval_state(state);
						const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);
						const threads = yield* database.client
							.select()
							.from(Threads)
							.pipe(
								Effect.map((rows) =>
									rows.filter((thread) => thread.thread_id === thread_id),
								),
							);

						return { erased, threads };
					}),
				);

				expect(result.erased).toEqual([]);
				expect(result.threads).toHaveLength(1);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("removes terminal checkout state and only its current Git-session projections", async () => {
		const runtime = make_runtime(await make_database_path());
		const erased_thread_id = "thread_erased";
		const retained_thread_id = "thread_retained";
		const removed_workspace_id = "workspace_removed";
		const shared_workspace_id = "workspace_shared";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const checkouts = yield* WorkspaceGitCheckoutRepository;
					const erasure = yield* ThreadErasure;
					const sessions = yield* WorkspaceGitSessionRepository;

					yield* SeedThreads(
						[erased_thread_id, retained_thread_id],
						[retained_thread_id],
					);
					yield* sessions.Project(
						observation({
							operation_id: "operation_checkout_terminal",
							thread_id: erased_thread_id,
							workspace_id: removed_workspace_id,
						}),
					);
					yield* checkouts.Request(
						checkout_request(
							erased_thread_id,
							removed_workspace_id,
							"approval_terminal",
						),
					);
					yield* checkouts.Decide({
						approval_id: "approval_terminal",
						approved: true,
						decision_command: {
							message_id: "approval_terminal_decision",
							sent_at: created_at,
						},
						thread_id: erased_thread_id,
					});
					yield* checkouts.MarkExecuting("approval_terminal");
					yield* checkouts.MarkApplied("approval_terminal");
					yield* database.client.insert(WorkspaceGitCheckoutClaims).values({
						approval_id: "approval_terminal",
						claimed_at: created_at,
						thread_id: erased_thread_id,
						workspace_id: removed_workspace_id,
					});
					yield* sessions.Project(
						observation({
							changed: true,
							operation_id: "operation_removed_current",
							thread_id: erased_thread_id,
							workspace_id: removed_workspace_id,
						}),
					);
					yield* sessions.Project(
						observation({
							operation_id: "operation_shared_erased",
							thread_id: erased_thread_id,
							workspace_id: shared_workspace_id,
						}),
					);
					yield* sessions.Project(
						observation({
							operation_id: "operation_shared_retained",
							thread_id: retained_thread_id,
							workspace_id: shared_workspace_id,
						}),
					);

					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return {
						approvals: yield* database.client
							.select()
							.from(WorkspaceGitCheckoutApprovals),
						changed_files: yield* database.client
							.select()
							.from(WorkspaceGitChangedFiles),
						claims: yield* database.client.select().from(WorkspaceGitCheckoutClaims),
						erased,
						operations: yield* database.client.select().from(WorkspaceGitOperations),
						sessions: yield* database.client.select().from(WorkspaceGitSessions),
						worktrees: yield* database.client.select().from(WorkspaceGitWorktrees),
					};
				}),
			);

			expect(result.erased).toEqual([erased_thread_id]);
			expect(result.approvals).toEqual([]);
			expect(result.claims).toEqual([]);
			expect(result.operations.map((operation) => operation.thread_id)).toEqual([
				retained_thread_id,
			]);
			expect(result.sessions.map((session) => session.workspace_id)).toEqual([
				shared_workspace_id,
			]);
			expect(result.worktrees.map((worktree) => worktree.workspace_id)).toEqual([
				shared_workspace_id,
			]);
			expect(result.changed_files).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});
});
