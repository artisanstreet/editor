import { createHash } from "node:crypto";

import { Crypto, Effect, Layer, ManagedRuntime } from "effect";
import { describe, expect, it } from "vitest";

import type {
	GitDiffQueryEnvelope,
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
	GitMutationProjection,
	GitMutationResolveEnvelope,
	GitWorkspaceProjection,
	GitWorkspaceQueryEnvelope,
} from "@artisan/protocol";

import type { GitStatusSnapshot } from "../../modules/backend/src/git/git-model";
import {
	GitMutationDriver,
	GitMutationDriverError,
	type GitMutationRequest,
} from "../../modules/backend/src/git/git-mutation-driver";
import {
	GitRepository,
	GitRepositoryConflict,
	type GitMutationAcceptance,
	type GitMutationSuccessCommit,
	type GitWorkspaceCommit,
} from "../../modules/backend/src/git/git-repository";
import { GitReadService } from "../../modules/backend/src/git/git-read-service";
import {
	GitService,
	GitServiceError,
	GitServiceLive,
} from "../../modules/backend/src/git/git-service";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { WorkspaceEvidenceRecorder } from "../../modules/backend/src/workspace/workspace-evidence-recorder";

const observed_at = "2026-07-18T12:00:00.000Z";
const decided_at = "2026-07-18T12:01:00.000Z";
const dispatched_at = "2026-07-18T12:02:00.000Z";
const completed_at = "2026-07-18T12:03:00.000Z";
const snapshot_a = "1".repeat(64);
const snapshot_b = "2".repeat(64);
const oid_a = "a".repeat(40);
const oid_b = "b".repeat(40);

const deterministic_crypto = Crypto.make({
	digest: (algorithm, data) =>
		Effect.sync(
			() =>
				new Uint8Array(
					createHash(algorithm.toLowerCase().replaceAll("-", "")).update(data).digest(),
				),
		),
	randomBytes: (size) => new Uint8Array(size),
});

const metadata_layer = Layer.succeed(RuntimeMetadata, {
	instance_id: "backend_git_service_test",
	MakeId: (prefix) => Effect.succeed(`${prefix}_git_service_test`),
	Now: Effect.succeed(observed_at),
});

function fake_event(journal_sequence: number) {
	return { journal_sequence } as GitMutationAcceptance["event"];
}

function mutation_acceptance(
	mutation: GitMutationProjection,
	status: GitMutationAcceptance["status"] = "accepted",
): GitMutationAcceptance {
	return {
		event: fake_event(mutation.journal_sequence),
		mutation,
		status,
	};
}

function mutation(
	lifecycle: GitMutationProjection["lifecycle"],
	options: {
		readonly kind?: GitMutationProjection["kind"];
		readonly mutation_id?: string;
		readonly paths?: readonly [string, ...string[]];
		readonly snapshot_id?: string;
		readonly workspace_id?: string;
	} = {},
): GitMutationProjection {
	const kind = options.kind ?? "stage";
	const mutation_id = options.mutation_id ?? `mutation_${kind}`;
	const base = {
		approval_id: `approval_${mutation_id}`,
		expected_snapshot_id: options.snapshot_id ?? snapshot_a,
		expected_workspace_version: 3,
		journal_sequence: 20,
		kind,
		lifecycle,
		mutation_id,
		paths: options.paths ?? ([`${kind}.ts`] as const),
		requested_at: observed_at,
		source_message_id: `request_${mutation_id}`,
		thread_id: "thread_1",
		updated_at: observed_at,
		workspace_id: options.workspace_id ?? "workspace_1",
	};

	switch (lifecycle) {
		case "awaiting_approval":
			return base;
		case "approved":
			return {
				...base,
				decision_at: decided_at,
				decision_message_id: `decision_${mutation_id}`,
			};
		case "dispatching":
			return {
				...base,
				decision_at: decided_at,
				decision_message_id: `decision_${mutation_id}`,
				dispatched_at,
			};
		case "succeeded":
			return {
				...base,
				completed_at,
				decision_at: decided_at,
				decision_message_id: `decision_${mutation_id}`,
				dispatched_at,
				result_snapshot_id: snapshot_b,
				result_workspace_version: 4,
			};
		case "denied":
			return {
				...base,
				completed_at,
				decision_at: decided_at,
				decision_message_id: `decision_${mutation_id}`,
			};
		case "failed":
		case "ambiguous":
			return {
				...base,
				completed_at,
				decision_at: decided_at,
				decision_message_id: `decision_${mutation_id}`,
				dispatched_at,
				failure: { code: lifecycle === "failed" ? "git_changed" : "git_result_ambiguous" },
			};
	}
}

function snapshot(snapshot_id = snapshot_a): GitStatusSnapshot {
	return {
		aggregate: { additions: 8, binary_files: 1, deletions: 3, files: 4 },
		files: [
			{
				conflicted: false,
				index_status: "?",
				kind: "untracked",
				path: "new-file.ts",
				staged: false,
				status: "??",
				untracked: true,
				unstaged: true,
				worktree_status: "?",
			},
			{
				conflicted: false,
				index_status: ".",
				kind: "ordinary",
				path: "modified.ts",
				staged: false,
				status: ".M",
				untracked: false,
				unstaged: true,
				worktree_status: "M",
			},
			{
				conflicted: false,
				index_status: "M",
				kind: "ordinary",
				path: "staged.ts",
				staged: true,
				status: "M.",
				untracked: false,
				unstaged: false,
				worktree_status: ".",
			},
			{
				conflicted: true,
				index_status: "U",
				kind: "unmerged",
				path: "conflict.ts",
				staged: true,
				status: "UU",
				untracked: false,
				unstaged: true,
				worktree_status: "U",
			},
		],
		head: { _tag: "attached", branch: "main", oid: oid_a },
		root: "c:\\work\\repo",
		snapshot_id,
		staged: { additions: 5, binary_files: 0, deletions: 1, files: 2 },
		unstaged: { additions: 3, binary_files: 1, deletions: 2, files: 3 },
		upstream: { _tag: "tracked", ahead: 2, behind: 1, ref: "origin/main" },
		workspace_id: "workspace_1",
		worktrees: [
			{
				bare: false,
				branch: "refs/heads/main",
				current: true,
				detached: false,
				head: oid_a,
				path: "c:\\work\\repo",
			},
			{
				bare: false,
				branch: "refs/heads/feature",
				current: false,
				detached: false,
				head: oid_b,
				locked_reason: "manual lock",
				path: "c:\\work\\linked",
				prunable_reason: "missing gitdir",
			},
			{
				bare: true,
				current: false,
				detached: false,
				path: "c:\\work\\bare",
			},
		],
	};
}

function workspace(
	snapshot_id = snapshot_a,
	options: {
		readonly branch?: GitWorkspaceProjection extends infer Projection
			? Projection extends { readonly branch: infer Branch }
				? Branch
				: never
			: never;
		readonly version?: number;
	} = {},
): GitWorkspaceProjection {
	return {
		aggregate: {
			binary_file_count: 1,
			lines_added: 8,
			lines_deleted: 3,
			tracked_file_count: 4,
		},
		branch: options.branch ?? { name: "main", type: "attached" },
		clean: false,
		files: [
			{
				flags: { conflicted: false, staged: false, unstaged: true, untracked: true },
				path: "new-file.ts",
				porcelain_status: "??",
			},
			{
				flags: { conflicted: false, staged: false, unstaged: true, untracked: false },
				path: "modified.ts",
				porcelain_status: ".M",
			},
			{
				flags: { conflicted: false, staged: true, unstaged: false, untracked: false },
				path: "staged.ts",
				porcelain_status: "M.",
			},
			{
				flags: { conflicted: true, staged: true, unstaged: true, untracked: false },
				path: "conflict.ts",
				porcelain_status: "UU",
			},
		],
		head: oid_a,
		journal_sequence: 10,
		observed_at,
		repository_state: "repository",
		snapshot_id,
		staged: {
			binary_file_count: 0,
			lines_added: 5,
			lines_deleted: 1,
			tracked_file_count: 2,
		},
		unstaged: {
			binary_file_count: 1,
			lines_added: 3,
			lines_deleted: 2,
			tracked_file_count: 3,
		},
		version: options.version ?? 3,
		workspace_id: "workspace_1",
		worktrees: [
			{
				bare: false,
				branch: { name: "main", type: "attached" },
				head: oid_a,
				is_current: true,
				locked: false,
				path: "C:/work/repo",
				prunable: false,
				worktree_id: "worktree_main",
			},
		],
	};
}

function query_envelope(): GitWorkspaceQueryEnvelope {
	return {
		kind: "git.workspace.query",
		message_id: "query_1",
		origin: "frontend",
		payload: { thread_id: "thread_1", workspace_id: "workspace_1" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: observed_at,
	};
}

function diff_envelope(
	overrides: Partial<GitDiffQueryEnvelope["payload"]> = {},
): GitDiffQueryEnvelope {
	return {
		kind: "git.diff.query",
		message_id: "diff_1",
		origin: "frontend",
		payload: {
			expected_snapshot_id: snapshot_a,
			expected_workspace_version: 3,
			max_bytes: 5,
			scope: "aggregate",
			workspace_id: "workspace_1",
			...overrides,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: observed_at,
	};
}

function request_envelope(
	kind: "stage" | "unstage",
	paths: readonly [string, ...string[]],
): GitIndexStageRequestEnvelope | GitIndexUnstageRequestEnvelope {
	const mutation_id = `mutation_request_${kind}_${paths.join("_")}`;
	const common = {
		message_id: `request_${mutation_id}`,
		origin: "frontend" as const,
		payload: {
			approval_id: `approval_${mutation_id}`,
			expected_snapshot_id: snapshot_a,
			expected_workspace_version: 3,
			mutation_id,
			paths,
			workspace_id: "workspace_1",
		},
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at: observed_at,
		thread_id: "thread_1",
	};

	return kind === "stage"
		? { ...common, kind: "git.index.stage.request" }
		: { ...common, kind: "git.index.unstage.request" };
}

function resolve_envelope(mutation_id: string): GitMutationResolveEnvelope {
	return {
		kind: "git.mutation.resolve",
		message_id: `decision_${mutation_id}`,
		origin: "frontend",
		payload: {
			approval_id: `approval_${mutation_id}`,
			approved: true,
			mutation_id,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: decided_at,
		thread_id: "thread_1",
	};
}

interface HarnessOptions {
	readonly driver?: Partial<GitMutationDriver["Service"]>;
	readonly reads?: Partial<GitReadService["Service"]>;
	readonly repository?: Partial<GitRepository["Service"]>;
}

function make_harness(options: HarnessOptions = {}) {
	const calls = {
		claim_approved: [] as Array<string>,
		commit_succeeded: [] as Array<unknown>,
		commit_terminal: [] as Array<unknown>,
		evidence: [] as Array<unknown>,
		patches: [] as Array<unknown>,
		read_workspaces: [] as Array<string>,
		record_workspaces: [] as Array<unknown>,
		refreshes: [] as Array<string>,
		request_mutations: [] as Array<unknown>,
		resolve_mutations: [] as Array<unknown>,
		stage: [] as Array<GitMutationRequest>,
		unstage: [] as Array<GitMutationRequest>,
	};
	const refresh = options.reads?.Refresh;
	const read_status = options.reads?.ReadStatus;
	const read_patch = options.reads?.ReadPatch;
	const stage = options.driver?.Stage;
	const unstage = options.driver?.Unstage;
	const claim_approved = options.repository?.ClaimApproved;
	const commit_succeeded = options.repository?.CommitSucceeded;
	const commit_terminal = options.repository?.CommitTerminal;
	const list_pending = options.repository?.ListPending;
	const read_mutation = options.repository?.ReadMutation;
	const read_workspace = options.repository?.ReadWorkspace;
	const record_workspace = options.repository?.RecordWorkspace;
	const recover_dispatching = options.repository?.RecoverDispatching;
	const request_mutation = options.repository?.RequestMutation;
	const resolve_mutation = options.repository?.ResolveMutation;
	const read_service: GitReadService["Service"] = {
		ReadPatch: (request) => {
			calls.patches.push(request);

			return read_patch === undefined
				? Effect.die("Unexpected GitReadService.ReadPatch")
				: read_patch(request);
		},
		ReadStatus: (workspace_id) =>
			read_status === undefined
				? Effect.die("Unexpected GitReadService.ReadStatus")
				: read_status(workspace_id),
		Refresh: (workspace_id) => {
			calls.refreshes.push(workspace_id);

			return refresh === undefined
				? Effect.die("Unexpected GitReadService.Refresh")
				: refresh(workspace_id);
		},
	};
	const driver_service: GitMutationDriver["Service"] = {
		Stage: (request) => {
			calls.stage.push(request);

			return stage === undefined ? Effect.void : stage(request);
		},
		Unstage: (request) => {
			calls.unstage.push(request);

			return unstage === undefined ? Effect.void : unstage(request);
		},
	};
	const repository_service: GitRepository["Service"] = {
		ClaimApproved: (mutation_id) => {
			calls.claim_approved.push(mutation_id);

			return claim_approved === undefined
				? Effect.die("Unexpected GitRepository.ClaimApproved")
				: claim_approved(mutation_id);
		},
		CommitSucceeded: (input) => {
			calls.commit_succeeded.push(input);

			return commit_succeeded === undefined
				? Effect.die("Unexpected GitRepository.CommitSucceeded")
				: commit_succeeded(input);
		},
		CommitTerminal: (input) => {
			calls.commit_terminal.push(input);

			return commit_terminal === undefined
				? Effect.die("Unexpected GitRepository.CommitTerminal")
				: commit_terminal(input);
		},
		ListPending: (workspace_id) =>
			list_pending === undefined ? Effect.succeed([]) : list_pending(workspace_id),
		ReadMutation: (mutation_id) =>
			read_mutation === undefined
				? Effect.die("Unexpected GitRepository.ReadMutation")
				: read_mutation(mutation_id),
		ReadWorkspace: (workspace_id) => {
			calls.read_workspaces.push(workspace_id);

			return read_workspace === undefined
				? Effect.die("Unexpected GitRepository.ReadWorkspace")
				: read_workspace(workspace_id);
		},
		RecordWorkspace: (input) => {
			calls.record_workspaces.push(input);

			return record_workspace === undefined
				? Effect.die("Unexpected GitRepository.RecordWorkspace")
				: record_workspace(input);
		},
		RecoverDispatching: () =>
			recover_dispatching === undefined
				? Effect.succeed({ ambiguous: [], approved: [] })
				: recover_dispatching(),
		RequestMutation: (envelope) => {
			calls.request_mutations.push(envelope);

			return request_mutation === undefined
				? Effect.die("Unexpected GitRepository.RequestMutation")
				: request_mutation(envelope);
		},
		ResolveMutation: (envelope) => {
			calls.resolve_mutations.push(envelope);

			return resolve_mutation === undefined
				? Effect.die("Unexpected GitRepository.ResolveMutation")
				: resolve_mutation(envelope);
		},
	};
	const evidence_service: WorkspaceEvidenceRecorder["Service"] = {
		RecordFilesystemMutation: () => Effect.die("Unexpected filesystem evidence"),
		RecordGitWorkspaceObserved: (input) => {
			calls.evidence.push(input);

			return Effect.succeed({ event: {} as never, status: "accepted" });
		},
		RecordProcessOwnership: () => Effect.die("Unexpected process evidence"),
	};
	const dependencies = Layer.mergeAll(
		Layer.succeed(Crypto.Crypto, deterministic_crypto),
		metadata_layer,
		Layer.succeed(GitReadService, read_service),
		Layer.succeed(GitMutationDriver, driver_service),
		Layer.succeed(GitRepository, repository_service),
		Layer.succeed(WorkspaceEvidenceRecorder, evidence_service),
	);

	return {
		calls,
		runtime: ManagedRuntime.make(GitServiceLive.pipe(Layer.provide(dependencies))),
	};
}

function success_commit(
	approved: GitMutationProjection,
	result_workspace: GitWorkspaceProjection,
): GitMutationSuccessCommit {
	return {
		mutation: mutation("succeeded", {
			kind: approved.kind,
			mutation_id: approved.mutation_id,
			paths: approved.paths,
			workspace_id: approved.workspace_id,
		}),
		mutation_event: fake_event(22),
		status: "accepted",
		workspace: result_workspace,
		workspace_event: fake_event(21),
	};
}

describe("GitService", () => {
	it("maps a coherent query into durable flags, summaries, and normalized worktrees", async () => {
		let committed_workspace: GitWorkspaceProjection | undefined;
		const harness = make_harness({
			reads: { Refresh: () => Effect.succeed(snapshot()) },
			repository: {
				RecordWorkspace: (input) => {
					committed_workspace = {
						...input.workspace,
						journal_sequence: 41,
						version: 3,
					} as GitWorkspaceProjection;

					return Effect.succeed({
						event: fake_event(41),
						status: "accepted",
						workspace: committed_workspace,
					} satisfies GitWorkspaceCommit);
				},
			},
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) => service.Query(query_envelope())),
			);
			const repository =
				result.workspace.repository_state === "repository" ? result.workspace : null;

			expect(repository?.files).toMatchObject([
				{
					flags: { conflicted: false, staged: false, unstaged: true, untracked: true },
					path: "new-file.ts",
					porcelain_status: "??",
				},
				{
					flags: { conflicted: false, staged: false, unstaged: true, untracked: false },
					path: "modified.ts",
				},
				{
					flags: { conflicted: false, staged: true, unstaged: false, untracked: false },
					path: "staged.ts",
				},
				{
					flags: { conflicted: true, staged: true, unstaged: true, untracked: false },
					path: "conflict.ts",
				},
			]);
			expect(repository).toMatchObject({
				aggregate: {
					binary_file_count: 1,
					lines_added: 8,
					lines_deleted: 3,
					tracked_file_count: 4,
				},
				branch: { name: "main", type: "attached" },
				clean: false,
				head: oid_a,
				staged: { tracked_file_count: 2 },
				unstaged: { tracked_file_count: 3 },
				version: 3,
			});
			expect(repository?.worktrees).toMatchObject([
				{
					bare: false,
					branch: { name: "main", type: "attached" },
					head: oid_a,
					is_current: true,
					path: "C:/work/repo",
					worktree_id: `git-worktree:${createHash("sha256").update("C:/work/repo").digest("hex")}`,
				},
				{
					bare: false,
					branch: { name: "feature", type: "attached" },
					head: oid_b,
					is_current: false,
					locked: true,
					locked_reason: "manual lock",
					path: "C:/work/linked",
					prunable: true,
					prunable_reason: "missing gitdir",
				},
				{
					bare: true,
					is_current: false,
					path: "C:/work/bare",
				},
			]);
			expect(result.journal_sequence).toBe(41);
			expect(harness.calls.record_workspaces).toHaveLength(1);
			expect(harness.calls.evidence).toMatchObject([
				{
					changed_file_count: 4,
					has_diff: true,
					root_path: "C:/work/repo",
					worktree_path: "C:/work/repo",
				},
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("preserves exact Git filenames and omits empty lock reasons", async () => {
		const base_snapshot = snapshot();
		const odd_snapshot: GitStatusSnapshot = {
			...base_snapshot,
			files: base_snapshot.files.map((file, index) =>
				index === 1 ? { ...file, path: "literal\\backslash\nname.ts" } : file,
			),
			root: "/tmp/literal\\worktree",
			worktrees: [
				...base_snapshot.worktrees.map((worktree, index) =>
					index === 0
						? {
								...worktree,
								locked_reason: "",
								path: "/tmp/literal\\worktree",
								prunable_reason: "",
							}
						: worktree,
				),
				{
					bare: false,
					current: false,
					detached: true,
					head: oid_b,
					path: "\\\\server\\share\\repo",
				},
			],
		};
		const harness = make_harness({
			reads: { Refresh: () => Effect.succeed(odd_snapshot) },
			repository: {
				RecordWorkspace: (input) =>
					Effect.succeed({
						event: fake_event(42),
						status: "accepted",
						workspace: {
							...input.workspace,
							journal_sequence: 42,
							version: 1,
						} as GitWorkspaceProjection,
					}),
			},
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) => service.Query(query_envelope())),
			);
			const repository =
				result.workspace.repository_state === "repository" ? result.workspace : undefined;

			expect(repository?.files[1]?.path).toBe("literal\\backslash\nname.ts");
			expect(repository?.worktrees[0]).toMatchObject({
				locked: true,
				path: "/tmp/literal\\worktree",
				prunable: true,
				worktree_id: `git-worktree:${createHash("sha256")
					.update("/tmp/literal\\worktree")
					.digest("hex")}`,
			});
			expect(repository?.worktrees[0]).not.toHaveProperty("locked_reason");
			expect(repository?.worktrees[0]).not.toHaveProperty("prunable_reason");
			expect(repository?.worktrees).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						path: "//server/share/repo",
						worktree_id: `git-worktree:${createHash("sha256")
							.update("//server/share/repo")
							.digest("hex")}`,
					}),
				]),
			);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("returns a bounded aggregate diff only after matching snapshots on both sides", async () => {
		const snapshots = [snapshot(), snapshot()];
		const harness = make_harness({
			reads: {
				ReadPatch: () => Effect.succeed({ bytes: 5, patch: "patch", truncated: true }),
				Refresh: () => Effect.succeed(snapshots.shift()!),
			},
			repository: { ReadWorkspace: () => Effect.succeed(workspace()) },
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) => service.Diff(diff_envelope())),
			);

			expect(result).toEqual({
				byte_count: 5,
				format: "unified",
				format_version: 1,
				patch: "patch",
				scope: "aggregate",
				snapshot_id: snapshot_a,
				truncated: true,
				workspace_id: "workspace_1",
				workspace_version: 3,
			});
			expect(harness.calls.patches).toEqual([
				{ max_bytes: 5, scope: "all", workspace_id: "workspace_1" },
			]);
			expect(harness.calls.refreshes).toHaveLength(2);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("fences a stale durable diff before reading live Git", async () => {
		const harness = make_harness({
			repository: { ReadWorkspace: () => Effect.succeed(workspace(snapshot_b)) },
		});

		try {
			const failure = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Diff(diff_envelope()).pipe(Effect.flip),
				),
			);

			expect(failure).toEqual(
				new GitServiceError({ operation: "diff", reason: "changed", retryable: false }),
			);
			expect(harness.calls.refreshes).toHaveLength(0);
			expect(harness.calls.patches).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("fences a stale live diff before and after patch generation", async () => {
		const before = make_harness({
			reads: { Refresh: () => Effect.succeed(snapshot(snapshot_b)) },
			repository: { ReadWorkspace: () => Effect.succeed(workspace()) },
		});

		try {
			const failure = await before.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Diff(diff_envelope()).pipe(Effect.flip),
				),
			);

			expect(failure).toMatchObject({
				operation: "diff",
				reason: "changed",
				retryable: false,
			});
			expect(before.calls.patches).toHaveLength(0);
		} finally {
			await before.runtime.dispose();
		}

		const snapshots = [snapshot(), snapshot(snapshot_b)];
		const after = make_harness({
			reads: {
				ReadPatch: () => Effect.succeed({ bytes: 5, patch: "patch", truncated: false }),
				Refresh: () => Effect.succeed(snapshots.shift()!),
			},
			repository: { ReadWorkspace: () => Effect.succeed(workspace()) },
		});

		try {
			const failure = await after.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Diff(diff_envelope()).pipe(Effect.flip),
				),
			);

			expect(failure).toMatchObject({
				operation: "diff",
				reason: "changed",
				retryable: false,
			});
			expect(after.calls.patches).toHaveLength(1);
			expect(after.calls.refreshes).toHaveLength(2);
		} finally {
			await after.runtime.dispose();
		}
	});

	it("admits only paths eligible for the requested index direction", async () => {
		const accepted = mutation("awaiting_approval");
		const harness = make_harness({
			repository: {
				ReadWorkspace: () => Effect.succeed(workspace()),
				RequestMutation: () => Effect.succeed(mutation_acceptance(accepted)),
			},
		});

		try {
			const service = await harness.runtime.runPromise(GitService);

			for (const [kind, path] of [
				["stage", "new-file.ts"],
				["stage", "modified.ts"],
				["stage", "conflict.ts"],
				["unstage", "staged.ts"],
				["unstage", "conflict.ts"],
			] as const) {
				await harness.runtime.runPromise(service.Request(request_envelope(kind, [path])));
			}

			for (const [kind, path] of [
				["stage", "staged.ts"],
				["unstage", "modified.ts"],
				["stage", "missing.ts"],
			] as const) {
				const failure = await harness.runtime.runPromise(
					service.Request(request_envelope(kind, [path])).pipe(Effect.flip),
				);

				expect(failure).toMatchObject({
					operation: "request",
					reason: "invalid_path",
					retryable: false,
				});
			}

			expect(harness.calls.request_mutations).toHaveLength(5);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("rejects unstaging an unborn branch before persistence", async () => {
		const unborn = {
			...workspace(),
			branch: { name: "main", type: "unborn" as const },
			head: undefined,
		} as GitWorkspaceProjection;
		const harness = make_harness({
			repository: { ReadWorkspace: () => Effect.succeed(unborn) },
		});

		try {
			const failure = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Request(request_envelope("unstage", ["staged.ts"])).pipe(Effect.flip),
				),
			);

			expect(failure).toMatchObject({
				operation: "request",
				reason: "unsupported_state",
				retryable: false,
			});
			expect(harness.calls.request_mutations).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("preserves stable conflict semantics from the durable repository", async () => {
		const harness = make_harness({
			repository: {
				ReadWorkspace: () => Effect.succeed(workspace()),
				RequestMutation: () =>
					Effect.fail(new GitRepositoryConflict({ reason: "mutation_conflict" })),
			},
		});

		try {
			const failure = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Request(request_envelope("stage", ["modified.ts"])).pipe(Effect.flip),
				),
			);

			expect(failure).toMatchObject({
				operation: "request",
				reason: "id_conflict",
				retryable: false,
			});
		} finally {
			await harness.runtime.dispose();
		}
	});

	it.each(["stage", "unstage"] as const)(
		"dispatches an approved %s exactly once and commits its refreshed snapshot",
		async (kind) => {
			const approved = mutation("approved", { kind });
			const dispatching = mutation("dispatching", { kind });
			const expected_workspace = workspace();
			const result_workspace = workspace(snapshot_b, { version: 4 });
			const snapshots = [snapshot(), snapshot(snapshot_b)];
			const durable_workspaces = [expected_workspace, result_workspace];
			const harness = make_harness({
				reads: { Refresh: () => Effect.succeed(snapshots.shift()!) },
				repository: {
					ClaimApproved: () => Effect.succeed(mutation_acceptance(dispatching)),
					CommitSucceeded: () =>
						Effect.succeed(success_commit(approved, result_workspace)),
					ReadWorkspace: () => Effect.succeed(durable_workspaces.shift()!),
					ResolveMutation: () => Effect.succeed(mutation_acceptance(approved)),
				},
			});

			try {
				const result = await harness.runtime.runPromise(
					Effect.flatMap(GitService, (service) =>
						service.Resolve(resolve_envelope(approved.mutation_id)),
					),
				);

				expect(result.mutation.lifecycle).toBe("succeeded");
				expect(harness.calls.claim_approved).toEqual([approved.mutation_id]);
				expect(harness.calls[kind]).toEqual([
					{ paths: approved.paths, workspace_id: approved.workspace_id },
				]);
				expect(harness.calls[kind === "stage" ? "unstage" : "stage"]).toHaveLength(0);
				expect(harness.calls.commit_succeeded).toMatchObject([
					{
						mutation_id: approved.mutation_id,
						workspace: { snapshot_id: snapshot_b },
					},
				]);
				expect(harness.calls.evidence).toHaveLength(1);
			} finally {
				await harness.runtime.dispose();
			}
		},
	);

	it("terminally fails a stale approval before invoking the mutation driver", async () => {
		const approved = mutation("approved", { kind: "stage" });
		const dispatching = mutation("dispatching", { kind: "stage" });
		const failed = mutation("failed", { kind: "stage" });
		const harness = make_harness({
			reads: { Refresh: () => Effect.succeed(snapshot(snapshot_b)) },
			repository: {
				ClaimApproved: () => Effect.succeed(mutation_acceptance(dispatching)),
				CommitTerminal: () => Effect.succeed(mutation_acceptance(failed)),
				ReadWorkspace: () => Effect.succeed(workspace()),
				ResolveMutation: () => Effect.succeed(mutation_acceptance(approved)),
			},
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Resolve(resolve_envelope(approved.mutation_id)),
				),
			);

			expect(result.mutation.lifecycle).toBe("failed");
			expect(harness.calls.commit_terminal).toEqual([
				{
					failure: { code: "git_changed" },
					mutation_id: approved.mutation_id,
					state: "failed",
				},
			]);
			expect(harness.calls.stage).toHaveLength(0);
			expect(harness.calls.unstage).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("fences the durable workspace version immediately before spawning Git", async () => {
		const approved = mutation("approved", { kind: "stage" });
		const dispatching = mutation("dispatching", { kind: "stage" });
		const failed = mutation("failed", { kind: "stage" });
		const harness = make_harness({
			repository: {
				ClaimApproved: () => Effect.succeed(mutation_acceptance(dispatching)),
				CommitTerminal: () => Effect.succeed(mutation_acceptance(failed)),
				ReadWorkspace: () => Effect.succeed(workspace(snapshot_a, { version: 4 })),
				ResolveMutation: () => Effect.succeed(mutation_acceptance(approved)),
			},
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Resolve(resolve_envelope(approved.mutation_id)),
				),
			);

			expect(result.mutation.lifecycle).toBe("failed");
			expect(harness.calls.commit_terminal).toEqual([
				{
					failure: { code: "git_changed" },
					mutation_id: approved.mutation_id,
					state: "failed",
				},
			]);
			expect(harness.calls.refreshes).toHaveLength(0);
			expect(harness.calls.stage).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("records a post-claim driver failure as ambiguous", async () => {
		const approved = mutation("approved", { kind: "stage" });
		const dispatching = mutation("dispatching", { kind: "stage" });
		const ambiguous = mutation("ambiguous", { kind: "stage" });
		const harness = make_harness({
			driver: {
				Stage: () =>
					Effect.fail(
						new GitMutationDriverError({
							operation: "stage",
							reason: "command_failed",
							workspace_id: "workspace_1",
						}),
					),
			},
			reads: { Refresh: () => Effect.succeed(snapshot()) },
			repository: {
				ClaimApproved: () => Effect.succeed(mutation_acceptance(dispatching)),
				CommitTerminal: () => Effect.succeed(mutation_acceptance(ambiguous)),
				ReadWorkspace: () => Effect.succeed(workspace()),
				ResolveMutation: () => Effect.succeed(mutation_acceptance(approved)),
			},
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Resolve(resolve_envelope(approved.mutation_id)),
				),
			);

			expect(result.mutation.lifecycle).toBe("ambiguous");
			expect(harness.calls.commit_terminal).toEqual([
				{
					failure: { code: "git_stage_ambiguous" },
					mutation_id: approved.mutation_id,
					state: "ambiguous",
				},
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("does not redispatch an exact resolve retry that is already succeeded", async () => {
		const succeeded = mutation("succeeded", { kind: "stage" });
		const harness = make_harness({
			repository: {
				ReadWorkspace: () => Effect.succeed(workspace(snapshot_b, { version: 4 })),
				ResolveMutation: () => Effect.succeed(mutation_acceptance(succeeded, "duplicate")),
			},
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.flatMap(GitService, (service) =>
					service.Resolve(resolve_envelope(succeeded.mutation_id)),
				),
			);

			expect(result.status).toBe("duplicate");
			expect(harness.calls.claim_approved).toHaveLength(0);
			expect(harness.calls.refreshes).toHaveLength(0);
			expect(harness.calls.stage).toHaveLength(0);
			expect(harness.calls.unstage).toHaveLength(0);
			expect(harness.calls.evidence).toHaveLength(1);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("never replays recovered dispatching work but dispatches recovered approved work", async () => {
		const dispatching = mutation("dispatching", {
			kind: "stage",
			mutation_id: "mutation_dispatching",
			paths: ["do-not-run.ts"],
		});
		const approved = mutation("approved", {
			kind: "unstage",
			mutation_id: "mutation_approved",
			paths: ["run-once.ts"],
		});
		const claimed = mutation("dispatching", {
			kind: "unstage",
			mutation_id: approved.mutation_id,
			paths: approved.paths,
		});
		const result_workspace = workspace(snapshot_b, { version: 4 });
		const snapshots = [snapshot(), snapshot(snapshot_b)];
		const durable_workspaces = [workspace(), result_workspace];
		const harness = make_harness({
			reads: { Refresh: () => Effect.succeed(snapshots.shift()!) },
			repository: {
				ClaimApproved: () => Effect.succeed(mutation_acceptance(claimed)),
				CommitSucceeded: () => Effect.succeed(success_commit(approved, result_workspace)),
				ReadWorkspace: () => Effect.succeed(durable_workspaces.shift()!),
				RecoverDispatching: () =>
					Effect.succeed({ ambiguous: [dispatching], approved: [approved] }),
			},
		});

		try {
			await harness.runtime.runPromise(GitService);

			expect(harness.calls.claim_approved).toEqual([approved.mutation_id]);
			expect(harness.calls.stage).toHaveLength(0);
			expect(harness.calls.unstage).toEqual([
				{ paths: ["run-once.ts"], workspace_id: "workspace_1" },
			]);
			expect(harness.calls.commit_succeeded).toHaveLength(1);
		} finally {
			await harness.runtime.dispose();
		}
	});
});
