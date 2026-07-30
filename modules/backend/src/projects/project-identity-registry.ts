import { eq } from "drizzle-orm";
import { Context, Data, Effect, Layer } from "effect";

import { SnowflakeId } from "@artisan/protocol";

import { Database } from "../persistence/database";
import { ProjectIdentities } from "../persistence/tables";
import { RuntimeMetadata } from "../runtime/metadata";

/** Reports a workspace-identity allocation failure without leaking storage details. */
export class ProjectIdentityRegistryError extends Data.TaggedError("ProjectIdentityRegistryError")<{
	readonly cause: unknown;
	readonly root_path: string;
}> {}

/**
 * Owns the durable workspace id for every project root.
 *
 * Ids are bare Snowflakes, exactly like thread ids, so they cannot be derived
 * from the path — identity is state. The registry allocates on first sight and
 * answers from storage forever after, which is what keeps an id stable across
 * detach, re-attach, and every replay of historical journal events.
 */
export class ProjectIdentityRegistry extends Context.Service<
	ProjectIdentityRegistry,
	{
		readonly Resolve: (
			root_path: string,
		) => Effect.Effect<string, ProjectIdentityRegistryError>;
	}
>()("Artisan/ProjectIdentityRegistry") {}

export const ProjectIdentityRegistryLive = Layer.effect(
	ProjectIdentityRegistry,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const snowflake_id = yield* SnowflakeId;

		const Lookup = (root_path: string) =>
			database.client
				.select()
				.from(ProjectIdentities)
				.where(eq(ProjectIdentities.root_path, root_path))
				.limit(1);

		const Resolve = (root_path: string) =>
			Effect.gen(function* () {
				const [existing] = yield* Lookup(root_path);
				if (existing) return existing.project_id;

				const project_id = yield* snowflake_id.MakeBare;
				const created_at = yield* metadata.Now;

				yield* database.client
					.insert(ProjectIdentities)
					.values({ created_at, project_id, root_path })
					.onConflictDoNothing({ target: ProjectIdentities.root_path });

				/** A concurrent resolve may have won the insert; the stored row is the authority. */
				const [row] = yield* Lookup(root_path);
				return row?.project_id ?? project_id;
			}).pipe(
				Effect.mapError((cause) => new ProjectIdentityRegistryError({ cause, root_path })),
			);

		return { Resolve };
	}),
);
