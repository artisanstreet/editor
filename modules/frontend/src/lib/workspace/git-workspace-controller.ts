import { Cache, Context, Effect, Exit, Fiber, Layer, Ref, Stream, SubscriptionRef } from "effect";

import type { GitRepositoryProjection, GitWorkspaceQuery } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

const maximum_retained_workspaces = 32;
const workspace_cache_ttl = "30 seconds";

export type GitWorkspaceState = ReadonlyMap<string, GitRepositoryProjection | undefined>;

/**
 * The Git query is authorized and journaled for the exact thread/workspace
 * pair. Keeping the compound identity opaque avoids accidentally sharing a
 * projection across threads that happen to name the same workspace.
 */
export const GitWorkspaceKey = (input: GitWorkspaceQuery): string =>
	JSON.stringify([input.thread_id, input.workspace_id]);

const GitWorkspaceInputForKey = (key: string): GitWorkspaceQuery => {
	const [thread_id, workspace_id] = JSON.parse(key) as [string, string];
	return { thread_id, workspace_id };
};

/**
 * Retains completed Git workspace projections for short-lived route surfaces.
 * Cache owns compound-keyed admission, expiry, and capacity; this service owns
 * the app-scoped refresh fibers and synchronous Svelte-facing projection.
 */
export class GitWorkspaceController extends Context.Service<
	GitWorkspaceController,
	{
		readonly Changes: Stream.Stream<GitWorkspaceState>;
		readonly Current: Effect.Effect<GitWorkspaceState>;
		readonly Invalidate: (input: GitWorkspaceQuery) => Effect.Effect<void>;
		readonly Load: (
			input: GitWorkspaceQuery,
		) => Effect.Effect<GitRepositoryProjection | undefined, ArtisanClientError>;
		readonly Refresh: (input: GitWorkspaceQuery | undefined) => Effect.Effect<void>;
	}
>()("Artisan/GitWorkspaceController") {}

export const GitWorkspaceControllerLive = Layer.effect(
	GitWorkspaceController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<GitWorkspaceState>(new Map());
		const generations = yield* Ref.make<ReadonlyMap<string, number>>(new Map());
		const cache = yield* Cache.makeWith<
			string,
			GitRepositoryProjection | undefined,
			ArtisanClientError
		>(
			(key) =>
				client
					.GetGitWorkspace(GitWorkspaceInputForKey(key))
					.pipe(
						Effect.map((result) =>
							result.workspace.repository_state === "repository"
								? result.workspace
								: undefined,
						),
					),
			{
				capacity: maximum_retained_workspaces,
				timeToLive: (exit) => (Exit.isSuccess(exit) ? workspace_cache_ttl : "0 millis"),
			},
		);

		const GenerationFor = (key: string) =>
			Ref.get(generations).pipe(Effect.map((current) => current.get(key) ?? 0));

		const Retain = (key: string, workspace: GitRepositoryProjection | undefined) =>
			SubscriptionRef.update(state, (current) => {
				const retained = new Map(current);
				retained.delete(key);
				retained.set(key, workspace);
				while (retained.size > maximum_retained_workspaces) {
					const oldest = retained.keys().next().value;
					if (oldest === undefined) break;
					retained.delete(oldest);
				}
				return retained;
			});

		const LoadAndPublish = (input: GitWorkspaceQuery, generation: number) => {
			const key = GitWorkspaceKey(input);
			return Cache.get(cache, key).pipe(
				Effect.tap((workspace) =>
					Effect.gen(function* () {
						if ((yield* GenerationFor(key)) !== generation) return;
						yield* Retain(key, workspace);
					}),
				),
			);
		};

		/** A consumer may leave, but its admitted app-owned lookup must continue. */
		const Load = (input: GitWorkspaceQuery) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const key = GitWorkspaceKey(input);
					const generation = yield* GenerationFor(key);
					const fiber = yield* Effect.forkIn(
						LoadAndPublish(input, generation),
						controller_scope,
					);
					return yield* restore(Fiber.join(fiber));
				}),
			);

		/**
		 * A background refresh cannot fail its caller, but it must not vanish
		 * either: swallowing it outright is what let every workspace-scoped Git
		 * read fail for months while the surface simply omitted its row. The
		 * failure is logged and then absorbed.
		 */
		const Refresh = (input: GitWorkspaceQuery | undefined) =>
			input === undefined
				? Effect.void
				: Load(input).pipe(
						Effect.tapError((cause) =>
							Effect.logWarning("Git workspace refresh failed", {
								cause,
								workspace_id: input.workspace_id,
							}),
						),
						Effect.ignore,
						Effect.asVoid,
						Effect.forkIn(controller_scope),
						Effect.asVoid,
					);

		const Invalidate = (input: GitWorkspaceQuery) => {
			const key = GitWorkspaceKey(input);
			return Effect.gen(function* () {
				yield* Ref.update(generations, (current) =>
					new Map(current).set(key, (current.get(key) ?? 0) + 1),
				);
				yield* Cache.invalidate(cache, key);
				yield* SubscriptionRef.update(state, (current) => {
					if (!current.has(key)) return current;
					const next = new Map(current);
					next.delete(key);
					return next;
				});
			});
		};

		return GitWorkspaceController.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Invalidate,
			Load,
			Refresh,
		});
	}),
);
