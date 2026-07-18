import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";

import type {
	GitDiffQueryEnvelope,
	GitDiffQueryResult,
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
	GitMutationProjection,
	GitMutationResolveEnvelope,
	GitWorkspaceProjection,
	GitWorkspaceQueryEnvelope,
	GitWorkspaceQueryResult,
} from "@artisan/protocol";

import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { WorkspaceEvidenceRecorder } from "../workspace/workspace-evidence-recorder";
import type { GitStatusSnapshot } from "./git-model";
import { GitMutationDriver } from "./git-mutation-driver";
import {
	GitRepository,
	GitRepositoryConflict,
	GitRepositoryInvalid,
	GitRepositoryInvariantError,
	GitRepositoryNotFound,
	GitRepositoryPersistenceFailure,
	GitWorkspaceObservation,
	type GitMutationAcceptance,
} from "./git-repository";
import { GitReadError, GitReadService } from "./git-read-service";

export type GitServiceOperation = "diff" | "query" | "request" | "resolve" | "recovery";

/** Reports a sanitized failure at the public Git orchestration boundary. */
export class GitServiceError extends Data.TaggedError("GitServiceError")<{
	readonly cause?: unknown;
	readonly operation: GitServiceOperation;
	readonly reason:
		| "busy"
		| "changed"
		| "id_conflict"
		| "invalid_path"
		| "invariant"
		| "not_repository"
		| "unsupported_state"
		| "unavailable";
	readonly retryable: boolean;
}> {}

/** Coordinates coherent Git reads and approval-bound index mutations. */
export class GitService extends Context.Service<
	GitService,
	{
		readonly Diff: (
			envelope: GitDiffQueryEnvelope,
		) => Effect.Effect<GitDiffQueryResult, GitServiceError>;
		readonly Query: (
			envelope: GitWorkspaceQueryEnvelope,
		) => Effect.Effect<GitWorkspaceQueryResult, GitServiceError>;
		readonly Request: (
			envelope: GitIndexStageRequestEnvelope | GitIndexUnstageRequestEnvelope,
		) => Effect.Effect<GitMutationAcceptance, GitServiceError>;
		readonly Resolve: (
			envelope: GitMutationResolveEnvelope,
		) => Effect.Effect<GitMutationAcceptance, GitServiceError>;
	}
>()("Artisan/GitService") {}

const service_error = (
	operation: GitServiceOperation,
	reason: GitServiceError["reason"],
	retryable: boolean,
	cause?: unknown,
) =>
	new GitServiceError({
		...(cause === undefined ? {} : { cause }),
		operation,
		reason,
		retryable,
	});

const normalize_service_error = (operation: GitServiceOperation, cause: unknown) => {
	if (cause instanceof GitServiceError) {
		return cause;
	}

	if (cause instanceof GitRepositoryConflict) {
		switch (cause.reason) {
			case "workspace_changed":
				return service_error(operation, "changed", false, cause);
			case "decision_conflict":
			case "mutation_conflict":
				return service_error(operation, "id_conflict", false, cause);
			case "workspace_busy":
				return service_error(operation, "busy", true, cause);
			case "dispatch_conflict":
			case "terminal_conflict":
				return service_error(operation, "invariant", false, cause);
			case "thread_unavailable":
				return service_error(operation, "unavailable", false, cause);
		}
	}

	if (cause instanceof GitRepositoryInvalid || cause instanceof GitRepositoryInvariantError) {
		return service_error(operation, "invariant", false, cause);
	}

	if (cause instanceof GitRepositoryNotFound) {
		return service_error(operation, "unavailable", false, cause);
	}

	if (cause instanceof GitRepositoryPersistenceFailure) {
		return service_error(operation, "unavailable", true, cause);
	}

	if (cause instanceof GitReadError) {
		switch (cause.reason) {
			case "not_repository":
				return service_error(operation, "not_repository", false, cause);
			case "snapshot_changed":
				return service_error(operation, "changed", false, cause);
			case "configuration":
			case "invalid_output":
			case "output_limit":
				return service_error(operation, "invariant", false, cause);
			case "root_changed":
			case "workspace_not_found":
				return service_error(operation, "unavailable", false, cause);
			case "command_failed":
				return service_error(operation, "unavailable", true, cause);
		}
	}

	return service_error(operation, "unavailable", true, cause);
};

const normalize_absolute_path = (path: string) => {
	const windows_path = /^[a-z]:[\\/]/iu.test(path) || path.startsWith("\\\\");
	const slash_path = windows_path ? path.replaceAll("\\", "/") : path;

	return /^[a-z]:\//u.test(slash_path)
		? `${slash_path[0]!.toUpperCase()}${slash_path.slice(1)}`
		: slash_path;
};

const branch_name = (branch: string) => branch.replace(/^refs\/heads\//u, "");

const branch_from_snapshot = (snapshot: GitStatusSnapshot) => {
	switch (snapshot.head._tag) {
		case "attached":
			return { name: snapshot.head.branch, type: "attached" as const };
		case "detached":
			return { type: "detached" as const };
		case "unborn":
			return { name: snapshot.head.branch, type: "unborn" as const };
	}
};

const head_from_snapshot = (snapshot: GitStatusSnapshot) =>
	snapshot.head._tag === "unborn" ? undefined : snapshot.head.oid;

const summary = (stats: GitStatusSnapshot["aggregate"]) => ({
	binary_file_count: stats.binary_files,
	lines_added: stats.additions,
	lines_deleted: stats.deletions,
	tracked_file_count: stats.files,
});

const observation_branch_name = (workspace: GitWorkspaceProjection) =>
	workspace.repository_state === "repository" && workspace.branch.type !== "detached"
		? workspace.branch.name
		: undefined;

/** Builds the production orchestration service over read, mutation, persistence, and evidence capabilities. */
export const GitServiceLive = Layer.effect(
	GitService,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const driver = yield* GitMutationDriver;
		const evidence = yield* WorkspaceEvidenceRecorder;
		const metadata = yield* RuntimeMetadata;
		const reads = yield* GitReadService;
		const repository = yield* GitRepository;
		const Hash = (value: string) =>
			crypto
				.digest("SHA-256", new TextEncoder().encode(value))
				.pipe(Effect.map(Encoding.encodeHex));

		const ToObservation = (snapshot: GitStatusSnapshot, observed_at: string) =>
			Effect.gen(function* () {
				const branch = branch_from_snapshot(snapshot);
				const head = head_from_snapshot(snapshot);
				const worktrees = yield* Effect.forEach(snapshot.worktrees, (worktree) =>
					Effect.gen(function* () {
						const path = normalize_absolute_path(worktree.path);
						const locked_reason = worktree.locked_reason;
						const prunable_reason = worktree.prunable_reason;
						const worktree_id = `git-worktree:${yield* Hash(path)}`;

						if (worktree.bare) {
							return {
								bare: true as const,
								is_current: false,
								locked: locked_reason !== undefined,
								...(locked_reason === undefined || locked_reason.length === 0
									? {}
									: { locked_reason }),
								path,
								prunable: prunable_reason !== undefined,
								...(prunable_reason === undefined || prunable_reason.length === 0
									? {}
									: { prunable_reason }),
								worktree_id,
							};
						}

						const worktree_branch = worktree.current
							? branch
							: worktree.detached
								? ({ type: "detached" } as const)
								: worktree.branch === undefined
									? ({ type: "detached" } as const)
									: ({
											name: branch_name(worktree.branch),
											type: "attached",
										} as const);

						return {
							bare: false as const,
							branch: worktree_branch,
							...(worktree.current && head === undefined
								? {}
								: { head: worktree.current ? head : worktree.head! }),
							is_current: worktree.current,
							locked: locked_reason !== undefined,
							...(locked_reason === undefined || locked_reason.length === 0
								? {}
								: { locked_reason }),
							path,
							prunable: prunable_reason !== undefined,
							...(prunable_reason === undefined || prunable_reason.length === 0
								? {}
								: { prunable_reason }),
							worktree_id,
						};
					}),
				);

				return yield* Schema.decodeUnknownEffect(GitWorkspaceObservation, {
					onExcessProperty: "error",
				})({
					aggregate: summary(snapshot.aggregate),
					branch,
					clean: snapshot.files.length === 0,
					files: snapshot.files.map((file) => ({
						flags: {
							conflicted: ["DD", "AU", "UD", "UA", "DU", "AA", "UU"].includes(
								file.status,
							),
							staged: file.status !== "??" && file.status[0] !== ".",
							unstaged: file.status === "??" || file.status[1] !== ".",
							untracked: file.status === "??",
						},
						...(file.original_path === undefined
							? {}
							: { original_path: file.original_path }),
						path: file.path,
						porcelain_status: file.status,
					})),
					...(head === undefined ? {} : { head }),
					observed_at,
					repository_state: "repository" as const,
					snapshot_id: snapshot.snapshot_id,
					staged: summary(snapshot.staged),
					unstaged: summary(snapshot.unstaged),
					workspace_id: snapshot.workspace_id,
					worktrees,
				});
			});

		const RecordEvidence = (
			workspace: GitWorkspaceProjection,
			trace: {
				readonly agent_id?: string;
				readonly operation_id: string;
				readonly raw_origin?: { readonly provider: string; readonly reference: string };
				readonly run_id?: string;
				readonly thread_id: string;
			},
		) =>
			workspace.repository_state === "not_repository"
				? Effect.void
				: evidence.RecordGitWorkspaceObserved({
						...(trace.agent_id === undefined ? {} : { agent_id: trace.agent_id }),
						...(observation_branch_name(workspace) === undefined
							? {}
							: { branch: observation_branch_name(workspace) }),
						changed_file_count: workspace.files.length,
						has_diff: workspace.files.length > 0,
						operation_id: trace.operation_id,
						...(trace.raw_origin === undefined ? {} : { raw_origin: trace.raw_origin }),
						root_path: workspace.worktrees.find((worktree) => worktree.is_current)!
							.path,
						...(trace.run_id === undefined ? {} : { run_id: trace.run_id }),
						thread_id: trace.thread_id,
						worktree_path: workspace.worktrees.find((worktree) => worktree.is_current)!
							.path,
					});

		const RefreshObservation = (workspace_id: string) =>
			Effect.gen(function* () {
				const observed_at = yield* metadata.Now;

				return yield* reads.Refresh(workspace_id).pipe(
					Effect.flatMap((snapshot) => ToObservation(snapshot, observed_at)),
					Effect.catch((error) =>
						error instanceof GitReadError && error.reason === "not_repository"
							? Hash(`not_repository\0${workspace_id}`).pipe(
									Effect.map(
										(snapshot_id) =>
											({
												observed_at,
												repository_state: "not_repository" as const,
												snapshot_id,
												workspace_id,
											}) satisfies GitWorkspaceObservation,
									),
								)
							: Effect.fail(error),
					),
				);
			});

		const Query = (envelope: GitWorkspaceQueryEnvelope) =>
			Effect.gen(function* () {
				const observation = yield* RefreshObservation(envelope.payload.workspace_id);
				const commit = yield* repository.RecordWorkspace({
					cause: "refresh",
					causation_id: envelope.message_id,
					correlation_id: envelope.message_id,
					thread_id: envelope.payload.thread_id,
					workspace: observation,
				});
				const pending_mutations = yield* repository.ListPending(
					envelope.payload.workspace_id,
				);
				yield* RecordEvidence(commit.workspace, {
					operation_id: `git-query:${envelope.message_id}`,
					thread_id: envelope.payload.thread_id,
				});

				return {
					journal_sequence: Math.max(
						commit.workspace.journal_sequence,
						...pending_mutations.map((mutation) => mutation.journal_sequence),
					),
					pending_mutations,
					workspace: commit.workspace,
				} satisfies GitWorkspaceQueryResult;
			}).pipe(Effect.mapError((cause) => normalize_service_error("query", cause)));

		const Diff = (envelope: GitDiffQueryEnvelope) =>
			Effect.gen(function* () {
				const durable = yield* repository.ReadWorkspace(envelope.payload.workspace_id);

				if (durable.repository_state !== "repository") {
					return yield* Effect.fail(service_error("diff", "not_repository", false));
				}

				if (
					durable.snapshot_id !== envelope.payload.expected_snapshot_id ||
					durable.version !== envelope.payload.expected_workspace_version
				) {
					return yield* Effect.fail(service_error("diff", "changed", false));
				}

				const before = yield* reads.Refresh(envelope.payload.workspace_id);

				if (before.snapshot_id !== durable.snapshot_id) {
					return yield* Effect.fail(service_error("diff", "changed", false));
				}

				const patch = yield* reads.ReadPatch({
					...(envelope.payload.max_bytes === undefined
						? {}
						: { max_bytes: envelope.payload.max_bytes }),
					scope: envelope.payload.scope === "aggregate" ? "all" : envelope.payload.scope,
					workspace_id: envelope.payload.workspace_id,
				});
				const after = yield* reads.Refresh(envelope.payload.workspace_id);

				if (after.snapshot_id !== durable.snapshot_id) {
					return yield* Effect.fail(service_error("diff", "changed", false));
				}

				return {
					byte_count: patch.bytes,
					format: "unified" as const,
					format_version: 1 as const,
					patch: patch.patch,
					scope: envelope.payload.scope,
					snapshot_id: durable.snapshot_id,
					truncated: patch.truncated,
					workspace_id: durable.workspace_id,
					workspace_version: durable.version,
				} satisfies GitDiffQueryResult;
			}).pipe(Effect.mapError((cause) => normalize_service_error("diff", cause)));

		const Request = (envelope: GitIndexStageRequestEnvelope | GitIndexUnstageRequestEnvelope) =>
			Effect.gen(function* () {
				const workspace = yield* repository.ReadWorkspace(envelope.payload.workspace_id);

				if (workspace.repository_state !== "repository") {
					return yield* Effect.fail(service_error("request", "not_repository", false));
				}

				if (
					workspace.snapshot_id !== envelope.payload.expected_snapshot_id ||
					workspace.version !== envelope.payload.expected_workspace_version
				) {
					return yield* Effect.fail(service_error("request", "changed", false));
				}

				if (
					envelope.kind === "git.index.unstage.request" &&
					workspace.branch.type === "unborn"
				) {
					return yield* Effect.fail(service_error("request", "unsupported_state", false));
				}

				const by_path = new Map(workspace.files.map((file) => [file.path, file] as const));
				const eligible = envelope.payload.paths.every((path) => {
					const file = by_path.get(path);

					return envelope.kind === "git.index.stage.request"
						? file !== undefined &&
								(file.flags.unstaged ||
									file.flags.untracked ||
									file.flags.conflicted)
						: file !== undefined && file.flags.staged;
				});

				if (!eligible) {
					return yield* Effect.fail(service_error("request", "invalid_path", false));
				}

				return yield* repository.RequestMutation(envelope);
			}).pipe(Effect.mapError((cause) => normalize_service_error("request", cause)));

		const RecordMutationEvidence = (mutation: GitMutationProjection) =>
			repository.ReadWorkspace(mutation.workspace_id).pipe(
				Effect.flatMap((workspace) =>
					RecordEvidence(workspace, {
						...(mutation.agent_id === undefined ? {} : { agent_id: mutation.agent_id }),
						operation_id: `git-mutation:${mutation.mutation_id}`,
						...(mutation.raw_origin === undefined
							? {}
							: { raw_origin: mutation.raw_origin }),
						...(mutation.run_id === undefined ? {} : { run_id: mutation.run_id }),
						thread_id: mutation.thread_id,
					}),
				),
			);

		const Dispatch = (mutation: GitMutationProjection) =>
			Effect.gen(function* () {
				yield* repository.ClaimApproved(mutation.mutation_id);
				const durable = yield* repository.ReadWorkspace(mutation.workspace_id);

				if (
					durable.snapshot_id !== mutation.expected_snapshot_id ||
					durable.version !== mutation.expected_workspace_version
				) {
					return yield* repository.CommitTerminal({
						failure: { code: "git_changed" },
						mutation_id: mutation.mutation_id,
						state: "failed",
					});
				}

				const live = yield* reads.Refresh(mutation.workspace_id).pipe(Effect.result);

				if (
					live._tag === "Failure" ||
					live.success.snapshot_id !== mutation.expected_snapshot_id
				) {
					return yield* repository.CommitTerminal({
						failure: {
							code: live._tag === "Failure" ? "git_refresh_failed" : "git_changed",
						},
						mutation_id: mutation.mutation_id,
						state: "failed",
					});
				}

				const dispatched = yield* (
					mutation.kind === "stage"
						? driver.Stage({
								paths: mutation.paths,
								workspace_id: mutation.workspace_id,
							})
						: driver.Unstage({
								paths: mutation.paths,
								workspace_id: mutation.workspace_id,
							})
				).pipe(Effect.result);

				if (dispatched._tag === "Failure") {
					return yield* repository.CommitTerminal({
						failure: { code: `git_${mutation.kind}_ambiguous` },
						mutation_id: mutation.mutation_id,
						state: "ambiguous",
					});
				}

				const observed_at = yield* metadata.Now;
				const refreshed = yield* reads.Refresh(mutation.workspace_id).pipe(
					Effect.flatMap((snapshot) => ToObservation(snapshot, observed_at)),
					Effect.result,
				);

				if (refreshed._tag === "Failure") {
					return yield* repository.CommitTerminal({
						failure: { code: "git_result_ambiguous" },
						mutation_id: mutation.mutation_id,
						state: "ambiguous",
					});
				}

				const committed = yield* repository
					.CommitSucceeded({
						mutation_id: mutation.mutation_id,
						workspace: refreshed.success,
					})
					.pipe(Effect.result);

				if (committed._tag === "Failure") {
					return yield* repository.CommitTerminal({
						failure: { code: "git_result_ambiguous" },
						mutation_id: mutation.mutation_id,
						state: "ambiguous",
					});
				}

				yield* RecordMutationEvidence(committed.success.mutation);

				return {
					event: committed.success.mutation_event,
					mutation: committed.success.mutation,
					status: committed.success.status,
				} satisfies GitMutationAcceptance;
			}).pipe(Effect.mapError((cause) => normalize_service_error("resolve", cause)));

		const Resolve = (envelope: GitMutationResolveEnvelope) =>
			Effect.gen(function* () {
				const acceptance = yield* repository.ResolveMutation(envelope);

				if (acceptance.mutation.lifecycle === "approved") {
					return yield* Dispatch(acceptance.mutation);
				}

				if (acceptance.mutation.lifecycle === "succeeded") {
					yield* RecordMutationEvidence(acceptance.mutation);
				}

				return acceptance;
			}).pipe(Effect.mapError((cause) => normalize_service_error("resolve", cause)));

		const recovery = yield* repository
			.RecoverDispatching()
			.pipe(Effect.mapError((cause) => normalize_service_error("recovery", cause)));
		yield* Effect.forEach(
			recovery.approved,
			(mutation) => Dispatch(mutation).pipe(Effect.ignore),
			{
				concurrency: 1,
				discard: true,
			},
		);

		return { Diff, Query, Request, Resolve };
	}),
);
