import { lstat } from "node:fs/promises";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Fiber, FileSystem, Layer, Option, Schema } from "effect";

import {
	Git,
	GitMutation,
	make_backend_runtime,
	NodeProcessRunnerLive,
	ProcessRunner,
	ProtocolRouter,
	WorkspaceGitMutationCoordinator,
	WorkspaceGitRegistry,
	WorkspaceGitSessionService,
} from "@artisan/backend";
import {
	GitMutationError,
	GitMutationPlan,
	GitMutationPreparation,
} from "../../modules/backend/src/git/git-mutation";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const database_path_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_DATABASE;
const credential_id_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_CREDENTIAL;
const invocation_path_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_INVOCATION;
const migrations_path_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_MIGRATIONS;
const repository_root_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_REPOSITORY;
const socket_path_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_SOCKET;
const source_head_environment = process.env.ARTISAN_GIT_MUTATION_CRASH_SOURCE_HEAD;

if (
	!database_path_environment ||
	!credential_id_environment ||
	!invocation_path_environment ||
	!migrations_path_environment ||
	!repository_root_environment ||
	!socket_path_environment ||
	!source_head_environment
) {
	throw new Error("Git mutation crash fixture paths are required");
}

const database_path = database_path_environment;
const credential_id = credential_id_environment;
const invocation_path = invocation_path_environment;
const migrations_path = migrations_path_environment;
const repository_root = repository_root_environment;
const socket_path = socket_path_environment;
const source_head = source_head_environment;
const workspace_id = "workspace_git_mutation_crash";
const thread_id = "thread_git_mutation_crash";
let next_id = 0;
const source = {
	branch: "main",
	configuration_identity: "1".repeat(64),
	head: source_head,
	index_identity: "2".repeat(64),
	repository_identity: "3".repeat(64),
	state: "none" as const,
	state_identity: "4".repeat(64),
	status_identity: "5".repeat(64),
	tracked_identity: "6".repeat(64),
	untracked_identity: "7".repeat(64),
	worktree_identity: "8".repeat(64),
};
const text_encoder = new TextEncoder();
const credential_input = text_encoder.encode(
	[
		"protocol=https",
		"host=artisan-editor.test",
		`username=${credential_id}`,
		`password=${credential_id}`,
		"",
	].join("\n"),
);

function metadata_layer() {
	return Layer.succeed(RuntimeMetadata, {
		instance_id: "backend_git_mutation_crash",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_git_mutation_crash_${++next_id}`),
		Now: Effect.succeed("2026-07-14T12:00:00.000Z"),
	});
}

function WaitForSocket(path: string) {
	return Effect.tryPromise({
		try: async () => {
			for (let attempt = 0; attempt < 500; attempt += 1) {
				try {
					await lstat(path);
					return;
				} catch (cause) {
					const code =
						typeof cause === "object" &&
						cause !== null &&
						"code" in cause &&
						typeof cause.code === "string"
							? cause.code
							: undefined;

					/** Git for Windows exposes a live AF_UNIX socket as an EACCES reparse point. */
					if (process.platform === "win32" && code === "EACCES") {
						return;
					}

					if (code !== "ENOENT") {
						throw cause;
					}

					await new Promise<void>((resolve) => setTimeout(resolve, 10));
				}
			}

			throw new Error(`Git credential-cache socket did not appear at ${path}`);
		},
		catch: (cause) => new GitMutationError({ cause, operation: "process" }),
	});
}

function AppendInvocation(path: string) {
	return Effect.scoped(
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const file = yield* file_system.open(path, { flag: "a" });

			yield* file.writeAll(text_encoder.encode("execute\n"));
			yield* file.sync;
		}),
	).pipe(
		Effect.provide(NodeFileSystem.layer),
		Effect.mapError((cause) => new GitMutationError({ cause, operation: "process" })),
	);
}

function StoreCredential(process_runner: typeof ProcessRunner.Service) {
	return process_runner
		.Run({
			args: ["credential-cache", "--timeout", "10", "--socket", socket_path, "store"],
			command: "git",
			cwd: repository_root,
			max_stderr_bytes: 64 * 1024,
			max_stdout_bytes: 64 * 1024,
			stdin: credential_input,
		})
		.pipe(
			Effect.mapError((cause) => new GitMutationError({ cause, operation: "process" })),
			Effect.flatMap((result) =>
				result.exit_code === 0
					? Effect.void
					: Effect.fail(new GitMutationError({ cause: result, operation: "process" })),
			),
		);
}

function make_registry_service(process_runner: typeof ProcessRunner.Service) {
	const read: typeof Git.Service = {
		DiffPatch: () => Effect.succeed({ bytes: 0, patch: "", truncated: false }),
		DiffStats: Effect.succeed({ additions: 0, deletions: 0, files: 0 }),
		Discover: Effect.succeed({
			branch: "main",
			head: Option.some(source.head),
			root: repository_root,
		}),
		ProbeRepository: Effect.succeed(
			Option.some({
				branch: "main",
				head: Option.some(source.head),
				root: repository_root,
			}),
		),
		ResolveLocalBranch: () => Effect.succeed(Option.none()),
		Status: Effect.succeed([]),
		Worktrees: Effect.succeed([
			{
				adapter_path: repository_root,
				bare: false,
				branch: Option.some("refs/heads/main"),
				detached: false,
				head: Option.some(source.head),
				locked: false,
				prunable: false,
			},
		]),
	};
	const Prepare = (input: unknown) =>
		Effect.try({
			try: () => {
				const preparation = Schema.decodeUnknownSync(GitMutationPreparation)(input);
				const operation = "operation" in preparation ? preparation.operation : preparation;

				if (operation.type !== "commit") {
					throw new Error("Only commit preparation is used by this fixture");
				}

				return Schema.decodeUnknownSync(GitMutationPlan)({
					binding: "d".repeat(64),
					message: operation.message,
					source,
					type: "commit",
				});
			},
			catch: (cause) => new GitMutationError({ cause, operation: "invalid_plan" }),
		});
	const Execute = (input: unknown) =>
		Schema.decodeUnknownEffect(GitMutationPlan, { onExcessProperty: "error" })(input).pipe(
			Effect.mapError((cause) => new GitMutationError({ cause, operation: "invalid_plan" })),
			Effect.flatMap(() =>
				Effect.scoped(
					Effect.gen(function* () {
						const daemon = yield* process_runner
							.Run({
								args: ["credential-cache--daemon", "--debug", socket_path],
								command: "git",
								cwd: repository_root,
								max_stderr_bytes: 64 * 1024,
								max_stdout_bytes: 64 * 1024,
							})
							.pipe(
								Effect.mapError(
									(cause) =>
										new GitMutationError({ cause, operation: "process" }),
								),
								Effect.flatMap((result) =>
									Effect.fail(
										new GitMutationError({
											cause: result,
											operation: "process",
										}),
									),
								),
								Effect.forkScoped,
							);

						yield* WaitForSocket(socket_path);
						yield* StoreCredential(process_runner);
						yield* Effect.yieldNow;

						if (daemon.pollUnsafe() !== undefined) {
							return yield* new GitMutationError({
								cause: new Error(
									"Production Git daemon exited while storing proof",
								),
								operation: "process",
							});
						}

						yield* AppendInvocation(invocation_path);
						yield* Effect.sync(() =>
							process.stdout.write("GIT_MUTATION_CRASH_READY\n"),
						);

						return yield* Fiber.join(daemon);
					}),
				),
			),
		);

	const mutation: typeof GitMutation.Service = {
		Execute,
		Prepare,
		Reconcile: () =>
			Effect.die("Fixture reconciliation is unreachable before the parent crash"),
	};

	return {
		Get: (requested_workspace_id: string) =>
			requested_workspace_id === workspace_id
				? Effect.succeed({
						canonical_root: repository_root,
						fetch: { Fetch: () => Effect.die("fixture invoked GitFetch") },
						mutation,
						read,
						workspace_id,
					})
				: Effect.die(`Unexpected workspace ${requested_workspace_id}`),
		ListWorkspaceIds: Effect.succeed([workspace_id]),
	};
}

const registry = Layer.effect(
	WorkspaceGitRegistry,
	Effect.map(ProcessRunner, make_registry_service),
).pipe(Layer.provide(NodeProcessRunnerLive));
const runtime = make_backend_runtime({
	database_path,
	migrations_path,
	runtime_metadata: metadata_layer(),
	workspace_git_registry: registry,
});

const route = (
	message_id: string,
	payload: { readonly title: string; readonly type: "thread.create" },
) =>
	runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route({
				kind: "command",
				message_id,
				origin: "frontend",
				payload,
				protocol_version: 1,
				schema_version: 1,
				sent_at: "2026-07-14T12:00:00.000Z",
				thread_id,
			});
		}),
	);

await route("create_git_mutation_crash_thread", {
	title: "Git mutation crash",
	type: "thread.create",
});
await runtime.runPromise(
	Effect.gen(function* () {
		const sessions = yield* WorkspaceGitSessionService;
		const coordinator = yield* WorkspaceGitMutationCoordinator;

		yield* sessions.Refresh({
			message_id: "refresh_git_mutation_crash_session",
			sent_at: "2026-07-14T12:00:00.000Z",
			thread_id,
			workspace_id,
		});
		const requested = yield* coordinator.Request({
			expected_session_version: 1,
			message_id: "request_git_mutation_crash",
			operation: { message: "Crash-window commit", type: "commit" },
			sent_at: "2026-07-14T12:00:00.000Z",
			thread_id,
			workspace_id,
		});

		yield* coordinator.Respond({
			approval_id: requested.approval.approval_id,
			approved: true,
			message_id: "approve_git_mutation_crash",
			sent_at: "2026-07-14T12:00:00.000Z",
			thread_id,
		});
	}),
);

await new Promise<never>(() => {});
