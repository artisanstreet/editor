import { Context, Deferred, Effect, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { Project, ThreadListItem } from "@artisan/protocol";
import {
	ArtisanClient,
	type ArtisanClientError,
	type ProjectCatalogUpdate,
	type ThreadListUpdate,
} from "@artisan/transport/client";
import { RunAuthoritativeSubscription } from "../conversation/subscription";
import { ApplyRootThreadListUpdate } from "./thread-navigation";

/** The shell-owned projection shared by every route that names a project or thread. */
export interface WorkspaceCatalogState {
	readonly projects: ReadonlyArray<Project>;
	readonly projects_loaded: boolean;
	readonly threads: ReadonlyArray<ThreadListItem>;
	readonly threads_loaded: boolean;
}

const InitialState: WorkspaceCatalogState = {
	projects: [],
	projects_loaded: false,
	threads: [],
	threads_loaded: false,
};

type RefreshResult = WorkspaceCatalogState;
type RefreshError = ArtisanClientError;

type RefreshKind = "projects" | "snapshot" | "threads";

type RefreshRevision = {
	readonly projects: number;
	readonly threads: number;
};

type RefreshFlight = {
	readonly deferred: Deferred.Deferred<RefreshResult, RefreshError>;
	readonly revision: RefreshRevision;
};

type CatalogControl = {
	readonly in_flight: Readonly<Partial<Record<RefreshKind, RefreshFlight>>>;
	readonly projects_revision: number;
	readonly threads_revision: number;
};

export class WorkspaceCatalogController extends Context.Service<
	WorkspaceCatalogController,
	{
		readonly ApplyThreadListUpdate: (
			update: ThreadListUpdate,
		) => Effect.Effect<WorkspaceCatalogState>;
		readonly ApplyProjectCatalogUpdate: (
			update: ProjectCatalogUpdate,
		) => Effect.Effect<WorkspaceCatalogState>;
		readonly Changes: Stream.Stream<WorkspaceCatalogState>;
		readonly Current: Effect.Effect<WorkspaceCatalogState>;
		readonly Refresh: Effect.Effect<WorkspaceCatalogState, RefreshError>;
		readonly RefreshProjects: Effect.Effect<WorkspaceCatalogState, RefreshError>;
		readonly RefreshThreads: Effect.Effect<WorkspaceCatalogState, RefreshError>;
		readonly SubscribeProjects: Effect.Effect<void>;
		readonly SubscribeThreadList: Effect.Effect<void>;
	}
>()("Artisan/WorkspaceCatalogController") {}

/**
 * Retains the one authoritative project/thread snapshot for the browser runtime.
 * Snapshot reads publish once only after both independent Forge queries answer;
 * concurrent callers join the same deferred work instead of creating a waterfall.
 */
export const WorkspaceCatalogControllerLive = Layer.effect(
	WorkspaceCatalogController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<WorkspaceCatalogState>(InitialState);
		const publication_lock = yield* Semaphore.make(1);
		const control = yield* Ref.make<CatalogControl>({
			in_flight: {},
			projects_revision: 0,
			threads_revision: 0,
		});

		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});

		const ApplyThreadListUpdate = (update: ThreadListUpdate) =>
			publication_lock.withPermits(1)(
				Effect.gen(function* () {
					yield* Ref.update(control, (current) => ({
						...current,
						threads_revision: current.threads_revision + 1,
					}));
					return yield* SubscriptionRef.updateAndGet(state, (current) => ({
						...current,
						threads: ApplyRootThreadListUpdate(current.threads, update),
						threads_loaded: true,
					}));
				}),
			);

		const ApplyProjectCatalogUpdate = (update: ProjectCatalogUpdate) =>
			publication_lock.withPermits(1)(
				Effect.gen(function* () {
					yield* Ref.update(control, (current) => ({
						...current,
						projects_revision: current.projects_revision + 1,
					}));
					return yield* SubscriptionRef.updateAndGet(state, (current) => ({
						...current,
						projects: update.snapshot.projects,
						projects_loaded: true,
					}));
				}),
			);

		const Commit = (
			revision: RefreshRevision,
			next: {
				readonly projects?: ReadonlyArray<Project>;
				readonly threads?: ReadonlyArray<ThreadListItem>;
			},
		) =>
			publication_lock.withPermits(1)(
				Effect.gen(function* () {
					const current_control = yield* Ref.get(control);
					return yield* SubscriptionRef.updateAndGet(state, (current) => ({
						...current,
						...(next.projects === undefined ||
						current_control.projects_revision !== revision.projects
							? {}
							: { projects: next.projects, projects_loaded: true }),
						...(next.threads === undefined ||
						current_control.threads_revision !== revision.threads
							? {}
							: {
									threads: ApplyRootThreadListUpdate(current.threads, {
										journal_sequence: 0,
										threads: next.threads,
										type: "snapshot",
									}),
									threads_loaded: true,
								}),
					}));
				}),
			);

		const Complete = (
			kind: RefreshKind,
			flight: RefreshFlight,
			load: Effect.Effect<RefreshResult, RefreshError>,
		) =>
			load.pipe(
				Effect.exit,
				Effect.flatMap((exit) =>
					Effect.gen(function* () {
						yield* Ref.update(control, (current) => {
							if (current.in_flight[kind] !== flight) return current;
							const in_flight = { ...current.in_flight };
							delete in_flight[kind];
							return { ...current, in_flight };
						});
						yield* Deferred.done(flight.deferred, exit);
					}),
				),
				Effect.asVoid,
			);

		const JoinOrStart = (
			kind: RefreshKind,
			load: (revision: RefreshRevision) => Effect.Effect<RefreshResult, RefreshError>,
		) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const deferred = yield* Deferred.make<RefreshResult, RefreshError>();
					const claimed = yield* Ref.modify(control, (current) => {
						const active = current.in_flight[kind];
						if (active !== undefined) return [active, current] as const;
						const projects_revision =
							kind === "projects" || kind === "snapshot"
								? current.projects_revision + 1
								: current.projects_revision;
						const threads_revision =
							kind === "threads" || kind === "snapshot"
								? current.threads_revision + 1
								: current.threads_revision;
						const flight = {
							deferred,
							revision: { projects: projects_revision, threads: threads_revision },
						} satisfies RefreshFlight;
						return [
							flight,
							{
								in_flight: { ...current.in_flight, [kind]: flight },
								projects_revision,
								threads_revision,
							},
						] as const;
					});
					if (claimed.deferred !== deferred)
						return yield* restore(Deferred.await(claimed.deferred));

					yield* Effect.forkIn(
						Complete(kind, claimed, load(claimed.revision)),
						controller_scope,
					);
					return yield* restore(Deferred.await(deferred));
				}),
			);

		const Refresh = JoinOrStart("snapshot", (revision) =>
			Effect.gen(function* () {
				const [projects, threads] = yield* Effect.all(
					[client.ListProjects, client.ListThreads],
					{
						concurrency: "unbounded",
					},
				);
				return yield* Commit(revision, { projects: projects.projects, threads });
			}),
		);

		const RefreshProjects = JoinOrStart("projects", (revision) =>
			Effect.gen(function* () {
				const projects = yield* client.ListProjects;
				return yield* Commit(revision, { projects: projects.projects });
			}),
		);

		const RefreshThreads = JoinOrStart("threads", (revision) =>
			Effect.gen(function* () {
				const threads = yield* client.ListThreads;
				return yield* Commit(revision, { threads });
			}),
		);

		/**
		 * The subscription runner already treats a failed durable refresh as a
		 * retryable recovery boundary. Keep that failure local to this recovery
		 * attempt while adapting the refresh result to the runner's void contract.
		 */
		const RecoverProjects = RefreshProjects.pipe(
			Effect.asVoid,
			Effect.catch(() => Effect.void),
		);
		const RecoverThreads = RefreshThreads.pipe(
			Effect.asVoid,
			Effect.catch(() => Effect.void),
		);

		const SubscribeProjects = RunAuthoritativeSubscription(
			client.SubscribeProjects,
			ApplyProjectCatalogUpdate,
			RecoverProjects,
		).pipe(Effect.catch(() => Effect.void));

		const SubscribeThreadList = RunAuthoritativeSubscription(
			client.SubscribeThreadList,
			ApplyThreadListUpdate,
			RecoverThreads,
		).pipe(Effect.catch(() => Effect.void));

		return WorkspaceCatalogController.of({
			ApplyProjectCatalogUpdate,
			ApplyThreadListUpdate,
			Changes: SubscriptionRef.changes(state),
			Current,
			Refresh,
			RefreshProjects,
			RefreshThreads,
			SubscribeProjects,
			SubscribeThreadList,
		});
	}),
);
