import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	WorkspaceFileDiscoveryQuery,
	WorkspaceLanguageCapabilitiesQuery,
	type WorkspaceFileDiscoveryQuery as WorkspaceFileDiscoveryQueryValue,
	type WorkspaceFileDiscoveryQueryResult,
	type WorkspaceLanguageCapabilitiesQueryResult,
} from "@artisan/protocol";

import { WorkspaceFilesystemRegistry } from "../../filesystem/workspace-filesystem-registry";

/**
 * The only directory discovery refuses to walk.
 *
 * Everything else a repository contains is shown, including ignored and
 * generated trees: an editor that silently omits paths is worse than one that
 * shows a folder nobody opens, and the caller can already bound the walk with
 * `depth`. `.git` is excluded because it is object storage rather than content
 * anybody browses, and walking it is pure cost.
 */
const excluded_directory_names = new Set([".git"]);

/** Matches on the final segment so a nested `packages/a/.git` is excluded too. */
const IsExcludedDirectory = (path: string) =>
	excluded_directory_names.has(path.split("/").at(-1) ?? path);

/** Reports a root-confined discovery request that could not be fulfilled. */
export class WorkspaceFileDiscoveryError extends Data.TaggedError("WorkspaceFileDiscoveryError")<{
	readonly reason: "invalid" | "unavailable";
}> {}

/** Owns renderer-safe, content-free workspace discovery and language capability projections. */
export class WorkspaceFileDiscovery extends Context.Service<
	WorkspaceFileDiscovery,
	{
		readonly Discover: (
			query: WorkspaceFileDiscoveryQueryValue,
		) => Effect.Effect<WorkspaceFileDiscoveryQueryResult, WorkspaceFileDiscoveryError>;
		readonly LanguageCapabilities: (
			query: typeof WorkspaceLanguageCapabilitiesQuery.Type,
		) => Effect.Effect<WorkspaceLanguageCapabilitiesQueryResult, WorkspaceFileDiscoveryError>;
	}
>()("Artisan/WorkspaceFileDiscovery") {}

const unavailable_capabilities = ["diagnostics", "language_detection", "symbols"] as const;

/** Provides bounded recursive metadata only; renderer code never receives a filesystem capability. */
export const WorkspaceFileDiscoveryLive = Layer.effect(
	WorkspaceFileDiscovery,
	Effect.gen(function* () {
		const registry = yield* WorkspaceFilesystemRegistry;
		const Discover = (query: WorkspaceFileDiscoveryQueryValue) =>
			Schema.decodeUnknownEffect(WorkspaceFileDiscoveryQuery, { onExcessProperty: "error" })(
				query,
			).pipe(
				Effect.mapError(() => new WorkspaceFileDiscoveryError({ reason: "invalid" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const { filesystem } = yield* registry
							.Get(decoded.workspace_id)
							.pipe(
								Effect.mapError(
									() =>
										new WorkspaceFileDiscoveryError({ reason: "unavailable" }),
								),
							);
						const maximum = decoded.limit ?? 200;
						/**
						 * Depth is measured from the prefix, not from the workspace root:
						 * a caller asking for one level under `modules/frontend` means its
						 * children, not the two levels it takes to walk down to it.
						 * Absent depth keeps the previous unbounded behaviour.
						 */
						const prefix_depth =
							decoded.prefix === undefined ? 0 : decoded.prefix.split("/").length;
						const maximum_depth =
							decoded.depth === undefined
								? Number.POSITIVE_INFINITY
								: decoded.depth + prefix_depth;
						const maximum_visited_entries = 10_000;
						let visited_entries = 0;
						let traversal_limited = false;
						const entries: Array<{
							kind: "directory" | "file";
							modified_at: string;
							path: string;
							size: number;
						}> = [];
						const Walk = (
							path: string,
							depth: number,
						): Effect.Effect<void, WorkspaceFileDiscoveryError> => {
							if (visited_entries >= maximum_visited_entries) {
								traversal_limited = true;
								return Effect.void;
							}
							if (
								depth > maximum_depth ||
								entries.length >= maximum + 1 ||
								(decoded.prefix !== undefined &&
									path !== "." &&
									path !== decoded.prefix &&
									!decoded.prefix.startsWith(`${path}/`))
							)
								return Effect.void;
							return filesystem.List(path).pipe(
								Effect.mapError(
									() =>
										new WorkspaceFileDiscoveryError({ reason: "unavailable" }),
								),
								Effect.flatMap((children) =>
									Effect.forEach(
										children.toSorted((left, right) =>
											left.path.localeCompare(right.path),
										),
										(entry) =>
											Effect.gen(function* () {
												visited_entries += 1;
												if (visited_entries > maximum_visited_entries)
													traversal_limited = true;
												if (
													visited_entries > maximum_visited_entries ||
													entries.length >= maximum + 1 ||
													entry.kind === "symlink" ||
													entry.kind === "other"
												)
													return;
												/** Only `.git`; see `excluded_directory_names`. */
												if (
													entry.kind === "directory" &&
													IsExcludedDirectory(entry.path)
												)
													return;
												if (
													decoded.prefix &&
													entry.path !== decoded.prefix &&
													!entry.path.startsWith(`${decoded.prefix}/`)
												) {
													if (
														entry.kind === "directory" &&
														decoded.prefix.startsWith(`${entry.path}/`)
													)
														yield* Walk(entry.path, depth + 1);
													return;
												}
												if (
													!decoded.after_path ||
													entry.path > decoded.after_path
												)
													entries.push({
														kind: entry.kind,
														modified_at: entry.modified_at,
														path: entry.path,
														size: entry.size,
													});
												if (entry.kind === "directory")
													yield* Walk(entry.path, depth + 1);
											}),
										{ concurrency: 1, discard: true },
									),
								),
							);
						};
						yield* Walk(".", 1);
						const truncated = entries.length > maximum || traversal_limited;
						const page = entries.slice(0, maximum);
						const last_entry = page.at(-1);
						return {
							entries: page,
							truncated,
							workspace_id: decoded.workspace_id,
							...(truncated && last_entry !== undefined
								? { next_path: last_entry.path }
								: {}),
						};
					}),
				),
			);
		const LanguageCapabilities = (query: typeof WorkspaceLanguageCapabilitiesQuery.Type) =>
			Schema.decodeUnknownEffect(WorkspaceLanguageCapabilitiesQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(() => new WorkspaceFileDiscoveryError({ reason: "invalid" })),
				Effect.flatMap((decoded) =>
					registry.Get(decoded.workspace_id).pipe(
						Effect.mapError(
							() => new WorkspaceFileDiscoveryError({ reason: "unavailable" }),
						),
						Effect.as({
							capabilities: unavailable_capabilities.map((feature) => ({
								feature,
								reason: "No backend language service is configured",
								source: "unavailable" as const,
								state: "unavailable" as const,
							})),
							workspace_id: decoded.workspace_id,
						}),
					),
				),
			);
		return { Discover, LanguageCapabilities };
	}),
);
