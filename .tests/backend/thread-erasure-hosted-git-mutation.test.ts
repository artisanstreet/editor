import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Deferred, Effect, Exit, Fiber, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	HostedGitMutationApprovals,
	HostedGitMutationArtifacts,
	HostedGitMutationClaims,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import {
	ThreadErasure,
	ThreadErasureFailure,
	ThreadErasureLive,
} from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const created_at = "2026-07-16T12:00:00.000Z";
const cutoff = "2026-07-16T12:01:00.000Z";
const deleted_at = "2026-07-16T12:02:00.000Z";
const digest = "a".repeat(64);
const expected_head_commit = "b".repeat(40);
const repository = {
	host: "github.com",
	name: "editor",
	owner: "artisan",
	provider_id: "github",
};
const selection = {
	account_login: "alice",
	host: "github.com",
	provider_id: "github",
};
const pull_request_origin = {
	native_id: "PR_1",
	provider_id: "github",
	resource_kind: "pull_request",
};
const review_thread_origin = {
	native_id: "RT_1",
	provider_id: "github",
	resource_kind: "review_thread",
};
const review_comment_origin = {
	native_id: "RC_1",
	provider_id: "github",
	resource_kind: "review_comment",
};

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-hosted-git-mutation-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(
	database_path: string,
	quiesce: (thread_id: string) => Effect.Effect<void> = () => Effect.void,
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		Layer.succeed(ThreadResourceQuiescer, { Quiesce: quiesce }),
		JournalNotifierLive,
	);

	return ManagedRuntime.make(ThreadErasureLive.pipe(Layer.provideMerge(infrastructure)));
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected failure");
}

const SeedThread = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(EventStreams).values({
			last_sequence: 0,
			stream_id: `thread:${thread_id}`,
		});
	});

const SeedHostedMutation = (
	state:
		| "requested"
		| "approved"
		| "executing"
		| "applied"
		| "rejected"
		| "outcome_unknown"
		| "denied",
	thread_id: string,
	workspace_id = "workspace_1",
) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const approval_id = `approval_${thread_id}_${state}`;
		const has_decision = state !== "requested";
		const approved = state !== "denied";
		const has_execution = state === "applied" || state === "outcome_unknown";
		const private_artifact =
			state === "requested" || state === "approved" || state === "executing";
		const mutation = {
			body: "private review reply",
			expected_head_commit,
			operation: "reply_review_thread",
			pull_request_number: 42,
			pull_request_origin,
			repository,
			selected_branch: "feature",
			snapshot_version: 1,
			thread_origin: review_thread_origin,
			workspace_id,
		};
		const summary = {
			expected_head_commit,
			operation: "reply_review_thread",
			pull_request_number: 42,
			pull_request_origin,
			repository,
			selected_branch: "feature",
			snapshot_version: 1,
			thread_origin: review_thread_origin,
			workspace_id,
		};
		const updated_at = private_artifact || state === "denied" ? created_at : cutoff;

		yield* database.client.insert(HostedGitMutationApprovals).values({
			approval_id,
			approved: has_decision ? approved : null,
			created_at,
			decided_at: has_decision ? created_at : null,
			decision_message_id: has_decision ? `${approval_id}_decision` : null,
			expected_head_commit,
			execution_started_at: has_execution ? created_at : null,
			operation_summary_json: JSON.stringify(summary),
			pull_request_number: 42,
			pull_request_origin_json: JSON.stringify(pull_request_origin),
			repository_json: JSON.stringify(repository),
			request_fingerprint: digest,
			rejection_reason: state === "rejected" ? "remote_rejected" : null,
			result_json:
				state === "applied"
					? JSON.stringify({
							operation: "reply_review_thread",
							origin: review_comment_origin,
							status: "applied",
						})
					: null,
			selection_json: JSON.stringify(selection),
			snapshot_version: 1,
			source_command_id: `${approval_id}_request`,
			state,
			thread_id,
			unknown_reason: state === "outcome_unknown" ? "provider_outcome_unknown" : null,
			updated_at,
			workspace_id,
		});
		yield* database.client.insert(HostedGitMutationArtifacts).values({
			approval_id,
			operation_binding: digest,
			operation_json: private_artifact ? JSON.stringify({ mutation, selection }) : null,
			provider_result_json: null,
			selection_json: private_artifact ? JSON.stringify(selection) : null,
			updated_at,
		});

		if (state === "executing") {
			yield* database.client.insert(HostedGitMutationClaims).values({
				approval_id,
				claim_token: `claim_${thread_id}_${state}`,
				claimed_at: created_at,
				lease_expires_at: cutoff,
				owner_instance_id: "test_runtime",
				thread_id,
				workspace_id,
			});
		}

		return approval_id;
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

describe("ThreadErasure hosted Git mutation state", () => {
	it.each(["requested", "approved", "executing"] as const)(
		"fences %s hosted mutations during selection",
		async (state) => {
			const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));
			const thread_id = `thread_${state}`;

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;

						yield* SeedThread(thread_id);
						yield* SeedHostedMutation(state, thread_id);
						const selected = yield* erasure.CleanupExpired(cutoff, deleted_at);

						return {
							approvals: yield* database.client
								.select()
								.from(HostedGitMutationApprovals),
							erasure_claims: yield* database.client
								.select()
								.from(ThreadErasureClaims),
							selected,
							threads: yield* database.client.select().from(Threads),
						};
					}),
				);

				expect(result.selected).toEqual([]);
				expect(result.erasure_claims).toEqual([]);
				expect(result.approvals[0]?.state).toBe(state);
				expect(result.threads.map((thread) => thread.thread_id)).toEqual([thread_id]);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("rechecks pending hosted mutations created while thread resources quiesce", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath), () =>
			Deferred.succeed(entered, undefined).pipe(Effect.andThen(Deferred.await(release))),
		);
		const thread_id = "thread_quiesce_race";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;

					yield* SeedThread(thread_id);
					const cleanup = yield* erasure
						.CleanupExpired(cutoff, deleted_at)
						.pipe(Effect.forkChild({ startImmediately: true }));
					yield* Deferred.await(entered);
					const claims_during_quiescence = yield* database.client
						.select()
						.from(ThreadErasureClaims);
					yield* SeedHostedMutation("requested", thread_id);
					yield* Deferred.succeed(release, undefined);
					const erased = yield* Fiber.join(cleanup);

					return {
						approvals: yield* database.client.select().from(HostedGitMutationApprovals),
						claims_after_recheck: yield* database.client
							.select()
							.from(ThreadErasureClaims),
						claims_during_quiescence,
						erased,
						threads: yield* database.client.select().from(Threads),
					};
				}),
			);

			expect(result.claims_during_quiescence).toHaveLength(1);
			expect(result.erased).toEqual([]);
			expect(result.claims_after_recheck).toEqual([]);
			expect(result.approvals[0]?.state).toBe("requested");
			expect(result.threads.map((thread) => thread.thread_id)).toEqual([thread_id]);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when a hosted claim disagrees with its approval thread", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));
		const target_thread_id = "thread_erasure_target";
		const owner_thread_id = "thread_claim_owner";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;

					yield* SeedThread(target_thread_id);
					yield* SeedThread(owner_thread_id);
					yield* SeedHostedMutation("denied", target_thread_id, "workspace_1");
					const owner_approval_id = yield* SeedHostedMutation(
						"executing",
						owner_thread_id,
						"workspace_2",
					);
					yield* database.client
						.update(HostedGitMutationClaims)
						.set({ thread_id: target_thread_id });
					const outcome = yield* Effect.exit(erasure.CleanupExpired(cutoff, deleted_at));

					return {
						approvals: yield* database.client.select().from(HostedGitMutationApprovals),
						claims: yield* database.client.select().from(HostedGitMutationClaims),
						erasure_claims: yield* database.client.select().from(ThreadErasureClaims),
						outcome,
						owner_approval_id,
						threads: yield* database.client.select().from(Threads),
					};
				}),
			);

			expect(failure_from(result.outcome)).toBeInstanceOf(ThreadErasureFailure);
			expect(result.approvals).toHaveLength(2);
			expect(result.claims).toEqual([
				expect.objectContaining({
					approval_id: result.owner_approval_id,
					thread_id: target_thread_id,
					workspace_id: "workspace_2",
				}),
			]);
			expect(result.erasure_claims.map((claim) => claim.thread_id)).toEqual([
				target_thread_id,
			]);
			expect(result.threads.map((thread) => thread.thread_id).sort()).toEqual(
				[owner_thread_id, target_thread_id].sort(),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it.each(["denied", "applied", "rejected", "outcome_unknown"] as const)(
		"erases terminal %s hosted mutations with their scrubbed artifacts",
		async (state) => {
			const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));
			const thread_id = `thread_${state}`;

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;

						yield* SeedThread(thread_id);
						yield* SeedHostedMutation(state, thread_id);
						const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

						return {
							approvals: yield* database.client
								.select()
								.from(HostedGitMutationApprovals),
							artifacts: yield* database.client
								.select()
								.from(HostedGitMutationArtifacts),
							claims: yield* database.client.select().from(HostedGitMutationClaims),
							erased,
							threads: yield* database.client.select().from(Threads),
						};
					}),
				);

				expect(result.erased).toEqual([thread_id]);
				expect(result.approvals).toEqual([]);
				expect(result.artifacts).toEqual([]);
				expect(result.claims).toEqual([]);
				expect(result.threads).toEqual([]);
			} finally {
				await runtime.dispose();
			}
		},
	);
});
