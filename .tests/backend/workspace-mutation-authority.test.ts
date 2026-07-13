import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, ManagedRuntime, Redacted } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_workspace_bounded_regular_file_store_registry_layer,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreLive } from "../../modules/backend/src/persistence/journal-store";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	OrchestrationCoordinators,
	OrchestrationGroups,
	OrchestrationRuns,
	ThreadErasureClaims,
	Threads,
	WorkspaceChanges,
	WorkspaceChangeOperations,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceChangeRepository,
	WorkspaceChangeRepositoryLive,
} from "../../modules/backend/src/workspace/workspace-change-repository";
import {
	WorkspaceMutationAuthority,
	WorkspaceMutationAuthorityLive,
} from "../../modules/backend/src/workspace/workspace-mutation-authority";
import type { PreparedWorkspaceChangeDiff } from "../../modules/backend/src/workspace/workspace-change-diff-service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-12T08:00:00.000Z";
const receipt_authentication_key = Redacted.make(new Uint8Array(32).fill(4));

type RaceProbe = {
	readonly authorize_attempts: { value: number };
	readonly continue_proof: Deferred.Deferred<void>;
	readonly proof_started: Deferred.Deferred<void>;
};

async function make_workspace() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-mutation-authority-"));

	temporary_directories.push(directory);

	return { database_path: join(directory, "artisan.db"), root: join(directory, "workspace") };
}

function make_metadata_layer(instance_id = "authority_test") {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_runtime(
	database_path: string,
	root: string,
	options: { readonly instance_id?: string; readonly race_probe?: RaceProbe } = {},
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(options.instance_id),
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
	const race_probe = options.race_probe;
	const gated_registry = race_probe
		? Layer.effect(
				WorkspaceBoundedRegularFileStoreRegistry,
				Effect.gen(function* () {
					const live = yield* WorkspaceBoundedRegularFileStoreRegistry;
					let paused = false;

					return {
						...live,
						Authorize: (input: Parameters<typeof live.Authorize>[0]) => {
							race_probe.authorize_attempts.value += 1;

							if (paused) {
								return live.Authorize(input);
							}

							paused = true;

							return Deferred.succeed(race_probe.proof_started, undefined).pipe(
								Effect.andThen(Deferred.await(race_probe.continue_proof)),
								Effect.andThen(live.Authorize(input)),
							);
						},
					};
				}),
			).pipe(Layer.provide(registry))
		: registry;
	const authority = WorkspaceMutationAuthorityLive.pipe(
		Layer.provideMerge(gated_registry),
		Layer.provideMerge(changes),
		Layer.provideMerge(infrastructure),
	);

	return ManagedRuntime.make(authority);
}

function claim(
	overrides: Partial<Parameters<WorkspaceMutationAuthority["Service"]["ClaimReplace"]>[0]> = {},
) {
	return {
		_tag: "replace" as const,
		agent_id: "agent_1",
		change_id: "change_1",
		expected_before: {
			algorithm: "sha256" as const,
			byte_count: 2,
			content_hash: "a".repeat(64),
		},
		intended_after: {
			algorithm: "sha256" as const,
			byte_count: 3,
			content_hash: "b".repeat(64),
		},
		message_id: "message_1",
		path: "src/example.ts",
		request_fingerprint: "c".repeat(64),
		run_id: "run_1",
		sent_at: now,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
		...overrides,
	};
}

function rollback_claim(
	overrides: Partial<Parameters<WorkspaceMutationAuthority["Service"]["ClaimRollback"]>[0]> = {},
) {
	return {
		_tag: "rollback" as const,
		change_id: "change_1",
		expected_after: {
			algorithm: "sha256" as const,
			byte_count: 3,
			content_hash: "b".repeat(64),
		},
		message_id: "message_rollback_1",
		request_fingerprint: "d".repeat(64),
		sent_at: now,
		thread_id: "thread_1",
		...overrides,
	};
}

function expected_rollback_source() {
	return {
		after_identity: claim().intended_after,
		before_identity: claim().expected_before,
		path: "src/example.ts",
		workspace_id: "workspace_1",
	};
}

function SeedBase(
	root: string,
	options: {
		readonly run_status?: string;
		readonly active_run_id?: string;
		readonly agent_id?: string;
		readonly run_agent_id?: string;
		readonly run_thread_id?: string;
	} = {},
) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const coordinator_agent_id = options.agent_id ?? "agent_1";

		yield* database.client.insert(Threads).values({
			created_at: now,
			thread_id: "thread_1",
			title: "thread_1",
			title_source: "initial",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: options.active_run_id ?? "run_1",
			agent_id: coordinator_agent_id,
			created_at: now,
			display_name: "Coordinator",
			engine_id: "engine_1",
			role: "primary",
			thread_id: "thread_1",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: options.run_agent_id ?? "agent_1",
			created_at: now,
			engine_id: "engine_1",
			run_id: "run_1",
			status: options.run_status ?? "running",
			thread_id: options.run_thread_id ?? "thread_1",
			updated_at: now,
			working_directory: root,
		});
	});
}

function SeedGraph(
	root: string,
	options: {
		readonly skip_thread?: boolean;
		readonly assignment_active_run_id?: string;
		readonly assignment_state?: string;
		readonly dispatch_status?: string;
		readonly group_state?: string;
		readonly permission_policy?: string;
		readonly scope?: string;
		readonly workspace?: string;
		readonly run_agent_id?: string;
		readonly run_state?: string;
		readonly assignment_agent_id?: string;
		readonly group_id?: string;
		readonly group_thread_id?: string;
	} = {},
) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const group_id = options.group_id ?? "group_1";
		const agent_id = options.run_agent_id ?? "agent_1";

		if (!options.skip_thread) {
			yield* database.client.insert(Threads).values({
				created_at: now,
				thread_id: "thread_1",
				title: "thread_1",
				title_source: "initial",
				updated_at: now,
			});
		}
		yield* database.client.insert(OrchestrationGroups).values({
			coordinator_agent_id: "coordinator_1",
			created_at: now,
			group_id,
			journal_sequence: 0,
			max_concurrency: 1,
			state: options.group_state ?? "running",
			thread_id: options.group_thread_id ?? "thread_1",
			updated_at: now,
			version: 1,
		});
		yield* database.client.insert(AgentInstances).values(
			["agent_1", agent_id]
				.toReversed()
				.filter((value, index, values) => values.indexOf(value) === index)
				.map((agent_instance_id) => ({
					agent_id: agent_instance_id,
					created_at: now,
					display_name: `Worker ${agent_instance_id}`,
					group_id,
					role: "worker",
					updated_at: now,
				})),
		);
		yield* database.client.insert(Assignments).values({
			active_run_id: options.assignment_active_run_id ?? "run_1",
			agent_id: options.assignment_agent_id ?? "agent_1",
			assignment_id: "assignment_1",
			created_at: now,
			current_attempt: 1,
			engine_id: "engine_1",
			expected_result: "result",
			group_id,
			instructions: "instructions",
			max_attempts: 1,
			parent_node_id: "node_1",
			permission_policy_json:
				options.permission_policy ??
				JSON.stringify({
					approval: "on_request",
					network_access: false,
					write_access: true,
				}),
			profile: "default",
			role: "worker",
			scope_json:
				options.scope ??
				JSON.stringify({ kind: "files", value: "src", write_access: true }),
			state: options.assignment_state ?? "running",
			summary_contract: "summary",
			updated_at: now,
			workspace_json:
				options.workspace ??
				JSON.stringify({
					isolation: "shared",
					working_directory: root,
					workspace_id: "workspace_1",
				}),
		});
		yield* database.client.insert(AgentRuns).values({
			agent_id,
			assignment_id: "assignment_1",
			attempt: 1,
			created_at: now,
			dispatch_status: options.dispatch_status ?? "active",
			engine_id: "engine_1",
			group_id,
			last_observation_sequence: 0,
			profile: "default",
			run_id: "run_1",
			state: options.run_state ?? "running",
			updated_at: now,
		});
	});
}

function Admit(input = claim()) {
	return Effect.service(WorkspaceMutationAuthority).pipe(
		Effect.flatMap((authority) => authority.ClaimReplace(input)),
	);
}

function AdmitRollback(input = rollback_claim()) {
	return Effect.service(WorkspaceMutationAuthority).pipe(
		Effect.flatMap((authority) => authority.ClaimRollback(input)),
	);
}

function prepared_diff(
	overrides: Partial<PreparedWorkspaceChangeDiff> = {},
): PreparedWorkspaceChangeDiff {
	const path = overrides.path ?? "src/example.ts";
	const patch = new TextEncoder().encode(
		`--- a/${path}\n+++ b/${path}\n@@ -1,1 +1,1 @@\n-old\n+new\n`,
	);

	return {
		added_line_count: 1,
		after_identity: claim().intended_after,
		before_identity: claim().expected_before,
		change_id: "change_1",
		context_lines: 3,
		format: "unified",
		format_version: 1,
		message_id: "message_1",
		patch,
		patch_identity: {
			algorithm: "sha256",
			byte_count: patch.byteLength,
			content_hash: createHash("sha256").update(patch).digest("hex"),
		},
		path,
		removed_line_count: 1,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
		...overrides,
	};
}

function CommitReplace() {
	return Effect.gen(function* () {
		const repository = yield* WorkspaceChangeRepository;

		yield* repository.MarkApplied({
			_tag: "replace",
			message_id: "message_1",
			result_identity: claim().intended_after,
		});

		return yield* repository.CommitRecorded("message_1", prepared_diff());
	});
}

function ReadAtomicRows() {
	return Effect.gen(function* () {
		const database = yield* Database;

		return {
			authorities: yield* database.client.select().from(WorkspaceMutationAuthorities),
			operations: yield* database.client.select().from(WorkspaceChangeOperations),
		};
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceMutationAuthority", () => {
	it("returns only the bounded store capability for an inferred base-run claim", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);

					return {
						accepted: yield* Admit(),
						rows: yield* ReadAtomicRows(),
					};
				}),
			);

			expect(result.accepted.claim._tag).toBe("claimed");
			expect(Object.keys(result.accepted).toSorted()).toEqual([
				"authority",
				"claim",
				"store",
			]);
			expect(result.rows.authorities).toHaveLength(1);
			expect(result.rows.operations).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("accepts base and graph claims atomically, then replays a terminal base run exactly", async () => {
		const workspace = await make_workspace();
		await mkdir(workspace.root);
		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					const base = yield* Admit();
					const database = yield* Database;
					yield* database.client.update(OrchestrationRuns).set({ status: "complete" });
					const retry = yield* Admit();
					return { base, retry, rows: yield* ReadAtomicRows() };
				}),
			);

			expect(result.base).toMatchObject({
				authority: { _tag: "base_run" },
				claim: { _tag: "claimed" },
			});
			expect(result.retry).toMatchObject({
				authority: { _tag: "base_run" },
				claim: { _tag: "incomplete_retry" },
			});
			expect(result.rows.authorities).toHaveLength(1);
			expect(result.rows.operations).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}

		const graph_workspace = await make_workspace();
		await mkdir(graph_workspace.root);
		const graph_runtime = make_runtime(graph_workspace.database_path, graph_workspace.root);

		try {
			const result = await graph_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedGraph(graph_workspace.root);
					return {
						accepted: yield* Admit(claim()),
						rows: yield* ReadAtomicRows(),
					};
				}),
			);
			await graph_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client
						.update(AgentRuns)
						.set({ dispatch_status: "terminal", state: "complete" });
					yield* database.client.update(Assignments).set({ state: "complete" });
				}),
			);
			const retry = await graph_runtime.runPromise(Admit(claim()));

			expect(result.accepted).toMatchObject({
				authority: { _tag: "graph_run", scope: "files" },
				claim: { _tag: "claimed" },
			});
			expect(result.rows.authorities).toHaveLength(1);
			expect(result.rows.operations).toHaveLength(1);
			expect(retry.claim._tag).toBe("incomplete_retry");
		} finally {
			await graph_runtime.dispose();
		}
	});

	it("fails closed when one run ID exists in both authority domains", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					yield* SeedGraph(workspace.root, { skip_thread: true });

					return {
						failure: yield* Effect.exit(Admit()),
						rows: yield* ReadAtomicRows(),
					};
				}),
			);

			expect(JSON.stringify(result.failure)).toContain("invalid_persisted_state");
			expect(result.rows).toEqual({ authorities: [], operations: [] });
		} finally {
			await runtime.dispose();
		}
	});

	it("returns a typed conflict without a filesystem for a rejected exact retry", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					yield* Admit();

					const repository = yield* WorkspaceChangeRepository;

					yield* repository.RejectChanged("message_1");

					return yield* Effect.exit(Admit());
				}),
			);

			expect(JSON.stringify(result)).toContain("WorkspaceMutationAuthorityConflict");
			expect(JSON.stringify(result)).toContain("operation_rejected");
			expect(JSON.stringify(result)).not.toContain("filesystem");
			expect(JSON.stringify(result)).not.toContain(workspace.root);
			expect(JSON.stringify(result)).not.toContain("src/example.ts");
		} finally {
			await runtime.dispose();
		}
	});

	it("admits a rollback from one committed source authority after its run terminates", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					yield* Admit();
					yield* CommitReplace();
					const database = yield* Database;

					yield* database.client.update(OrchestrationRuns).set({ status: "complete" });

					const admitted = yield* AdmitRollback();
					const repository = yield* WorkspaceChangeRepository;

					yield* repository.MarkApplied({
						_tag: "rollback",
						message_id: "message_rollback_1",
					});
					yield* repository.CommitRolledBack("message_rollback_1");

					return {
						admitted,
						duplicate: yield* AdmitRollback(),
						rows: yield* ReadAtomicRows(),
					};
				}),
			);

			expect(result.admitted).toMatchObject({
				_tag: "authorized",
				authority: { _tag: "base_run", run_id: "run_1" },
				claim: { _tag: "claimed" },
				source: expected_rollback_source(),
			});
			expect(result.admitted.source).toEqual(expected_rollback_source());
			expect(result.duplicate).toMatchObject({
				_tag: "duplicate",
				authority: { _tag: "base_run", run_id: "run_1" },
				claim: { _tag: "duplicate" },
				source: expected_rollback_source(),
			});
			expect(result.duplicate.source).toEqual(expected_rollback_source());
			expect(Object.keys(result.duplicate).toSorted()).toEqual([
				"_tag",
				"authority",
				"claim",
				"source",
			]);
			expect("store" in result.duplicate).toBe(false);
			expect(result.rows.authorities).toHaveLength(1);
			expect(result.rows.operations).toHaveLength(2);
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves rollback identity conflicts and transition validation", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					yield* Admit();
					yield* CommitReplace();
					const database = yield* Database;

					yield* database.client.update(OrchestrationRuns).set({ status: "complete" });

					return {
						wrong_expected: yield* Effect.exit(
							AdmitRollback(
								rollback_claim({
									expected_after: {
										algorithm: "sha256",
										byte_count: 3,
										content_hash: "e".repeat(64),
									},
									message_id: "message_rollback_expected",
								}),
							),
						),
						wrong_thread: yield* Effect.exit(
							AdmitRollback(rollback_claim({ thread_id: "thread_other" })),
						),
						accepted: yield* AdmitRollback(),
						command_reuse: yield* Effect.exit(
							AdmitRollback(rollback_claim({ request_fingerprint: "f".repeat(64) })),
						),
					};
				}),
			);

			expect(JSON.stringify(result.wrong_expected)).toContain("thread_unavailable");
			expect(JSON.stringify(result.wrong_thread)).toContain("authority_conflict");
			expect(result.accepted).toMatchObject({
				_tag: "authorized",
				claim: { _tag: "claimed" },
			});
			expect(JSON.stringify(result.command_reuse)).toContain("operation_conflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed for missing or forged rollback source proof", async () => {
		const corruptions = [
			"missing_authority",
			"forged_authority",
			"forged_source_command",
			"forged_path",
			"malformed_operation_before",
			"mismatched_projection_before",
			"malformed_projection_after",
			"mismatched_operation_after",
			"malformed_operation_raw_origin",
			"malformed_projection_raw_origin",
			"mismatched_raw_origin",
			"null_operation_before",
			"null_operation_after",
			"null_operation_path",
			"non_replace_source_action",
			"jointly_forged_identities",
			"jointly_forged_raw_origin",
		] as const;

		for (const corruption of corruptions) {
			const workspace = await make_workspace();

			await mkdir(workspace.root);

			const runtime = make_runtime(workspace.database_path, workspace.root);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedBase(workspace.root);
						yield* Admit();
						yield* CommitReplace();
						const database = yield* Database;

						if (corruption === "missing_authority") {
							yield* database.client.delete(WorkspaceMutationAuthorities);
						} else if (corruption === "forged_authority") {
							yield* database.client
								.update(WorkspaceMutationAuthorities)
								.set({ agent_id: "agent_other" });
						} else if (corruption === "forged_source_command") {
							yield* database.client
								.update(WorkspaceChanges)
								.set({ source_command_id: "message_forged" });
						} else if (corruption === "forged_path") {
							yield* database.client
								.update(WorkspaceChanges)
								.set({ path: "outside/forged.ts" });
						} else if (corruption === "malformed_operation_before") {
							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ expected_identity_json: "{malformed" });
						} else if (corruption === "mismatched_projection_before") {
							yield* database.client.update(WorkspaceChanges).set({
								before_identity_json: JSON.stringify({
									algorithm: "sha256",
									byte_count: 2,
									content_hash: "e".repeat(64),
								}),
							});
						} else if (corruption === "malformed_projection_after") {
							yield* database.client
								.update(WorkspaceChanges)
								.set({ after_identity_json: "{malformed" });
						} else if (corruption === "mismatched_operation_after") {
							yield* database.client.update(WorkspaceChangeOperations).set({
								result_identity_json: JSON.stringify({
									algorithm: "sha256",
									byte_count: 3,
									content_hash: "f".repeat(64),
								}),
							});
						} else if (corruption === "malformed_operation_raw_origin") {
							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ raw_origin_json: "{malformed" });
						} else if (corruption === "malformed_projection_raw_origin") {
							yield* database.client
								.update(WorkspaceChanges)
								.set({ raw_origin_json: "{malformed" });
						} else if (corruption === "mismatched_raw_origin") {
							yield* database.client.update(WorkspaceChangeOperations).set({
								raw_origin_json: JSON.stringify({
									provider: "provider_one",
									reference: "origin_one",
								}),
							});
							yield* database.client.update(WorkspaceChanges).set({
								raw_origin_json: JSON.stringify({
									provider: "provider_two",
									reference: "origin_one",
								}),
							});
						} else if (corruption === "null_operation_before") {
							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ expected_identity_json: null });
						} else if (corruption === "null_operation_after") {
							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ result_identity_json: null });
						} else if (corruption === "null_operation_path") {
							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ path: null });
						} else if (corruption === "non_replace_source_action") {
							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ action: "review" });
						} else if (corruption === "jointly_forged_identities") {
							const alternate_before = JSON.stringify({
								algorithm: "sha256",
								byte_count: 4,
								content_hash: "1".repeat(64),
							});
							const alternate_after = JSON.stringify({
								algorithm: "sha256",
								byte_count: 5,
								content_hash: "2".repeat(64),
							});

							yield* database.client.update(WorkspaceChangeOperations).set({
								expected_identity_json: alternate_before,
								result_identity_json: alternate_after,
							});
							yield* database.client.update(WorkspaceChanges).set({
								after_identity_json: alternate_after,
								before_identity_json: alternate_before,
							});
						} else {
							const alternate_origin = JSON.stringify({
								provider: "provider_alternate",
								reference: "origin_alternate",
							});

							yield* database.client
								.update(WorkspaceChangeOperations)
								.set({ raw_origin_json: alternate_origin });
							yield* database.client
								.update(WorkspaceChanges)
								.set({ raw_origin_json: alternate_origin });
						}

						return yield* Effect.exit(AdmitRollback());
					}),
				);

				expect(JSON.stringify(result)).toContain("invalid_persisted_state");
				expect(JSON.stringify(result)).not.toContain(workspace.root);
			} finally {
				await runtime.dispose();
			}
		}
	});

	it("keeps terminal graph rollback inside its pinned files scope", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedGraph(workspace.root);
					yield* Admit(
						claim({
							raw_origin: {
								provider: "provider_one",
								reference: "origin_one",
							},
						}),
					);
					yield* CommitReplace();
					const database = yield* Database;

					yield* database.client
						.update(AgentRuns)
						.set({ dispatch_status: "terminal", state: "complete" });
					yield* database.client.update(Assignments).set({ state: "complete" });

					const admitted = yield* AdmitRollback();

					yield* database.client
						.update(WorkspaceChanges)
						.set({ path: "src/alternate.ts" });

					const forged_projection = yield* Effect.exit(AdmitRollback());

					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ path: "src/alternate.ts" });

					const jointly_forged_in_scope = yield* Effect.exit(AdmitRollback());

					yield* database.client
						.update(WorkspaceChanges)
						.set({ path: "outside/forged.ts" });
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ path: "outside/forged.ts" });

					return {
						admitted,
						forged_projection,
						jointly_forged_in_scope,
						jointly_forged_outside: yield* Effect.exit(AdmitRollback()),
					};
				}),
			);

			expect(result.admitted).toMatchObject({
				_tag: "authorized",
				authority: { _tag: "graph_run", scope: "files" },
				claim: { _tag: "claimed" },
				source: expected_rollback_source(),
			});
			expect(JSON.stringify(result.forged_projection)).toContain("invalid_persisted_state");
			expect(JSON.stringify(result.jointly_forged_in_scope)).toContain(
				"invalid_persisted_state",
			);
			expect(JSON.stringify(result.jointly_forged_outside)).toContain("path_outside_scope");
			expect(JSON.stringify(result.jointly_forged_outside)).not.toContain(workspace.root);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns an exact rejected rollback with its source and no mutation store", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					yield* Admit();
					yield* CommitReplace();
					const repository = yield* WorkspaceChangeRepository;

					yield* repository.ClaimRollback(rollback_claim());
					yield* repository.RejectChanged("message_rollback_1");

					return yield* AdmitRollback();
				}),
			);

			expect(result).toMatchObject({
				_tag: "rejected",
				authority: { _tag: "base_run", run_id: "run_1" },
				claim: { _tag: "rejected" },
				source: expected_rollback_source(),
			});
			expect(result.source).toEqual(expected_rollback_source());
			expect(Object.keys(result).toSorted()).toEqual([
				"_tag",
				"authority",
				"claim",
				"source",
			]);
			expect("store" in result).toBe(false);
			expect(JSON.stringify(result)).not.toContain(workspace.root);
		} finally {
			await runtime.dispose();
		}
	});

	it("does not admit rollback once thread erasure has fenced its source authority", async () => {
		const workspace = await make_workspace();

		await mkdir(workspace.root);

		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					yield* Admit();
					yield* CommitReplace();
					const database = yield* Database;

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: now,
						thread_id: "thread_1",
					});

					return {
						failure: yield* Effect.exit(AdmitRollback()),
						rows: yield* ReadAtomicRows(),
					};
				}),
			);

			expect(JSON.stringify(result.failure)).toContain("thread_unavailable");
			expect(result.rows.authorities).toHaveLength(1);
			expect(result.rows.operations).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("denies base coordinator, run, thread, agent, and active-run mismatches without partial rows", async () => {
		for (const options of [
			{ active_run_id: "other_run" },
			{ run_status: "complete" },
			{ agent_id: "other_agent" },
			{ run_agent_id: "other_agent" },
			{ run_thread_id: "other_thread" },
		]) {
			const workspace = await make_workspace();
			await mkdir(workspace.root);
			const runtime = make_runtime(workspace.database_path, workspace.root);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedBase(workspace.root, options);
						return {
							failure: yield* Effect.exit(Admit()),
							rows: yield* ReadAtomicRows(),
						};
					}),
				);

				expect(JSON.stringify(result.failure)).toContain(
					"WorkspaceMutationAuthorityDenied",
				);
				expect(result.rows).toEqual({ authorities: [], operations: [] });
			} finally {
				await runtime.dispose();
			}
		}
	});

	it("enforces graph liveness, identity, workspace, policy, scope, file-prefix, and canonical repo roots", async () => {
		const scenarios = [
			{ options: { run_state: "complete" }, reason: "run_not_active" },
			{ options: { assignment_active_run_id: "other_run" }, reason: "run_not_active" },
			{ options: { assignment_state: "complete" }, reason: "run_not_active" },
			{ options: { dispatch_status: "terminal" }, reason: "run_not_active" },
			{ options: { group_state: "complete" }, reason: "run_not_active" },
			{ options: { group_thread_id: "other_thread" }, reason: "identity_mismatch" },
			{ options: { run_agent_id: "other_agent" }, reason: "identity_mismatch" },
			{ options: { assignment_agent_id: "other_agent" }, reason: "identity_mismatch" },
			{
				options: {
					workspace: JSON.stringify({
						isolation: "shared",
						working_directory: "C:/elsewhere",
						workspace_id: "workspace_2",
					}),
				},
				reason: "workspace_mismatch",
			},
			{
				options: {
					workspace: JSON.stringify({
						isolation: "shared",
						working_directory: tmpdir(),
						workspace_id: "workspace_1",
					}),
				},
				reason: "workspace_unavailable",
			},
			{
				options: {
					permission_policy: JSON.stringify({
						approval: "never",
						network_access: false,
						write_access: false,
					}),
				},
				reason: "write_not_allowed",
			},
			{
				options: {
					scope: JSON.stringify({ kind: "terminal", value: "src", write_access: true }),
				},
				reason: "unsupported_scope",
			},
			{
				options: {
					scope: JSON.stringify({ kind: "files", value: "src", write_access: false }),
				},
				reason: "write_not_allowed",
			},
		];

		for (const scenario of scenarios) {
			const workspace = await make_workspace();
			await mkdir(workspace.root);
			const runtime = make_runtime(workspace.database_path, workspace.root);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedGraph(workspace.root, scenario.options);
						return {
							failure: yield* Effect.exit(Admit(claim())),
							rows: yield* ReadAtomicRows(),
						};
					}),
				);

				expect(JSON.stringify(result.failure)).toContain(scenario.reason);
				expect(JSON.stringify(result.failure)).not.toContain(workspace.root);
				expect(result.rows).toEqual({ authorities: [], operations: [] });
			} finally {
				await runtime.dispose();
			}
		}

		const workspace = await make_workspace();
		await mkdir(workspace.root);
		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedGraph(workspace.root);
					const exact = yield* Admit(claim({ path: "src" }));
					const prefix = yield* Admit(
						claim({
							change_id: "change_prefix",
							message_id: "message_prefix",
							path: "src/child.ts",
						}),
					);
					const outside = yield* Effect.exit(
						Admit(
							claim({
								change_id: "change_outside",
								message_id: "message_outside",
								path: "src-other/file.ts",
							}),
						),
					);
					const database = yield* Database;

					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ path: "outside/file.ts" });

					const tampered = yield* Effect.exit(Admit(claim({ path: "outside/file.ts" })));

					return { exact, outside, prefix, rows: yield* ReadAtomicRows(), tampered };
				}),
			);

			expect(result.exact.claim._tag).toBe("claimed");
			expect(result.prefix.claim._tag).toBe("claimed");
			expect(JSON.stringify(result.outside)).toContain("path_outside_scope");
			expect(JSON.stringify(result.tampered)).toContain("path_outside_scope");
			expect(result.rows.authorities).toHaveLength(2);
		} finally {
			await runtime.dispose();
		}

		const repo_workspace = await make_workspace();
		await mkdir(repo_workspace.root);
		await mkdir(join(repo_workspace.root, "subdir"));
		const repo_runtime = make_runtime(repo_workspace.database_path, repo_workspace.root);

		try {
			const result = await repo_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedGraph(repo_workspace.root, {
						scope: JSON.stringify({
							kind: "repo",
							value: repo_workspace.root,
							write_access: true,
						}),
					});
					const accepted = yield* Admit(claim());
					const database = yield* Database;
					yield* database.client.update(Assignments).set({
						scope_json: JSON.stringify({
							kind: "repo",
							value: join(repo_workspace.root, "subdir"),
							write_access: true,
						}),
					});
					const denied = yield* Effect.exit(
						Admit(
							claim({
								change_id: "change_bad_root",
								message_id: "message_bad_root",
							}),
						),
					);

					return { accepted, denied };
				}),
			);

			expect(result.accepted.claim._tag).toBe("claimed");
			expect(JSON.stringify(result.denied)).toContain("workspace_unavailable");
			expect(JSON.stringify(result.denied)).not.toContain(repo_workspace.root);
		} finally {
			await repo_runtime.dispose();
		}
	});

	it("fails closed for malformed graph persistence and refuses unpinned or changed immutable authority", async () => {
		for (const field of ["scope_json", "workspace_json", "permission_policy_json"] as const) {
			const workspace = await make_workspace();
			await mkdir(workspace.root);
			const runtime = make_runtime(workspace.database_path, workspace.root);

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						yield* SeedGraph(workspace.root);
						const database = yield* Database;
						yield* database.client.update(Assignments).set({ [field]: "{not-json" });
						return {
							failure: yield* Effect.exit(Admit(claim())),
							rows: yield* ReadAtomicRows(),
						};
					}),
				);

				expect(JSON.stringify(result.failure)).toContain("invalid_persisted_state");
				expect(result.rows).toEqual({ authorities: [], operations: [] });
			} finally {
				await runtime.dispose();
			}
		}

		const workspace = await make_workspace();
		await mkdir(workspace.root);
		const runtime = make_runtime(workspace.database_path, workspace.root);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(workspace.root);
					const repository = yield* WorkspaceChangeRepository;
					const { raw_origin: _raw_origin, ...generic_claim } = claim();

					yield* repository.ClaimReplace(generic_claim);
					const unpinned = yield* Effect.exit(Admit());
					const database = yield* Database;
					yield* database.client.delete(WorkspaceChangeOperations);
					const accepted = yield* Admit();
					const changed = yield* Effect.exit(Admit(claim({ agent_id: "other_agent" })));
					const changed_authority = yield* Effect.exit(
						Admit({ ...claim(), authority: "graph_run" } as never),
					);
					const changed_intent = yield* Effect.exit(
						Admit(claim({ request_fingerprint: "d".repeat(64) })),
					);

					yield* database.client
						.update(WorkspaceMutationAuthorities)
						.set({ working_directory: "" });

					const malformed_authority = yield* Effect.exit(Admit());

					return {
						accepted,
						changed,
						changed_authority,
						changed_intent,
						malformed_authority,
						unpinned,
					};
				}),
			);

			expect(JSON.stringify(result.unpinned)).toContain("unpinned_operation");
			expect(JSON.stringify(result.changed)).toContain("authority_conflict");
			expect(JSON.stringify(result.changed_authority)).toContain(
				"WorkspaceMutationAuthorityInvalid",
			);
			expect(JSON.stringify(result.changed_intent)).toContain("operation_conflict");
			expect(JSON.stringify(result.malformed_authority)).toContain("invalid_persisted_state");
			expect(JSON.stringify(result.unpinned)).not.toContain("src/example.ts");
			expect(JSON.stringify(result.changed)).not.toContain("src/example.ts");
			expect(JSON.stringify(result.changed_authority)).not.toContain("src/example.ts");
			expect(JSON.stringify(result.changed_intent)).not.toContain("src/example.ts");
			expect(JSON.stringify(result.malformed_authority)).not.toContain("src/example.ts");
		} finally {
			await runtime.dispose();
		}
	});

	it("converges exact concurrent claims and retries a stale proof as a complete denial", async () => {
		const workspace = await make_workspace();
		await mkdir(workspace.root);
		const first_runtime = make_runtime(workspace.database_path, workspace.root, {
			instance_id: "authority_one",
		});
		const second_runtime = make_runtime(workspace.database_path, workspace.root, {
			instance_id: "authority_two",
		});

		try {
			await first_runtime.runPromise(SeedBase(workspace.root));
			const results = await Promise.all([
				first_runtime.runPromise(Admit()),
				second_runtime.runPromise(Admit()),
			]);
			const rows = await first_runtime.runPromise(ReadAtomicRows());

			expect(results.map((result) => result.claim._tag).toSorted()).toEqual([
				"claimed",
				"incomplete_retry",
			]);
			expect(rows.authorities).toHaveLength(1);
			expect(rows.operations).toHaveLength(1);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}

		const race_workspace = await make_workspace();
		await mkdir(race_workspace.root);
		const race_probe: RaceProbe = {
			authorize_attempts: { value: 0 },
			continue_proof: await Effect.runPromise(Deferred.make<void>()),
			proof_started: await Effect.runPromise(Deferred.make<void>()),
		};
		const racing_runtime = make_runtime(race_workspace.database_path, race_workspace.root, {
			race_probe,
		});
		const mutating_runtime = make_runtime(race_workspace.database_path, race_workspace.root);

		try {
			await racing_runtime.runPromise(SeedBase(race_workspace.root));
			const pending = racing_runtime.runPromise(Effect.exit(Admit()));

			await Effect.runPromise(Deferred.await(race_probe.proof_started));
			await mutating_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.update(OrchestrationRuns).set({ status: "complete" }),
					),
				),
			);
			await Effect.runPromise(Deferred.succeed(race_probe.continue_proof, undefined));
			const result = await pending;
			const rows = await racing_runtime.runPromise(ReadAtomicRows());

			expect(JSON.stringify(result)).toContain("run_not_active");
			expect(rows).toEqual({ authorities: [], operations: [] });
			expect(JSON.stringify(result)).not.toContain(race_workspace.root);
		} finally {
			await Effect.runPromise(Deferred.succeed(race_probe.continue_proof, undefined));
			await Promise.all([racing_runtime.dispose(), mutating_runtime.dispose()]);
		}

		const erasure_workspace = await make_workspace();

		await mkdir(erasure_workspace.root);

		const erasure_probe: RaceProbe = {
			authorize_attempts: { value: 0 },
			continue_proof: await Effect.runPromise(Deferred.make<void>()),
			proof_started: await Effect.runPromise(Deferred.make<void>()),
		};
		const erasure_racing_runtime = make_runtime(
			erasure_workspace.database_path,
			erasure_workspace.root,
			{ race_probe: erasure_probe },
		);
		const erasure_claiming_runtime = make_runtime(
			erasure_workspace.database_path,
			erasure_workspace.root,
		);

		try {
			await erasure_racing_runtime.runPromise(SeedBase(erasure_workspace.root));

			const pending = erasure_racing_runtime.runPromise(Effect.exit(Admit()));

			await Effect.runPromise(Deferred.await(erasure_probe.proof_started));
			await erasure_claiming_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.insert(ThreadErasureClaims).values({
							claimed_at: now,
							thread_id: "thread_1",
						}),
					),
				),
			);
			await Effect.runPromise(Deferred.succeed(erasure_probe.continue_proof, undefined));

			const result = await pending;
			const rows = await erasure_racing_runtime.runPromise(ReadAtomicRows());

			expect(JSON.stringify(result)).toContain("thread_unavailable");
			expect(rows).toEqual({ authorities: [], operations: [] });
			expect(erasure_probe.authorize_attempts.value).toBeGreaterThanOrEqual(2);
			expect(JSON.stringify(result)).not.toContain(erasure_workspace.root);
		} finally {
			await Effect.runPromise(Deferred.succeed(erasure_probe.continue_proof, undefined));
			await Promise.all([
				erasure_racing_runtime.dispose(),
				erasure_claiming_runtime.dispose(),
			]);
		}

		const exact_workspace = await make_workspace();

		await mkdir(exact_workspace.root);

		const setup_runtime = make_runtime(exact_workspace.database_path, exact_workspace.root);

		try {
			await setup_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedBase(exact_workspace.root);
					yield* Admit();
				}),
			);
		} finally {
			await setup_runtime.dispose();
		}

		const exact_probe: RaceProbe = {
			authorize_attempts: { value: 0 },
			continue_proof: await Effect.runPromise(Deferred.make<void>()),
			proof_started: await Effect.runPromise(Deferred.make<void>()),
		};
		const exact_racing_runtime = make_runtime(
			exact_workspace.database_path,
			exact_workspace.root,
			{ race_probe: exact_probe },
		);
		const exact_erasure_runtime = make_runtime(
			exact_workspace.database_path,
			exact_workspace.root,
		);

		try {
			const pending = exact_racing_runtime.runPromise(Effect.exit(Admit()));

			await Effect.runPromise(Deferred.await(exact_probe.proof_started));
			await exact_erasure_runtime.runPromise(
				Effect.service(Database).pipe(
					Effect.flatMap((database) =>
						database.client.insert(ThreadErasureClaims).values({
							claimed_at: now,
							thread_id: "thread_1",
						}),
					),
				),
			);
			await Effect.runPromise(Deferred.succeed(exact_probe.continue_proof, undefined));

			const result = await pending;
			const rows = await exact_racing_runtime.runPromise(ReadAtomicRows());

			expect(JSON.stringify(result)).toContain("thread_unavailable");
			expect(rows.authorities).toHaveLength(1);
			expect(rows.operations).toHaveLength(1);
			expect(exact_probe.authorize_attempts.value).toBeGreaterThanOrEqual(2);
			expect(JSON.stringify(result)).not.toContain(exact_workspace.root);
		} finally {
			await Effect.runPromise(Deferred.succeed(exact_probe.continue_proof, undefined));
			await Promise.all([exact_racing_runtime.dispose(), exact_erasure_runtime.dispose()]);
		}
	}, 10_000);
});
