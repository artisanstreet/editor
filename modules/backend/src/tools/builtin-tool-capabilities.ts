import { Effect, Layer } from "effect";

import type { ArtisanToolId } from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceFilesystemRegistry } from "../filesystem/workspace-filesystem-registry";
import { WorkspaceGitRegistry } from "../git/workspace-git-registry";
import { ArtisanToolCapabilityState } from "./artisan-tool-registry";

const file_store_tools = new Set<ArtisanToolId>(["workspace.file.read", "workspace.file.write"]);
const filesystem_tools = new Set<ArtisanToolId>(["workspace.file.list", "terminal.open"]);
const git_tools = new Set<ArtisanToolId>([
	"git.status.read",
	"git.diff.read",
	"git.index.stage",
	"git.index.unstage",
]);

/** Resolves production capability availability from the opaque, workspace-scoped backend registries. */
export const ArtisanBuiltInToolCapabilityStateLive = Layer.effect(
	ArtisanToolCapabilityState,
	Effect.gen(function* () {
		const bounded = yield* WorkspaceBoundedRegularFileStoreRegistry;
		const filesystems = yield* WorkspaceFilesystemRegistry;
		const git = yield* WorkspaceGitRegistry;
		const Get = (tool_id: ArtisanToolId, workspace_id?: string) => {
			if (tool_id.startsWith("preview."))
				return Effect.succeed({
					state: "unavailable" as const,
					tool_id,
					unavailable_reason: "No preview adapter is configured",
				});
			if (tool_id === "workspace.language.status")
				return Effect.succeed({
					state: "unavailable" as const,
					tool_id,
					unavailable_reason: "No backend language service is configured",
				});
			if (
				!file_store_tools.has(tool_id) &&
				!filesystem_tools.has(tool_id) &&
				!git_tools.has(tool_id)
			)
				return Effect.succeed({ state: "available" as const, tool_id });
			if (workspace_id === undefined)
				return Effect.succeed({
					state: "unavailable" as const,
					tool_id,
					unavailable_reason: "A registered workspace is required",
				});
			const Available = <A, E>(capability: Effect.Effect<A, E>) =>
				capability.pipe(
					Effect.as({ state: "available" as const, tool_id }),
					Effect.catch(() =>
						Effect.succeed({
							state: "unavailable" as const,
							tool_id,
							unavailable_reason: "Workspace capability is not registered",
						}),
					),
				);
			return file_store_tools.has(tool_id)
				? Available(bounded.Get(workspace_id))
				: filesystem_tools.has(tool_id)
					? Available(filesystems.Get(workspace_id))
					: Available(git.Get(workspace_id));
		};
		return { Get };
	}),
);
