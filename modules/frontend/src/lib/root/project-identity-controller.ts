import { Context, Effect, Layer, Schedule, Stream, SubscriptionRef } from "effect";

import type { ProjectIdentitySource } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

const maximum_retained_project_identities = 128;
const ColdStartRetrySchedule = Schedule.exponential("100 millis").pipe(
	Schedule.upTo({ duration: "5 seconds" }),
);

type ProjectIdentityState = ReadonlyMap<string, ProjectIdentitySource>;

/** Keeps safe project identity metadata available synchronously to picker rows. */
export class ProjectIdentityController extends Context.Service<
	ProjectIdentityController,
	{
		readonly Changes: Stream.Stream<ProjectIdentityState>;
		readonly Current: Effect.Effect<ProjectIdentityState>;
		readonly Refresh: (
			project_ids: ReadonlyArray<string>,
		) => Effect.Effect<void, ArtisanClientError>;
	}
>()("Artisan/ProjectIdentityController") {}

export const ProjectIdentityControllerLive = Layer.effect(
	ProjectIdentityController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const state = yield* SubscriptionRef.make<ProjectIdentityState>(new Map());

		const Refresh = (project_ids: ReadonlyArray<string>) =>
			project_ids.length === 0
				? Effect.void
				: client.GetProjectIdentities(project_ids).pipe(
						Effect.retry({ schedule: ColdStartRetrySchedule }),
						Effect.flatMap(({ identities }) =>
							SubscriptionRef.update(state, (current) => {
								const retained = new Map(current);
								for (const identity of identities) {
									retained.delete(identity.project_id);
									retained.set(identity.project_id, identity);
								}
								while (retained.size > maximum_retained_project_identities) {
									const oldest = retained.keys().next().value;
									if (oldest === undefined) break;
									retained.delete(oldest);
								}
								return retained;
							}),
						),
					);

		return ProjectIdentityController.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Refresh,
		});
	}),
);
