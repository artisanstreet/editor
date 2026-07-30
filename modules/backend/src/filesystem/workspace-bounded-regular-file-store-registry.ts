import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Crypto, Data, Effect, FileSystem, Layer, Path, Ref, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import {
	BoundedRegularFileStore,
	type BoundedRegularFileReader,
} from "./bounded-regular-file-store";
import { make_node_non_adversarial_bounded_regular_file_store } from "./node-filesystem";
const WorkspaceBoundedRegularFileStoreAuthorization = Schema.Struct({
	working_directory: Schema.NonEmptyString,
	workspace_id: Identifier,
});

/** Supplies the workspace and directory that must prove the same bounded-store root. */
export type WorkspaceBoundedRegularFileStoreAuthorization =
	typeof WorkspaceBoundedRegularFileStoreAuthorization.Type;

/** Reports an opaque workspace identifier that has no registered bounded regular-file store. */
export class WorkspaceBoundedRegularFileStoreNotFoundError extends Data.TaggedError(
	"WorkspaceBoundedRegularFileStoreNotFoundError",
)<{
	readonly workspace_id: string;
}> {}

/** Reports a directory that cannot prove authority over its bounded regular-file store. */
export class WorkspaceBoundedRegularFileStoreAuthorizationError extends Data.TaggedError(
	"WorkspaceBoundedRegularFileStoreAuthorizationError",
)<{
	readonly workspace_id: string;
}> {}

/** Owns bounded regular-file capabilities supplied by an optional workspace authority. */
export class WorkspaceBoundedRegularFileStoreRegistry extends Context.Service<
	WorkspaceBoundedRegularFileStoreRegistry,
	{
		readonly Authorize: (input: WorkspaceBoundedRegularFileStoreAuthorization) => Effect.Effect<
			{
				readonly store: typeof BoundedRegularFileStore.Service;
				readonly workspace_id: string;
			},
			WorkspaceBoundedRegularFileStoreAuthorizationError
		>;
		readonly Get: (workspace_id: string) => Effect.Effect<
			{
				readonly reader: BoundedRegularFileReader;
				readonly workspace_id: string;
			},
			WorkspaceBoundedRegularFileStoreNotFoundError
		>;
		readonly ListWorkspaceIds: Effect.Effect<ReadonlyArray<string>>;
		/** Replaces all bounded capabilities from an authoritative workspace snapshot. */
		readonly Reconcile: (
			inputs: ReadonlyArray<{ readonly root: string; readonly workspace_id: string }>,
		) => Effect.Effect<ReadonlyArray<string>>;
	}
>()("Artisan/WorkspaceBoundedRegularFileStoreRegistry") {}

/** Supplies an inert registry when no external bounded-store authority is composed. */
export const EmptyWorkspaceBoundedRegularFileStoreRegistryLive = Layer.succeed(
	WorkspaceBoundedRegularFileStoreRegistry,
	{
		Authorize: ({ workspace_id }) =>
			Effect.fail(new WorkspaceBoundedRegularFileStoreAuthorizationError({ workspace_id })),
		Get: (workspace_id) =>
			Effect.fail(new WorkspaceBoundedRegularFileStoreNotFoundError({ workspace_id })),
		ListWorkspaceIds: Effect.succeed([]),
		Reconcile: () => Effect.succeed([]),
	},
);

interface RegisteredStore {
	readonly root: string;
	readonly store: typeof BoundedRegularFileStore.Service;
}

/**
 * Bounded reads and conditional replacement for the same roots the filesystem
 * registry serves. Both registries are fed from one place — the project
 * binding — so a workspace can never have a tree but no readable files.
 */
const BuildNodeWorkspaceBoundedRegularFileStoreRegistry = Effect.gen(function* () {
	const state = yield* Ref.make<ReadonlyMap<string, RegisteredStore>>(new Map());
	const platform = yield* Effect.context<Crypto.Crypto | FileSystem.FileSystem | Path.Path>();

	const Get = (workspace_id: string) =>
		Ref.get(state).pipe(
			Effect.flatMap((current) => {
				const registered = current.get(workspace_id);

				return registered === undefined
					? Effect.fail(
							new WorkspaceBoundedRegularFileStoreNotFoundError({ workspace_id }),
						)
					: Effect.succeed({
							reader: { ReadRegularFile: registered.store.ReadRegularFile },
							workspace_id,
						});
			}),
		);

	const Authorize = (input: WorkspaceBoundedRegularFileStoreAuthorization) =>
		Schema.decodeUnknownEffect(WorkspaceBoundedRegularFileStoreAuthorization, {
			onExcessProperty: "error",
		})(input).pipe(
			Effect.mapError(
				() =>
					new WorkspaceBoundedRegularFileStoreAuthorizationError({
						workspace_id: input.workspace_id,
					}),
			),
			Effect.flatMap((authorization) =>
				Ref.get(state).pipe(
					Effect.flatMap((current) => {
						const registered = current.get(authorization.workspace_id);

						return registered === undefined ||
							registered.root !== authorization.working_directory
							? Effect.fail(
									new WorkspaceBoundedRegularFileStoreAuthorizationError({
										workspace_id: authorization.workspace_id,
									}),
								)
							: Effect.succeed({
									store: registered.store,
									workspace_id: authorization.workspace_id,
								});
					}),
				),
			),
		);

	const ListWorkspaceIds = Ref.get(state).pipe(
		Effect.map((current) => [...current.keys()].toSorted()),
	);

	const Reconcile = (
		inputs: ReadonlyArray<{ readonly root: string; readonly workspace_id: string }>,
	) =>
		Effect.gen(function* () {
			const current = yield* Ref.get(state);
			const retained = new Map<string, RegisteredStore>();
			for (const input of inputs) {
				const existing = current.get(input.workspace_id);
				if (existing !== undefined && existing.root === input.root) {
					retained.set(input.workspace_id, existing);
				}
			}
			/**
			 * Revoke detached and root-changed capabilities before performing any
			 * filesystem work for additions. Unchanged entries retain their
			 * existing store and remain continuously available.
			 */
			yield* Ref.set(state, retained);

			const attempts = yield* Effect.forEach(
				inputs.filter((input) => !retained.has(input.workspace_id)),
				(input) =>
					make_node_non_adversarial_bounded_regular_file_store({
						root: input.root,
					}).pipe(
						Effect.map((store) => [input, store] as const),
						Effect.result,
					),
				{ concurrency: 1 },
			);
			const next = new Map(retained);
			for (const attempt of attempts) {
				if (attempt._tag === "Failure") {
					yield* Effect.logWarning("Project bounded file capability could not be bound", {
						cause: attempt.failure,
					});
					continue;
				}
				const [input, store] = attempt.success;
				next.set(input.workspace_id, { root: input.root, store });
			}
			yield* Ref.set(state, next);
			return [...next.keys()].toSorted();
		}).pipe(Effect.provide(platform));

	return { Authorize, Get, ListWorkspaceIds, Reconcile };
});

/** Builds the Node-backed bounded store registry extended at runtime by project binding. */
export const NodeWorkspaceBoundedRegularFileStoreRegistryLive = Layer.effect(
	WorkspaceBoundedRegularFileStoreRegistry,
	BuildNodeWorkspaceBoundedRegularFileStoreRegistry,
).pipe(
	Layer.provideMerge(NodeFileSystem.layer),
	Layer.provideMerge(NodePath.layer),
	Layer.provideMerge(NodeCrypto.layer),
);
