import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Crypto, Data, Effect, FileSystem, Layer, Path, Ref, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import { Filesystem } from "./filesystem";
import { make_node_filesystem } from "./node-filesystem";

const WorkspaceFilesystemRegistration = Schema.Struct({
	root: Schema.NonEmptyString,
	workspace_id: Identifier,
});

const WorkspaceFilesystemAuthorization = Schema.Struct({
	working_directory: Schema.NonEmptyString,
	workspace_id: Identifier,
});

/** Supplies one backend-owned root while retaining the opaque workspace identity. */
export type WorkspaceFilesystemRegistration = typeof WorkspaceFilesystemRegistration.Type;

/** Supplies the workspace and existing directory that must prove the same authority. */
export type WorkspaceFilesystemAuthorization = typeof WorkspaceFilesystemAuthorization.Type;

/** Exposes root-confined file operations without an absolute-path resolver. */
export type WorkspaceFilesystem = Omit<typeof Filesystem.Service, "Resolve">;

/** Reports malformed, missing, non-directory, duplicate, or aliased workspace registration. */
export class WorkspaceFilesystemRegistrationError extends Data.TaggedError(
	"WorkspaceFilesystemRegistrationError",
)<{
	readonly cause?: unknown;
	readonly message: string;
	readonly workspace_id?: string;
}> {}

/** Reports an opaque workspace identifier that has no registered filesystem capability. */
export class WorkspaceFilesystemNotFoundError extends Data.TaggedError(
	"WorkspaceFilesystemNotFoundError",
)<{
	readonly workspace_id: string;
}> {}

/** Reports a working directory that cannot prove authority over its workspace. */
export class WorkspaceFilesystemAuthorizationError extends Data.TaggedError(
	"WorkspaceFilesystemAuthorizationError",
)<{
	readonly workspace_id: string;
}> {}

/** Owns the opaque backend filesystem capabilities registered for known workspaces. */
export class WorkspaceFilesystemRegistry extends Context.Service<
	WorkspaceFilesystemRegistry,
	{
		readonly Authorize: (
			input: WorkspaceFilesystemAuthorization,
		) => Effect.Effect<
			{ readonly filesystem: WorkspaceFilesystem; readonly workspace_id: string },
			WorkspaceFilesystemAuthorizationError | WorkspaceFilesystemNotFoundError
		>;
		readonly Get: (
			workspace_id: string,
		) => Effect.Effect<
			{ readonly filesystem: WorkspaceFilesystem; readonly workspace_id: string },
			WorkspaceFilesystemNotFoundError
		>;
		readonly ListWorkspaceIds: Effect.Effect<ReadonlyArray<string>>;
		/**
		 * Adds one workspace root after construction. Projects are attached while
		 * Forge runs, so the set of workspaces cannot be known when this layer is
		 * built. Re-registering the same workspace at the same canonical root is a
		 * no-op, which is what lets a startup replay run over an already-bound
		 * catalog without failing.
		 */
		readonly Register: (
			registration: unknown,
		) => Effect.Effect<{ readonly workspace_id: string }, WorkspaceFilesystemRegistrationError>;
		/**
		 * Replaces the complete registry from an authoritative snapshot. Entries
		 * that cannot be prepared are omitted, which also revokes any older
		 * capability held under the same workspace id.
		 */
		readonly Reconcile: (
			registrations: ReadonlyArray<unknown>,
		) => Effect.Effect<
			ReadonlyArray<WorkspaceFilesystemRegistration>,
			WorkspaceFilesystemRegistrationError
		>;
	}
>()("Artisan/WorkspaceFilesystemRegistry") {}

function registration_error(message: string, cause?: unknown, workspace_id?: string) {
	return new WorkspaceFilesystemRegistrationError({
		...(cause === undefined ? {} : { cause }),
		...(workspace_id === undefined ? {} : { workspace_id }),
		message,
	});
}

function authorization_error(workspace_id: string) {
	return new WorkspaceFilesystemAuthorizationError({ workspace_id });
}

interface RegisteredWorkspace {
	readonly canonical_root: string;
	readonly filesystem: WorkspaceFilesystem;
}

/**
 * Validates one root and produces its confined capability. Every rule the
 * eager builder enforced lives here so a runtime registration is held to the
 * same standard as a composed one: the shape decodes, the root exists, and it
 * is a directory.
 */
function PrepareWorkspaceRegistration(registration: unknown) {
	return Effect.gen(function* () {
		const decoded = yield* Schema.decodeUnknownEffect(WorkspaceFilesystemRegistration, {
			onExcessProperty: "error",
		})(registration).pipe(
			Effect.mapError((cause) =>
				registration_error("Workspace filesystem registration is invalid", cause),
			),
		);

		const filesystem = yield* make_node_filesystem({ root: decoded.root }).pipe(
			Effect.mapError((cause) =>
				registration_error(
					"Workspace filesystem root could not be registered",
					cause,
					decoded.workspace_id,
				),
			),
		);

		const entry = yield* filesystem
			.Stat(".")
			.pipe(
				Effect.mapError((cause) =>
					registration_error(
						"Workspace filesystem root could not be registered",
						cause,
						decoded.workspace_id,
					),
				),
			);

		if (entry.kind !== "directory") {
			return yield* Effect.fail(
				registration_error(
					"Workspace filesystem root must be a directory",
					undefined,
					decoded.workspace_id,
				),
			);
		}

		const canonical_root = yield* filesystem
			.Resolve(".")
			.pipe(
				Effect.mapError((cause) =>
					registration_error(
						"Workspace filesystem root could not be canonicalized",
						cause,
						decoded.workspace_id,
					),
				),
			);

		const { Resolve: _resolve, ...workspace_filesystem } = filesystem;

		return {
			canonical_root,
			filesystem: workspace_filesystem,
			workspace_id: decoded.workspace_id,
		};
	});
}

function BuildWorkspaceFilesystemRegistry(registrations: ReadonlyArray<unknown>) {
	return Effect.gen(function* () {
		const state = yield* Ref.make<ReadonlyMap<string, RegisteredWorkspace>>(new Map());
		/**
		 * The layer supplies the platform capabilities once; capturing them here
		 * keeps `Register` callable from services that hold no platform context of
		 * their own, such as the project binding.
		 */
		const platform = yield* Effect.context<Crypto.Crypto | FileSystem.FileSystem | Path.Path>();

		/**
		 * Two workspaces may not share a canonical root: the pair would be
		 * indistinguishable to `Authorize`, which proves authority by comparing a
		 * working directory against exactly one root.
		 */
		const Register = (registration: unknown) =>
			Effect.gen(function* () {
				const prepared = yield* PrepareWorkspaceRegistration(registration);
				const current = yield* Ref.get(state);
				const existing = current.get(prepared.workspace_id);

				if (existing !== undefined && existing.canonical_root === prepared.canonical_root)
					return { workspace_id: prepared.workspace_id };

				if (existing !== undefined) {
					return yield* Effect.fail(
						registration_error(
							"Workspace filesystem is already registered at a different root",
							undefined,
							prepared.workspace_id,
						),
					);
				}

				const aliased = [...current.values()].some(
					(entry) => entry.canonical_root === prepared.canonical_root,
				);

				if (aliased) {
					return yield* Effect.fail(
						registration_error(
							"Workspace filesystem roots must be canonically unique",
							undefined,
							prepared.workspace_id,
						),
					);
				}

				yield* Ref.update(state, (latest) =>
					new Map(latest).set(prepared.workspace_id, {
						canonical_root: prepared.canonical_root,
						filesystem: prepared.filesystem,
					}),
				);

				return { workspace_id: prepared.workspace_id };
			}).pipe(Effect.provide(platform));

		yield* Effect.forEach(registrations, Register, { discard: true });

		const Reconcile = (next_registrations: ReadonlyArray<unknown>) =>
			Effect.gen(function* () {
				const attempts = yield* Effect.forEach(
					next_registrations,
					(registration) =>
						PrepareWorkspaceRegistration(registration).pipe(Effect.result),
					{ concurrency: 1 },
				);
				const prepared = attempts.flatMap((attempt) =>
					attempt._tag === "Success" ? [attempt.success] : [],
				);
				yield* Effect.forEach(
					attempts,
					(attempt) =>
						attempt._tag === "Failure"
							? Effect.logWarning("Project workspace could not be bound", {
									cause: attempt.failure,
									project_id: attempt.failure.workspace_id,
								})
							: Effect.void,
					{ discard: true },
				);
				const next = new Map<string, RegisteredWorkspace>();
				const accepted: Array<WorkspaceFilesystemRegistration> = [];

				for (const workspace of prepared) {
					const aliased = [...next.values()].some(
						(entry) => entry.canonical_root === workspace.canonical_root,
					);
					if (next.has(workspace.workspace_id) || aliased) {
						yield* Effect.logWarning(
							"Duplicate project workspace authority was omitted from the snapshot",
							{ workspace_id: workspace.workspace_id },
						);
						continue;
					}
					next.set(workspace.workspace_id, {
						canonical_root: workspace.canonical_root,
						filesystem: workspace.filesystem,
					});
					accepted.push({
						root: workspace.canonical_root,
						workspace_id: workspace.workspace_id,
					});
				}

				yield* Ref.set(state, next);

				return accepted;
			}).pipe(Effect.provide(platform));

		const ListWorkspaceIds = Ref.get(state).pipe(
			Effect.map((current) => [...current.keys()].toSorted()),
		);

		const Get = (workspace_id: string) =>
			Ref.get(state).pipe(
				Effect.flatMap((current) => {
					const registered = current.get(workspace_id);

					return registered === undefined
						? Effect.fail(new WorkspaceFilesystemNotFoundError({ workspace_id }))
						: Effect.succeed({ filesystem: registered.filesystem, workspace_id });
				}),
			);

		const file_system = yield* FileSystem.FileSystem;
		const Authorize = (input: WorkspaceFilesystemAuthorization) =>
			Schema.decodeUnknownEffect(WorkspaceFilesystemAuthorization, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => authorization_error(input.workspace_id)),
				Effect.flatMap((authorization) =>
					Effect.gen(function* () {
						const current = yield* Ref.get(state);
						const registered = current.get(authorization.workspace_id);
						if (registered === undefined) {
							return yield* Effect.fail(
								authorization_error(authorization.workspace_id),
							);
						}
						const working_directory = yield* file_system
							.realPath(authorization.working_directory)
							.pipe(
								Effect.mapError(() =>
									authorization_error(authorization.workspace_id),
								),
							);
						const entry = yield* file_system
							.stat(working_directory)
							.pipe(
								Effect.mapError(() =>
									authorization_error(authorization.workspace_id),
								),
							);

						if (
							entry.type !== "Directory" ||
							working_directory !== registered.canonical_root
						) {
							return yield* Effect.fail(
								authorization_error(authorization.workspace_id),
							);
						}

						return {
							filesystem: registered.filesystem,
							workspace_id: authorization.workspace_id,
						};
					}),
				),
			);

		return { Authorize, Get, ListWorkspaceIds, Reconcile, Register };
	});
}

/** Builds backend-owned workspace filesystem capabilities, seeded and extended at runtime. */
export function make_node_workspace_filesystem_registry_layer(
	registrations: ReadonlyArray<unknown>,
) {
	return Layer.effect(
		WorkspaceFilesystemRegistry,
		BuildWorkspaceFilesystemRegistry(registrations),
	).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
		Layer.provideMerge(NodeCrypto.layer),
	);
}
