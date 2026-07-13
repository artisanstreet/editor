import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceGitCheckoutApprovals,
	WorkspaceGitCheckoutClaims,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceGitCheckoutConflict,
	WorkspaceGitCheckoutRepository,
	WorkspaceGitCheckoutRepositoryLive,
	WorkspaceGitCheckoutUnavailable,
	type RequestWorkspaceGitCheckout,
	type WorkspaceGitCheckoutDecision,
} from "../../modules/backend/src/git/workspace-git-checkout-repository";
import {
	WorkspaceGitSessionRepository,
	WorkspaceGitSessionRepositoryLive,
	type ProjectObservation,
} from "../../modules/backend/src/git/workspace-git-session-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-13T15:00:00.000Z";
const later = "2026-07-13T15:01:00.000Z";
let next_id = 0;
let next_time = Date.parse("2026-07-13T16:00:00.000Z");

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-git-checkout-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function make_metadata_layer() {
	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_git_checkout_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_git_checkout_${++next_id}`),
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

	expect(failure).toBeInstanceOf(WorkspaceGitCheckoutConflict);
	expect(failure).toMatchObject({ reason });
}

const SeedThreads = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values([
		{
			created_at: now,
			thread_id: "thread_1",
			title: "Git checkout repository",
			title_source: "initial",
			updated_at: now,
		},
		{
			created_at: now,
			thread_id: "thread_2",
			title: "Git checkout conflict",
			title_source: "initial",
			updated_at: now,
		},
	]);
});

function session_observation(
	operation_id: string,
	overrides: {
		readonly dirty?: boolean;
		readonly fingerprint?: string;
		readonly head?: string;
		readonly workspace_id?: string;
	} = {},
): ProjectObservation {
	const dirty = overrides.dirty ?? false;
	const head = overrides.head ?? "a".repeat(40);

	return {
		kind: "recovery",
		observed_at: now,
		operation_id,
		repository_root: "C:/workspace",
		request_fingerprint: overrides.fingerprint ?? "a".repeat(64),
		selected_worktree_path: "C:/workspace",
		session: {
			blockers: [],
			branch: "main",
			changed_files: dirty
				? [
						{
							conflicted: false,
							path: "src/dirty.ts",
							staged: false,
							status: "modified",
							untracked: false,
							unstaged: true,
						},
					]
				: [],
			diff_stats: dirty
				? { additions: 1, deletions: 0, files: 1 }
				: { additions: 0, deletions: 0, files: 0 },
			has_diff: dirty,
			head,
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
	overrides: Partial<RequestWorkspaceGitCheckout> = {},
): RequestWorkspaceGitCheckout {
	return {
		approval_id: "checkout_approval_1",
		expected_session_version: 1,
		request_fingerprint: "b".repeat(64),
		source_command: {
			message_id: "checkout_request_1",
			sent_at: now,
		},
		target_branch: "release",
		target_head: "b".repeat(40),
		thread_id: "thread_1",
		workspace_id: "workspace_1",
		...overrides,
	};
}

function checkout_decision(
	overrides: Partial<WorkspaceGitCheckoutDecision> = {},
): WorkspaceGitCheckoutDecision {
	return {
		approval_id: "checkout_approval_1",
		approved: true,
		decision_command: {
			message_id: "checkout_decision_1",
			sent_at: later,
		},
		thread_id: "thread_1",
		...overrides,
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

describe("WorkspaceGitCheckoutRepository", () => {
	it("replays a moving target ref from its source command without weakening frontend intent", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitCheckoutRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_observation_1"));
					const first = yield* repository.Request(checkout_request());
					const moved_ref = yield* repository.Request(
						checkout_request({ target_head: "c".repeat(40) }),
					);
					const by_command = yield* repository.ReadBySourceCommand("checkout_request_1");
					const changed_intent = yield* Effect.forEach(
						[
							checkout_request({ workspace_id: "workspace_2" }),
							checkout_request({ target_branch: "develop" }),
							checkout_request({ expected_session_version: 2 }),
							checkout_request({ thread_id: "thread_2" }),
							checkout_request({ request_fingerprint: "f".repeat(64) }),
							checkout_request({
								source_command: {
									message_id: "checkout_request_1",
									sent_at: later,
								},
							}),
						],
						(input) => repository.Request(input).pipe(Effect.exit),
					);

					yield* database.client.insert(JournalCommands).values({
						accepted_at: now,
						message_id: "occupied_command",
						origin: "frontend",
						payload_json: JSON.stringify({ type: "thread.create" }),
						payload_type: "thread.create",
						schema_version: 1,
						sent_at: now,
						status: "accepted",
						thread_id: "thread_1",
					});
					const occupied = yield* repository
						.Request(
							checkout_request({
								approval_id: "checkout_approval_occupied",
								source_command: { message_id: "occupied_command", sent_at: now },
							}),
						)
						.pipe(Effect.exit);

					return {
						approvals: yield* database.client
							.select()
							.from(WorkspaceGitCheckoutApprovals),
						by_command,
						changed_intent,
						first,
						moved_ref,
						occupied,
					};
				}),
			);

			expect(result.moved_ref).toEqual({ ...result.first, status: "duplicate" });
			expect(Option.getOrThrow(result.by_command)).toEqual(result.moved_ref);
			expect(result.approvals).toHaveLength(1);
			expect(result.approvals[0]!.target_head).toBe("b".repeat(40));
			result.changed_intent.forEach((exit) => expect_conflict(exit, "request_conflict"));
			expect_conflict(result.occupied, "command_conflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects stale and dirty source sessions before writing a checkout command", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitCheckoutRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_observation_stale"));
					const stale = yield* repository
						.Request(checkout_request({ expected_session_version: 2 }))
						.pipe(Effect.exit);
					yield* sessions.Project(
						session_observation("session_observation_dirty", {
							dirty: true,
							fingerprint: "c".repeat(64),
						}),
					);
					const dirty = yield* repository
						.Request(
							checkout_request({
								approval_id: "checkout_approval_dirty",
								expected_session_version: 2,
								source_command: {
									message_id: "checkout_request_dirty",
									sent_at: now,
								},
							}),
						)
						.pipe(Effect.exit);

					return {
						approvals: yield* database.client
							.select()
							.from(WorkspaceGitCheckoutApprovals),
						commands: yield* database.client.select().from(JournalCommands),
						dirty,
						stale,
					};
				}),
			);

			expect_conflict(result.stale, "session_stale");
			expect_conflict(result.dirty, "session_dirty");
			expect(result.approvals).toEqual([]);
			expect(result.commands).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists approved and denied decisions with exact replay conflicts", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitCheckoutRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_observation_decision"));
					yield* repository.Request(checkout_request());
					const approved = yield* repository.Decide(checkout_decision());
					const duplicate = yield* repository.Decide(checkout_decision());
					const conflict = yield* repository
						.Decide(checkout_decision({ approved: false }))
						.pipe(Effect.exit);
					const late_command = yield* repository
						.Decide(
							checkout_decision({
								decision_command: {
									message_id: "checkout_decision_late",
									sent_at: later,
								},
							}),
						)
						.pipe(Effect.exit);
					const request_replay =
						yield* repository.ReadBySourceCommand("checkout_request_1");
					const listed = yield* repository.ListApproved;

					const denied_request = checkout_request({
						approval_id: "checkout_approval_denied",
						source_command: { message_id: "checkout_request_denied", sent_at: now },
						target_branch: "develop",
						target_head: "d".repeat(40),
					});
					yield* repository.Request(denied_request);
					const denied_input = checkout_decision({
						approval_id: "checkout_approval_denied",
						approved: false,
						decision_command: {
							message_id: "checkout_decision_denied",
							sent_at: later,
						},
					});
					const denied = yield* repository.Decide(denied_input);
					const denied_duplicate = yield* repository.Decide(denied_input);
					const query = yield* repository.Query({
						approval_id: "checkout_approval_denied",
						thread_id: "thread_1",
					});

					return {
						approved,
						conflict,
						denied,
						denied_duplicate,
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						late_command,
						listed,
						query,
						request_replay,
					};
				}),
			);

			expect(result.duplicate).toEqual({ ...result.approved, status: "duplicate" });
			expect(result.denied_duplicate).toEqual({ ...result.denied, status: "duplicate" });
			expect(result.query.approval.state).toBe("denied");
			expect(result.listed).toEqual(["checkout_approval_1"]);
			expect(Option.getOrThrow(result.request_replay).approval.state).toBe("requested");
			expect_conflict(result.conflict, "decision_conflict");
			expect_conflict(result.late_command, "decision_conflict");
			expect(
				result.events.every((event) => !event.payload_json.includes("C:/workspace")),
			).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("blocks execution behind an active workspace mutation and restores execution inputs", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);

		try {
			const blocked = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitCheckoutRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_observation_execution"));
					yield* repository.Request(checkout_request());
					yield* repository.Decide(checkout_decision());
					yield* database.client.insert(WorkspaceChangeOperations).values({
						action: "replace",
						change_id: "change_active",
						created_at: now,
						lifecycle: "claimed",
						message_id: "workspace_change_active",
						request_fingerprint: "e".repeat(64),
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

					const result = yield* repository
						.MarkExecuting("checkout_approval_1")
						.pipe(Effect.exit);
					const claims = yield* database.client.select().from(WorkspaceGitCheckoutClaims);

					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "committed", updated_at: later });
					yield* repository.MarkExecuting("checkout_approval_1");

					return { claims, result };
				}),
			);

			expect_conflict(blocked.result, "workspace_mutation_active");
			expect(blocked.claims).toEqual([]);
		} finally {
			await first_runtime.dispose();
		}

		const restarted = make_runtime(database_path);

		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceGitCheckoutRepository;
					const listed = yield* repository.ListExecuting;
					const execution = yield* repository.ReadExecution("checkout_approval_1");
					const duplicate = yield* repository.MarkExecuting("checkout_approval_1");

					return { duplicate, execution, listed };
				}),
			);

			expect(result.listed).toEqual(["checkout_approval_1"]);
			expect(result.execution).toMatchObject({
				approval: { state: "executing" },
				repository_root: "C:/workspace",
				selected_worktree_path: "C:/workspace",
				target_head: "b".repeat(40),
			});
			expect(result.duplicate.status).toBe("duplicate");
		} finally {
			await restarted.dispose();
		}
	});

	it("serializes workspace checkouts and releases the claim on terminal transitions", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitCheckoutRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_observation_claim"));
					yield* repository.Request(checkout_request());
					yield* repository.Decide(checkout_decision());
					yield* repository.MarkExecuting("checkout_approval_1");

					const second_request = checkout_request({
						approval_id: "checkout_approval_2",
						source_command: { message_id: "checkout_request_2", sent_at: now },
						target_branch: "develop",
						target_head: "d".repeat(40),
					});
					yield* repository.Request(second_request);
					yield* repository.Decide(
						checkout_decision({
							approval_id: "checkout_approval_2",
							decision_command: { message_id: "checkout_decision_2", sent_at: later },
						}),
					);
					const collision = yield* repository
						.MarkExecuting("checkout_approval_2")
						.pipe(Effect.exit);
					const unknown = yield* repository.MarkUnknown("checkout_approval_1");
					const unknown_duplicate = yield* repository.MarkUnknown("checkout_approval_1");
					yield* repository.MarkExecuting("checkout_approval_2");
					const applied = yield* repository.MarkApplied("checkout_approval_2");
					const applied_duplicate = yield* repository.MarkApplied("checkout_approval_2");
					const impossible = yield* repository
						.MarkRejected("checkout_approval_2")
						.pipe(Effect.exit);

					return {
						applied,
						applied_duplicate,
						claims: yield* database.client.select().from(WorkspaceGitCheckoutClaims),
						collision,
						impossible,
						unknown,
						unknown_duplicate,
					};
				}),
			);

			expect_conflict(result.collision, "claim_conflict");
			expect(result.unknown_duplicate).toEqual({ ...result.unknown, status: "duplicate" });
			expect(result.applied_duplicate).toEqual({ ...result.applied, status: "duplicate" });
			expect(result.claims).toEqual([]);
			expect_conflict(result.impossible, "invalid_transition");
		} finally {
			await runtime.dispose();
		}
	});

	it("conceals checkout approvals behind thread erasure claims and tombstones", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionRepository;
					const repository = yield* WorkspaceGitCheckoutRepository;

					yield* SeedThreads;
					yield* sessions.Project(session_observation("session_observation_erasure"));
					yield* repository.Request(checkout_request());
					yield* repository.Decide(checkout_decision());
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: later,
						thread_id: "thread_1",
					});

					const query = yield* repository
						.Query({ approval_id: "checkout_approval_1", thread_id: "thread_1" })
						.pipe(Effect.exit);
					const read = yield* repository
						.ReadBySourceCommand("checkout_request_1")
						.pipe(Effect.exit);
					const list = yield* repository.ListApproved.pipe(Effect.exit);

					yield* database.client.insert(ThreadTombstones).values({
						deleted_at: later,
						thread_id: "thread_2",
					});
					const request = yield* repository
						.Request(
							checkout_request({
								approval_id: "checkout_approval_erased",
								source_command: {
									message_id: "checkout_request_erased",
									sent_at: now,
								},
								thread_id: "thread_2",
							}),
						)
						.pipe(Effect.exit);

					return { list, query, read, request };
				}),
			);

			for (const exit of [result.query, result.read, result.list, result.request]) {
				const failure = failure_from(exit);

				expect(failure).toBeInstanceOf(WorkspaceGitCheckoutUnavailable);
				expect(failure).toMatchObject({ reason: "erased" });
			}
		} finally {
			await runtime.dispose();
		}
	});
});
