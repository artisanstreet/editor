import { createHash } from "node:crypto";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime, Redacted } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_workspace_bounded_regular_file_store_registry_layer } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreLive } from "../../modules/backend/src/persistence/journal-store";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
	Threads,
	WorkspaceChangeOperations,
	WorkspaceGitCheckoutApprovals,
	WorkspaceGitCheckoutClaims,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceChangeRepository,
	WorkspaceChangeRepositoryLive,
} from "../../modules/backend/src/workspace/workspace-change-repository";
import type { PreparedWorkspaceChangeDiff } from "../../modules/backend/src/workspace/workspace-change-diff-service";
import {
	WorkspaceMutationAuthority,
	WorkspaceMutationAuthorityDenied,
	WorkspaceMutationAuthorityLive,
} from "../../modules/backend/src/workspace/workspace-mutation-authority";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-13T19:00:00.000Z";
const receipt_authentication_key = Redacted.make(new Uint8Array(32).fill(7));

async function make_workspace() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-git-mutation-fence-"));

	temporary_directories.push(directory);

	return { database_path: join(directory, "artisan.db"), root: join(directory, "workspace") };
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_git_mutation_fence_test",
		MakeId: (prefix) => Effect.succeed(`workspace_git_mutation_fence_${prefix}_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_runtime(database_path: string, root: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const changes = Layer.mergeAll(WorkspaceChangeRepositoryLive, JournalStoreLive).pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const registry = make_workspace_bounded_regular_file_store_registry_layer(
		[{ root, workspace_id: "workspace_1" }],
		{
			load_native_module: () => ({
				NativeBoundedRegularFileStore: class {
					authorizeRoot(candidate_root: string) {
						return Promise.resolve(candidate_root === root);
					}

					close() {}
					finalizeRegularFileReplacement() {
						return Promise.resolve();
					}
					readRegularFile() {
						return Promise.resolve(new Uint8Array());
					}
					replaceRegularFile() {
						return Promise.resolve("Replaced");
					}
				},
				getNativeBuildDescriptor: () => ({
					architecture: "x86_64",
					operatingSystem: "windows",
					target: "x86_64-pc-windows-msvc",
					testHooksEnabled: false,
				}),
			}),
			receipt_authentication_key,
		},
	).pipe(Layer.provide(NodeFileSystem.layer));
	const authority = WorkspaceMutationAuthorityLive.pipe(
		Layer.provideMerge(registry),
		Layer.provideMerge(changes),
		Layer.provideMerge(infrastructure),
	);

	return ManagedRuntime.make(authority);
}

function replace_claim(
	overrides: Partial<Parameters<WorkspaceMutationAuthority["Service"]["ClaimReplace"]>[0]> = {},
) {
	return {
		_tag: "replace" as const,
		agent_id: "agent_1",
		change_id: "change_1",
		expected_before: {
			algorithm: "sha256" as const,
			byte_count: 6,
			content_hash: "a".repeat(64),
		},
		intended_after: {
			algorithm: "sha256" as const,
			byte_count: 5,
			content_hash: "b".repeat(64),
		},
		message_id: "replace_1",
		path: "src/example.ts",
		request_fingerprint: "c".repeat(64),
		run_id: "run_1",
		sent_at: now,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
		...overrides,
	};
}

function rollback_claim(message_id = "rollback_1") {
	return {
		_tag: "rollback" as const,
		change_id: "change_1",
		expected_after: replace_claim().intended_after,
		message_id,
		request_fingerprint: "d".repeat(64),
		sent_at: now,
		thread_id: "thread_1",
	};
}

const SeedBaseRun = (root: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: now,
			thread_id: "thread_1",
			title: "Workspace git mutation fence",
			title_source: "initial",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: "run_1",
			agent_id: "agent_1",
			created_at: now,
			display_name: "Coordinator",
			engine_id: "engine_1",
			role: "primary",
			thread_id: "thread_1",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_1",
			created_at: now,
			engine_id: "engine_1",
			run_id: "run_1",
			status: "running",
			thread_id: "thread_1",
			updated_at: now,
			working_directory: root,
		});
	});

const AdmitReplace = (input = replace_claim()) =>
	Effect.flatMap(WorkspaceMutationAuthority, (authority) => authority.ClaimReplace(input));

const AdmitRollback = (input = rollback_claim()) =>
	Effect.flatMap(WorkspaceMutationAuthority, (authority) => authority.ClaimRollback(input));

const CommitReplace = Effect.gen(function* () {
	const repository = yield* WorkspaceChangeRepository;
	const input = replace_claim();
	const patch = new TextEncoder().encode("--- a/src/example.ts\n+++ b/src/example.ts\n");
	const prepared_diff: PreparedWorkspaceChangeDiff = {
		added_line_count: 1,
		after_identity: input.intended_after,
		before_identity: input.expected_before,
		change_id: input.change_id,
		context_lines: 3,
		format: "unified",
		format_version: 1,
		message_id: input.message_id,
		patch,
		patch_identity: {
			algorithm: "sha256",
			byte_count: patch.byteLength,
			content_hash: createHash("sha256").update(patch).digest("hex"),
		},
		path: input.path,
		removed_line_count: 1,
		thread_id: input.thread_id,
		workspace_id: input.workspace_id,
	};

	yield* repository.MarkApplied({
		_tag: "replace",
		message_id: input.message_id,
		result_identity: input.intended_after,
	});

	return yield* repository.CommitRecorded(input.message_id, prepared_diff);
});

const CreateCheckoutClaim = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(WorkspaceGitCheckoutApprovals).values({
		approval_id: "checkout_approval_1",
		approved: true,
		created_at: now,
		decided_at: now,
		decision_message_id: "checkout_decision_1",
		execution_started_at: now,
		expected_session_version: 1,
		request_fingerprint: "e".repeat(64),
		source_branch: "main",
		source_command_id: "checkout_request_1",
		source_head: "f".repeat(40),
		state: "executing",
		target_branch: "release",
		target_head: "0".repeat(40),
		thread_id: "thread_1",
		updated_at: now,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(WorkspaceGitCheckoutClaims).values({
		approval_id: "checkout_approval_1",
		claimed_at: now,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	});
});

const ReadRows = Effect.gen(function* () {
	const database = yield* Database;

	return {
		authorities: yield* database.client.select().from(WorkspaceMutationAuthorities),
		checkout_claims: yield* database.client.select().from(WorkspaceGitCheckoutClaims),
		operations: yield* database.client.select().from(WorkspaceChangeOperations),
	};
});

function DenialReason<A, E, R>(effect: Effect.Effect<A, E, R>) {
	return effect.pipe(
		Effect.matchEffect({
			onFailure: (error) =>
				error instanceof WorkspaceMutationAuthorityDenied
					? Effect.succeed(error.reason)
					: Effect.die("Expected workspace checkout fence denial"),
			onSuccess: () => Effect.die("Expected workspace checkout fence denial"),
		}),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceGitCheckoutClaims mutation fence", () => {
	it("blocks new replace and rollback claims while preserving committed replacement replay", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBaseRun(workspace.root);
					yield* AdmitReplace();
					yield* CommitReplace;
					yield* CreateCheckoutClaim;

					return {
						new_replace: yield* DenialReason(
							AdmitReplace(
								replace_claim({
									change_id: "change_2",
									message_id: "replace_2",
									request_fingerprint: "1".repeat(64),
								}),
							),
						),
						replay: yield* AdmitReplace(),
						new_rollback: yield* DenialReason(
							AdmitRollback(rollback_claim("rollback_2")),
						),
						rows: yield* ReadRows,
					};
				}),
			);

			expect(result.new_replace).toBe("workspace_git_checkout_active");
			expect(result.replay.claim._tag).toBe("duplicate");
			expect(result.new_rollback).toBe("workspace_git_checkout_active");
			expect(result.rows.checkout_claims).toHaveLength(1);
			expect(result.rows.authorities).toHaveLength(1);
			expect(result.rows.operations).toMatchObject([
				{ lifecycle: "committed", message_id: "replace_1" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an exact terminal rollback while a checkout claim remains active", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBaseRun(workspace.root);
					yield* AdmitReplace();
					yield* CommitReplace;

					const repository = yield* WorkspaceChangeRepository;

					yield* repository.ClaimReview({
						_tag: "review",
						change_id: "change_1",
						message_id: "review_1",
						request_fingerprint: "2".repeat(64),
						sent_at: now,
						thread_id: "thread_1",
					});
					yield* repository.CommitReviewed("review_1");

					const admitted = yield* AdmitRollback();

					yield* repository.MarkApplied({ _tag: "rollback", message_id: "rollback_1" });
					yield* repository.CommitRolledBack("rollback_1");
					yield* CreateCheckoutClaim;

					return {
						admitted,
						duplicate: yield* AdmitRollback(),
						rows: yield* ReadRows,
					};
				}),
			);

			expect(result.admitted._tag).toBe("authorized");
			expect(result.duplicate._tag).toBe("duplicate");
			expect(result.rows.checkout_claims).toHaveLength(1);
			expect(
				result.rows.operations.map((operation) => [
					operation.message_id,
					operation.lifecycle,
				]),
			).toEqual([
				["replace_1", "committed"],
				["review_1", "committed"],
				["rollback_1", "committed"],
			]);
		} finally {
			await runtime.dispose();
		}
	});
});
