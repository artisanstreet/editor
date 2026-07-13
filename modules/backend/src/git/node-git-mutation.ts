import { Effect, Layer } from "effect";

import { GitMutation, GitMutationError } from "./git-mutation";
import { ProcessRunner, type ProcessRunnerShape } from "./process-runner";
import {
	make_node_process_runner_layer,
	type NodeProcessRunnerOptions,
} from "./node-process-runner";

/** Configures bounded local Git checkout mutations for one project directory. */
export interface NodeGitMutationOptions {
	readonly cwd: string;
	readonly max_stderr_bytes?: number;
	readonly max_stdout_bytes?: number;
	readonly process?: NodeProcessRunnerOptions;
}

function mutation_error(operation: GitMutationError["operation"], cause: unknown) {
	return new GitMutationError({ cause, operation });
}

function is_valid_limit(value: number) {
	return Number.isSafeInteger(value) && value >= 0;
}

function decode_output(bytes: Uint8Array) {
	return new TextDecoder().decode(bytes);
}

function checkout_local_branch(
	runner: ProcessRunnerShape,
	cwd: string,
	branch: string,
	max_stdout_bytes: number,
	max_stderr_bytes: number,
) {
	return Effect.gen(function* () {
		if (branch.trim().length === 0 || branch.includes("\0")) {
			return yield* Effect.fail(
				mutation_error(
					"checkout",
					new Error("branch must be non-empty and must not contain NUL"),
				),
			);
		}

		const result = yield* runner
			.Run({
				args: ["switch", "--no-guess", "--", branch],
				command: "git",
				cwd,
				max_stderr_bytes,
				max_stdout_bytes,
			})
			.pipe(Effect.mapError((cause) => mutation_error("checkout", cause)));

		if (result.exit_code !== 0) {
			return yield* Effect.fail(mutation_error("checkout", decode_output(result.stderr)));
		}

		if (result.stdout_truncated || result.stderr_truncated) {
			return yield* Effect.fail(
				mutation_error(
					"checkout",
					new Error("Git checkout output exceeded its configured byte limit"),
				),
			);
		}

		return yield* Effect.succeed<void>(undefined);
	});
}

/** Builds an injectable Git mutation layer that requires a ProcessRunner. */
export function make_git_mutation_layer(options: NodeGitMutationOptions) {
	const max_stdout_bytes = options.max_stdout_bytes ?? 64 * 1024;
	const max_stderr_bytes = options.max_stderr_bytes ?? 64 * 1024;

	return Layer.effect(
		GitMutation,
		Effect.gen(function* () {
			if (!is_valid_limit(max_stdout_bytes) || !is_valid_limit(max_stderr_bytes)) {
				return yield* Effect.fail(
					mutation_error(
						"configuration",
						new Error("Git mutation output limits must be non-negative safe integers"),
					),
				);
			}

			const runner = yield* ProcessRunner;

			return {
				CheckoutLocalBranch: (branch: string) =>
					checkout_local_branch(
						runner,
						options.cwd,
						branch,
						max_stdout_bytes,
						max_stderr_bytes,
					),
			};
		}),
	);
}

/** Builds the production Git mutation layer with a bounded Node process runner. */
export function make_node_git_mutation_layer(options: NodeGitMutationOptions) {
	return make_git_mutation_layer(options).pipe(
		Layer.provide(make_node_process_runner_layer(options.process)),
	);
}
