import { Effect, Layer, Option } from "effect";

import { Git, GitError, type GitDiffStats, type GitFileSummary, type GitOperation } from "./git";
import {
	ProcessRunner,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
	type ProcessRunnerShape,
} from "./process-runner";
import {
	make_node_process_runner_layer,
	type NodeProcessRunnerOptions,
} from "./node-process-runner";

/** Configures bounded Git reads for one project directory. */
export interface NodeGitOptions {
	readonly cwd: string;
	readonly max_patch_bytes?: number;
	readonly max_status_bytes?: number;
	readonly process?: NodeProcessRunnerOptions;
}

const conflicted_statuses = new Set(["DD", "AU", "UD", "UA", "DU", "AA", "UU"]);

function git_error(operation: GitOperation, cause: unknown) {
	return new GitError({ cause, operation });
}

function decode_output(bytes: Uint8Array) {
	return new TextDecoder().decode(bytes);
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

	if ((byte & 0b1111_1000) === 0b1111_0000) {
		return 4;
	}

	return 1;
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

	const expected_length = utf8_sequence_length(bytes[lead_index]!);
	const actual_length = bytes.byteLength - lead_index;

	return actual_length < expected_length ? lead_index : bytes.byteLength;
}

function bounded_utf8(bytes: Uint8Array, max_bytes: number) {
	const bounded = bytes.subarray(0, max_bytes);
	const prefix_length = complete_utf8_prefix_length(bounded);
	const prefix = bounded.subarray(0, prefix_length);
	const decoded = decode_output(prefix);
	const encoded = new TextEncoder().encode(decoded);

	if (encoded.byteLength <= max_bytes) {
		return {
			bytes: encoded.byteLength,
			patch: decoded,
			trimmed: prefix.byteLength < bytes.byteLength,
		};
	}

	let patch = "";
	let patch_bytes = 0;

	for (const character of decoded) {
		const character_bytes = new TextEncoder().encode(character).byteLength;

		if (patch_bytes + character_bytes > max_bytes) {
			break;
		}

		patch += character;
		patch_bytes += character_bytes;
	}

	return {
		bytes: patch_bytes,
		patch,
		trimmed: true,
	};
}

function is_valid_limit(value: number) {
	return Number.isSafeInteger(value) && value >= 0;
}

function run_git_process(
	runner: ProcessRunnerShape,
	cwd: string,
	args: ReadonlyArray<string>,
	operation: GitOperation,
	limits: Pick<ProcessRunnerInput, "max_stderr_bytes" | "max_stdout_bytes"> = {},
) {
	return runner
		.Run({ args, command: "git", cwd, ...limits })
		.pipe(Effect.mapError((cause) => git_error(operation, cause)));
}

function require_success(result: ProcessRunnerResult, operation: GitOperation) {
	if (result.exit_code !== 0) {
		return Effect.fail(git_error(operation, decode_output(result.stderr)));
	}

	if (result.stdout_truncated) {
		return Effect.fail(
			git_error(operation, new Error("Git output exceeded its configured byte limit")),
		);
	}

	return Effect.succeed(decode_output(result.stdout));
}

function run_git(
	runner: ProcessRunnerShape,
	cwd: string,
	args: ReadonlyArray<string>,
	operation: GitOperation,
	max_stdout_bytes: number,
) {
	return run_git_process(runner, cwd, args, operation, { max_stdout_bytes }).pipe(
		Effect.flatMap((result) => require_success(result, operation)),
	);
}

function check_head_exists(runner: ProcessRunnerShape, cwd: string, max_stdout_bytes: number) {
	return run_git_process(runner, cwd, ["rev-parse", "--verify", "--quiet", "HEAD"], "diff", {
		max_stdout_bytes,
	}).pipe(
		Effect.flatMap((result) => {
			if (result.exit_code === 0) {
				return Effect.succeed(true);
			}

			if (result.exit_code === 1) {
				return Effect.succeed(false);
			}

			return Effect.fail(git_error("diff", decode_output(result.stderr)));
		}),
	);
}

function parse_status(output: string) {
	return Effect.try({
		try: () => {
			const fields = output.split("\0");
			const summaries: Array<GitFileSummary> = [];
			let index = 0;

			while (index < fields.length) {
				const record = fields[index++];

				if (record === undefined || record.length === 0) {
					continue;
				}

				if (record.length < 4 || record[2] !== " ") {
					throw new Error("malformed Git porcelain status record");
				}

				const staged_code = record[0]!;
				const unstaged_code = record[1]!;
				const status = `${staged_code}${unstaged_code}`;
				const untracked = status === "??";
				const conflicted = conflicted_statuses.has(status);
				const has_original_path =
					staged_code === "R" ||
					staged_code === "C" ||
					unstaged_code === "R" ||
					unstaged_code === "C";
				const original_path = has_original_path ? fields[index++] : undefined;

				if (has_original_path && !original_path) {
					throw new Error("rename or copy status is missing its original path");
				}

				const summary = {
					conflicted,
					path: record.slice(3),
					staged: !untracked && !conflicted && staged_code !== " ",
					status,
					untracked,
					unstaged: !untracked && !conflicted && unstaged_code !== " ",
				};

				summaries.push(
					original_path === undefined ? summary : { ...summary, original_path },
				);
			}

			return summaries;
		},
		catch: (cause) => git_error("status", cause),
	});
}

function parse_stats(output: string): GitDiffStats {
	const match = output.match(
		/(\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?/,
	);

	return {
		additions: Number(match?.[2] ?? 0),
		deletions: Number(match?.[3] ?? 0),
		files: Number(match?.[1] ?? 0),
	};
}

/** Builds an injectable Git layer that requires a ProcessRunner. */
export function make_git_layer(options: NodeGitOptions) {
	const max_patch_bytes = options.max_patch_bytes ?? 1_000_000;
	const max_status_bytes = options.max_status_bytes ?? 8_000_000;

	return Layer.effect(
		Git,
		Effect.gen(function* () {
			if (!is_valid_limit(max_patch_bytes) || !is_valid_limit(max_status_bytes)) {
				return yield* Effect.fail(
					git_error(
						"configuration",
						new Error("Git output limits must be non-negative safe integers"),
					),
				);
			}

			const runner = yield* ProcessRunner;
			const git_text = (
				args: ReadonlyArray<string>,
				operation: GitOperation,
				max_stdout_bytes = max_status_bytes,
			) => run_git(runner, options.cwd, args, operation, max_stdout_bytes);

			const discover = Effect.gen(function* () {
				const root = (yield* git_text(["rev-parse", "--show-toplevel"], "discover")).trim();
				const branch = (yield* git_text(["branch", "--show-current"], "discover")).trim();
				const head_result = yield* run_git_process(
					runner,
					options.cwd,
					["rev-parse", "--verify", "--quiet", "HEAD"],
					"discover",
					{ max_stdout_bytes: max_status_bytes },
				);

				if (head_result.exit_code !== 0 && head_result.exit_code !== 1) {
					return yield* Effect.fail(
						git_error("discover", decode_output(head_result.stderr)),
					);
				}

				const head =
					head_result.exit_code === 0
						? Option.some(decode_output(head_result.stdout).trim())
						: Option.none<string>();

				return { branch, head, root };
			});
			const status = git_text(["status", "--porcelain=v1", "-z", "-uall"], "status").pipe(
				Effect.flatMap(parse_status),
			);
			const diff_stats = Effect.gen(function* () {
				const has_head = yield* check_head_exists(runner, options.cwd, max_status_bytes);
				const reference = has_head ? ["HEAD"] : ["--cached"];
				const output = yield* git_text(["diff", "--shortstat", ...reference], "diff");

				return parse_stats(output);
			});
			const diff_patch = (requested_bytes = max_patch_bytes) =>
				Effect.gen(function* () {
					if (!is_valid_limit(requested_bytes)) {
						return yield* Effect.fail(
							git_error(
								"configuration",
								new Error("patch byte limit must be a non-negative safe integer"),
							),
						);
					}

					const limit = Math.min(requested_bytes, max_patch_bytes);
					const has_head = yield* check_head_exists(
						runner,
						options.cwd,
						max_status_bytes,
					);
					const reference = has_head ? ["HEAD"] : ["--cached"];
					const result = yield* run_git_process(
						runner,
						options.cwd,
						["diff", "--no-ext-diff", "--binary", ...reference],
						"diff",
						{ max_stdout_bytes: limit },
					);

					if (result.exit_code !== 0) {
						return yield* Effect.fail(git_error("diff", decode_output(result.stderr)));
					}

					const bounded = bounded_utf8(result.stdout, limit);

					return {
						bytes: bounded.bytes,
						patch: bounded.patch,
						truncated:
							bounded.trimmed ||
							result.stdout_truncated ||
							result.stdout_bytes > bounded.bytes,
					};
				});

			return {
				DiffPatch: diff_patch,
				DiffStats: diff_stats,
				Discover: discover,
				Status: status,
			};
		}),
	);
}

/** Builds the production Git layer with a bounded Node process runner. */
export function make_node_git_layer(options: NodeGitOptions) {
	return make_git_layer(options).pipe(
		Layer.provide(make_node_process_runner_layer(options.process)),
	);
}
