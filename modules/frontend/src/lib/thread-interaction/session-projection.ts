import { Context, Effect, Layer, Stream, SubscriptionRef } from "effect";

import type { ThreadSessionSnapshot } from "@artisan/protocol";

export interface ThreadSessionProjectionState {
	readonly sessions: ReadonlyMap<string, ThreadSessionSnapshot>;
}

const maximum_retained_sessions = 64;

/** Retains authoritative sessions already opened by thread routes for shell consumers. */
export class ThreadSessionProjection extends Context.Service<
	ThreadSessionProjection,
	{
		readonly Changes: Stream.Stream<ThreadSessionProjectionState>;
		readonly Current: (thread_id: string) => Effect.Effect<ThreadSessionSnapshot | undefined>;
		readonly Publish: (session: ThreadSessionSnapshot) => Effect.Effect<ThreadSessionSnapshot>;
	}
>()("Artisan/ThreadSessionProjection") {}

export const ThreadSessionProjectionLive = Layer.effect(
	ThreadSessionProjection,
	Effect.gen(function* () {
		const state = yield* SubscriptionRef.make<ThreadSessionProjectionState>({
			sessions: new Map(),
		});

		const Current = (thread_id: string) =>
			SubscriptionRef.get(state).pipe(
				Effect.map((current) => current.sessions.get(thread_id)),
			);

		const Publish = (session: ThreadSessionSnapshot) =>
			SubscriptionRef.updateAndGet(state, (current) => {
				const sessions = new Map(current.sessions);
				sessions.delete(session.thread_id);
				sessions.set(session.thread_id, session);
				while (sessions.size > maximum_retained_sessions) {
					const oldest = sessions.keys().next().value;
					if (oldest === undefined) break;
					sessions.delete(oldest);
				}
				return { sessions };
			}).pipe(Effect.as(session));

		return ThreadSessionProjection.of({
			Changes: SubscriptionRef.changes(state),
			Current,
			Publish,
		});
	}),
);
