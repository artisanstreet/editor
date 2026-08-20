import { Context, Effect, Layer, Option, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { ConversationItem, ConversationTurn } from "@artisan/protocol";

export type ConversationPlan = Extract<ConversationItem, { type: "plan" }>;

export type ThreadChecklistState =
	| { readonly _tag: "None" }
	| {
			readonly _tag: "Ready";
			readonly plan: ConversationPlan;
			readonly thread_id: string;
	  };

export interface ThreadChecklistLease {
	readonly Publish: (plan: Option.Option<ConversationPlan>) => Effect.Effect<void>;
	readonly Release: Effect.Effect<void>;
}

/** A Checklist remains useful only while at least one entry still needs work. */
export const conversation_plan_has_open_entries = (plan: ConversationPlan): boolean =>
	plan.entries.some((entry) => entry.state === "pending" || entry.state === "active");

/** Selects the canonical plan that appears last in conversation order. */
export const LatestConversationPlan = (
	items: ReadonlyArray<ConversationItem>,
	turns: ReadonlyArray<ConversationTurn>,
): Option.Option<ConversationPlan> => {
	let latest: ConversationPlan | undefined;
	for (const item of items) {
		if (item.type !== "plan") continue;
		if (
			latest === undefined ||
			item.ordinal > latest.ordinal ||
			(item.ordinal === latest.ordinal && item.id.localeCompare(latest.id) > 0) ||
			(item.ordinal === latest.ordinal &&
				item.id === latest.id &&
				item.revision > latest.revision)
		) {
			latest = item;
		}
	}
	if (latest === undefined || latest.state !== "active") return Option.none();
	const owning_turn = turns.find((turn) => turn.id === latest.turn_id);
	return owning_turn !== undefined &&
		(owning_turn.lifecycle === "pending" ||
			owning_turn.lifecycle === "streaming" ||
			owning_turn.lifecycle === "active" ||
			owning_turn.lifecycle === "waiting")
		? Option.some(latest)
		: Option.none();
};

export class ThreadChecklist extends Context.Service<
	ThreadChecklist,
	{
		readonly Acquire: (thread_id: string) => Effect.Effect<ThreadChecklistLease>;
		readonly Changes: Stream.Stream<ThreadChecklistState>;
		readonly Current: Effect.Effect<ThreadChecklistState>;
	}
>()("Artisan/ThreadChecklist") {}

/**
 * Bridges the thread route's canonical projection to the shell-owned inspector.
 * Leased ownership prevents an overlapping route's late finalizer or patch from
 * clearing the checklist already published by its replacement.
 */
export const ThreadChecklistLive = Layer.effect(
	ThreadChecklist,
	Effect.gen(function* () {
		const state = yield* SubscriptionRef.make<ThreadChecklistState>({ _tag: "None" });
		const active_owner = yield* Ref.make<number | undefined>(undefined);
		const next_owner_id = yield* Ref.make(0);
		const publication_lock = yield* Semaphore.make(1);

		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});

		const Acquire = (thread_id: string) =>
			Effect.gen(function* () {
				const owner_id = yield* Ref.updateAndGet(next_owner_id, (current) => current + 1);
				yield* publication_lock.withPermit(
					Effect.gen(function* () {
						yield* Ref.set(active_owner, owner_id);
						yield* SubscriptionRef.set(state, { _tag: "None" });
					}),
				);

				const Publish = (plan: Option.Option<ConversationPlan>) =>
					Effect.gen(function* () {
						yield* publication_lock.withPermit(
							Effect.gen(function* () {
								if ((yield* Ref.get(active_owner)) !== owner_id) return;
								yield* SubscriptionRef.set(
									state,
									Option.match(plan, {
										onNone: () => ({ _tag: "None" as const }),
										onSome: (value) => ({
											plan: value,
											thread_id,
											_tag: "Ready" as const,
										}),
									}),
								);
							}),
						);
					});

				const Release = publication_lock.withPermit(
					Effect.gen(function* () {
						if ((yield* Ref.get(active_owner)) !== owner_id) return;
						yield* Ref.set(active_owner, undefined);
						yield* SubscriptionRef.set(state, { _tag: "None" });
					}),
				);

				return { Publish, Release } satisfies ThreadChecklistLease;
			});

		return ThreadChecklist.of({
			Acquire,
			Changes: SubscriptionRef.changes(state),
			Current,
		});
	}),
);
