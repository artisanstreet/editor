import { Effect, Layer } from "effect";

import {
	WorkspaceBoundedRegularFileStoreAuthorizationError,
	WorkspaceBoundedRegularFileStoreNotFoundError,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../filesystem/workspace-bounded-regular-file-store-registry";
import {
	WorkspaceFilesystemAuthorizationError,
	WorkspaceFilesystemNotFoundError,
	WorkspaceFilesystemRegistry,
} from "../filesystem/workspace-filesystem-registry";
import {
	WorkspaceGitAuthorizationError,
	WorkspaceGitNotFoundError,
	WorkspaceGitRegistry,
} from "../git/workspace-git-registry";
import { ProjectWorkspaceBindingGate } from "./projects";

/**
 * Re-publishes raw workspace authority under its existing service tags after
 * binding is ready. Reconcile and Register intentionally stay raw: they are
 * binder-owned operations and routing either through the gate would recurse.
 */
export const GatedWorkspaceFilesystemRegistryLive = Layer.effect(
	WorkspaceFilesystemRegistry,
	Effect.gen(function* () {
		const gate = yield* ProjectWorkspaceBindingGate;
		const raw = yield* WorkspaceFilesystemRegistry;

		return {
			...raw,
			Authorize: (input) =>
				gate.Use(raw.Authorize(input)).pipe(
					Effect.mapError((error) =>
						error instanceof WorkspaceFilesystemAuthorizationError
							? error
							: new WorkspaceFilesystemAuthorizationError({
									workspace_id: input.workspace_id,
								}),
					),
				),
			Get: (workspace_id) =>
				gate
					.Use(raw.Get(workspace_id))
					.pipe(
						Effect.mapError((error) =>
							error instanceof WorkspaceFilesystemNotFoundError
								? error
								: new WorkspaceFilesystemNotFoundError({ workspace_id }),
						),
					),
			ListWorkspaceIds: gate
				.Use(raw.ListWorkspaceIds)
				.pipe(Effect.catch(() => Effect.succeed([]))),
		};
	}),
);

/**
 * See {@link GatedWorkspaceFilesystemRegistryLive}.
 *
 * Git was the one workspace authority published raw, so its consumers raced the
 * binding flight instead of waiting for it: a read arriving before the catalog
 * had been reconciled saw an unregistered workspace and reported "not found"
 * rather than waiting the moment out.
 */
export const GatedWorkspaceGitRegistryLive = Layer.effect(
	WorkspaceGitRegistry,
	Effect.gen(function* () {
		const gate = yield* ProjectWorkspaceBindingGate;
		const raw = yield* WorkspaceGitRegistry;

		return {
			...raw,
			Authorize: (input) =>
				gate.Use(raw.Authorize(input)).pipe(
					Effect.mapError((error) =>
						error instanceof WorkspaceGitAuthorizationError ||
						error instanceof WorkspaceGitNotFoundError
							? error
							: new WorkspaceGitAuthorizationError({
									workspace_id: input.workspace_id,
								}),
					),
				),
			Get: (workspace_id) =>
				gate
					.Use(raw.Get(workspace_id))
					.pipe(
						Effect.mapError((error) =>
							error instanceof WorkspaceGitNotFoundError
								? error
								: new WorkspaceGitNotFoundError({ workspace_id }),
						),
					),
			ListWorkspaceIds: gate
				.Use(raw.ListWorkspaceIds)
				.pipe(Effect.catch(() => Effect.succeed([]))),
		};
	}),
);

/** See {@link GatedWorkspaceFilesystemRegistryLive}. */
export const GatedWorkspaceBoundedRegularFileStoreRegistryLive = Layer.effect(
	WorkspaceBoundedRegularFileStoreRegistry,
	Effect.gen(function* () {
		const gate = yield* ProjectWorkspaceBindingGate;
		const raw = yield* WorkspaceBoundedRegularFileStoreRegistry;

		return {
			...raw,
			Authorize: (input) =>
				gate.Use(raw.Authorize(input)).pipe(
					Effect.mapError((error) =>
						error instanceof WorkspaceBoundedRegularFileStoreAuthorizationError
							? error
							: new WorkspaceBoundedRegularFileStoreAuthorizationError({
									workspace_id: input.workspace_id,
								}),
					),
				),
			Get: (workspace_id) =>
				gate.Use(raw.Get(workspace_id)).pipe(
					Effect.mapError((error) =>
						error instanceof WorkspaceBoundedRegularFileStoreNotFoundError
							? error
							: new WorkspaceBoundedRegularFileStoreNotFoundError({
									workspace_id,
								}),
					),
				),
			ListWorkspaceIds: gate
				.Use(raw.ListWorkspaceIds)
				.pipe(Effect.catch(() => Effect.succeed([]))),
		};
	}),
);
