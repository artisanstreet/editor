import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Git } from "../../modules/backend/src/git/git";
import { GitMutation, GitMutationError } from "../../modules/backend/src/git/git-mutation";
import {
	WorkspaceGitCheckoutCoordinator,
	WorkspaceGitCheckoutCoordinatorLive,
} from "../../modules/backend/src/git/workspace-git-checkout-coordinator";
import {
	WorkspaceGitCheckoutRepository,
	WorkspaceGitCheckoutRepositoryLive,
} from "../../modules/backend/src/git/workspace-git-checkout-repository";
import {
	WorkspaceGitObserver,
	type WorkspaceGitObservation,
} from "../../modules/backend/src/git/workspace-git-observer";
import {
	WorkspaceGitRegistry,
	type WorkspaceGitCapability,
} from "../../modules/backend/src/git/workspace-git-registry";
import { WorkspaceGitSessionRepositoryLive } from "../../modules/backend/src/git/workspace-git-session-repository";
import {
	WorkspaceGitSessionService,
	WorkspaceGitSessionServiceLive,
} from "../../modules/backend/src/git/workspace-git-session-service";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { Threads, WorkspaceGitCheckoutClaims } from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { WorkspaceEvidenceRecorder } from "../../modules/backend/src/workspace/workspace-evidence-recorder";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const workspace_id = "workspace_checkout";
const thread_id = "thread_checkout";
const source_head = "a".repeat(40);
const target_head = "b".repeat(40);
let next_id = 0;
let next_time = Date.parse("2026-07-13T18:00:00.000Z");

type FakeGitState = {
	branch: string;
	checkout_calls: number;
	commits: Array<string>;
	fail_checkout: boolean;
	worktrees: Array<string>;
};

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-git-checkout-coordinator-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function make_metadata_layer() {
	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_git_checkout_coordinator_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_checkout_coordinator_${++next_id}`),
		Now: Effect.sync(() => new Date(next_time++).toISOString()),
	});
}

function observation(state: FakeGitState, dirty = false): WorkspaceGitObservation {
	const changed_files = dirty
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
		: [];
	const head = state.branch === "main" ? source_head : target_head;

	return {
		adapter_worktrees: [
			{
				adapter_path: "C:/workspace",
				bare: false,
				branch: state.branch,
				detached: false,
				head,
				locked: false,
				location: "selected",
				prunable: false,
			},
		],
		blockers: [],
		branch: state.branch,
		changed_files,
		diff_stats: dirty
			? { additions: 1, deletions: 0, files: 1 }
			: { additions: 0, deletions: 0, files: 0 },
		has_diff: dirty,
		head,
		observed_at: "2026-07-13T18:00:00.000Z",
		repository_root: "C:/workspace",
		selected_worktree_path: "C:/workspace",
		state: "ready",
		worktrees: [
			{
				bare: false,
				branch: state.branch,
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

function make_git_layers(state: FakeGitState, dirty = { value: false }) {
	const read: typeof Git.Service = {
		DiffPatch: () => Effect.succeed({ bytes: 0, patch: "", truncated: false }),
		DiffStats: Effect.succeed({ additions: 0, deletions: 0, files: 0 }),
		Discover: Effect.succeed({
			branch: state.branch,
			head: Option.some(source_head),
			root: "C:/workspace",
		}),
		ProbeRepository: Effect.succeed(
			Option.some({
				branch: state.branch,
				head: Option.some(source_head),
				root: "C:/workspace",
			}),
		),
		ResolveLocalBranch: (branch) =>
			Effect.succeed(branch === "release" ? Option.some(target_head) : Option.none()),
		Status: Effect.succeed([]),
		Worktrees: Effect.succeed([]),
	};
	const mutation: typeof GitMutation.Service = {
		CheckoutLocalBranch: (branch) =>
			Effect.gen(function* () {
				state.checkout_calls += 1;

				if (state.fail_checkout) {
					return yield* Effect.fail(
						new GitMutationError({
							cause: "injected checkout failure",
							operation: "checkout",
						}),
					);
				}

				state.branch = branch;
			}),
	};
	const capability: WorkspaceGitCapability = {
		canonical_root: "C:/workspace",
		mutation,
		read,
		workspace_id,
	};
	const observer = Layer.succeed(WorkspaceGitObserver, {
		Observe: () => Effect.succeed(observation(state, dirty.value)),
	});
	const registry = Layer.succeed(WorkspaceGitRegistry, {
		Get: () => Effect.succeed(capability),
		ListWorkspaceIds: Effect.succeed([workspace_id]),
	});

	return { observer, registry };
}

function make_evidence_layer() {
	return Layer.succeed(WorkspaceEvidenceRecorder, {
		RecordFilesystemMutation: () => Effect.die("unused"),
		RecordGitWorkspaceObserved: () =>
			Effect.succeed({ event: {} as never, status: "accepted" as const }),
		RecordProcessOwnership: () => Effect.die("unused"),
	});
}

function make_runtime(
	database_path: string,
	state: FakeGitState,
	dirty = { value: false },
	include_coordinator = true,
): ManagedRuntime.ManagedRuntime<any, any> {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const repositories = Layer.mergeAll(
		WorkspaceGitCheckoutRepositoryLive,
		WorkspaceGitSessionRepositoryLive,
	).pipe(Layer.provideMerge(infrastructure));
	const { observer, registry } = make_git_layers(state, dirty);
	const support = Layer.mergeAll(NodeCrypto.layer, make_evidence_layer(), observer, registry);
	const session = WorkspaceGitSessionServiceLive.pipe(
		Layer.provideMerge(repositories),
		Layer.provideMerge(support),
	);
	const services = Layer.mergeAll(infrastructure, repositories, support, session);
	const application = include_coordinator
		? Layer.merge(
				services,
				WorkspaceGitCheckoutCoordinatorLive.pipe(Layer.provideMerge(services)),
			)
		: services;

	return ManagedRuntime.make(application);
}

const SeedThreadAndSession = Effect.gen(function* () {
	const database = yield* Database;
	const sessions = yield* WorkspaceGitSessionService;

	yield* database.client.insert(Threads).values({
		created_at: "2026-07-13T18:00:00.000Z",
		thread_id,
		title: "Checkout coordinator test",
		title_source: "initial",
		updated_at: "2026-07-13T18:00:00.000Z",
	});
	yield* sessions.ProjectObserved(
		{
			kind: "recovery",
			operation_id: "seed_checkout_session",
			sent_at: "2026-07-13T18:00:00.000Z",
			thread_id,
			workspace_id,
		},
		observation({
			branch: "main",
			checkout_calls: 0,
			commits: [],
			fail_checkout: false,
			worktrees: [],
		}),
	);
});

const RequestAndApprove = Effect.gen(function* () {
	const coordinator = yield* WorkspaceGitCheckoutCoordinator;
	const requested = yield* coordinator.Request({
		expected_session_version: 1,
		message_id: "checkout_request",
		sent_at: "2026-07-13T18:01:00.000Z",
		target_branch: "release",
		thread_id,
		workspace_id,
	});

	yield* coordinator.Respond({
		approval_id: requested.approval.approval_id,
		approved: true,
		message_id: "checkout_decision",
		sent_at: "2026-07-13T18:02:00.000Z",
		thread_id,
	});

	return requested.approval.approval_id;
});

async function wait_for_terminal(
	runtime: ManagedRuntime.ManagedRuntime<any, any>,
	approval_id: string,
) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const repository = yield* WorkspaceGitCheckoutRepository;

			for (let attempt = 0; attempt < 100; attempt += 1) {
				const result = yield* repository.Query({ approval_id, thread_id });
				const approval = result.approval;

				if (approval?.state !== "approved" && approval?.state !== "executing") {
					return approval;
				}

				yield* Effect.yieldNow;
			}

			return yield* Effect.die("Checkout coordinator did not settle deterministically");
		}),
	);
}

async function read_claims(runtime: ManagedRuntime.ManagedRuntime<any, any>) {
	return runtime.runPromise(
		Effect.flatMap(Database, (database) =>
			database.client.select().from(WorkspaceGitCheckoutClaims),
		),
	);
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

describe("WorkspaceGitCheckoutCoordinator", () => {
	it("recovers an interrupted execution as unknown, releases its claim, and never re-checks out", async () => {
		const database_path = await make_database_path();
		const state = {
			branch: "main",
			checkout_calls: 0,
			commits: [],
			fail_checkout: false,
			worktrees: [],
		};
		const first_runtime = make_runtime(database_path, state, { value: false }, false);

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const approval_id = await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceGitCheckoutRepository;

					const requested = yield* repository.Request({
						approval_id: "checkout_approval_restart",
						expected_session_version: 1,
						request_fingerprint: "c".repeat(64),
						source_command: {
							message_id: "checkout_request_restart",
							sent_at: "2026-07-13T18:01:00.000Z",
						},
						target_branch: "release",
						target_head,
						thread_id,
						workspace_id,
					});

					yield* repository.Decide({
						approval_id: requested.approval.approval_id,
						approved: true,
						decision_command: {
							message_id: "checkout_decision_restart",
							sent_at: "2026-07-13T18:02:00.000Z",
						},
						thread_id,
					});
					yield* repository.MarkExecuting(requested.approval.approval_id);

					return requested.approval.approval_id;
				}),
			);

			await first_runtime.dispose();
			const restarted_runtime = make_runtime(database_path, state);

			try {
				const approval = await wait_for_terminal(restarted_runtime, approval_id);

				expect(approval?.state).toBe("unknown");
				expect(await read_claims(restarted_runtime)).toEqual([]);
				expect(state.checkout_calls).toBe(0);
			} finally {
				await restarted_runtime.dispose();
			}
		} finally {
			await first_runtime.dispose();
		}
	});

	it("resumes an approved checkout once after restart and settles it as applied", async () => {
		const database_path = await make_database_path();
		const state = {
			branch: "main",
			checkout_calls: 0,
			commits: [],
			fail_checkout: false,
			worktrees: [],
		};
		const first_runtime = make_runtime(database_path, state, { value: false }, false);

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const approval_id = await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceGitCheckoutRepository;

					const requested = yield* repository.Request({
						approval_id: "checkout_approval_approved",
						expected_session_version: 1,
						request_fingerprint: "d".repeat(64),
						source_command: {
							message_id: "checkout_request_approved",
							sent_at: "2026-07-13T18:01:00.000Z",
						},
						target_branch: "release",
						target_head,
						thread_id,
						workspace_id,
					});

					yield* repository.Decide({
						approval_id: requested.approval.approval_id,
						approved: true,
						decision_command: {
							message_id: "checkout_decision_approved",
							sent_at: "2026-07-13T18:02:00.000Z",
						},
						thread_id,
					});

					return requested.approval.approval_id;
				}),
			);

			await first_runtime.dispose();
			const restarted_runtime = make_runtime(database_path, state);

			try {
				const approval = await wait_for_terminal(restarted_runtime, approval_id);

				expect(approval?.state).toBe("applied");
				expect(state.checkout_calls).toBe(1);
				expect(state.branch).toBe("release");
				expect(await read_claims(restarted_runtime)).toEqual([]);
			} finally {
				await restarted_runtime.dispose();
			}
		} finally {
			await first_runtime.dispose();
		}
	});

	it("rejects an approved checkout when its durable source session advances", async () => {
		const database_path = await make_database_path();
		const state = {
			branch: "main",
			checkout_calls: 0,
			commits: [],
			fail_checkout: false,
			worktrees: [],
		};
		const first_runtime = make_runtime(database_path, state, { value: false }, false);

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const approval_id = await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceGitCheckoutRepository;
					const sessions = yield* WorkspaceGitSessionService;
					const requested = yield* repository.Request({
						approval_id: "checkout_approval_stale_session",
						expected_session_version: 1,
						request_fingerprint: "e".repeat(64),
						source_command: {
							message_id: "checkout_request_stale_session",
							sent_at: "2026-07-13T18:01:00.000Z",
						},
						target_branch: "release",
						target_head,
						thread_id,
						workspace_id,
					});

					yield* repository.Decide({
						approval_id: requested.approval.approval_id,
						approved: true,
						decision_command: {
							message_id: "checkout_decision_stale_session",
							sent_at: "2026-07-13T18:02:00.000Z",
						},
						thread_id,
					});
					yield* sessions.ProjectObserved(
						{
							kind: "recovery",
							operation_id: "advance_checkout_session",
							sent_at: "2026-07-13T18:03:00.000Z",
							thread_id,
							workspace_id,
						},
						observation(state, true),
					);

					return requested.approval.approval_id;
				}),
			);

			await first_runtime.dispose();
			const restarted_runtime = make_runtime(database_path, state);

			try {
				const approval = await wait_for_terminal(restarted_runtime, approval_id);

				expect(approval?.state).toBe("rejected");
				expect(state.checkout_calls).toBe(0);
				expect(await read_claims(restarted_runtime)).toEqual([]);
			} finally {
				await restarted_runtime.dispose();
			}
		} finally {
			await first_runtime.dispose();
		}
	});

	it("releases claims when checkout fails without retaining Git artifacts", async () => {
		const state = {
			branch: "main",
			checkout_calls: 0,
			commits: [],
			fail_checkout: true,
			worktrees: [],
		};
		const runtime = make_runtime(await make_database_path(), state);

		try {
			await runtime.runPromise(SeedThreadAndSession);
			const approval_id = await runtime.runPromise(RequestAndApprove);
			const approval = await wait_for_terminal(runtime, approval_id);

			expect(approval?.state).toBe("rejected");
			expect(await read_claims(runtime)).toEqual([]);
			expect(state).toMatchObject({
				branch: "main",
				checkout_calls: 1,
				commits: [],
				worktrees: [],
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a stale preflight without issuing a checkout or retaining a claim", async () => {
		const state = {
			branch: "main",
			checkout_calls: 0,
			commits: [],
			fail_checkout: false,
			worktrees: [],
		};
		const dirty = { value: false };
		const runtime = make_runtime(await make_database_path(), state, dirty);

		try {
			await runtime.runPromise(SeedThreadAndSession);
			dirty.value = true;
			const approval_id = await runtime.runPromise(RequestAndApprove);
			const approval = await wait_for_terminal(runtime, approval_id);

			expect(approval?.state).toBe("rejected");
			expect(await read_claims(runtime)).toEqual([]);
			expect(state).toMatchObject({
				branch: "main",
				checkout_calls: 0,
				commits: [],
				worktrees: [],
			});
		} finally {
			await runtime.dispose();
		}
	});
});
