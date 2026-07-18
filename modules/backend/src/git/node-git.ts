import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";

import { Git, GitError, type GitOperation } from "./git";
import {
	make_node_git_command_executor_layer,
	type GitCommandExecutorOptions,
} from "./git-command-executor";
import { GitReadService, make_git_read_service_layer } from "./git-read-service";
import { type GitHead } from "./git-model";
import { type NodeProcessRunnerOptions } from "./node-process-runner";
import { make_workspace_git_registry_layer } from "./workspace-git-registry";

const legacy_workspace_id = "legacy_git_workspace";

/** Configures bounded Git reads for one project directory. */
export interface NodeGitOptions {
	readonly cwd: string;
	readonly max_patch_bytes?: number;
	readonly max_status_bytes?: number;
	/** Retained only for compatibility; production execution uses Effect ChildProcess. */
	readonly process?: NodeProcessRunnerOptions;
}

function git_error(operation: GitOperation, cause: unknown) {
	return new GitError({ cause, operation });
}

function branch_name(head: GitHead) {
	return head._tag === "detached" ? "" : head.branch;
}

/**
 * Builds the compatibility Git facade over the new workspace-scoped read core.
 * The caller supplies only `GitCommandExecutor`; filesystem and crypto remain
 * Effect platform services composed inside this legacy Node adapter.
 */
export function make_git_layer(options: NodeGitOptions) {
	const max_patch_bytes = options.max_patch_bytes ?? 1_000_000;
	const max_status_bytes =
		options.max_status_bytes ?? options.process?.max_stdout_bytes ?? 8 * 1024 * 1024;
	const registry = make_workspace_git_registry_layer([
		{ root: options.cwd, workspace_id: legacy_workspace_id },
	]).pipe(Layer.provideMerge(NodeFileSystem.layer));
	const read = make_git_read_service_layer({
		max_patch_bytes,
		max_status_bytes,
		...(options.process?.max_stderr_bytes === undefined
			? {}
			: { max_stderr_bytes: options.process.max_stderr_bytes }),
	}).pipe(Layer.provideMerge(registry), Layer.provideMerge(NodeCrypto.layer));

	return Layer.effect(
		Git,
		Effect.gen(function* () {
			const git = yield* GitReadService;
			const Refresh = (operation: GitOperation) =>
				git
					.Refresh(legacy_workspace_id)
					.pipe(Effect.mapError((cause) => git_error(operation, cause)));

			return {
				DiffPatch: (max_bytes?: number) =>
					git
						.ReadPatch({
							...(max_bytes === undefined ? {} : { max_bytes }),
							scope: "all",
							workspace_id: legacy_workspace_id,
						})
						.pipe(Effect.mapError((cause) => git_error("diff", cause))),
				DiffStats: Refresh("diff").pipe(
					Effect.map(({ aggregate }) => ({
						additions: aggregate.additions,
						deletions: aggregate.deletions,
						files: aggregate.files,
					})),
				),
				Discover: Refresh("discover").pipe(
					Effect.map((snapshot) => ({
						branch: branch_name(snapshot.head),
						head:
							snapshot.head._tag === "unborn"
								? Option.none<string>()
								: Option.some(snapshot.head.oid),
						root: snapshot.root,
					})),
				),
				Status: Refresh("status").pipe(
					Effect.map(({ files }) =>
						files.map((file) => ({
							conflicted: file.conflicted,
							...(file.original_path === undefined
								? {}
								: { original_path: file.original_path }),
							path: file.path,
							staged: file.staged,
							status: file.status.replaceAll(".", " "),
							untracked: file.untracked,
							unstaged: file.unstaged,
						})),
					),
				),
			};
		}),
	).pipe(Layer.provide(read));
}

/** Builds production Git reads with Effect's scoped Node child-process layer. */
export function make_node_git_layer(options: NodeGitOptions) {
	const executor_options: GitCommandExecutorOptions =
		options.process?.kill_timeout_ms === undefined
			? {}
			: { force_kill_after: `${options.process.kill_timeout_ms} millis` };

	return make_git_layer(options).pipe(
		Layer.provide(make_node_git_command_executor_layer(executor_options)),
	);
}
