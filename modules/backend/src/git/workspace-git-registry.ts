import { NodeFileSystem } from "@effect/platform-node-shared";
import { Context, Data, Effect, FileSystem, Layer, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import { GitFetch } from "./git-fetch";
import { Git } from "./git";
import { GitMutation } from "./git-mutation";
import { make_node_git_fetch_layer, make_node_git_mutation_layer } from "./node-git-mutation";
import { make_node_git_layer } from "./node-git";

const WorkspaceGitRegistration = Schema.Struct({
	root: Schema.NonEmptyString,
	workspace_id: Identifier,
});

/** Registers one opaque workspace identity with its backend-owned Git capability. */
export type WorkspaceGitRegistration = typeof WorkspaceGitRegistration.Type;

/** Keeps native paths and process-backed services on the backend side of the boundary. */
export interface WorkspaceGitCapability {
	readonly canonical_root: string;
	readonly fetch: typeof GitFetch.Service;
	readonly mutation: typeof GitMutation.Service;
	readonly read: typeof Git.Service;
	readonly workspace_id: string;
}

/** Reports an invalid, missing, aliased, or duplicate Git workspace registration. */
export class WorkspaceGitRegistrationError extends Data.TaggedError(
	"WorkspaceGitRegistrationError",
)<{
	readonly cause?: unknown;
	readonly message: string;
	readonly workspace_id?: string;
}> {}

/** Reports an opaque workspace identifier without a registered Git capability. */
export class WorkspaceGitNotFoundError extends Data.TaggedError("WorkspaceGitNotFoundError")<{
	readonly workspace_id: string;
}> {}

/** Owns process-backed Git capabilities without exposing native paths to the renderer. */
export class WorkspaceGitRegistry extends Context.Service<
	WorkspaceGitRegistry,
	{
		readonly Get: (
			workspace_id: string,
		) => Effect.Effect<WorkspaceGitCapability, WorkspaceGitNotFoundError>;
		readonly ListWorkspaceIds: Effect.Effect<ReadonlyArray<string>>;
	}
>()("Artisan/WorkspaceGitRegistry") {}

function registration_error(message: string, cause?: unknown, workspace_id?: string) {
	return new WorkspaceGitRegistrationError({
		...(cause === undefined ? {} : { cause }),
		...(workspace_id === undefined ? {} : { workspace_id }),
		message,
	});
}

function BuildWorkspaceGitRegistry(registrations: ReadonlyArray<unknown>) {
	return Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const decoded = yield* Effect.forEach(registrations, (registration) =>
			Schema.decodeUnknownEffect(WorkspaceGitRegistration, {
				onExcessProperty: "error",
			})(registration).pipe(
				Effect.mapError((cause) =>
					registration_error("Workspace Git registration is invalid", cause),
				),
			),
		);
		const workspace_ids = decoded.map((registration) => registration.workspace_id);

		if (new Set(workspace_ids).size !== workspace_ids.length) {
			return yield* Effect.fail(
				registration_error("Workspace Git registration IDs must be unique"),
			);
		}

		const canonical = yield* Effect.forEach(decoded, (registration) =>
			Effect.gen(function* () {
				const canonical_root = yield* file_system.realPath(registration.root);
				const entry = yield* file_system.stat(canonical_root);

				if (entry.type !== "Directory") {
					return yield* Effect.fail(
						registration_error(
							"Workspace Git root must be a directory",
							undefined,
							registration.workspace_id,
						),
					);
				}

				return { ...registration, canonical_root };
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof WorkspaceGitRegistrationError
						? cause
						: registration_error(
								"Workspace Git root could not be canonicalized",
								cause,
								registration.workspace_id,
							),
				),
			),
		);

		if (
			new Set(canonical.map((registration) => registration.canonical_root)).size !==
			canonical.length
		) {
			return yield* Effect.fail(
				registration_error("Workspace Git roots must be canonically unique"),
			);
		}

		const capabilities = yield* Effect.forEach(canonical, (registration) =>
			Effect.gen(function* () {
				const fetch = yield* GitFetch.pipe(
					Effect.provide(make_node_git_fetch_layer({ cwd: registration.canonical_root })),
				);
				const read = yield* Git.pipe(
					Effect.provide(make_node_git_layer({ cwd: registration.canonical_root })),
				);
				const mutation = yield* GitMutation.pipe(
					Effect.provide(
						make_node_git_mutation_layer({ cwd: registration.canonical_root }),
					),
				);

				return {
					canonical_root: registration.canonical_root,
					fetch,
					mutation,
					read,
					workspace_id: registration.workspace_id,
				} satisfies WorkspaceGitCapability;
			}),
		);
		const by_workspace_id = new Map(
			capabilities.map((capability) => [capability.workspace_id, capability]),
		);
		const ListWorkspaceIds = Effect.succeed([...by_workspace_id.keys()].toSorted());
		const Get = (workspace_id: string) => {
			const capability = by_workspace_id.get(workspace_id);

			return capability === undefined
				? Effect.fail(new WorkspaceGitNotFoundError({ workspace_id }))
				: Effect.succeed(capability);
		};

		return { Get, ListWorkspaceIds };
	});
}

/** Builds immutable process-backed Git capabilities for registered workspace roots. */
export function make_node_workspace_git_registry_layer(registrations: ReadonlyArray<unknown>) {
	return Layer.effect(WorkspaceGitRegistry, BuildWorkspaceGitRegistry(registrations)).pipe(
		Layer.provide(NodeFileSystem.layer),
	);
}

/** Supplies an empty registry for portable compositions without registered workspaces. */
export const EmptyWorkspaceGitRegistryLive = make_node_workspace_git_registry_layer([]);
