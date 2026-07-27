import { asc, eq } from "drizzle-orm";
import { Context, Data, Effect, Layer, PubSub, Schema, Scope } from "effect";

import { Project, ProjectCatalogSnapshot, type ProjectRef } from "@artisan/protocol";

import { Database } from "../persistence/database";
import { Projects } from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

export class ProjectCatalogError extends Data.TaggedError("ProjectCatalogError")<{
	readonly cause?: unknown;
	readonly code: "invalid_projection" | "not_found" | "persistence_failed";
	readonly project_id?: string;
}> {}

const DecodeProject = (value: unknown) =>
	Schema.decodeUnknownEffect(Project)(value).pipe(
		Effect.mapError((cause) => new ProjectCatalogError({ cause, code: "invalid_projection" })),
	);

const normalize_error = (cause: unknown) =>
	cause instanceof ProjectCatalogError
		? cause
		: new ProjectCatalogError({ cause, code: "persistence_failed" as const });

export class ProjectCatalog extends Context.Service<
	ProjectCatalog,
	{
		readonly Attach: (project: ProjectRef) => Effect.Effect<Project, ProjectCatalogError>;
		readonly Detach: (
			project_id: string,
		) => Effect.Effect<ProjectCatalogSnapshot, ProjectCatalogError>;
		readonly Find: (project_id: string) => Effect.Effect<Project, ProjectCatalogError>;
		readonly Snapshot: Effect.Effect<ProjectCatalogSnapshot, ProjectCatalogError>;
		readonly Subscribe: Effect.Effect<
			PubSub.Subscription<ProjectCatalogSnapshot>,
			never,
			Scope.Scope
		>;
	}
>()("Artisan/ProjectCatalog") {}

export const ProjectCatalogLive = Layer.effect(
	ProjectCatalog,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const changes = yield* Effect.acquireRelease(
			PubSub.unbounded<ProjectCatalogSnapshot>(),
			PubSub.shutdown,
		);

		const Snapshot = Effect.gen(function* () {
			const rows = yield* database.client
				.select()
				.from(Projects)
				.orderBy(asc(Projects.display_name), asc(Projects.project_id));
			const projects = yield* Effect.forEach(rows, DecodeProject);
			return yield* Schema.decodeUnknownEffect(ProjectCatalogSnapshot)({ projects });
		}).pipe(Effect.mapError(normalize_error));

		const Find = (project_id: string) =>
			Effect.gen(function* () {
				const [row] = yield* database.client
					.select()
					.from(Projects)
					.where(eq(Projects.project_id, project_id))
					.limit(1);
				if (!row) {
					return yield* new ProjectCatalogError({
						code: "not_found",
						project_id,
					});
				}
				return yield* DecodeProject(row);
			}).pipe(Effect.mapError(normalize_error));

		const Attach = (project: ProjectRef) =>
			Effect.gen(function* () {
				const now = yield* metadata.Now;
				yield* database.client
					.insert(Projects)
					.values({
						attached_at: now,
						display_name: project.display_name,
						project_id: project.project_id,
						root_path: project.root_path,
						updated_at: now,
					})
					.onConflictDoUpdate({
						set: {
							display_name: project.display_name,
							root_path: project.root_path,
							updated_at: now,
						},
						target: Projects.project_id,
					});
				const attached = yield* Find(project.project_id);
				yield* Snapshot.pipe(
					Effect.flatMap((snapshot) => PubSub.publish(changes, snapshot)),
				);
				return attached;
			}).pipe(Effect.mapError(normalize_error));

		const Detach = (project_id: string) =>
			database.client
				.delete(Projects)
				.where(eq(Projects.project_id, project_id))
				.pipe(
					Effect.andThen(Snapshot),
					Effect.tap((snapshot) => PubSub.publish(changes, snapshot)),
					Effect.mapError(normalize_error),
				);

		return { Attach, Detach, Find, Snapshot, Subscribe: PubSub.subscribe(changes) };
	}),
);
