import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	WorkspaceFileDiscoveryQuery,
	WorkspaceLanguageCapabilitiesQuery,
	type WorkspaceFileDiscoveryQuery as WorkspaceFileDiscoveryQueryValue,
	type WorkspaceFileDiscoveryQueryResult,
	type WorkspaceLanguageCapabilitiesQueryResult,
} from "@artisan/protocol";

import { WorkspaceFilesystemRegistry } from "../filesystem/workspace-filesystem-registry";

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
						): Effect.Effect<void, WorkspaceFileDiscoveryError> => {
							if (visited_entries >= maximum_visited_entries) {
								traversal_limited = true;
								return Effect.void;
							}
							if (
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
												if (
													decoded.prefix &&
													entry.path !== decoded.prefix &&
													!entry.path.startsWith(`${decoded.prefix}/`)
												) {
													if (
														entry.kind === "directory" &&
														decoded.prefix.startsWith(`${entry.path}/`)
													)
														yield* Walk(entry.path);
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
													yield* Walk(entry.path);
											}),
										{ concurrency: 1, discard: true },
									),
								),
							);
						};
						yield* Walk(".");
						const truncated = entries.length > maximum || traversal_limited;
						const page = entries.slice(0, maximum);
						return {
							entries: page,
							truncated,
							workspace_id: decoded.workspace_id,
							...(truncated && page.at(-1) ? { next_path: page.at(-1)!.path } : {}),
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
