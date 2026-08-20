import {
	Cache,
	Context,
	Effect,
	Exit,
	Fiber,
	Layer,
	Ref,
	Schedule,
	Stream,
	SubscriptionRef,
} from "effect";

import type { ProjectRepository } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

const maximum_retained_repositories = 32;
const repository_cache_ttl = "30 seconds";
const ColdStartRetrySchedule = Schedule.exponential("100 millis").pipe(
	Schedule.upTo({ duration: "5 seconds" }),
);

type RepositoryState = ReadonlyMap<string, ProjectRepository | undefined>;

/**
 * Keeps project repository inspection resident for the shell's short-lived
 * presentation surfaces. Cache owns keyed admission, TTL, capacity, and
 * single-flight lookup; this projection only makes completed values available
 * to Svelte synchronously while a stale entry is refreshed in the background.
 */
export class ProjectRepositoryController extends Context.Service<
	ProjectRepositoryController,
	{
		readonly Changes: Stream.Stream<RepositoryState>;
		readonly Current: Effect.Effect<RepositoryState>;
		readonly Invalidate: (project_id: string) => Effect.Effect<void>;
		readonly Load: (
			project_id: string,
		) => Effect.Effect<ProjectRepository | undefined, ArtisanClientError>;
		readonly Refresh: (project_id: string | undefined) => Effect.Effect<void>;
	}
>()("Artisan/ProjectRepositoryController") {}

export const ProjectRepositoryControllerLive = Layer.effect(
	ProjectRepositoryController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<RepositoryState>(new Map());
		const generations = yield* Ref.make<ReadonlyMap<string, number>>(new Map());
		const cache = yield* Cache.makeWith<
			string,
			ProjectRepository | undefined,
			ArtisanClientError
		>(
			(project_id) =>
				client.GetProjectRepositories([project_id]).pipe(
					Effect.map(
						(result) =>
							result.repositories.find((entry) => entry.project_id === project_id)
								?.repository,
					),
					Effect.retry({ schedule: ColdStartRetrySchedule }),
				),
			{
				capacity: maximum_retained_repositories,
				timeToLive: (exit) => (Exit.isSuccess(exit) ? repository_cache_ttl : "0 millis"),
			},
		);

		const GenerationFor = (project_id: string) =>
			Ref.get(generations).pipe(Effect.map((current) => current.get(project_id) ?? 0));

		const Retain = (project_id: string, repository: ProjectRepository | undefined) =>
			SubscriptionRef.update(state, (current) => {
				const retained = new Map(current);
				retained.delete(project_id);
				retained.set(project_id, repository);
				while (retained.size > maximum_retained_repositories) {
					const oldest = retained.keys().next().value;
					if (oldest === undefined) break;
					retained.delete(oldest);
				}
				return retained;
			});

		const LoadAndPublish = (project_id: string, generation: number) =>
			Cache.get(cache, project_id).pipe(
				Effect.tap((repository) =>
					Effect.gen(function* () {
						if ((yield* GenerationFor(project_id)) !== generation) return;
						yield* Retain(project_id, repository);
					}),
				),
			);

		/**
		 * Admission happens in the app scope before the consumer joins it. A route
		 * leaving mid-flight therefore cannot cancel the Cache lookup a later
		 * header or context card can still reuse.
		 */
		const Load = (project_id: string) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const generation = yield* GenerationFor(project_id);
					const fiber = yield* Effect.forkIn(
						LoadAndPublish(project_id, generation),
						controller_scope,
					);
					return yield* restore(Fiber.join(fiber));
				}),
			);

		const Refresh = (project_id: string | undefined) =>
			project_id === undefined
				? Effect.void
				: Load(project_id).pipe(
						Effect.ignore,
						Effect.asVoid,
						Effect.forkIn(controller_scope),
						Effect.asVoid,
					);

		const Invalidate = (project_id: string) =>
			Effect.gen(function* () {
				yield* Ref.update(generations, (current) =>
					new Map(current).set(project_id, (current.get(project_id) ?? 0) + 1),
				);
				yield* Cache.invalidate(cache, project_id);
				yield* SubscriptionRef.update(state, (current) => {
					if (!current.has(project_id)) return current;
					const next = new Map(current);
					next.delete(project_id);
					return next;
				});
			});

		return ProjectRepositoryController.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Invalidate,
			Load,
			Refresh,
		});
	}),
);
