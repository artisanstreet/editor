import { NodeFileSystem } from "@effect/platform-node-shared";
import { Context, Data, Effect, FileSystem, Layer, Schema } from "effect";

import {
	GitCommandExecutor,
	type GitCommandExecutorError,
	type GitCommandInput,
	type GitCommandResult,
	make_node_git_command_executor_layer,
} from "./git-command-executor";

const WorkspaceGitRegistration = Schema.Struct({
	root: Schema.NonEmptyString,
	workspace_id: Schema.NonEmptyString,
});

const WorkspaceGitAuthorization = Schema.Struct({
	working_directory: Schema.NonEmptyString,
	workspace_id: Schema.NonEmptyString,
});

/** Registers one opaque workspace identity at a backend-owned repository root. */
export type WorkspaceGitRegistration = typeof WorkspaceGitRegistration.Type;

/** Requires an existing directory to prove the same canonical workspace authority. */
export type WorkspaceGitAuthorization = typeof WorkspaceGitAuthorization.Type;

export type WorkspaceGitCommandInput = Omit<GitCommandInput, "cwd">;

/** Provides Git execution only after the registered root is revalidated. */
export interface WorkspaceGit {
	readonly root: string;
	readonly Run: (
		input: WorkspaceGitCommandInput,
	) => Effect.Effect<GitCommandResult, GitCommandExecutorError | WorkspaceGitRootChangedError>;
}

/** Reports invalid, duplicate, aliased, missing, or non-directory registration roots. */
export class WorkspaceGitRegistrationError extends Data.TaggedError(
	"WorkspaceGitRegistrationError",
)<{
	readonly cause?: unknown;
	readonly reason: "aliased_root" | "duplicate_id" | "invalid" | "unavailable";
	readonly workspace_id?: string;
}> {}

/** Reports an opaque workspace identifier without a registered Git capability. */
export class WorkspaceGitNotFoundError extends Data.TaggedError("WorkspaceGitNotFoundError")<{
	readonly workspace_id: string;
}> {}

/** Reports a working directory that does not prove exact canonical-root authority. */
export class WorkspaceGitAuthorizationError extends Data.TaggedError(
	"WorkspaceGitAuthorizationError",
)<{
	readonly workspace_id: string;
}> {}

/** Fails closed when a configured root no longer resolves to its registered directory. */
export class WorkspaceGitRootChangedError extends Data.TaggedError("WorkspaceGitRootChangedError")<{
	readonly cause?: unknown;
	readonly workspace_id: string;
}> {}

export interface WorkspaceGitCapability {
	readonly git: WorkspaceGit;
	readonly workspace_id: string;
}

/** Owns canonical, workspace-scoped Git capabilities without accepting arbitrary roots. */
export class WorkspaceGitRegistry extends Context.Service<
	WorkspaceGitRegistry,
	{
		readonly Authorize: (
			input: WorkspaceGitAuthorization,
		) => Effect.Effect<
			WorkspaceGitCapability,
			WorkspaceGitAuthorizationError | WorkspaceGitNotFoundError
		>;
		readonly Get: (
			workspace_id: string,
		) => Effect.Effect<WorkspaceGitCapability, WorkspaceGitNotFoundError>;
		readonly ListWorkspaceIds: Effect.Effect<ReadonlyArray<string>>;
	}
>()("Artisan/WorkspaceGitRegistry") {}

function registration_error(
	reason: WorkspaceGitRegistrationError["reason"],
	cause?: unknown,
	workspace_id?: string,
) {
	return new WorkspaceGitRegistrationError({
		...(cause === undefined ? {} : { cause }),
		...(workspace_id === undefined ? {} : { workspace_id }),
		reason,
	});
}

function root_changed(workspace_id: string, cause?: unknown) {
	return new WorkspaceGitRootChangedError({
		...(cause === undefined ? {} : { cause }),
		workspace_id,
	});
}

interface CanonicalRegistration {
	readonly canonical_root: string;
	readonly configured_root: string;
	readonly workspace_id: string;
}

/** Builds an immutable registry over the supplied Effect filesystem and Git executor. */
export function make_workspace_git_registry_layer(registrations: ReadonlyArray<unknown>) {
	return Layer.effect(
		WorkspaceGitRegistry,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const executor = yield* GitCommandExecutor;
			const decoded = yield* Effect.forEach(registrations, (registration) =>
				Schema.decodeUnknownEffect(WorkspaceGitRegistration, {
					onExcessProperty: "error",
				})(registration).pipe(
					Effect.mapError((cause) => registration_error("invalid", cause)),
				),
			);
			const workspace_ids = decoded.map((registration) => registration.workspace_id);

			if (new Set(workspace_ids).size !== workspace_ids.length) {
				return yield* Effect.fail(registration_error("duplicate_id"));
			}

			const canonical = yield* Effect.forEach(decoded, (registration) =>
				Effect.gen(function* () {
					const canonical_root = yield* file_system.realPath(registration.root);
					const metadata = yield* file_system.stat(canonical_root);

					if (metadata.type !== "Directory") {
						return yield* Effect.fail(
							new Error("Workspace Git root is not a directory"),
						);
					}

					return {
						canonical_root,
						configured_root: registration.root,
						workspace_id: registration.workspace_id,
					} satisfies CanonicalRegistration;
				}).pipe(
					Effect.mapError((cause) =>
						registration_error("unavailable", cause, registration.workspace_id),
					),
				),
			);

			if (
				new Set(canonical.map((registration) => registration.canonical_root)).size !==
				canonical.length
			) {
				return yield* Effect.fail(registration_error("aliased_root"));
			}

			const by_workspace_id = new Map(
				canonical.map((registration) => [registration.workspace_id, registration] as const),
			);
			const ValidateRoot = (registration: CanonicalRegistration) =>
				Effect.gen(function* () {
					const current_root = yield* file_system.realPath(registration.configured_root);
					const metadata = yield* file_system.stat(current_root);

					if (
						metadata.type !== "Directory" ||
						current_root !== registration.canonical_root
					) {
						return yield* Effect.fail(root_changed(registration.workspace_id));
					}

					return current_root;
				}).pipe(
					Effect.mapError((cause) =>
						cause instanceof WorkspaceGitRootChangedError
							? cause
							: root_changed(registration.workspace_id, cause),
					),
				);
			const Get = (workspace_id: string) => {
				const registration = by_workspace_id.get(workspace_id);

				if (registration === undefined) {
					return Effect.fail(new WorkspaceGitNotFoundError({ workspace_id }));
				}

				const git: WorkspaceGit = {
					root: registration.canonical_root,
					Run: (input) =>
						ValidateRoot(registration).pipe(
							Effect.flatMap((cwd) => executor.Run({ ...input, cwd })),
						),
				};

				return Effect.succeed({ git, workspace_id });
			};
			const Authorize = (input: WorkspaceGitAuthorization) =>
				Schema.decodeUnknownEffect(WorkspaceGitAuthorization, {
					onExcessProperty: "error",
				})(input).pipe(
					Effect.mapError(
						() =>
							new WorkspaceGitAuthorizationError({
								workspace_id: input.workspace_id,
							}),
					),
					Effect.flatMap((authorization) =>
						Effect.gen(function* () {
							const capability = yield* Get(authorization.workspace_id);
							const working_directory = yield* file_system
								.realPath(authorization.working_directory)
								.pipe(
									Effect.mapError(
										() =>
											new WorkspaceGitAuthorizationError({
												workspace_id: authorization.workspace_id,
											}),
									),
								);

							if (working_directory !== capability.git.root) {
								return yield* Effect.fail(
									new WorkspaceGitAuthorizationError({
										workspace_id: authorization.workspace_id,
									}),
								);
							}

							return capability;
						}),
					),
				);
			const ListWorkspaceIds = Effect.succeed([...by_workspace_id.keys()].toSorted());

			return { Authorize, Get, ListWorkspaceIds };
		}),
	);
}

/** Builds the production Node registry and supplies the Effect Git process capability. */
export function make_node_workspace_git_registry_layer(registrations: ReadonlyArray<unknown>) {
	return make_workspace_git_registry_layer(registrations).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(make_node_git_command_executor_layer()),
	);
}
