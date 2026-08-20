import {
	Context,
	Effect,
	Fiber,
	FiberMap,
	Layer,
	Ref,
	Semaphore,
	Stream,
	SubscriptionRef,
} from "effect";

import type { OrchestrationGraph, OrchestrationGroupListSnapshot } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

import { RunAuthoritativeSubscription } from "../conversation/subscription";
import { RosterForOrchestrationGraphs, type ThreadOrchestrationState } from "./roster";

export type { ThreadOrchestrationState } from "./roster";

export interface ThreadOrchestrationLease {
	readonly Release: Effect.Effect<void>;
	readonly Select: (thread_id: string | undefined) => Effect.Effect<void>;
}

export interface ThreadAgentInspection {
	readonly agent_id: string;
	readonly display_name: string;
	readonly group_id: string;
	readonly run_id: string | undefined;
	readonly state: import("@artisan/protocol").OrchestrationLifecycleState;
	readonly thread_id: string;
}

type ActiveRoster = {
	readonly fiber?: Fiber.Fiber<void, unknown>;
	readonly owner_id?: number;
	readonly thread_id?: string;
};

/**
 * Owns the one authoritative orchestration roster for the shell's active
 * thread. A lease makes a late route teardown unable to clear its successor.
 */
export class ThreadOrchestrationRoster extends Context.Service<
	ThreadOrchestrationRoster,
	{
		readonly Acquire: (
			thread_id: string | undefined,
		) => Effect.Effect<ThreadOrchestrationLease>;
		readonly Changes: Stream.Stream<ThreadOrchestrationState>;
		readonly Current: Effect.Effect<ThreadOrchestrationState>;
		readonly CurrentInspection: Effect.Effect<ThreadAgentInspection | undefined>;
		readonly InspectionChanges: Stream.Stream<ThreadAgentInspection | undefined>;
		readonly Inspect: (
			entry: import("./roster").OrchestrationRosterEntry,
		) => Effect.Effect<void>;
		readonly ReturnToRoot: Effect.Effect<void>;
	}
>()("Artisan/ThreadOrchestrationRoster") {}

export const ThreadOrchestrationRosterLive = Layer.effect(
	ThreadOrchestrationRoster,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const active = yield* Ref.make<ActiveRoster>({});
		const next_owner_id = yield* Ref.make(0);
		const state = yield* SubscriptionRef.make<ThreadOrchestrationState>({ _tag: "None" });
		const inspection = yield* SubscriptionRef.make<ThreadAgentInspection | undefined>(
			undefined,
		);
		const selection_lock = yield* Semaphore.make(1);

		const Publish = (
			owner_id: number,
			thread_id: string,
			graphs: ReadonlyArray<OrchestrationGraph>,
		) =>
			Effect.gen(function* () {
				const selected = yield* Ref.get(active);
				if (selected.owner_id !== owner_id || selected.thread_id !== thread_id) return;
				const entries = RosterForOrchestrationGraphs(graphs);
				const next_state: ThreadOrchestrationState =
					entries.length === 0 ? { _tag: "None" } : { _tag: "Ready", entries, thread_id };
				yield* SubscriptionRef.set(state, next_state);
				const current_inspection = yield* SubscriptionRef.get(inspection);
				if (current_inspection?.thread_id !== thread_id) return;
				const refreshed = entries.find(
					(entry) =>
						entry.group_id === current_inspection.group_id &&
						entry.agent_id === current_inspection.agent_id,
				);
				if (refreshed !== undefined) {
					yield* SubscriptionRef.set(inspection, { ...refreshed, thread_id });
				}
			});

		const ConsumeGroups = (owner_id: number, thread_id: string) =>
			Effect.gen(function* () {
				const graph_fibers = yield* FiberMap.make<string>();
				const graphs = yield* Ref.make<ReadonlyMap<string, OrchestrationGraph>>(new Map());
				const group_ids = yield* Ref.make<ReadonlySet<string>>(new Set());
				const RefreshGraph = (group_id: string) =>
					Effect.gen(function* () {
						const graph = yield* client.GetOrchestrationGraph(group_id);
						const current_group_ids = yield* Ref.get(group_ids);
						if (!current_group_ids.has(graph.group.group_id)) return;
						const current = yield* Ref.get(graphs);
						const next = new Map(current).set(graph.group.group_id, graph);
						yield* Ref.set(graphs, next);
						yield* Publish(owner_id, thread_id, [...next.values()]);
					});

				const ReconcileGraphs = (snapshot: OrchestrationGroupListSnapshot) =>
					Effect.gen(function* () {
						const next_group_ids = new Set(
							snapshot.groups.map((group) => group.group_id),
						);
						yield* Ref.set(group_ids, next_group_ids);
						const current = yield* Ref.get(graphs);
						const next = new Map(
							[...current].filter(([group_id]) => next_group_ids.has(group_id)),
						);
						yield* Ref.set(graphs, next);
						yield* Publish(owner_id, thread_id, [...next.values()]);
						yield* Effect.forEach(
							[...current.keys()].filter((group_id) => !next_group_ids.has(group_id)),
							(group_id) => FiberMap.remove(graph_fibers, group_id),
							{ discard: true },
						);
						yield* Effect.forEach(
							snapshot.groups,
							(group) =>
								FiberMap.run(graph_fibers, group.group_id, { onlyIfMissing: true })(
									RunAuthoritativeSubscription(
										client.SubscribeOrchestrationGraph(group.group_id),
										(update) =>
											Effect.gen(function* () {
												const current_group_ids = yield* Ref.get(group_ids);
												if (
													!current_group_ids.has(
														update.graph.group.group_id,
													)
												)
													return;
												const current = yield* Ref.get(graphs);
												const next = new Map(current).set(
													update.graph.group.group_id,
													update.graph,
												);
												yield* Ref.set(graphs, next);
												yield* Publish(owner_id, thread_id, [
													...next.values(),
												]);
											}),
										RefreshGraph(group.group_id).pipe(
											Effect.catch(() => Effect.gen(function* () {})),
										),
									),
								),
							{ discard: true },
						);
					});

				const Refresh = Effect.gen(function* () {
					yield* ReconcileGraphs(yield* client.ListOrchestrationGroups(thread_id, false));
				});

				/**
				 * The group stream emits the cold snapshot, avoiding control reads for a
				 * healthy connection. A failed stream performs one authoritative list
				 * recovery before the bounded retry subscribes again.
				 */
				yield* RunAuthoritativeSubscription(
					client.SubscribeOrchestrationGroups(thread_id, false),
					(update) =>
						ReconcileGraphs(update.snapshot).pipe(
							Effect.catch(() => Effect.gen(function* () {})),
						),
					Refresh.pipe(Effect.catch(() => Effect.gen(function* () {}))),
				);
			});

		const Select = (owner_id: number, thread_id: string | undefined) =>
			Effect.gen(function* () {
				yield* selection_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* Ref.get(active);
						if (current.owner_id !== owner_id || current.thread_id === thread_id)
							return;
						if (current.fiber !== undefined) yield* Fiber.interrupt(current.fiber);
						if (thread_id === undefined) {
							yield* Ref.set(active, { owner_id });
							yield* SubscriptionRef.set(state, { _tag: "None" });
							yield* SubscriptionRef.set(inspection, undefined);
							return;
						}
						yield* Ref.set(active, { owner_id, thread_id });
						yield* SubscriptionRef.set(state, { _tag: "None" });
						yield* SubscriptionRef.set(inspection, undefined);
						const fiber = yield* Effect.gen(function* () {
							yield* Effect.scoped(ConsumeGroups(owner_id, thread_id));
						}).pipe(Effect.forkIn(controller_scope));
						yield* Ref.set(active, { fiber, owner_id, thread_id });
					}),
				);
			});

		const Release = (owner_id: number) =>
			Effect.gen(function* () {
				yield* selection_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* Ref.get(active);
						if (current.owner_id !== owner_id) return;
						if (current.fiber !== undefined) yield* Fiber.interrupt(current.fiber);
						yield* Ref.set(active, {});
						yield* SubscriptionRef.set(state, { _tag: "None" });
						yield* SubscriptionRef.set(inspection, undefined);
					}),
				);
			});

		const Acquire = (thread_id: string | undefined) =>
			Effect.gen(function* () {
				const owner_id = yield* Ref.updateAndGet(next_owner_id, (current) => current + 1);
				yield* selection_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* Ref.get(active);
						if (current.fiber !== undefined) yield* Fiber.interrupt(current.fiber);
						yield* Ref.set(active, { owner_id });
						yield* SubscriptionRef.set(state, { _tag: "None" });
						yield* SubscriptionRef.set(inspection, undefined);
					}),
				);
				yield* Select(owner_id, thread_id);
				return {
					Release: Release(owner_id),
					Select: (next_thread_id) =>
						Effect.gen(function* () {
							yield* Select(owner_id, next_thread_id);
						}),
				} satisfies ThreadOrchestrationLease;
			});
		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});
		const Inspect = (entry: import("./roster").OrchestrationRosterEntry) =>
			Effect.gen(function* () {
				const current = yield* Ref.get(active);
				if (current.thread_id === undefined) return;
				yield* SubscriptionRef.set(inspection, { ...entry, thread_id: current.thread_id });
			});
		const ReturnToRoot = Effect.gen(function* () {
			yield* SubscriptionRef.set(inspection, undefined);
		});
		const CurrentInspection = Effect.gen(function* () {
			return yield* SubscriptionRef.get(inspection);
		});

		return ThreadOrchestrationRoster.of({
			Acquire,
			Changes: SubscriptionRef.changes(state),
			Current,
			CurrentInspection,
			InspectionChanges: SubscriptionRef.changes(inspection),
			Inspect,
			ReturnToRoot,
		});
	}),
);
