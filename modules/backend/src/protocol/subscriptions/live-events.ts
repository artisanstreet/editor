import { Effect, Option, Ref } from "effect";
import type { EventEnvelope } from "@artisan/protocol";
import { AgentGraphOrchestrator } from "../../orchestration/agent-graph-orchestrator";
import { OrchestrationRepository } from "../../persistence/orchestration/repository";
import { ThreadReadModel } from "../../persistence/thread-read-model";
import { TranscriptReadModel } from "../../persistence/transcript-read-model";
import { RuntimeMetadata } from "../../runtime/metadata";
import { SurfaceService } from "../../surfaces/service";
import { thread_activity_kind_from_event } from "../../threads/internal/thread-activity";
import { WorkspaceChangeRepository } from "../../workspace/changes/repository";
import { ApplyEventCursors, LatestJournalSequence, type ReadyState } from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";
import { ConnectionConversationDelivery } from "./conversation-delivery";
import { DirectThreadListPatch, GraphGroupId } from "./patch-selection";

/** Constructs ordered live-event and projection delivery for one connection. */
export const MakeLiveEventDelivery = Effect.gen(function* () {
	const { state, Enqueue } = yield* ConnectionSubscriptionControl;
	const conversation_delivery = yield* ConnectionConversationDelivery;
	const graph = yield* AgentGraphOrchestrator;
	const orchestration = yield* OrchestrationRepository;
	const surfaces = yield* SurfaceService;
	const thread_read_model = yield* ThreadReadModel;
	const transcript_read_model = yield* TranscriptReadModel;
	const workspace_changes = yield* WorkspaceChangeRepository;
	const metadata = yield* RuntimeMetadata;
	const ReadWorkspaceConflictSnapshot = (thread_id: string) =>
		workspace_changes.ListConflictSnapshot(thread_id);
	const EnqueueProjectionPatches = (current: ReadyState, event: EventEnvelope) =>
		Effect.gen(function* () {
			let subscriptions = current.subscriptions;
			const has_thread_list = Object.values(current.subscriptions).some(
				(subscription) => subscription._tag === "thread.list",
			);
			let thread_patch = DirectThreadListPatch(event);

			/**
			 * Every upsert is re-read from the thread read model rather than
			 * trusted from the event. Emitters build their embedded items without
			 * the coordinator join, so a rename or affinity event would otherwise
			 * strip the engine the list already showed; the read model is the one
			 * place that joins the launch policy in. The policy event is a trigger
			 * of its own because applying it is what first creates the coordinator
			 * — the moment a new thread's engine becomes knowable at all.
			 */
			if (
				has_thread_list &&
				thread_patch?._tag !== "Remove" &&
				(thread_patch !== undefined ||
					event.payload.type === "thread.session_policy.updated" ||
					thread_activity_kind_from_event(event.payload) !== undefined)
			) {
				const embedded = thread_patch;
				const thread = yield* thread_read_model.Lookup(event.thread_id);

				thread_patch = Option.match(thread, {
					onNone: () => embedded,
					onSome: (item) => ({ _tag: "Upsert" as const, thread: item }),
				});
			}

			for (const [subscription_id, subscription] of Object.entries(current.subscriptions)) {
				if (subscription._tag === "conversation" || subscription._tag === "project.list")
					continue;
				const message_id = yield* metadata.MakeId("message");
				const sequence = subscription.sequence + 1;
				let next_journal_sequence = event.journal_sequence;
				let next_usage_thread_id =
					subscription._tag === "surface.usage.aggregate"
						? subscription.thread_id
						: undefined;

				if (subscription._tag === "thread.list") {
					if (!thread_patch) {
						continue;
					}

					if (thread_patch._tag === "Remove") {
						yield* Enqueue({
							journal_sequence: event.journal_sequence,
							kind: "thread.list.remove",
							message_id,
							origin: "backend",
							payload: { thread_id: thread_patch.thread_id },
							protocol_version: 1,
							schema_version: 1,
							sent_at: event.sent_at,
							sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					} else {
						yield* Enqueue({
							journal_sequence: event.journal_sequence,
							kind: "thread.list.upsert",
							message_id,
							origin: "backend",
							payload: thread_patch.thread,
							protocol_version: 1,
							schema_version: 1,
							sent_at: event.sent_at,
							sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					}
				} else if (subscription._tag === "thread.transcript") {
					if (event.journal_sequence <= subscription.journal_sequence) continue;
					if (event.thread_id !== subscription.thread_id) continue;
					const snapshot = yield* transcript_read_model.Read({
						after_journal_sequence: Math.max(0, event.journal_sequence - 1),
						limit: 500,
						thread_id: event.thread_id,
					});
					if (snapshot.status !== "available") {
						yield* Enqueue({
							journal_sequence: event.journal_sequence,
							kind: "thread.transcript.snapshot",
							message_id,
							origin: "backend",
							payload: snapshot,
							protocol_version: 1,
							schema_version: 1,
							sent_at: event.sent_at,
							sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					} else {
						const entries = snapshot.entries.filter(
							(entry) => entry.journal_sequence <= event.journal_sequence,
						);
						if (entries.length === 0) continue;
						yield* Enqueue({
							journal_sequence: event.journal_sequence,
							kind: "thread.transcript.append",
							message_id,
							origin: "backend",
							payload: { entries },
							protocol_version: 1,
							schema_version: 1,
							sent_at: event.sent_at,
							sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
					}
				} else if (subscription._tag === "orchestration.group.list") {
					if (event.journal_sequence <= subscription.journal_sequence) continue;
					if (event.thread_id !== subscription.thread_id) continue;
					if (event.payload.type === "thread.erased") {
						yield* Enqueue({
							journal_sequence: event.journal_sequence,
							kind: "orchestration.group.list.patch",
							message_id,
							origin: "backend",
							payload: {
								groups: [],
								journal_sequence: event.journal_sequence,
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: event.sent_at,
							sequence,
							stream_id: subscription.stream_id,
							subscription_id,
						});
						subscriptions = {
							...subscriptions,
							[subscription_id]: {
								...subscription,
								journal_sequence: event.journal_sequence,
								sequence,
							},
						};
						continue;
					}
					const group_id = GraphGroupId(event);
					if (!group_id) continue;
					const groups = yield* graph.ListGroups(
						subscription.thread_id,
						subscription.include_terminal,
					);
					yield* Enqueue({
						journal_sequence: event.journal_sequence,
						kind: "orchestration.group.list.patch",
						message_id,
						origin: "backend",
						payload: { groups, journal_sequence: event.journal_sequence },
						protocol_version: 1,
						schema_version: 1,
						sent_at: event.sent_at,
						sequence,
						stream_id: subscription.stream_id,
						subscription_id,
					});
				} else if (subscription._tag === "thread.session") {
					const refreshes_thread_session =
						event.payload.type === "intake.assessed" ||
						event.payload.type === "intake.assumption_recorded" ||
						event.payload.type === "thread.auto_steer.updated" ||
						event.payload.type === "thread.session_policy.updated" ||
						event.payload.type === "thread.message_routed" ||
						event.payload.type === "thread.erased";
					if (
						event.journal_sequence <= subscription.journal_sequence ||
						event.thread_id !== subscription.thread_id ||
						!refreshes_thread_session
					)
						continue;
					const snapshot = yield* orchestration.GetSession(subscription.thread_id);
					yield* Enqueue({
						journal_sequence: event.journal_sequence,
						kind: "thread.session.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at: event.sent_at,
						sequence,
						stream_id: subscription.stream_id,
						subscription_id,
					});
				} else if (subscription._tag === "surface.list") {
					if (
						event.journal_sequence <= subscription.journal_sequence ||
						event.thread_id !== subscription.query.thread_id
					)
						continue;
					const snapshot = yield* surfaces.ListSnapshot({
						thread_id: subscription.query.thread_id,
						...(subscription.query.run_id === undefined
							? {}
							: { run_id: subscription.query.run_id }),
						...(subscription.query.group_id === undefined
							? {}
							: { group_id: subscription.query.group_id }),
					});
					next_journal_sequence = snapshot.journal_sequence;
					yield* Enqueue({
						journal_sequence: snapshot.journal_sequence,
						kind: "surface.list.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at: event.sent_at,
						sequence,
						stream_id: subscription.stream_id,
						subscription_id,
					});
				} else if (subscription._tag === "workspace.conflict.list") {
					if (
						event.journal_sequence <= subscription.journal_sequence ||
						event.thread_id !== subscription.thread_id
					)
						continue;
					const snapshot = yield* ReadWorkspaceConflictSnapshot(subscription.thread_id);
					next_journal_sequence = snapshot.journal_sequence;
					yield* Enqueue({
						journal_sequence: snapshot.journal_sequence,
						kind: "workspace.conflict.list.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at: event.sent_at,
						sequence,
						stream_id: subscription.stream_id,
						subscription_id,
					});
				} else if (subscription._tag === "surface.usage.aggregate") {
					if (event.journal_sequence <= subscription.journal_sequence) continue;
					const erases_usage_scope =
						event.payload.type === "thread.erased" &&
						subscription.thread_id !== undefined &&
						event.thread_id === subscription.thread_id;
					if (
						!erases_usage_scope &&
						!(yield* surfaces.UsageEventAffects(subscription.query, event.run_id))
					)
						continue;
					const snapshot = yield* surfaces.AggregateUsageSnapshot(subscription.query);
					if (!erases_usage_scope) {
						next_usage_thread_id = yield* surfaces.UsageScopeThread(subscription.query);
					}
					next_journal_sequence = snapshot.journal_sequence;
					yield* Enqueue({
						journal_sequence: snapshot.journal_sequence,
						kind: "surface.usage.aggregate.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at: event.sent_at,
						sequence,
						stream_id: subscription.stream_id,
						subscription_id,
					});
				} else {
					const group_id = GraphGroupId(event);

					if (group_id !== subscription.group_id) {
						continue;
					}

					const projection = yield* graph.GetGraph(group_id);

					yield* Enqueue({
						journal_sequence: projection.journal_sequence,
						kind: "orchestration.graph.patch",
						message_id,
						origin: "backend" as const,
						payload: { graph: projection },
						protocol_version: 1 as const,
						schema_version: 1 as const,
						sent_at: event.sent_at,
						sequence,
						stream_id: subscription.stream_id,
						subscription_id,
					});
				}

				const next_subscription =
					subscription._tag === "thread.transcript" ||
					subscription._tag === "orchestration.group.list" ||
					subscription._tag === "thread.session" ||
					subscription._tag === "surface.list" ||
					subscription._tag === "surface.usage.aggregate" ||
					subscription._tag === "workspace.conflict.list"
						? {
								...subscription,
								journal_sequence: next_journal_sequence,
								sequence,
							}
						: { ...subscription, sequence };
				subscriptions = {
					...subscriptions,
					[subscription_id]:
						subscription._tag === "surface.usage.aggregate"
							? {
									...next_subscription,
									...(next_usage_thread_id === undefined
										? {}
										: { thread_id: next_usage_thread_id }),
								}
							: next_subscription,
				};
			}

			return subscriptions;
		});

	const DeliverLiveEvents = (events: ReadonlyArray<EventEnvelope>) =>
		Effect.gen(function* () {
			const current = yield* Ref.get(state);

			if (current._tag !== "Ready") {
				return;
			}

			let subscriptions = current.subscriptions;
			const new_events = events.filter(
				(event) => event.journal_sequence > current.delivered_journal_sequence,
			);

			for (const event of new_events) {
				yield* Enqueue(event);
				subscriptions = yield* EnqueueProjectionPatches(
					{ ...current, subscriptions },
					event,
				);
			}

			const delivered_journal_sequence = LatestJournalSequence(
				current.delivered_journal_sequence,
				new_events,
			);
			subscriptions = yield* conversation_delivery.EnqueuePatches({
				...current,
				delivered_journal_sequence,
				subscriptions,
			});
			yield* Ref.set(state, {
				...current,
				delivered_cursors: ApplyEventCursors(current.delivered_cursors, new_events),
				delivered_journal_sequence,
				subscriptions,
			});
		});

	return { DeliverLiveEvents };
});
