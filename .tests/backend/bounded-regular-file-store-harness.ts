import { realpath, stat } from "node:fs/promises";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Data, Effect, Layer } from "effect";

import {
	BoundedRegularFileStore,
	type BoundedRegularFileReader,
} from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import {
	WorkspaceBoundedRegularFileStoreAuthorizationError,
	WorkspaceBoundedRegularFileStoreNotFoundError,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { make_node_non_adversarial_bounded_regular_file_store } from "../../modules/backend/src/filesystem/node-filesystem";

export interface TestWorkspaceBoundedRegularFileStoreRegistration {
	readonly root: string;
	readonly store: typeof BoundedRegularFileStore.Service;
	readonly workspace_id: string;
}

export class TestWorkspaceBoundedRegularFileStoreRegistrationError extends Data.TaggedError(
	"TestWorkspaceBoundedRegularFileStoreRegistrationError",
)<{
	readonly cause?: unknown;
	readonly workspace_id: string;
}> {}

const CanonicalizeDirectory = (registration: TestWorkspaceBoundedRegularFileStoreRegistration) =>
	Effect.tryPromise({
		try: async () => {
			const canonical_root = await realpath(registration.root);
			const metadata = await stat(canonical_root);

			if (!metadata.isDirectory()) {
				throw new Error("bounded-store root is not a directory");
			}

			return { ...registration, canonical_root };
		},
		catch: (cause) =>
			new TestWorkspaceBoundedRegularFileStoreRegistrationError({
				cause,
				workspace_id: registration.workspace_id,
			}),
	});

/** Builds the generic workspace registry used by bounded-store behavior tests. */
const MakeRegistry = (
	registrations: ReadonlyArray<TestWorkspaceBoundedRegularFileStoreRegistration>,
) =>
	Effect.gen(function* () {
		const canonical = yield* Effect.forEach(registrations, CanonicalizeDirectory);
		const by_workspace = new Map(canonical.map((entry) => [entry.workspace_id, entry]));
		const by_root = new Map(canonical.map((entry) => [entry.canonical_root, entry]));

		if (by_workspace.size !== registrations.length || by_root.size !== registrations.length) {
			return yield* new TestWorkspaceBoundedRegularFileStoreRegistrationError({
				workspace_id: "duplicate",
			});
		}

		return {
			Authorize: ({
				working_directory,
				workspace_id,
			}: {
				readonly working_directory: string;
				readonly workspace_id: string;
			}) =>
				Effect.gen(function* () {
					const entry = by_workspace.get(workspace_id);
					const candidate = yield* Effect.tryPromise(() =>
						realpath(working_directory),
					).pipe(Effect.option);

					if (
						entry === undefined ||
						candidate._tag === "None" ||
						candidate.value !== entry.canonical_root
					) {
						return yield* new WorkspaceBoundedRegularFileStoreAuthorizationError({
							workspace_id,
						});
					}

					return { store: entry.store, workspace_id };
				}),
			Get: (workspace_id: string) => {
				const entry = by_workspace.get(workspace_id);

				if (entry === undefined) {
					return Effect.fail(
						new WorkspaceBoundedRegularFileStoreNotFoundError({ workspace_id }),
					);
				}

				const reader: BoundedRegularFileReader = {
					ReadRegularFile: entry.store.ReadRegularFile,
				};

				return Effect.succeed({ reader, workspace_id });
			},
			ListWorkspaceIds: Effect.succeed([...by_workspace.keys()].toSorted()),
		} satisfies typeof WorkspaceBoundedRegularFileStoreRegistry.Service;
	});

export const MakeCheckedTestWorkspaceBoundedRegularFileStoreRegistryLayer = (
	registrations: ReadonlyArray<TestWorkspaceBoundedRegularFileStoreRegistration>,
) => Layer.effect(WorkspaceBoundedRegularFileStoreRegistry, MakeRegistry(registrations));

export const MakeTestWorkspaceBoundedRegularFileStoreRegistryLayer = (
	registrations: ReadonlyArray<TestWorkspaceBoundedRegularFileStoreRegistration>,
) => MakeCheckedTestWorkspaceBoundedRegularFileStoreRegistryLayer(registrations).pipe(Layer.orDie);

/** Uses the generic Node algorithm as an Effect-native store for integration tests. */
export const MakeNodeTestWorkspaceBoundedRegularFileStoreRegistryLayer = (
	registrations: ReadonlyArray<{ readonly root: string; readonly workspace_id: string }>,
) =>
	Layer.effect(
		WorkspaceBoundedRegularFileStoreRegistry,
		Effect.forEach(registrations, (registration) =>
			make_node_non_adversarial_bounded_regular_file_store({
				root: registration.root,
			}).pipe(Effect.map((store) => ({ ...registration, store }))),
		).pipe(
			Effect.flatMap(MakeRegistry),
			Effect.provide(NodeFileSystem.layer),
			Effect.provide(NodePath.layer),
			Effect.provide(NodeCrypto.layer),
		),
	).pipe(Layer.orDie);
