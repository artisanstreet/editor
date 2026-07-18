import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitRepository,
	GitRepositoryConflict,
	GitRepositoryLive,
} from "../../modules/backend/src/git/git-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	GitMutationOperations,
	JournalEvents,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const observed_at = "2026-07-18T12:00:00.000Z";
const snapshot_a = "a".repeat(64);
const snapshot_b = "b".repeat(64);

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-git-repository-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(instance: string, time_offset_seconds = 0) {
	let next_id = 0;
	let next_time = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: instance,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance}_${++next_id}`),
		Now: Effect.sync(() =>
			new Date(
				Date.parse(observed_at) + (time_offset_seconds + ++next_time) * 1_000,
			).toISOString(),
		),
	});
}

function make_runtime(database_path: string, instance: string, time_offset_seconds = 0) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(instance, time_offset_seconds),
		JournalNotifierLive,
	);

	return ManagedRuntime.make(
		GitRepositoryLive.pipe(
			Layer.provideMerge(NodeCrypto.layer),
			Layer.provideMerge(infrastructure),
		),
	);
}

function seed_thread(database: Database["Service"], thread_id = "thread_1") {
	return database.client.insert(Threads).values({
		created_at: observed_at,
		thread_id,
		title: thread_id,
		updated_at: observed_at,
	});
}

function workspace_record(snapshot_id = snapshot_a, workspace_id = "workspace_1") {
	return {
		causation_id: `cause_${workspace_id}_${snapshot_id.slice(0, 1)}`,
		correlation_id: `refresh_${workspace_id}_${snapshot_id.slice(0, 1)}`,
		cause: "refresh" as const,
		thread_id: "thread_1",
		workspace: {
			observed_at,
			repository_state: "not_repository" as const,
			snapshot_id,
			workspace_id,
		},
	};
}

function mutation_request(
	mutation_id = "mutation_1",
	workspace_id = "workspace_1",
	paths: readonly [string, ...string[]] = ["src/b.ts", "src/a.ts"],
) {
	return {
		agent_id: "agent_1",
		kind: "git.index.stage.request" as const,
		message_id: `request_${mutation_id}`,
		origin: "frontend" as const,
		payload: {
			approval_id: `approval_${mutation_id}`,
			expected_snapshot_id: snapshot_a,
			expected_workspace_version: 1,
			mutation_id,
			paths,
			workspace_id,
		},
		protocol_version: 1 as const,
		raw_origin: { provider: "codex", reference: "origin_1" },
		run_id: "run_1",
		schema_version: 1 as const,
		sent_at: "2026-07-18T12:01:00.000Z",
		thread_id: "thread_1",
	};
}

function mutation_decision(
	mutation_id = "mutation_1",
	approved = true,
	message_id = `decision_${mutation_id}`,
) {
	return {
		agent_id: "agent_1",
		kind: "git.mutation.resolve" as const,
		message_id,
		origin: "frontend" as const,
		payload: {
			approval_id: `approval_${mutation_id}`,
			approved,
			mutation_id,
		},
		protocol_version: 1 as const,
		raw_origin: { provider: "codex", reference: "origin_1" },
		run_id: "run_1",
		schema_version: 1 as const,
		sent_at: "2026-07-18T12:02:00.000Z",
		thread_id: "thread_1",
	};
}

function initialize(repository: GitRepository["Service"], database: Database["Service"]) {
	return Effect.gen(function* () {
		yield* seed_thread(database);
		return yield* repository.RecordWorkspace(workspace_record());
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("GitRepository", () => {
	it("persists workspace projections across restart and exactly replays observations", async () => {
		const database_path = await make_database_path();
		const first = make_runtime(database_path, "first");

		try {
			const result = await first.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* GitRepository;
					const accepted = yield* initialize(repository, database);
					const duplicate = yield* repository.RecordWorkspace({
						...workspace_record(),
						workspace: {
							...workspace_record().workspace,
							observed_at: "2026-07-18T12:30:00.000Z",
						},
					});

					return { accepted, duplicate };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.accepted.workspace.version).toBe(1);
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.event.journal_sequence).toBe(
				result.accepted.event.journal_sequence,
			);
			expect(result.duplicate.workspace.observed_at).toBe(observed_at);
		} finally {
			await first.dispose();
		}

		const restarted = make_runtime(database_path, "restarted");

		try {
			const projection = await restarted.runPromise(
				Effect.flatMap(GitRepository, (repository) =>
					repository.ReadWorkspace("workspace_1"),
				),
			);

			expect(projection.snapshot_id).toBe(snapshot_a);
			expect(projection.version).toBe(1);
		} finally {
			await restarted.dispose();
		}
	});

	it("deduplicates exact requests and rejects changed identity reuse", async () => {
		const runtime = make_runtime(await make_database_path(), "request");

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* GitRepository;
					yield* initialize(repository, database);
					const accepted = yield* repository.RequestMutation(
						mutation_request("mutation_1", "workspace_1", [
							"src/ø.ts",
							"src/Z.ts",
							"src/ä.ts",
						]),
					);
					const duplicate = yield* repository.RequestMutation(
						mutation_request("mutation_1", "workspace_1", [
							"src/ä.ts",
							"src/ø.ts",
							"src/Z.ts",
						]),
					);
					const conflict = yield* Effect.flip(
						repository.RequestMutation(
							mutation_request("mutation_1", "workspace_1", ["src/c.ts"]),
						),
					);
					const rows = yield* database.client.select().from(GitMutationOperations);

					return { accepted, conflict, duplicate, rows };
				}),
			);

			expect(result.accepted.mutation.paths).toEqual(["src/Z.ts", "src/ä.ts", "src/ø.ts"]);
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.event.message_id).toBe(result.accepted.event.message_id);
			expect(result.conflict).toBeInstanceOf(GitRepositoryConflict);
			expect(result.conflict).toMatchObject({ reason: "mutation_conflict" });
			expect(result.rows).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("makes denial terminal and decision retries exact", async () => {
		const runtime = make_runtime(await make_database_path(), "denial");

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* GitRepository;
					yield* initialize(repository, database);
					yield* repository.RequestMutation(mutation_request());
					const denied = yield* repository.ResolveMutation(
						mutation_decision("mutation_1", false),
					);
					const duplicate = yield* repository.ResolveMutation(
						mutation_decision("mutation_1", false),
					);
					const changed = yield* Effect.flip(
						repository.ResolveMutation(mutation_decision("mutation_1", true)),
					);
					const claim = yield* Effect.flip(repository.ClaimApproved("mutation_1"));

					return { changed, claim, denied, duplicate };
				}),
			);

			expect(result.denied.mutation.lifecycle).toBe("denied");
			expect(result.denied.mutation.completed_at).toBeDefined();
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.changed).toMatchObject({ reason: "decision_conflict" });
			expect(result.claim).toMatchObject({ reason: "dispatch_conflict" });
		} finally {
			await runtime.dispose();
		}
	});

	it("claims an approved mutation once across two runtimes", async () => {
		const database_path = await make_database_path();
		const first = make_runtime(database_path, "claim_a");

		try {
			await first.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* GitRepository;
					yield* initialize(repository, database);
					yield* repository.RequestMutation(mutation_request());
					yield* repository.ResolveMutation(mutation_decision());
				}),
			);

			const second = make_runtime(database_path, "claim_b");

			try {
				const [left, right] = await Promise.allSettled([
					first.runPromise(
						Effect.flatMap(GitRepository, (repository) =>
							repository.ClaimApproved("mutation_1"),
						),
					),
					second.runPromise(
						Effect.flatMap(GitRepository, (repository) =>
							repository.ClaimApproved("mutation_1"),
						),
					),
				]);
				const outcomes = [left, right];

				expect(outcomes.filter((outcome) => outcome.status === "fulfilled")).toHaveLength(
					1,
				);
				expect(outcomes.filter((outcome) => outcome.status === "rejected")).toHaveLength(1);
				const mutation = await first.runPromise(
					Effect.flatMap(GitRepository, (repository) =>
						repository.ReadMutation("mutation_1"),
					),
				);
				const [stored] = await first.runPromise(
					Effect.flatMap(Database, (database) =>
						database.client
							.select({
								dispatch_lease_expires_at:
									GitMutationOperations.dispatch_lease_expires_at,
								dispatch_owner_id: GitMutationOperations.dispatch_owner_id,
							})
							.from(GitMutationOperations),
					),
				);

				expect(mutation.lifecycle).toBe("dispatching");
				expect(mutation.dispatched_at).toBeDefined();
				expect(stored).toMatchObject({
					dispatch_owner_id: "claim_a",
				});
				expect(stored?.dispatch_lease_expires_at).toBeDefined();
				const workspace_busy = await first.runPromise(
					Effect.gen(function* () {
						const repository = yield* GitRepository;
						return yield* Effect.flip(
							repository.RecordWorkspace(workspace_record(snapshot_b)),
						);
					}),
				);

				expect(workspace_busy).toMatchObject({ reason: "workspace_busy" });
			} finally {
				await second.dispose();
			}
		} finally {
			await first.dispose();
		}
	});

	it("orders success events and recovers dispatching work as ambiguous", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "terminal");

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* GitRepository;
					yield* initialize(repository, database);
					yield* repository.RequestMutation(mutation_request("mutation_success"));
					yield* repository.ResolveMutation(mutation_decision("mutation_success"));
					yield* repository.ClaimApproved("mutation_success");
					const success = yield* repository.CommitSucceeded({
						mutation_id: "mutation_success",
						workspace: workspace_record(snapshot_b).workspace,
					});
					const duplicate = yield* repository.CommitSucceeded({
						mutation_id: "mutation_success",
						workspace: workspace_record(snapshot_b).workspace,
					});
					yield* repository.RecordWorkspace(workspace_record(snapshot_a, "workspace_2"));
					yield* repository.RequestMutation(
						mutation_request("mutation_crash", "workspace_2"),
					);
					yield* repository.ResolveMutation(mutation_decision("mutation_crash"));
					yield* repository.ClaimApproved("mutation_crash");
					yield* repository.RecordWorkspace(workspace_record(snapshot_a, "workspace_3"));
					yield* repository.RequestMutation(
						mutation_request("mutation_approved", "workspace_3"),
					);
					yield* repository.ResolveMutation(mutation_decision("mutation_approved"));

					return { duplicate, success };
				}),
			);

			expect(result.success.workspace_event.journal_sequence).toBeLessThan(
				result.success.mutation_event.journal_sequence,
			);
			expect(result.duplicate.status).toBe("duplicate");
		} finally {
			await runtime.dispose();
		}

		const restarted = make_runtime(database_path, "recover");

		try {
			const { pending, recovery } = await restarted.runPromise(
				Effect.gen(function* () {
					const repository = yield* GitRepository;
					const recovery = yield* repository.RecoverDispatching();
					const pending = yield* repository.ListPending();

					return { pending, recovery };
				}),
			);

			expect(recovery.ambiguous).toHaveLength(0);
			expect(pending.map((mutation) => mutation.lifecycle).toSorted()).toEqual([
				"approved",
				"dispatching",
			]);
		} finally {
			await restarted.dispose();
		}

		const expired = make_runtime(database_path, "recover_expired", 120);

		try {
			const { pending, recovery, repeated } = await expired.runPromise(
				Effect.gen(function* () {
					const repository = yield* GitRepository;
					const recovery = yield* repository.RecoverDispatching();
					const pending = yield* repository.ListPending();
					const repeated = yield* repository.RecoverDispatching();

					return { pending, recovery, repeated };
				}),
			);
			const events = await expired.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(JournalEvents),
				),
			);

			expect(recovery.ambiguous).toHaveLength(1);
			expect(recovery.ambiguous[0]?.lifecycle).toBe("ambiguous");
			expect(recovery.ambiguous[0]?.failure).toEqual({ code: "git_dispatch_recovery" });
			expect(recovery.approved.map((mutation) => mutation.mutation_id)).toEqual([
				"mutation_approved",
			]);
			expect(pending.map((mutation) => mutation.lifecycle).toSorted()).toEqual([
				"ambiguous",
				"approved",
			]);
			expect(repeated.ambiguous).toHaveLength(0);
			expect(
				events.filter((event) => event.event_type === "git.workspace.updated"),
			).not.toHaveLength(0);
		} finally {
			await expired.dispose();
		}
	});
});
