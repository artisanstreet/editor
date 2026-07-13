import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	WorkspaceEvidenceConflict,
	WorkspaceEvidenceInvalid,
	WorkspaceEvidenceRecorder,
} from "../../modules/backend/src/workspace/workspace-evidence-recorder";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	Threads,
	WorkspaceGitOperations,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceGitObserver,
	type WorkspaceGitObservation,
} from "../../modules/backend/src/git/workspace-git-observer";
import { WorkspaceGitSessionRepositoryLive } from "../../modules/backend/src/git/workspace-git-session-repository";
import {
	WorkspaceGitSessionService,
	WorkspaceGitSessionServiceLive,
} from "../../modules/backend/src/git/workspace-git-session-service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const workspace_id = "workspace_session";
const thread_id = "thread_session";
const root_path = "C:/private/project";
const worktree_path = "C:/private/project";
const head = "0123456789012345678901234567890123456789";

type EvidenceRecorderState = {
	readonly calls: Array<string>;
	failed: boolean;
	conflict: boolean;
};

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-git-session-service-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function observation(state: WorkspaceGitObservation["state"] = "ready"): WorkspaceGitObservation {
	return {
		adapter_worktrees: [
			{
				adapter_path: worktree_path,
				bare: false,
				branch: "main",
				detached: false,
				head,
				locked: false,
				location: "selected",
				prunable: false,
			},
		],
		blockers: [],
		branch: "main",
		changed_files: [],
		diff_stats: { additions: 0, deletions: 0, files: 0 },
		has_diff: false,
		head,
		observed_at: "2026-07-13T10:00:00.000Z",
		repository_root: root_path,
		selected_worktree_path: worktree_path,
		state,
		worktrees: [
			{
				bare: false,
				branch: "main",
				detached: false,
				head,
				locked: false,
				location: "selected",
				prunable: false,
			},
		],
		workspace_id,
	};
}

function unavailable_observation(): WorkspaceGitObservation {
	return {
		adapter_worktrees: [],
		blockers: ["not_repository"],
		changed_files: [],
		diff_stats: { additions: 0, deletions: 0, files: 0 },
		has_diff: false,
		observed_at: "2026-07-13T10:00:00.000Z",
		state: "unavailable",
		worktrees: [],
		workspace_id,
	};
}

function make_runtime(
	database_path: string,
	current_observation: { value: WorkspaceGitObservation },
	evidence_state: EvidenceRecorderState,
) {
	const observer = Layer.succeed(WorkspaceGitObserver, {
		Observe: () => Effect.succeed(current_observation.value),
	});
	const evidence = Layer.succeed(WorkspaceEvidenceRecorder, {
		RecordFilesystemMutation: () => Effect.die("unused"),
		RecordGitWorkspaceObserved: (input) => {
			evidence_state.calls.push(input.operation_id);

			if (evidence_state.conflict) {
				return Effect.fail(
					new WorkspaceEvidenceConflict({ operation_id: input.operation_id }),
				);
			}

			if (evidence_state.failed) {
				return Effect.fail(
					new WorkspaceEvidenceInvalid({ message: "injected evidence failure" }),
				);
			}

			return Effect.succeed({ event: {} as never, status: "accepted" as const });
		},
		RecordProcessOwnership: () => Effect.die("unused"),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		RuntimeMetadataLive,
		JournalNotifierLive,
	);
	const repository = WorkspaceGitSessionRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const service = WorkspaceGitSessionServiceLive.pipe(
		Layer.provideMerge(repository),
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(observer),
		Layer.provideMerge(evidence),
	);

	return ManagedRuntime.make(service);
}

async function seed_thread(runtime: ManagedRuntime.ManagedRuntime<any, any>) {
	await runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;

			yield* database.client.insert(Threads).values({
				created_at: "2026-07-13T10:00:00.000Z",
				thread_id,
				title: thread_id,
				updated_at: "2026-07-13T10:00:00.000Z",
			});
		}),
	);
}

async function read_rows(runtime: ManagedRuntime.ManagedRuntime<any, any>) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;

			return {
				commands: yield* database.client.select().from(JournalCommands),
				operations: yield* database.client.select().from(WorkspaceGitOperations),
			};
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

describe("WorkspaceGitSessionService", () => {
	it("refreshes, accepts an attributed event, and returns a path-free query", async () => {
		const current_observation = { value: observation() };
		const evidence_state = {
			calls: [],
			failed: false,
			conflict: false,
		} satisfies EvidenceRecorderState;
		const runtime = make_runtime(
			await make_database_path(),
			current_observation,
			evidence_state,
		);

		try {
			await seed_thread(runtime);
			const acceptance = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Refresh({
						message_id: "refresh_1",
						sent_at: "2026-07-13T10:01:00.000Z",
						thread_id,
						workspace_id,
					});
				}),
			);
			const query = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Query({ workspace_id });
				}),
			);

			expect(acceptance.status).toBe("accepted");
			expect(acceptance.event.payload).toMatchObject({
				type: "workspace.git.session.updated",
			});
			expect(query.session).toMatchObject({
				branch: "main",
				version: 1,
				worktrees: [{ location: "selected" }],
			});
			expect(JSON.stringify(query)).not.toContain(root_path);
			expect(JSON.stringify(query)).not.toContain(worktree_path);
			expect(query.journal_sequence).toBe(acceptance.event.journal_sequence);
		} finally {
			await runtime.dispose();
		}
	});

	it("makes an exact refresh retry a duplicate and rejects changed intent", async () => {
		const current_observation = { value: observation() };
		const evidence_state = {
			calls: [],
			failed: false,
			conflict: false,
		} satisfies EvidenceRecorderState;
		const runtime = make_runtime(
			await make_database_path(),
			current_observation,
			evidence_state,
		);
		const input = {
			message_id: "refresh_2",
			sent_at: "2026-07-13T10:01:00.000Z",
			thread_id,
			workspace_id,
		};

		try {
			await seed_thread(runtime);
			const first = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Refresh(input);
				}),
			);
			const duplicate = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Refresh(input);
				}),
			);

			expect(duplicate.status).toBe("duplicate");
			expect(duplicate.event.journal_sequence).toBe(first.event.journal_sequence);
			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						const service = yield* WorkspaceGitSessionService;

						return yield* service.Refresh({
							...input,
							sent_at: "2026-07-13T10:02:00.000Z",
						});
					}),
				),
			).rejects.toMatchObject({ _tag: "WorkspaceGitSessionConflict" });
		} finally {
			await runtime.dispose();
		}
	});

	it("projects internal checkout and skips evidence for unavailable Git", async () => {
		const current_observation = { value: observation() };
		const evidence_state = {
			calls: [],
			failed: false,
			conflict: false,
		} satisfies EvidenceRecorderState;
		const runtime = make_runtime(
			await make_database_path(),
			current_observation,
			evidence_state,
		);

		try {
			await seed_thread(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					yield* service.Project({
						kind: "checkout",
						operation_id: "checkout_1",
						sent_at: "2026-07-13T10:03:00.000Z",
						thread_id,
						workspace_id,
					});
				}),
			);
			current_observation.value = unavailable_observation();
			await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Refresh({
						message_id: "refresh_unavailable",
						sent_at: "2026-07-13T10:04:00.000Z",
						thread_id,
						workspace_id,
					});
				}),
			);

			const rows = await read_rows(runtime);
			expect(rows.commands).toHaveLength(1);
			expect(rows.commands[0]!.message_id).toBe("refresh_unavailable");
			expect(evidence_state.calls).toEqual(["checkout_1"]);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps pending evidence durable across failure and restarted recovery", async () => {
		const database_path = await make_database_path();
		const current_observation = { value: observation() };
		const failed_evidence = {
			calls: [],
			failed: true,
			conflict: false,
		} satisfies EvidenceRecorderState;
		const first_runtime = make_runtime(database_path, current_observation, failed_evidence);

		try {
			await seed_thread(first_runtime);
			await expect(
				first_runtime.runPromise(
					Effect.gen(function* () {
						const service = yield* WorkspaceGitSessionService;

						return yield* service.Project({
							kind: "recovery",
							operation_id: "recovery_1",
							sent_at: "2026-07-13T10:05:00.000Z",
							thread_id,
							workspace_id,
						});
					}),
				),
			).rejects.toMatchObject({ _tag: "WorkspaceEvidenceInvalid" });
		} finally {
			await first_runtime.dispose();
		}

		const recovered_evidence = {
			calls: [],
			failed: false,
			conflict: false,
		} satisfies EvidenceRecorderState;
		const second_runtime = make_runtime(database_path, current_observation, recovered_evidence);

		try {
			const query = await second_runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Query({ workspace_id });
				}),
			);
			const retry = await second_runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitSessionService;

					return yield* service.Project({
						kind: "recovery",
						operation_id: "recovery_1",
						sent_at: "2026-07-13T10:05:00.000Z",
						thread_id,
						workspace_id,
					});
				}),
			);

			expect(recovered_evidence.calls).toEqual(["recovery_1"]);
			expect(retry.status).toBe("duplicate");
			expect(query.session?.version).toBe(1);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("surfaces evidence conflict without losing the committed projection", async () => {
		const current_observation = { value: observation() };
		const evidence_state = {
			calls: [],
			failed: false,
			conflict: true,
		} satisfies EvidenceRecorderState;
		const runtime = make_runtime(
			await make_database_path(),
			current_observation,
			evidence_state,
		);

		try {
			await seed_thread(runtime);
			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						const service = yield* WorkspaceGitSessionService;

						return yield* service.Project({
							kind: "checkout",
							operation_id: "checkout_conflict",
							sent_at: "2026-07-13T10:06:00.000Z",
							thread_id,
							workspace_id,
						});
					}),
				),
			).rejects.toMatchObject({ _tag: "WorkspaceEvidenceConflict" });
			const rows = await read_rows(runtime);

			expect(rows.operations).toHaveLength(1);
			expect(rows.operations[0]!.evidence_recorded).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});
});
