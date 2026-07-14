import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import {
	Cause,
	Deferred,
	Effect,
	Exit,
	FileSystem,
	Layer,
	ManagedRuntime,
	Option,
	Schema,
} from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Git } from "../../modules/backend/src/git/git";
import {
	GitMutation,
	GitMutationAttempt,
	GitMutationError,
	GitMutationPlan,
	GitMutationPreparation,
	type GitMutationActionAnchor,
	type GitMutationPlan as GitMutationPlanValue,
	type GitMutationReconciliation,
} from "../../modules/backend/src/git/git-mutation";
import {
	WorkspaceGitMutationCoordinator,
	WorkspaceGitMutationCoordinatorLive,
	type WorkspaceGitMutationRequestInput,
} from "../../modules/backend/src/git/workspace-git-mutation-coordinator";
import {
	WorkspaceGitMutationConflict,
	WorkspaceGitMutationRepository,
	WorkspaceGitMutationRepositoryLive,
} from "../../modules/backend/src/git/workspace-git-mutation-repository";
import {
	WorkspaceGitObserver,
	type WorkspaceGitObservation,
} from "../../modules/backend/src/git/workspace-git-observer";
import {
	WorkspaceGitRegistry,
	type WorkspaceGitCapability,
} from "../../modules/backend/src/git/workspace-git-registry";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import { WorkspaceGitSessionRepositoryLive } from "../../modules/backend/src/git/workspace-git-session-repository";
import {
	WorkspaceGitSessionService,
	WorkspaceGitSessionServiceLive,
} from "../../modules/backend/src/git/workspace-git-session-service";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalCommands,
	JournalEvents,
	Threads,
	WorkspaceGitMutationApprovals,
	WorkspaceGitMutationArtifacts,
	WorkspaceGitMutationClaims,
	WorkspaceGitOperations,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { WorkspaceEvidenceRecorder } from "../../modules/backend/src/workspace/workspace-evidence-recorder";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const workspace_id = "workspace_mutation";
const thread_id = "thread_mutation";
const source_head = "a".repeat(40);
const result_head = "b".repeat(40);
const target_head = "c".repeat(40);
const digest = "d".repeat(64);
let next_id = 0;
let next_time = Date.parse("2026-07-14T12:00:00.000Z");

type ReconciliationMode = "action_required" | "applied" | "outcome_unknown" | "rejected" | "source";

interface FakeMutationState {
	branch: string;
	execute_calls: number;
	execute_release?: Deferred.Deferred<void>;
	fail_execute: boolean;
	git_state: "merge" | "none" | "rebase";
	head: string;
	mode: ReconciliationMode;
	observer_head?: string;
	observer_calls: number;
	pause_after_execute: boolean;
	prepare_inputs: Array<GitMutationPreparation>;
	reconcile_attempts: Array<boolean>;
}

interface ExecuteClaimedGate {
	completed: Deferred.Deferred<void>;
	entered: Deferred.Deferred<void>;
	release: Deferred.Deferred<void>;
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-git-mutation-coordinator-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function fake_state(overrides: Partial<FakeMutationState> = {}): FakeMutationState {
	return {
		branch: "main",
		execute_calls: 0,
		fail_execute: false,
		git_state: "none",
		head: source_head,
		mode: "applied",
		observer_calls: 0,
		pause_after_execute: false,
		prepare_inputs: [],
		reconcile_attempts: [],
		...overrides,
	};
}

function make_metadata_layer(instance_id: string) {
	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_mutation_coordinator_${++next_id}`),
		Now: Effect.sync(() => new Date(next_time++).toISOString()),
	});
}

function source_proof(state: FakeMutationState) {
	return {
		branch: state.branch,
		configuration_identity: "1".repeat(64),
		head: state.head,
		index_identity: "2".repeat(64),
		repository_identity: "3".repeat(64),
		state: state.git_state,
		state_identity: "4".repeat(64),
		status_identity: "5".repeat(64),
		tracked_identity: "6".repeat(64),
		untracked_identity: "7".repeat(64),
		worktree_identity: "8".repeat(64),
	};
}

function observation(state: FakeMutationState, head: string = state.head): WorkspaceGitObservation {
	const conflicted = state.git_state !== "none";
	const changed_files = conflicted
		? [
				{
					conflicted: true,
					path: "src/conflict.ts",
					staged: false,
					status: "unmerged",
					untracked: false,
					unstaged: true,
				},
			]
		: [];

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
		diff_stats: conflicted
			? { additions: 1, deletions: 1, files: 1 }
			: { additions: 0, deletions: 0, files: 0 },
		has_diff: conflicted,
		head,
		observed_at: new Date(next_time++).toISOString(),
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

function operation_from(preparation: GitMutationPreparation) {
	return "operation" in preparation ? preparation.operation : preparation;
}

function plan_for(
	state: FakeMutationState,
	preparation: GitMutationPreparation,
): Effect.Effect<GitMutationPlanValue, GitMutationError> {
	const operation = operation_from(preparation);
	const source = source_proof(state);

	if (operation.type === "commit") {
		return Effect.succeed({
			binding: digest,
			message: operation.message,
			source,
			type: "commit",
		});
	}

	if (operation.type === "merge") {
		if (operation.action === "start") {
			return Effect.succeed({
				action: "start",
				binding: digest,
				source,
				target_branch: operation.target_branch,
				target_head,
				type: "merge",
			});
		}

		if (!("operation" in preparation) || preparation.action_anchor.type !== "merge") {
			return Effect.fail(new GitMutationError({ operation: "invalid_plan" }));
		}

		return Effect.succeed({
			action: operation.action,
			anchor: preparation.action_anchor,
			binding: digest,
			source,
			type: "merge",
		});
	}

	return Effect.fail(new GitMutationError({ operation: "invalid_plan" }));
}

function attempt_for(state: FakeMutationState, plan: GitMutationPlanValue) {
	return Schema.decodeUnknownSync(GitMutationAttempt)({
		binding: "9".repeat(64),
		exit_code: 0,
		operation_head: state.head,
		output_complete: true,
		output_identity: "a".repeat(64),
		phase: "mutation",
		plan_binding: plan.binding,
		result: source_proof(state),
		type: "attempt",
	});
}

function reconciliation_for(
	state: FakeMutationState,
	plan: GitMutationPlanValue,
): GitMutationReconciliation {
	if (state.mode === "applied") {
		return { branch: state.branch, head: state.head, type: "applied" };
	}

	if (state.mode === "action_required" && plan.type === "merge" && plan.action === "start") {
		const anchor: GitMutationActionAnchor = {
			branch: state.branch,
			identity: "b".repeat(64),
			original_head: source_head,
			plan_binding: plan.binding,
			state: "merge",
			target_head: plan.target_head,
			type: "merge",
		};

		return { action: "merge_conflict", anchor, type: "action_required" };
	}

	if (state.mode === "rejected") {
		return { reason: "git_rejected", type: "rejected" };
	}

	return { type: state.mode === "source" ? "source" : "outcome_unknown" };
}

function make_git_layers(state: FakeMutationState) {
	const read: typeof Git.Service = {
		DiffPatch: () => Effect.succeed({ bytes: 0, patch: "", truncated: false }),
		DiffStats: Effect.succeed({ additions: 0, deletions: 0, files: 0 }),
		Discover: Effect.succeed({
			branch: state.branch,
			head: Option.some(state.head),
			root: "C:/workspace",
		}),
		ProbeRepository: Effect.succeed(
			Option.some({
				branch: state.branch,
				head: Option.some(state.head),
				root: "C:/workspace",
			}),
		),
		ResolveLocalBranch: () => Effect.succeed(Option.none()),
		Status: Effect.succeed([]),
		Worktrees: Effect.succeed([]),
	};
	const mutation: typeof GitMutation.Service = {
		Prepare: (input) =>
			Schema.decodeUnknownEffect(GitMutationPreparation, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(() => new GitMutationError({ operation: "invalid_plan" })),
				Effect.tap((preparation) =>
					Effect.sync(() => state.prepare_inputs.push(preparation)),
				),
				Effect.flatMap((preparation) => plan_for(state, preparation)),
			),
		Execute: (input) =>
			Schema.decodeUnknownEffect(GitMutationPlan, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => new GitMutationError({ operation: "invalid_plan" })),
				Effect.flatMap((plan) =>
					Effect.gen(function* () {
						state.execute_calls += 1;

						if (state.fail_execute) {
							return yield* new GitMutationError({ operation: "process" });
						}

						if (state.mode === "applied") {
							state.head = result_head;
						} else if (state.mode === "action_required") {
							state.git_state = "merge";
						}

						if (state.execute_release !== undefined) {
							yield* Deferred.await(state.execute_release);
						} else if (state.pause_after_execute) {
							yield* Effect.never;
						}

						return attempt_for(state, plan);
					}),
				),
			),
		Reconcile: (input, attempt) =>
			Schema.decodeUnknownEffect(GitMutationPlan, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => new GitMutationError({ operation: "invalid_plan" })),
				Effect.tap(() =>
					Effect.sync(() => state.reconcile_attempts.push(attempt !== undefined)),
				),
				Effect.map((plan) =>
					attempt === undefined && state.mode === "applied"
						? state.head === plan.source.head
							? ({ type: "source" } as const)
							: ({ type: "outcome_unknown" } as const)
						: reconciliation_for(state, plan),
				),
			),
	};
	const capability: WorkspaceGitCapability = {
		canonical_root: "C:/workspace",
		mutation,
		read,
		workspace_id,
	};
	const observer = Layer.succeed(WorkspaceGitObserver, {
		Observe: () =>
			Effect.sync(() => {
				state.observer_calls += 1;

				return observation(state, state.observer_head ?? state.head);
			}),
	});
	const registry = Layer.succeed(WorkspaceGitRegistry, {
		Get: (requested_workspace_id) =>
			requested_workspace_id === workspace_id
				? Effect.succeed(capability)
				: Effect.die("unexpected workspace"),
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
	state: FakeMutationState,
	include_coordinator = true,
	instance_id = `workspace_git_mutation_coordinator_${++next_id}`,
	record_attempt_failures?: { remaining: number },
	execute_claimed_gate?: ExecuteClaimedGate,
): ManagedRuntime.ManagedRuntime<any, any> {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		make_metadata_layer(instance_id),
		JournalNotifierLive,
	);
	const mutation_repository = WorkspaceGitMutationRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const session_repository = WorkspaceGitSessionRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const repositories = Layer.merge(mutation_repository, session_repository);
	const coordinator_repository =
		record_attempt_failures === undefined && execute_claimed_gate === undefined
			? mutation_repository
			: Layer.effect(
					WorkspaceGitMutationRepository,
					Effect.map(WorkspaceGitMutationRepository, (repository) => ({
						...repository,
						ExecuteClaimed:
							execute_claimed_gate === undefined
								? repository.ExecuteClaimed
								: (identity, execution) =>
										Effect.gen(function* () {
											yield* Deferred.succeed(
												execute_claimed_gate.entered,
												undefined,
											);
											yield* Deferred.await(execute_claimed_gate.release);

											return yield* repository.ExecuteClaimed(
												identity,
												execution,
											);
										}).pipe(
											Effect.ensuring(
												Deferred.succeed(
													execute_claimed_gate.completed,
													undefined,
												),
											),
										),
						RecordAttempt: (identity, attempt) =>
							Effect.suspend(() => {
								if (
									record_attempt_failures !== undefined &&
									record_attempt_failures.remaining > 0
								) {
									record_attempt_failures.remaining -= 1;

									return Effect.fail(
										new WorkspaceGitMutationConflict({
											reason: "artifact_conflict",
										}),
									);
								}

								return repository.RecordAttempt(identity, attempt);
							}),
					})),
				).pipe(Layer.provide(mutation_repository));
	const { observer, registry } = make_git_layers(state);
	const support = Layer.mergeAll(NodeCrypto.layer, make_evidence_layer(), observer, registry);
	const session = WorkspaceGitSessionServiceLive.pipe(
		Layer.provide(Layer.merge(repositories, support)),
	);
	const services = Layer.mergeAll(infrastructure, repositories, support, session);
	const coordinator_services = Layer.mergeAll(
		infrastructure,
		coordinator_repository,
		support,
		session,
	);
	const application = include_coordinator
		? Layer.merge(
				services,
				WorkspaceGitMutationCoordinatorLive.pipe(Layer.provideMerge(coordinator_services)),
			)
		: services;

	return ManagedRuntime.make(application);
}

const SeedThreadAndSession = Effect.gen(function* () {
	const database = yield* Database;
	const sessions = yield* WorkspaceGitSessionService;
	const state = fake_state();

	yield* database.client.insert(Threads).values({
		created_at: "2026-07-14T12:00:00.000Z",
		thread_id,
		title: "Git mutation coordinator",
		title_source: "initial",
		updated_at: "2026-07-14T12:00:00.000Z",
	});
	yield* sessions.ProjectObserved(
		{
			kind: "recovery",
			operation_id: "seed_mutation_session",
			sent_at: "2026-07-14T12:00:00.000Z",
			thread_id,
			workspace_id,
		},
		observation(state),
	);
});

function request_input(
	overrides: Partial<WorkspaceGitMutationRequestInput> = {},
): WorkspaceGitMutationRequestInput {
	return {
		expected_session_version: 1,
		message_id: "mutation_request_1",
		operation: { message: "PRIVATE COMMIT MESSAGE", type: "commit" },
		sent_at: "2026-07-14T12:01:00.000Z",
		thread_id,
		workspace_id,
		...overrides,
	};
}

async function wait_for_terminal(
	runtime: ManagedRuntime.ManagedRuntime<any, any>,
	approval_id: string,
) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const repository = yield* WorkspaceGitMutationRepository;

			for (let attempt = 0; attempt < 500; attempt += 1) {
				const approval = (yield* repository.Query({ approval_id, thread_id })).approval;

				if (!["approved", "executing", "requested"].includes(approval.state)) {
					return approval;
				}

				yield* Effect.yieldNow;
			}

			return yield* Effect.die("Git mutation coordinator did not settle");
		}),
	);
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

async function seed_executing(
	runtime: ManagedRuntime.ManagedRuntime<any, any>,
	state: FakeMutationState,
	options: { readonly attempt: boolean; readonly reconciliation: boolean },
) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;
			const registry = yield* WorkspaceGitRegistry;
			const repository = yield* WorkspaceGitMutationRepository;

			yield* SeedThreadAndSession;

			const operation = request_input().operation;
			const capability = yield* registry.Get(workspace_id);
			const plan = yield* capability.mutation.Prepare(operation);
			yield* repository.Request({
				approval_id: "workspace_git_mutation:mutation_request_1",
				expected_session_version: 1,
				operation,
				plan,
				request_fingerprint: digest,
				source_command: {
					message_id: "mutation_request_1",
					sent_at: "2026-07-14T12:01:00.000Z",
				},
				thread_id,
				workspace_id,
			});
			yield* repository.Decide({
				approval_id: "workspace_git_mutation:mutation_request_1",
				approved: true,
				decision_command: {
					message_id: "mutation_decision_1",
					sent_at: "2026-07-14T12:02:00.000Z",
				},
				thread_id,
			});
			yield* repository.MarkExecuting("workspace_git_mutation:mutation_request_1");

			let execution = yield* repository.ReadExecution(
				"workspace_git_mutation:mutation_request_1",
			);

			if (options.attempt) {
				state.head = result_head;

				const attempt = attempt_for(state, plan);

				yield* repository.RecordAttempt(
					{
						approval_id: execution.approval.approval_id,
						claim_token: execution.claim_token,
					},
					attempt,
				);
				execution = { ...execution, attempt };
			}

			if (options.reconciliation) {
				const reconciliation = reconciliation_for(state, plan);

				yield* repository.RecordReconciliation(
					{
						approval_id: execution.approval.approval_id,
						claim_token: execution.claim_token,
					},
					reconciliation,
				);
				execution = { ...execution, reconciliation };
			}

			yield* database.client
				.update(WorkspaceGitMutationClaims)
				.set({ lease_expires_at: "1970-01-01T00:00:00.000Z" });

			return execution;
		}),
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

describe("WorkspaceGitMutationCoordinator", () => {
	it("replays private intent before prepare, then executes and settles an approved mutation once", async () => {
		const state = fake_state();
		const runtime = make_runtime(await make_database_path(), state);

		try {
			await runtime.runPromise(SeedThreadAndSession);
			const requested = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			const replay = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			const changed = await runtime.runPromiseExit(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(
						request_input({
							operation: { message: "CHANGED PRIVATE INTENT", type: "commit" },
						}),
					),
				),
			);

			expect(replay).toEqual({ ...requested, status: "duplicate" });
			expect(failure_from(changed)).toMatchObject({
				_tag: "WorkspaceGitMutationConflict",
				reason: "request_conflict",
			});
			expect(state.prepare_inputs).toHaveLength(1);

			await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "mutation_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);

			const terminal = await wait_for_terminal(runtime, requested.approval.approval_id);
			const rows = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						artifacts: yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts),
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client.select().from(WorkspaceGitOperations),
					};
				}),
			);

			expect(terminal).toMatchObject({ resulting_head: result_head, state: "applied" });
			expect(state.execute_calls).toBe(1);
			expect(state.reconcile_attempts).toEqual([true]);
			expect(rows.claims).toEqual([]);
			expect(rows.artifacts[0]?.attempt_json).not.toBeNull();
			expect(rows.artifacts[0]?.reconciliation_json).toContain('"type":"applied"');
			expect(rows.operations.some(({ kind }) => kind === "mutation")).toBe(true);
			expect(JSON.stringify(rows.commands)).not.toContain("PRIVATE COMMIT MESSAGE");
			expect(JSON.stringify(rows.commands)).not.toContain("request_fingerprint");
			expect(JSON.stringify(rows.events)).not.toContain("PRIVATE COMMIT MESSAGE");
		} finally {
			await runtime.dispose();
		}
	});

	it("downgrades an applied result when the durable projection observed a different head", async () => {
		const state = fake_state();
		const runtime = make_runtime(await make_database_path(), state);

		try {
			await runtime.runPromise(SeedThreadAndSession);
			state.observer_head = target_head;

			const requested = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "mutation_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);

			const terminal = await wait_for_terminal(runtime, requested.approval.approval_id);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const sessions = yield* WorkspaceGitSessionService;

					return {
						artifacts: yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts),
						session: (yield* sessions.Query({ workspace_id })).session,
					};
				}),
			);

			expect(terminal).toMatchObject({
				reason: "verification_failed",
				state: "outcome_unknown",
			});
			expect(result.session?.head).toBe(target_head);
			expect(result.artifacts[0]?.reconciliation_json).toContain('"type":"outcome_unknown"');
		} finally {
			await runtime.dispose();
		}
	});

	it("recovers without executing again after the coordinator shuts down past the Git mutation", async () => {
		const database_path = await make_database_path();
		const state = fake_state({ pause_after_execute: true });
		const first_runtime = make_runtime(database_path, state);

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const requested = await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "mutation_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);
			await first_runtime.runPromise(
				Effect.gen(function* () {
					for (let attempt = 0; attempt < 500; attempt += 1) {
						if (state.execute_calls === 1 && state.head === result_head) {
							return;
						}

						yield* Effect.yieldNow;
					}

					return yield* Effect.die("Git mutation did not reach the crash window");
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		state.mode = "outcome_unknown";
		state.pause_after_execute = false;
		state.reconcile_attempts.length = 0;
		const restarted = make_runtime(database_path, state);

		try {
			const terminal = await wait_for_terminal(
				restarted,
				"workspace_git_mutation:mutation_request_1",
			);
			const artifacts = await restarted.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(WorkspaceGitMutationArtifacts),
				),
			);

			expect(terminal).toMatchObject({
				reason: "verification_failed",
				state: "outcome_unknown",
			});
			expect(state.execute_calls).toBe(1);
			expect(state.reconcile_attempts).toEqual([false]);
			expect(artifacts[0]?.attempt_json).toBeNull();
		} finally {
			await restarted.dispose();
		}
	});

	it("quarantines an incomplete process execution and keeps the workspace fenced", async () => {
		const database_path = await make_database_path();
		const state = fake_state();
		const first_runtime = make_runtime(database_path, state, false, "crashed_owner");

		try {
			await seed_executing(first_runtime, state, { attempt: false, reconciliation: false });
			await first_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.update(WorkspaceGitMutationClaims).set({
						execution_started_at: "2026-07-14T12:03:00.000Z",
						lease_expires_at: "1970-01-01T00:00:00.000Z",
					}),
				),
			);
		} finally {
			await first_runtime.dispose();
		}

		const restarted = make_runtime(database_path, state, true, "recovery_owner");

		try {
			const terminal = await wait_for_terminal(
				restarted,
				"workspace_git_mutation:mutation_request_1",
			);
			const rows = await restarted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						artifacts: yield* database.client
							.select()
							.from(WorkspaceGitMutationArtifacts),
						claims: yield* database.client.select().from(WorkspaceGitMutationClaims),
					};
				}),
			);
			expect(terminal).toMatchObject({ reason: "interrupted", state: "outcome_unknown" });
			expect(rows.claims).toMatchObject([
				{
					execution_completed_at: null,
					execution_started_at: "2026-07-14T12:03:00.000Z",
				},
			]);
			expect(rows.artifacts[0]?.reconciliation_json).toContain('"type":"outcome_unknown"');
			expect(state.execute_calls).toBe(0);
			expect(state.reconcile_attempts).toEqual([]);
		} finally {
			await restarted.dispose();
		}
	});

	it("reconciles without executing again when the attempt receipt cannot be persisted", async () => {
		const database_path = await make_database_path();
		const record_attempt_failures = { remaining: 1 };
		const state = fake_state();
		const runtime = make_runtime(
			database_path,
			state,
			true,
			"coordinator_attempt_fault",
			record_attempt_failures,
		);

		try {
			await runtime.runPromise(SeedThreadAndSession);
			const requested = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "mutation_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					for (let attempt = 0; attempt < 500; attempt += 1) {
						if (state.execute_calls === 1 && record_attempt_failures.remaining === 0) {
							return;
						}

						yield* Effect.yieldNow;
					}

					return yield* Effect.die("Git mutation did not reach the receipt fault");
				}),
			);
			await runtime.runPromise(
				Effect.flatMap(
					WorkspaceGitMutationCoordinator,
					(coordinator) => coordinator.Recover,
				),
			);

			const terminal = await wait_for_terminal(runtime, requested.approval.approval_id);
			const artifacts = await runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(WorkspaceGitMutationArtifacts),
				),
			);

			expect(terminal).toMatchObject({
				reason: "verification_failed",
				state: "outcome_unknown",
			});
			expect(state.execute_calls).toBe(1);
			expect(state.reconcile_attempts).toEqual([false]);
			expect(artifacts[0]?.attempt_json).toBeNull();
		} finally {
			await runtime.dispose();
		}
	});

	it("fences a stale owner before Git execution after another runtime recovers", async () => {
		const database_path = await make_database_path();
		const execute_claimed_gate = {
			completed: await Effect.runPromise(Deferred.make<void>()),
			entered: await Effect.runPromise(Deferred.make<void>()),
			release: await Effect.runPromise(Deferred.make<void>()),
		};
		const state = fake_state();
		const first_runtime = make_runtime(
			database_path,
			state,
			true,
			"coordinator_stale_owner_a",
			undefined,
			execute_claimed_gate,
		);
		let second_runtime: ReturnType<typeof make_runtime> | undefined;

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const requested = await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "mutation_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);
			await Effect.runPromise(Deferred.await(execute_claimed_gate.entered));

			second_runtime = make_runtime(
				database_path,
				state,
				true,
				"coordinator_recovery_owner_b",
			);
			await second_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client
						.update(WorkspaceGitMutationClaims)
						.set({ lease_expires_at: "1970-01-01T00:00:00.000Z" }),
				),
			);
			await second_runtime.runPromise(
				Effect.flatMap(
					WorkspaceGitMutationCoordinator,
					(coordinator) => coordinator.Recover,
				),
			);

			const terminal = await wait_for_terminal(
				second_runtime,
				requested.approval.approval_id,
			);

			expect(terminal).toMatchObject({ reason: "interrupted", state: "outcome_unknown" });

			await Effect.runPromise(Deferred.succeed(execute_claimed_gate.release, undefined));
			await Effect.runPromise(Deferred.await(execute_claimed_gate.completed));

			expect(state.execute_calls).toBe(0);
		} finally {
			await Effect.runPromise(Deferred.succeed(execute_claimed_gate.release, undefined));
			await first_runtime.dispose();

			if (second_runtime !== undefined) {
				await second_runtime.dispose();
			}
		}
	});

	it("does not recover an expired execution while its Git execution gate is held", async () => {
		const database_path = await make_database_path();
		const execute_release = await Effect.runPromise(Deferred.make<void>());
		const state = fake_state({ execute_release });
		const first_runtime = make_runtime(database_path, state, true, "coordinator_owner_a");
		let second_runtime: ReturnType<typeof make_runtime> | undefined;

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const requested = await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(request_input()),
				),
			);
			await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "mutation_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);
			await first_runtime.runPromise(
				Effect.gen(function* () {
					while (state.execute_calls === 0) {
						yield* Effect.yieldNow;
					}
				}),
			);

			second_runtime = make_runtime(database_path, state, true, "coordinator_observer_b");
			await second_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client
						.update(WorkspaceGitMutationClaims)
						.set({ lease_expires_at: "1970-01-01T00:00:00.000Z" }),
				),
			);
			await second_runtime.runPromise(
				Effect.flatMap(
					WorkspaceGitMutationCoordinator,
					(coordinator) => coordinator.Recover,
				),
			);

			const active = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceGitMutationRepository;
					const approval = (yield* repository.Query({
						approval_id: requested.approval.approval_id,
						thread_id,
					})).approval;
					const claims = yield* database.client.select().from(WorkspaceGitMutationClaims);

					return { approval, claims };
				}),
			);

			expect(active.approval.state).toBe("executing");
			expect(active.claims).toMatchObject([
				{
					owner_instance_id: "coordinator_owner_a",
				},
			]);
			expect(state.reconcile_attempts).toEqual([]);

			await Effect.runPromise(Deferred.succeed(execute_release, undefined));

			const terminal = await wait_for_terminal(first_runtime, requested.approval.approval_id);

			expect(terminal.state).toBe("applied");
			expect(state.execute_calls).toBe(1);
			expect(state.reconcile_attempts).toEqual([true]);
		} finally {
			await first_runtime.dispose();

			if (second_runtime !== undefined) {
				await second_runtime.dispose();
			}
		}
	});

	it("recovers an executing attempt without executing it again", async () => {
		const database_path = await make_database_path();
		const state = fake_state();
		const first_runtime = make_runtime(database_path, state, false);

		try {
			await seed_executing(first_runtime, state, { attempt: true, reconciliation: false });
		} finally {
			await first_runtime.dispose();
		}

		state.prepare_inputs.length = 0;
		state.reconcile_attempts.length = 0;
		const restarted = make_runtime(database_path, state);

		try {
			const terminal = await wait_for_terminal(
				restarted,
				"workspace_git_mutation:mutation_request_1",
			);

			expect(terminal.state).toBe("applied");
			expect(state.execute_calls).toBe(0);
			expect(state.prepare_inputs).toEqual([]);
			expect(state.reconcile_attempts).toEqual([true]);
		} finally {
			await restarted.dispose();
		}
	});

	it("reconciles an interrupted execution without inventing an attempt", async () => {
		const database_path = await make_database_path();
		const state = fake_state({ mode: "source" });
		const first_runtime = make_runtime(database_path, state, false);

		try {
			await seed_executing(first_runtime, state, { attempt: false, reconciliation: false });
		} finally {
			await first_runtime.dispose();
		}

		state.prepare_inputs.length = 0;
		const restarted = make_runtime(database_path, state);

		try {
			const terminal = await wait_for_terminal(
				restarted,
				"workspace_git_mutation:mutation_request_1",
			);
			const artifacts = await restarted.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(WorkspaceGitMutationArtifacts),
				),
			);

			expect(terminal).toMatchObject({ reason: "interrupted", state: "outcome_unknown" });
			expect(state.execute_calls).toBe(0);
			expect(state.reconcile_attempts).toEqual([false]);
			expect(artifacts[0]?.attempt_json).toBeNull();
		} finally {
			await restarted.dispose();
		}
	});

	it("replays a persisted projection after a crash and settles without reconcile or observe", async () => {
		const database_path = await make_database_path();
		const state = fake_state();
		const first_runtime = make_runtime(database_path, state, false);

		try {
			const execution = await seed_executing(first_runtime, state, {
				attempt: true,
				reconciliation: true,
			});

			await first_runtime.runPromise(
				Effect.flatMap(WorkspaceGitSessionService, (sessions) =>
					sessions.ProjectObserved(
						{
							kind: "mutation",
							operation_id: `workspace_git_mutation:${execution.approval.approval_id}:projection`,
							sent_at: execution.approval.updated_at,
							thread_id,
							workspace_id,
						},
						observation(state),
					),
				),
			);
		} finally {
			await first_runtime.dispose();
		}

		state.observer_calls = 0;
		state.prepare_inputs.length = 0;
		state.reconcile_attempts.length = 0;
		const restarted = make_runtime(database_path, state);

		try {
			const terminal = await wait_for_terminal(
				restarted,
				"workspace_git_mutation:mutation_request_1",
			);
			const session = await restarted.runPromise(
				Effect.flatMap(WorkspaceGitSessionService, (sessions) =>
					sessions.Query({ workspace_id }),
				),
			);

			expect(terminal.state).toBe("applied");
			expect(session.session?.version).toBe(2);
			expect(state.execute_calls).toBe(0);
			expect(state.observer_calls).toBe(0);
			expect(state.reconcile_attempts).toEqual([]);
		} finally {
			await restarted.dispose();
		}
	});

	it("binds one continuation to its action anchor before prepare", async () => {
		const state = fake_state({ mode: "action_required" });
		const runtime = make_runtime(await make_database_path(), state);

		try {
			await runtime.runPromise(SeedThreadAndSession);
			const parent = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(
						request_input({
							message_id: "merge_request_1",
							operation: { action: "start", target_branch: "feature", type: "merge" },
						}),
					),
				),
			);
			await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Respond({
						approval_id: parent.approval.approval_id,
						approved: true,
						message_id: "merge_decision_1",
						sent_at: "2026-07-14T12:02:00.000Z",
						thread_id,
					}),
				),
			);
			const terminal = await wait_for_terminal(runtime, parent.approval.approval_id);

			expect(terminal.state).toBe("action_required");

			const continuation_input = request_input({
				action_approval_id: parent.approval.approval_id,
				expected_session_version: 2,
				message_id: "merge_continue_1",
				operation: { action: "continue", type: "merge" },
				sent_at: "2026-07-14T12:03:00.000Z",
			});
			const continuation = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(continuation_input),
				),
			);
			const replay = await runtime.runPromise(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request(continuation_input),
				),
			);
			const second = await runtime.runPromiseExit(
				Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
					coordinator.Request({
						...continuation_input,
						message_id: "merge_continue_2",
						sent_at: "2026-07-14T12:04:00.000Z",
					}),
				),
			);

			expect(replay).toEqual({ ...continuation, status: "duplicate" });
			expect(failure_from(second)).toBeInstanceOf(WorkspaceGitMutationConflict);
			expect(state.prepare_inputs).toHaveLength(2);
			expect(state.prepare_inputs[1]).toMatchObject({
				action_anchor: { plan_binding: digest, type: "merge" },
				operation: { action: "continue", type: "merge" },
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes two live coordinators into one execution", async () => {
		const database_path = await make_database_path();
		const state = fake_state();
		const first_runtime = make_runtime(database_path, state);
		const second_runtime = make_runtime(database_path, state);

		try {
			await first_runtime.runPromise(SeedThreadAndSession);
			const request = request_input({ message_id: "concurrent_request_1" });
			const [first_request, second_request] = await Promise.all([
				first_runtime.runPromise(
					Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
						coordinator.Request(request),
					),
				),
				second_runtime.runPromise(
					Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
						coordinator.Request(request),
					),
				),
			]);
			const decision = {
				approval_id: first_request.approval.approval_id,
				approved: true,
				message_id: "concurrent_decision_1",
				sent_at: "2026-07-14T12:02:00.000Z",
				thread_id,
			};

			await Promise.all([
				first_runtime.runPromise(
					Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
						coordinator.Respond(decision),
					),
				),
				second_runtime.runPromise(
					Effect.flatMap(WorkspaceGitMutationCoordinator, (coordinator) =>
						coordinator.Respond(decision),
					),
				),
			]);
			const terminal = await wait_for_terminal(first_runtime, decision.approval_id);
			const approvals = await first_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(WorkspaceGitMutationApprovals),
				),
			);

			expect([first_request.status, second_request.status].toSorted()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(terminal.state).toBe("applied");
			expect(approvals).toHaveLength(1);
			expect(state.execute_calls).toBe(1);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});
});
