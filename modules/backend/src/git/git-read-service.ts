import { Context, Crypto, Data, Effect, Layer, Schema } from "effect";

import { type GitCommandResult } from "./git-command-executor";
import {
	GitDiffPatch,
	GitPatchRequest,
	GitStatusSnapshot,
	type GitDiffScope,
	type GitHead,
} from "./git-model";
import { GitParseError, ParseGitNumstat, ParseGitStatus, ParseGitWorktrees } from "./git-parsers";
import {
	WorkspaceGitNotFoundError,
	WorkspaceGitRegistry,
	type WorkspaceGit,
	WorkspaceGitRootChangedError,
} from "./workspace-git-registry";

export type GitReadOperation = "configuration" | "patch" | "snapshot";

/** Reports bounded Git read failures without widening the read service into mutation. */
export class GitReadError extends Data.TaggedError("GitReadError")<{
	readonly cause?: unknown;
	readonly operation: GitReadOperation;
	readonly reason:
		| "command_failed"
		| "configuration"
		| "invalid_output"
		| "not_repository"
		| "output_limit"
		| "root_changed"
		| "snapshot_changed"
		| "workspace_not_found";
	readonly workspace_id: string;
}> {}

/** Provides coherent, bounded Git reads for opaque registered workspaces. */
export class GitReadService extends Context.Service<
	GitReadService,
	{
		readonly ReadPatch: (
			request: typeof GitPatchRequest.Type,
		) => Effect.Effect<typeof GitDiffPatch.Type, GitReadError>;
		readonly ReadStatus: (
			workspace_id: string,
		) => Effect.Effect<typeof GitStatusSnapshot.Type, GitReadError>;
		readonly Refresh: (
			workspace_id: string,
		) => Effect.Effect<typeof GitStatusSnapshot.Type, GitReadError>;
	}
>()("Artisan/GitReadService") {}

const GitReadServiceConfiguration = Schema.Struct({
	max_patch_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(32 * 1024 * 1024),
	),
	max_status_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(1),
		Schema.isLessThanOrEqualTo(32 * 1024 * 1024),
	),
	max_stderr_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(1),
		Schema.isLessThanOrEqualTo(32 * 1024 * 1024),
	),
	max_worktree_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(1),
		Schema.isLessThanOrEqualTo(32 * 1024 * 1024),
	),
});

export interface GitReadServiceOptions {
	readonly max_patch_bytes?: number;
	readonly max_status_bytes?: number;
	readonly max_stderr_bytes?: number;
	readonly max_worktree_bytes?: number;
}

const text_decoder = new TextDecoder("utf-8");
const text_encoder = new TextEncoder();
const hash_object_batch_maximum_arguments = 128;
const hash_object_batch_maximum_bytes = 24 * 1024;

function read_error(
	workspace_id: string,
	operation: GitReadOperation,
	reason: GitReadError["reason"],
	cause?: unknown,
) {
	if (cause instanceof GitReadError) {
		return cause;
	}

	return new GitReadError({
		...(cause === undefined ? {} : { cause }),
		operation,
		reason,
		workspace_id,
	});
}

function map_read_error(workspace_id: string, operation: GitReadOperation, cause: unknown) {
	if (cause instanceof WorkspaceGitNotFoundError) {
		return read_error(workspace_id, operation, "workspace_not_found");
	}

	if (cause instanceof WorkspaceGitRootChangedError) {
		return read_error(workspace_id, operation, "root_changed");
	}

	if (cause instanceof GitParseError) {
		return read_error(workspace_id, operation, "invalid_output", cause);
	}

	return read_error(workspace_id, operation, "command_failed", cause);
}

function stderr_message(result: GitCommandResult) {
	return result.stderr.truncated
		? `Git exited ${result.exit_code} with stderr exceeding its byte limit`
		: `Git exited ${result.exit_code}: ${text_decoder.decode(result.stderr.bytes).trim()}`;
}

const RunChecked = (
	git: WorkspaceGit,
	workspace_id: string,
	operation: GitReadOperation,
	args: ReadonlyArray<string>,
	max_stdout_bytes: number,
	max_stderr_bytes: number,
) =>
	git
		.Run({
			args,
			max_stderr_bytes,
			max_stdin_bytes: 0,
			max_stdout_bytes,
			mode: "read",
		})
		.pipe(
			Effect.mapError((cause) => map_read_error(workspace_id, operation, cause)),
			Effect.flatMap((result) => {
				if (result.termination !== undefined) {
					return Effect.fail(
						read_error(
							workspace_id,
							operation,
							result.termination === "output_limit"
								? "output_limit"
								: "command_failed",
						),
					);
				}

				if (result.exit_code !== 0) {
					return Effect.fail(
						read_error(
							workspace_id,
							operation,
							"command_failed",
							new Error(stderr_message(result)),
						),
					);
				}

				if (result.stdout.truncated) {
					return Effect.fail(read_error(workspace_id, operation, "output_limit"));
				}

				return Effect.succeed(result.stdout.bytes);
			}),
		);

function batch_hash_paths(paths: ReadonlyArray<string>) {
	const batches: Array<Array<string>> = [];
	let batch: Array<string> = [];
	let bytes = 0;

	for (const path of paths) {
		const path_bytes = text_encoder.encode(path).byteLength + 1;

		if (
			batch.length > 0 &&
			(batch.length >= hash_object_batch_maximum_arguments ||
				bytes + path_bytes > hash_object_batch_maximum_bytes)
		) {
			batches.push(batch);
			batch = [];
			bytes = 0;
		}

		batch.push(path);
		bytes += path_bytes;
	}

	if (batch.length > 0) {
		batches.push(batch);
	}

	return batches;
}

const HashUntrackedFiles = (
	git: WorkspaceGit,
	workspace_id: string,
	paths: ReadonlyArray<string>,
	max_stdout_bytes: number,
	max_stderr_bytes: number,
) => {
	if (paths.length === 0) {
		return Effect.succeed(new Uint8Array());
	}

	return Effect.forEach(batch_hash_paths(paths), (batch) =>
		RunChecked(
			git,
			workspace_id,
			"snapshot",
			["hash-object", "--no-filters", "--", ...batch],
			max_stdout_bytes,
			max_stderr_bytes,
		),
	).pipe(Effect.map(length_prefixed));
};

const ProbeRepository = (
	git: WorkspaceGit,
	workspace_id: string,
	operation: GitReadOperation,
	max_stderr_bytes: number,
) =>
	git
		.Run({
			args: ["rev-parse", "--is-inside-work-tree"],
			max_stderr_bytes,
			max_stdin_bytes: 0,
			max_stdout_bytes: 1024,
			mode: "read",
		})
		.pipe(
			Effect.mapError((cause) => map_read_error(workspace_id, operation, cause)),
			Effect.flatMap((result) => {
				if (result.termination !== undefined) {
					return Effect.fail(
						read_error(
							workspace_id,
							operation,
							result.termination === "output_limit"
								? "output_limit"
								: "command_failed",
						),
					);
				}

				if (result.stdout.truncated) {
					return Effect.fail(read_error(workspace_id, operation, "output_limit"));
				}

				if (result.exit_code === 0) {
					const value = text_decoder.decode(result.stdout.bytes).trim();

					return value === "true"
						? Effect.void
						: value === "false"
							? Effect.fail(read_error(workspace_id, operation, "not_repository"))
							: Effect.fail(read_error(workspace_id, operation, "invalid_output"));
				}

				const stderr = text_decoder.decode(result.stderr.bytes);

				return /not a git repository/iu.test(stderr)
					? Effect.fail(read_error(workspace_id, operation, "not_repository"))
					: Effect.fail(
							read_error(
								workspace_id,
								operation,
								"command_failed",
								new Error(stderr_message(result)),
							),
						);
			}),
		);

function bytes_equal(left: Uint8Array, right: Uint8Array) {
	if (left.byteLength !== right.byteLength) {
		return false;
	}

	return left.every((byte, index) => byte === right[index]);
}

function diff_reference(head: GitHead, scope: GitDiffScope) {
	if (scope === "unstaged") {
		return [];
	}

	if (scope === "staged") {
		return head._tag === "unborn" ? ["--cached"] : ["--cached", "HEAD"];
	}

	return head._tag === "unborn" ? ["--cached"] : ["HEAD"];
}

function numstat_args(head: GitHead, scope: GitDiffScope) {
	return [
		"-c",
		"core.quotePath=true",
		"diff",
		"--no-ext-diff",
		"--no-textconv",
		"--numstat",
		"-z",
		...diff_reference(head, scope),
	];
}

function content_diff_args(head: GitHead, scope: GitDiffScope) {
	return [
		"-c",
		"core.quotePath=true",
		"diff",
		"--no-ext-diff",
		"--no-textconv",
		"--binary",
		...diff_reference(head, scope),
	];
}

function normalized_worktree_identity(path: string) {
	const windows_path = /^[a-z]:[\\/]/iu.test(path) || path.startsWith("\\\\");
	const slash_path = (windows_path ? path.replaceAll("\\", "/") : path).replace(/\/$/u, "");

	return /^[a-z]:/iu.test(slash_path)
		? `${slash_path[0]!.toLowerCase()}${slash_path.slice(1)}`
		: slash_path;
}

function length_prefixed(chunks: ReadonlyArray<Uint8Array>) {
	const byte_count = chunks.reduce((total, chunk) => total + 8 + chunk.byteLength, 0);
	const output = new Uint8Array(byte_count);
	const view = new DataView(output.buffer);
	let offset = 0;

	for (const chunk of chunks) {
		view.setBigUint64(offset, BigInt(chunk.byteLength));
		offset += 8;
		output.set(chunk, offset);
		offset += chunk.byteLength;
	}

	return output;
}

function hex(bytes: Uint8Array) {
	return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function utf8_sequence_length(byte: number) {
	if ((byte & 0b1000_0000) === 0) {
		return 1;
	}

	if ((byte & 0b1110_0000) === 0b1100_0000) {
		return 2;
	}

	if ((byte & 0b1111_0000) === 0b1110_0000) {
		return 3;
	}

	return (byte & 0b1111_1000) === 0b1111_0000 ? 4 : 1;
}

function complete_utf8_prefix_length(bytes: Uint8Array) {
	if (bytes.byteLength === 0) {
		return 0;
	}

	let lead_index = bytes.byteLength - 1;

	while (lead_index >= 0 && (bytes[lead_index]! & 0b1100_0000) === 0b1000_0000) {
		lead_index -= 1;
	}

	if (lead_index < 0) {
		return 0;
	}

	return bytes.byteLength - lead_index < utf8_sequence_length(bytes[lead_index]!)
		? lead_index
		: bytes.byteLength;
}

function decode_bounded_patch(result: GitCommandResult) {
	const prefix_length = complete_utf8_prefix_length(result.stdout.bytes);
	const prefix = result.stdout.bytes.subarray(0, prefix_length);
	const patch = text_decoder.decode(prefix);
	const bytes = new TextEncoder().encode(patch).byteLength;

	return {
		bytes,
		patch,
		truncated:
			prefix_length < result.stdout.bytes.byteLength ||
			result.stdout.truncated ||
			result.stdout.total_bytes > bytes,
	};
}

/** Builds the read-only service over registered workspace Git capabilities. */
export function make_git_read_service_layer(options: GitReadServiceOptions = {}) {
	const configuration = {
		max_patch_bytes: options.max_patch_bytes ?? 1_000_000,
		max_status_bytes: options.max_status_bytes ?? 8 * 1024 * 1024,
		max_stderr_bytes: options.max_stderr_bytes ?? 256 * 1024,
		max_worktree_bytes: options.max_worktree_bytes ?? 1024 * 1024,
	};

	return Layer.effect(
		GitReadService,
		Effect.gen(function* () {
			const registry = yield* WorkspaceGitRegistry;
			const crypto = yield* Crypto.Crypto;
			const limits = yield* Schema.decodeUnknownEffect(GitReadServiceConfiguration, {
				onExcessProperty: "error",
			})(configuration).pipe(
				Effect.mapError((cause) =>
					read_error("configuration", "configuration", "configuration", cause),
				),
			);
			const Refresh = (workspace_id: string) =>
				Effect.gen(function* () {
					const { git } = yield* registry.Get(workspace_id);

					yield* ProbeRepository(git, workspace_id, "snapshot", limits.max_stderr_bytes);

					const status_args = [
						"-c",
						"core.fsmonitor=false",
						"status",
						"--porcelain=v2",
						"--branch",
						"-z",
						"--untracked-files=all",
					];
					const worktree_args = ["worktree", "list", "--porcelain", "-z"];
					const initial_status_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						status_args,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const parsed_status = yield* ParseGitStatus(initial_status_bytes);
					const untracked_paths = parsed_status.files
						.filter((file) => file.untracked)
						.map((file) => file.path);
					const initial_untracked_hashes = yield* HashUntrackedFiles(
						git,
						workspace_id,
						untracked_paths,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const initial_worktree_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						worktree_args,
						limits.max_worktree_bytes,
						limits.max_stderr_bytes,
					);
					const aggregate_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						numstat_args(parsed_status.head, "all"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const staged_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						numstat_args(parsed_status.head, "staged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const unstaged_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						numstat_args(parsed_status.head, "unstaged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const aggregate_content = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						content_diff_args(parsed_status.head, "all"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const staged_content = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						content_diff_args(parsed_status.head, "staged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const unstaged_content = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						content_diff_args(parsed_status.head, "unstaged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_status_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						status_args,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_worktree_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						worktree_args,
						limits.max_worktree_bytes,
						limits.max_stderr_bytes,
					);
					const closing_aggregate_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						numstat_args(parsed_status.head, "all"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_staged_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						numstat_args(parsed_status.head, "staged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_unstaged_bytes = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						numstat_args(parsed_status.head, "unstaged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_aggregate_content = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						content_diff_args(parsed_status.head, "all"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_staged_content = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						content_diff_args(parsed_status.head, "staged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_unstaged_content = yield* RunChecked(
						git,
						workspace_id,
						"snapshot",
						content_diff_args(parsed_status.head, "unstaged"),
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const closing_untracked_hashes = yield* HashUntrackedFiles(
						git,
						workspace_id,
						untracked_paths,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);

					if (
						!bytes_equal(initial_status_bytes, closing_status_bytes) ||
						!bytes_equal(initial_worktree_bytes, closing_worktree_bytes) ||
						!bytes_equal(aggregate_bytes, closing_aggregate_bytes) ||
						!bytes_equal(staged_bytes, closing_staged_bytes) ||
						!bytes_equal(unstaged_bytes, closing_unstaged_bytes) ||
						!bytes_equal(aggregate_content, closing_aggregate_content) ||
						!bytes_equal(staged_content, closing_staged_content) ||
						!bytes_equal(unstaged_content, closing_unstaged_content) ||
						!bytes_equal(initial_untracked_hashes, closing_untracked_hashes)
					) {
						return yield* Effect.fail(
							read_error(workspace_id, "snapshot", "snapshot_changed"),
						);
					}

					const parsed_worktrees = yield* ParseGitWorktrees(initial_worktree_bytes);
					const root_identity = normalized_worktree_identity(git.root);
					const worktrees = parsed_worktrees.map((worktree) => ({
						...worktree,
						current: normalized_worktree_identity(worktree.path) === root_identity,
					}));

					if (worktrees.filter((worktree) => worktree.current).length !== 1) {
						return yield* Effect.fail(
							read_error(
								workspace_id,
								"snapshot",
								"invalid_output",
								new Error(
									"Git worktree inventory did not identify exactly one current root",
								),
							),
						);
					}

					const aggregate = yield* ParseGitNumstat(aggregate_bytes);
					const staged = yield* ParseGitNumstat(staged_bytes);
					const unstaged = yield* ParseGitNumstat(unstaged_bytes);
					const snapshot_id = yield* crypto
						.digest(
							"SHA-256",
							length_prefixed([
								initial_status_bytes,
								initial_worktree_bytes,
								aggregate_bytes,
								staged_bytes,
								unstaged_bytes,
								aggregate_content,
								staged_content,
								unstaged_content,
								initial_untracked_hashes,
							]),
						)
						.pipe(Effect.map(hex));

					return yield* Schema.decodeUnknownEffect(GitStatusSnapshot, {
						onExcessProperty: "error",
					})({
						aggregate,
						files: parsed_status.files,
						head: parsed_status.head,
						root: git.root,
						snapshot_id,
						staged,
						unstaged,
						upstream: parsed_status.upstream,
						workspace_id,
						worktrees,
					});
				}).pipe(
					Effect.mapError((cause) => map_read_error(workspace_id, "snapshot", cause)),
				);
			const ReadStatus = Refresh;
			const ReadPatch = (request: typeof GitPatchRequest.Type) =>
				Effect.gen(function* () {
					const decoded = yield* Schema.decodeUnknownEffect(GitPatchRequest, {
						onExcessProperty: "error",
					})(request).pipe(
						Effect.mapError((cause) =>
							read_error(request.workspace_id, "patch", "configuration", cause),
						),
					);
					const workspace_id = decoded.workspace_id;
					const max_bytes = decoded.max_bytes ?? limits.max_patch_bytes;

					if (
						!Number.isSafeInteger(max_bytes) ||
						max_bytes < 0 ||
						max_bytes > limits.max_patch_bytes
					) {
						return yield* Effect.fail(
							read_error(workspace_id, "patch", "configuration"),
						);
					}

					const initial_snapshot = yield* Refresh(workspace_id);
					const { git } = yield* registry.Get(workspace_id);

					yield* ProbeRepository(git, workspace_id, "patch", limits.max_stderr_bytes);

					const status_args = [
						"-c",
						"core.fsmonitor=false",
						"status",
						"--porcelain=v2",
						"--branch",
						"-z",
						"--untracked-files=all",
					];
					const initial_status_bytes = yield* RunChecked(
						git,
						workspace_id,
						"patch",
						status_args,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);
					const status = yield* ParseGitStatus(initial_status_bytes);
					const patch_args = [
						"-c",
						"core.quotePath=true",
						"diff",
						"--no-ext-diff",
						"--no-textconv",
						"--binary",
						...diff_reference(status.head, decoded.scope),
					];
					const RunPatch = git.Run({
						args: patch_args,
						max_stderr_bytes: limits.max_stderr_bytes,
						max_stdin_bytes: 0,
						max_stdout_bytes: max_bytes,
						mode: "read",
					});
					const first_result = yield* RunPatch;

					if (
						first_result.termination === "timeout" ||
						(first_result.termination === "output_limit" &&
							first_result.output_limit_channel !== "stdout") ||
						(first_result.termination === undefined && first_result.exit_code !== 0)
					) {
						return yield* Effect.fail(
							read_error(
								workspace_id,
								"patch",
								"command_failed",
								new Error(stderr_message(first_result)),
							),
						);
					}

					const closing_status_bytes = yield* RunChecked(
						git,
						workspace_id,
						"patch",
						status_args,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);

					if (!bytes_equal(initial_status_bytes, closing_status_bytes)) {
						return yield* Effect.fail(
							read_error(workspace_id, "patch", "snapshot_changed"),
						);
					}

					const second_result = yield* RunPatch;
					const final_status_bytes = yield* RunChecked(
						git,
						workspace_id,
						"patch",
						status_args,
						limits.max_status_bytes,
						limits.max_stderr_bytes,
					);

					if (
						second_result.termination === "timeout" ||
						(second_result.termination === "output_limit" &&
							second_result.output_limit_channel !== "stdout") ||
						(second_result.termination === undefined && second_result.exit_code !== 0)
					) {
						return yield* Effect.fail(
							read_error(
								workspace_id,
								"patch",
								"command_failed",
								new Error(stderr_message(second_result)),
							),
						);
					}

					if (
						!bytes_equal(closing_status_bytes, final_status_bytes) ||
						first_result.termination !== second_result.termination ||
						first_result.output_limit_channel !== second_result.output_limit_channel ||
						!bytes_equal(first_result.stdout.bytes, second_result.stdout.bytes)
					) {
						return yield* Effect.fail(
							read_error(workspace_id, "patch", "snapshot_changed"),
						);
					}

					const closing_snapshot = yield* Refresh(workspace_id);

					if (initial_snapshot.snapshot_id !== closing_snapshot.snapshot_id) {
						return yield* Effect.fail(
							read_error(workspace_id, "patch", "snapshot_changed"),
						);
					}

					return yield* Schema.decodeUnknownEffect(GitDiffPatch, {
						onExcessProperty: "error",
					})(decode_bounded_patch(second_result));
				}).pipe(
					Effect.mapError((cause) =>
						map_read_error(request.workspace_id, "patch", cause),
					),
				);

			return { ReadPatch, ReadStatus, Refresh };
		}),
	);
}

export const GitReadServiceLive = make_git_read_service_layer();
