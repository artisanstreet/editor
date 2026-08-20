import { NodeFileSystem } from "@effect/platform-node-shared";
import { Context, Data, Effect, FileSystem, Layer, Option, Ref, Schema } from "effect";

import {
	GitCommandExecutor,
	type GitCommandExecutorError,
	type GitCommandInput,
	type GitCommandResult,
	make_node_git_command_executor_layer,
} from "./executor";

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
	/**
	 * Resolves an inventory path through the registered filesystem authority and
	 * compares its canonical identity with this workspace root.
	 */
	readonly IsCurrentRoot: (path: string) => Effect.Effect<boolean>;
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
		/**
		 * Adds one repository root after construction. Projects are attached while
		 * Forge runs, so the set of workspaces cannot be known when this layer is
		 * built — and a registry that only ever held its construction argument held
		 * nothing at all in production, which is why no Git projection existed for
		 * any workspace. Re-registering the same workspace at the same canonical
		 * root is a no-op, so a startup replay may run over an already-bound
		 * catalog. Every rule construction enforced is enforced here.
		 */
		readonly Register: (
			registration: unknown,
		) => Effect.Effect<{ readonly workspace_id: string }, WorkspaceGitRegistrationError>;
		/**
		 * Replaces the complete registry from an authoritative snapshot. Entries
		 * that cannot be prepared are omitted, which also revokes any older
		 * capability held under the same workspace id.
		 */
		readonly Reconcile: (
			registrations: ReadonlyArray<unknown>,
		) => Effect.Effect<ReadonlyArray<WorkspaceGitRegistration>, WorkspaceGitRegistrationError>;
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
	readonly root_device: number;
	readonly root_inode: number | undefined;
	readonly workspace_id: string;
}

/** Whether two prepared roots name the same directory, by inode where available. */
function is_aliased_root(left: CanonicalRegistration, right: CanonicalRegistration) {
	return left.root_inode === undefined || right.root_inode === undefined
		? left.canonical_root === right.canonical_root
		: left.root_device === right.root_device && left.root_inode === right.root_inode;
}

/** Builds a registry over the supplied Effect filesystem and Git executor. */
export function make_workspace_git_registry_layer(registrations: ReadonlyArray<unknown>) {
	return Layer.effect(
		WorkspaceGitRegistry,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const executor = yield* GitCommandExecutor;
			const state = yield* Ref.make<ReadonlyMap<string, CanonicalRegistration>>(new Map());

			/**
			 * Validates one root and canonicalizes it. Every rule the eager builder
			 * enforced lives here, so a root registered while Forge runs is held to
			 * exactly the standard a composed one was.
			 */
			const Prepare = (registration: unknown) =>
				Effect.gen(function* () {
					const decoded = yield* Schema.decodeUnknownEffect(WorkspaceGitRegistration, {
						onExcessProperty: "error",
					})(registration).pipe(
						Effect.mapError((cause) => registration_error("invalid", cause)),
					);

					return yield* Effect.gen(function* () {
						const canonical_root = yield* file_system.realPath(decoded.root);
						const metadata = yield* file_system.stat(canonical_root);

						if (metadata.type !== "Directory") {
							return yield* Effect.fail(
								new Error("Workspace Git root is not a directory"),
							);
						}

						return {
							canonical_root,
							configured_root: decoded.root,
							root_device: metadata.dev,
							root_inode: Option.getOrUndefined(metadata.ino),
							workspace_id: decoded.workspace_id,
						} satisfies CanonicalRegistration;
					}).pipe(
						Effect.mapError((cause) =>
							registration_error("unavailable", cause, decoded.workspace_id),
						),
					);
				});

			/**
			 * Two workspaces may not share a canonical root: the pair would be
			 * indistinguishable to `Authorize`, which proves authority by comparing a
			 * working directory against exactly one root.
			 */
			const Register = (registration: unknown) =>
				Effect.gen(function* () {
					const prepared = yield* Prepare(registration);
					const current = yield* Ref.get(state);
					const existing = current.get(prepared.workspace_id);

					if (existing !== undefined) {
						return is_aliased_root(existing, prepared)
							? { workspace_id: prepared.workspace_id }
							: yield* Effect.fail(
									registration_error(
										"duplicate_id",
										undefined,
										prepared.workspace_id,
									),
								);
					}

					if ([...current.values()].some((entry) => is_aliased_root(entry, prepared))) {
						return yield* Effect.fail(
							registration_error("aliased_root", undefined, prepared.workspace_id),
						);
					}

					yield* Ref.update(state, (latest) =>
						new Map(latest).set(prepared.workspace_id, prepared),
					);

					return { workspace_id: prepared.workspace_id };
				});

			/** Construction still fails closed: a composed registration must be valid. */
			yield* Effect.forEach(registrations, Register, { discard: true });

			const Reconcile = (next_registrations: ReadonlyArray<unknown>) =>
				Effect.gen(function* () {
					const attempts = yield* Effect.forEach(
						next_registrations,
						(registration) => Prepare(registration).pipe(Effect.result),
						{ concurrency: 1 },
					);
					yield* Effect.forEach(
						attempts,
						(attempt) =>
							attempt._tag === "Failure"
								? Effect.logWarning(
										"Project workspace Git root could not be bound",
										{
											cause: attempt.failure,
											reason: attempt.failure.reason,
											workspace_id: attempt.failure.workspace_id,
										},
									)
								: Effect.void,
						{ discard: true },
					);

					const next = new Map<string, CanonicalRegistration>();
					const accepted: Array<WorkspaceGitRegistration> = [];

					for (const prepared of attempts.flatMap((attempt) =>
						attempt._tag === "Success" ? [attempt.success] : [],
					)) {
						if (
							next.has(prepared.workspace_id) ||
							[...next.values()].some((entry) => is_aliased_root(entry, prepared))
						) {
							yield* Effect.logWarning(
								"Duplicate project workspace Git authority was omitted from the snapshot",
								{ workspace_id: prepared.workspace_id },
							);
							continue;
						}
						next.set(prepared.workspace_id, prepared);
						accepted.push({
							root: prepared.canonical_root,
							workspace_id: prepared.workspace_id,
						});
					}

					yield* Ref.set(state, next);

					return accepted;
				});

			const ValidateRoot = (registration: CanonicalRegistration) =>
				Effect.gen(function* () {
					const current_root = yield* file_system.realPath(registration.configured_root);
					const metadata = yield* file_system.stat(current_root);
					const inode = Option.getOrUndefined(metadata.ino);

					if (
						metadata.type !== "Directory" ||
						metadata.dev !== registration.root_device ||
						(inode === undefined || registration.root_inode === undefined
							? current_root !== registration.canonical_root
							: inode !== registration.root_inode)
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
			const CapabilityFor = (registration: CanonicalRegistration, workspace_id: string) => {
				const git: WorkspaceGit = {
					IsCurrentRoot: (path) =>
						Effect.gen(function* () {
							const canonical_path = yield* file_system.realPath(path);
							const metadata = yield* file_system.stat(canonical_path);
							const inode = Option.getOrUndefined(metadata.ino);

							return (
								metadata.type === "Directory" &&
								metadata.dev === registration.root_device &&
								(inode === undefined || registration.root_inode === undefined
									? canonical_path === registration.canonical_root
									: inode === registration.root_inode)
							);
						}).pipe(
							/** A missing or inaccessible non-current inventory path is not root authority. */
							Effect.catch(() => Effect.succeed(false)),
						),
					root: registration.canonical_root,
					Run: (input) =>
						ValidateRoot(registration).pipe(
							Effect.flatMap((cwd) => executor.Run({ ...input, cwd })),
						),
				};

				return { git, workspace_id } satisfies WorkspaceGitCapability;
			};

			/** Read per call: a capability must reflect the catalog as it stands now. */
			const Get = (workspace_id: string) =>
				Ref.get(state).pipe(
					Effect.flatMap((current) => {
						const registration = current.get(workspace_id);

						return registration === undefined
							? Effect.fail(new WorkspaceGitNotFoundError({ workspace_id }))
							: Effect.succeed(CapabilityFor(registration, workspace_id));
					}),
				);
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
			const ListWorkspaceIds = Ref.get(state).pipe(
				Effect.map((current) => [...current.keys()].toSorted()),
			);

			return { Authorize, Get, ListWorkspaceIds, Reconcile, Register };
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
