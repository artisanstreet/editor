import { Context, Effect, Fiber, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { TerminalSession } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

export type ThreadTerminalsState =
	| { readonly _tag: "Loading"; readonly thread_id: string; readonly workspace_id: string }
	| {
			readonly _tag: "Ready";
			readonly terminals: ReadonlyArray<TerminalSession>;
			readonly thread_id: string;
			readonly workspace_id: string;
	  }
	| { readonly _tag: "Unavailable"; readonly thread_id: string; readonly workspace_id: string };

type ActiveRefresh = {
	readonly fiber?: Fiber.Fiber<void, unknown>;
	readonly generation: number;
};

const KeyFor = (thread_id: string, workspace_id: string) => `${thread_id}:${workspace_id}`;

const Loading = (thread_id: string, workspace_id: string): ThreadTerminalsState => ({
	_tag: "Loading",
	thread_id,
	workspace_id,
});

/**
 * Keeps the terminal snapshots for the two most recently visited threads.
 * Listing work is admitted into this app layer rather than the route scope, so
 * a navigation cannot cancel a request another mount is about to reuse.
 */
export class ThreadTerminalsController extends Context.Service<
	ThreadTerminalsController,
	{
		readonly Changes: Stream.Stream<ReadonlyMap<string, ThreadTerminalsState>>;
		readonly Current: (
			thread_id: string | undefined,
			workspace_id: string | undefined,
		) => Effect.Effect<ThreadTerminalsState | undefined>;
		readonly Refresh: (
			thread_id: string | undefined,
			workspace_id: string | undefined,
		) => Effect.Effect<void>;
	}
>()("Artisan/ThreadTerminalsController") {}

export const ThreadTerminalsControllerLive = Layer.effect(
	ThreadTerminalsController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const active = yield* Ref.make<ReadonlyMap<string, ActiveRefresh>>(new Map());
		const generation = yield* Ref.make(0);
		const refresh_lock = yield* Semaphore.make(1);
		const state = yield* SubscriptionRef.make<ReadonlyMap<string, ThreadTerminalsState>>(
			new Map(),
		);

		const Retain = (next: ThreadTerminalsState) =>
			SubscriptionRef.update(state, (current) => {
				const retained = new Map(current);
				retained.delete(next.thread_id);
				retained.set(next.thread_id, next);
				while (retained.size > 2) retained.delete(retained.keys().next().value as string);
				return retained;
			});

		const Complete = (key: string, request_generation: number) =>
			Ref.update(active, (current) => {
				if (current.get(key)?.generation !== request_generation) return current;
				const next = new Map(current);
				next.delete(key);
				return next;
			});

		const Load = (thread_id: string, workspace_id: string, request_generation: number) => {
			const key = KeyFor(thread_id, workspace_id);
			return client.ListTerminals(thread_id, workspace_id).pipe(
				Effect.flatMap((terminals) =>
					Effect.gen(function* () {
						if ((yield* Ref.get(active)).get(key)?.generation !== request_generation)
							return;
						yield* Retain({ _tag: "Ready", terminals, thread_id, workspace_id });
					}),
				),
				Effect.catch(() =>
					Effect.gen(function* () {
						if ((yield* Ref.get(active)).get(key)?.generation !== request_generation)
							return;
						yield* Retain({ _tag: "Unavailable", thread_id, workspace_id });
					}),
				),
				Effect.ensuring(Complete(key, request_generation)),
			);
		};

		const Current = (thread_id: string | undefined, workspace_id: string | undefined) =>
			Effect.gen(function* () {
				if (thread_id === undefined || workspace_id === undefined) return undefined;
				const current = (yield* SubscriptionRef.get(state)).get(thread_id);
				return current?.workspace_id === workspace_id ? current : undefined;
			});

		const Refresh = (thread_id: string | undefined, workspace_id: string | undefined) =>
			Effect.uninterruptible(
				refresh_lock.withPermit(
					Effect.gen(function* () {
						if (thread_id === undefined || workspace_id === undefined) return;
						const key = KeyFor(thread_id, workspace_id);
						if ((yield* Ref.get(active)).has(key)) return;
						const request_generation = yield* Ref.updateAndGet(
							generation,
							(current) => current + 1,
						);
						const retained = yield* Current(thread_id, workspace_id);
						if (retained?._tag !== "Ready")
							yield* Retain(Loading(thread_id, workspace_id));
						yield* Ref.update(active, (current) =>
							new Map(current).set(key, { generation: request_generation }),
						);
						const fiber = yield* Effect.forkIn(
							Load(thread_id, workspace_id, request_generation),
							controller_scope,
						);
						yield* Ref.update(active, (current) => {
							if (current.get(key)?.generation !== request_generation) return current;
							return new Map(current).set(key, {
								fiber,
								generation: request_generation,
							});
						});
					}),
				),
			);

		return ThreadTerminalsController.of({
			Changes: SubscriptionRef.changes(state),
			Current,
			Refresh,
		});
	}),
);
