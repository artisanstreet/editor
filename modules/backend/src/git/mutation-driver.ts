import { Context, Data, Effect, Layer, Schema } from "effect";

import { type GitCommandResult } from "./executor";
import { ParseGitStatus } from "./parsers";
import {
	WorkspaceGitNotFoundError,
	WorkspaceGitRegistry,
	WorkspaceGitRootChangedError,
	type WorkspaceGit,
} from "./workspace-git-registry";

const mutation_stdin_limit = 1024 * 1024;
const mutation_output_limit = 256 * 1024;
const mutation_status_limit = 8 * 1024 * 1024;

const GitMutationPath = Schema.String.check(
	Schema.makeFilter<string>((path) => {
		const invalid_segment = path
			.split("/")
			.some((segment) => segment === "" || segment === "." || segment === "..");

		return path.length === 0 ||
			path.includes("\0") ||
			path.startsWith("/") ||
			/^[a-z]:/iu.test(path) ||
			invalid_segment
			? "Expected a canonical repository-relative literal Git path"
			: undefined;
	}),
);

const GitMutationRequest = Schema.Struct({
	paths: Schema.Array(GitMutationPath).check(Schema.isMinLength(1), Schema.isMaxLength(10_000)),
	workspace_id: Schema.NonEmptyString,
});

export type GitMutationRequest = typeof GitMutationRequest.Type;

export type GitMutationDriverOperation = "stage" | "unstage";

/** Reports a private Git mutation primitive that could not safely execute. */
export class GitMutationDriverError extends Data.TaggedError("GitMutationDriverError")<{
	readonly cause?: unknown;
	readonly operation: GitMutationDriverOperation;
	readonly reason:
		| "command_failed"
		| "invalid_request"
		| "output_limit"
		| "root_changed"
		| "stdin_limit"
		| "unborn_head"
		| "workspace_not_found";
	readonly workspace_id: string;
}> {}

/** Exposes only the non-destructive index primitives used by the future mutation service. */
export class GitMutationDriver extends Context.Service<
	GitMutationDriver,
	{
		readonly Stage: (
			request: GitMutationRequest,
		) => Effect.Effect<void, GitMutationDriverError>;
		readonly Unstage: (
			request: GitMutationRequest,
		) => Effect.Effect<void, GitMutationDriverError>;
	}
>()("Artisan/GitMutationDriver") {}

const text_decoder = new TextDecoder("utf-8");
const text_encoder = new TextEncoder();

function mutation_error(
	workspace_id: string,
	operation: GitMutationDriverOperation,
	reason: GitMutationDriverError["reason"],
	cause?: unknown,
) {
	return new GitMutationDriverError({
		...(cause === undefined ? {} : { cause }),
		operation,
		reason,
		workspace_id,
	});
}

function map_mutation_error(
	workspace_id: string,
	operation: GitMutationDriverOperation,
	cause: unknown,
) {
	if (cause instanceof GitMutationDriverError) {
		return cause;
	}

	if (cause instanceof WorkspaceGitNotFoundError) {
		return mutation_error(workspace_id, operation, "workspace_not_found");
	}

	if (cause instanceof WorkspaceGitRootChangedError) {
		return mutation_error(workspace_id, operation, "root_changed");
	}

	return mutation_error(workspace_id, operation, "command_failed", cause);
}

function stderr_message(result: GitCommandResult) {
	return result.stderr.truncated
		? `Git exited ${result.exit_code} with stderr exceeding its byte limit`
		: `Git exited ${result.exit_code}: ${text_decoder.decode(result.stderr.bytes).trim()}`;
}

const RunMutation = (
	git: WorkspaceGit,
	workspace_id: string,
	operation: GitMutationDriverOperation,
	args: ReadonlyArray<string>,
	stdin: Uint8Array,
) =>
	git
		.Run({
			args,
			max_stderr_bytes: mutation_output_limit,
			max_stdin_bytes: mutation_stdin_limit,
			max_stdout_bytes: mutation_output_limit,
			mode: "mutation",
			stdin,
		})
		.pipe(
			Effect.mapError((cause) => map_mutation_error(workspace_id, operation, cause)),
			Effect.flatMap((result) => {
				if (result.termination !== undefined) {
					return Effect.fail(
						mutation_error(
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
						mutation_error(
							workspace_id,
							operation,
							"command_failed",
							new Error(stderr_message(result)),
						),
					);
				}

				return result.stdout.truncated || result.stderr.truncated
					? Effect.fail(mutation_error(workspace_id, operation, "output_limit"))
					: Effect.void;
			}),
		);

const ReadHead = (git: WorkspaceGit, workspace_id: string, operation: GitMutationDriverOperation) =>
	git
		.Run({
			args: [
				"-c",
				"core.fsmonitor=false",
				"status",
				"--porcelain=v2",
				"--branch",
				"-z",
				"--untracked-files=no",
			],
			max_stderr_bytes: mutation_output_limit,
			max_stdin_bytes: 0,
			max_stdout_bytes: mutation_status_limit,
			mode: "read",
		})
		.pipe(
			Effect.mapError((cause) => map_mutation_error(workspace_id, operation, cause)),
			Effect.flatMap((result) => {
				if (result.termination !== undefined) {
					return Effect.fail(
						mutation_error(
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
						mutation_error(
							workspace_id,
							operation,
							"command_failed",
							new Error(stderr_message(result)),
						),
					);
				}

				if (result.stdout.truncated) {
					return Effect.fail(mutation_error(workspace_id, operation, "output_limit"));
				}

				return ParseGitStatus(result.stdout.bytes).pipe(
					Effect.mapError((cause) =>
						mutation_error(workspace_id, operation, "command_failed", cause),
					),
				);
			}),
		);

/** Builds the private index mutation driver over canonical workspace capabilities. */
export const GitMutationDriverLive = Layer.effect(
	GitMutationDriver,
	Effect.gen(function* () {
		const registry = yield* WorkspaceGitRegistry;
		const Decode = (request: GitMutationRequest, operation: GitMutationDriverOperation) =>
			Schema.decodeUnknownEffect(GitMutationRequest, { onExcessProperty: "error" })(
				request,
			).pipe(
				Effect.mapError((cause) =>
					mutation_error(request.workspace_id, operation, "invalid_request", cause),
				),
				Effect.flatMap((decoded) =>
					new Set(decoded.paths).size === decoded.paths.length
						? Effect.succeed(decoded)
						: Effect.fail(
								mutation_error(
									decoded.workspace_id,
									operation,
									"invalid_request",
									new Error("Git mutation paths must be unique"),
								),
							),
				),
			);
		const EncodePaths = (
			request: GitMutationRequest,
			operation: GitMutationDriverOperation,
		) => {
			const stdin = text_encoder.encode(`${request.paths.join("\0")}\0`);

			return stdin.byteLength <= mutation_stdin_limit
				? Effect.succeed(stdin)
				: Effect.fail(mutation_error(request.workspace_id, operation, "stdin_limit"));
		};
		const Stage = (request: GitMutationRequest) =>
			Effect.gen(function* () {
				const decoded = yield* Decode(request, "stage");
				const stdin = yield* EncodePaths(decoded, "stage");
				const { git } = yield* registry.Get(decoded.workspace_id);

				yield* RunMutation(
					git,
					decoded.workspace_id,
					"stage",
					["--literal-pathspecs", "add", "--pathspec-from-file=-", "--pathspec-file-nul"],
					stdin,
				);
			}).pipe(
				Effect.mapError((cause) =>
					map_mutation_error(request.workspace_id, "stage", cause),
				),
			);
		const Unstage = (request: GitMutationRequest) =>
			Effect.gen(function* () {
				const decoded = yield* Decode(request, "unstage");
				const stdin = yield* EncodePaths(decoded, "unstage");
				const { git } = yield* registry.Get(decoded.workspace_id);
				const status = yield* ReadHead(git, decoded.workspace_id, "unstage");

				if (status.head._tag === "unborn") {
					return yield* Effect.fail(
						mutation_error(decoded.workspace_id, "unstage", "unborn_head"),
					);
				}

				yield* RunMutation(
					git,
					decoded.workspace_id,
					"unstage",
					[
						"--literal-pathspecs",
						"restore",
						"--staged",
						"--pathspec-from-file=-",
						"--pathspec-file-nul",
					],
					stdin,
				);
			}).pipe(
				Effect.mapError((cause) =>
					map_mutation_error(request.workspace_id, "unstage", cause),
				),
			);

		return { Stage, Unstage };
	}),
);
