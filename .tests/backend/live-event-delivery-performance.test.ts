import { Deferred, Effect, Ref, Semaphore } from "effect";
import { describe, expect, it } from "vitest";

import type { EventEnvelope } from "@artisan/protocol";
import { AgentGraphOrchestrator } from "../../modules/backend/src/orchestration/agent-graph-orchestrator";
import { ConversationReadModel } from "../../modules/backend/src/conversation";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { TranscriptReadModel } from "../../modules/backend/src/persistence/transcript-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { SurfaceService } from "../../modules/backend/src/surfaces/service";
import { WorkspaceChangeRepository } from "../../modules/backend/src/workspace/changes/repository";
import type {
	ConnectionState,
	ProjectionSubscription,
} from "../../modules/backend/src/protocol/connection-state";
import {
	ConnectionConversationDelivery,
	MakeConnectionConversationDelivery,
} from "../../modules/backend/src/protocol/subscriptions/conversation-delivery";
import { ConnectionSubscriptionControl } from "../../modules/backend/src/protocol/subscriptions/control";
import { MakeLiveEventDelivery } from "../../modules/backend/src/protocol/subscriptions/live-events";

const copies = 24;

const graph_event = {
	journal_sequence: 1,
	message_id: "event_live_projection",
	origin: "backend",
	payload: {
		group_id: "group_live_projection",
		node_id: "run_live_projection",
		node_type: "agent_run",
		state: "running",
		type: "orchestration.graph.lifecycle",
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-14T12:00:00.000Z",
	stream_id: "journal",
	sequence: 1,
	thread_id: "thread_live_projection",
} as EventEnvelope;

const session_event = {
	...graph_event,
	journal_sequence: 2,
	message_id: "event_live_session",
	payload: { type: "thread.session_policy.updated" },
	sequence: 2,
} as EventEnvelope;

const surface_event = {
	...graph_event,
	journal_sequence: 3,
	message_id: "event_live_surface",
	payload: {
		message_id: "assistant_live_surface",
		text: "Surface-producing assistant reply",
		type: "assistant.message_completed",
	},
	sequence: 3,
} as EventEnvelope;

const conflict_event = {
	...graph_event,
	journal_sequence: 4,
	message_id: "event_live_conflict",
	payload: {
		action: "updated",
		conflict: {},
		type: "workspace.conflict.updated",
	},
	sequence: 4,
} as EventEnvelope;

const provider_origin = { provider: "engine_live_projection", reference: "native_live_projection" };

const provider_run_state_event = {
	...graph_event,
	journal_sequence: 5,
	message_id: "event_live_provider_state",
	payload: {
		action: "provider_state",
		group_id: "group_live_projection",
		node_id: "run_live_projection",
		node_type: "agent_run",
		state: "running",
		type: "orchestration.graph.lifecycle",
	},
	raw_origin: provider_origin,
	sequence: 5,
} as EventEnvelope;

const provider_terminal_event = {
	...provider_run_state_event,
	journal_sequence: 6,
	message_id: "event_live_provider_terminal",
	payload: {
		action: "finished",
		group_id: "group_live_projection",
		node_id: "run_live_projection",
		node_type: "agent_run",
		state: "complete",
		type: "orchestration.graph.lifecycle",
	},
	sequence: 6,
} as EventEnvelope;

const provider_message_event = {
	...graph_event,
	journal_sequence: 7,
	message_id: "event_live_provider_message",
	payload: {
		artifact: {},
		group_id: "group_live_projection",
		type: "artifact.recorded",
	},
	raw_origin: provider_origin,
	sequence: 7,
} as EventEnvelope;

const unrelated_provider_graph_event = {
	...provider_run_state_event,
	journal_sequence: 8,
	message_id: "event_live_provider_graph_command",
	payload: {
		action: "assigned",
		group_id: "group_live_projection",
		node_id: "run_live_projection",
		node_type: "agent_run",
		state: "running",
		type: "orchestration.graph.lifecycle",
	},
	sequence: 8,
} as EventEnvelope;

const subscriptions_for = (tags: ReadonlyArray<ProjectionSubscription["_tag"]>) =>
	Object.fromEntries(
		tags.flatMap((tag) =>
			Array.from({ length: copies }, (_, index) => {
				const common = {
					journal_sequence: 0,
					sequence: 0,
					stream_id: `${tag}_${index}`,
				};
				const subscription: ProjectionSubscription =
					tag === "conversation"
						? {
								...common,
								_tag: tag,
								patch_sequence: 0,
								thread_id: graph_event.thread_id,
							}
						: tag === "thread.transcript"
							? { ...common, _tag: tag, thread_id: graph_event.thread_id }
							: tag === "orchestration.group.list"
								? {
										...common,
										_tag: tag,
										include_terminal: false,
										thread_id: graph_event.thread_id,
									}
								: tag === "thread.session"
									? { ...common, _tag: tag, thread_id: graph_event.thread_id }
									: tag === "thread.work"
										? { ...common, _tag: tag, thread_id: graph_event.thread_id }
										: tag === "surface.list"
											? {
													...common,
													_tag: tag,
													query: { thread_id: graph_event.thread_id },
												}
											: tag === "workspace.conflict.list"
												? {
														...common,
														_tag: tag,
														thread_id: graph_event.thread_id,
													}
												: tag === "surface.usage.aggregate"
													? {
															...common,
															_tag: tag,
															query: {
																scope: "run",
																scope_id: "run_live_projection",
															},
														}
													: {
															...common,
															_tag: "orchestration.graph",
															group_id: "group_live_projection",
														};
				return [`${tag}_${index}`, subscription];
			}),
		),
	) as Readonly<Record<string, ProjectionSubscription>>;

const Deliver = (
	events: ReadonlyArray<EventEnvelope>,
	tags: ReadonlyArray<ProjectionSubscription["_tag"]>,
	options: { readonly projection_only?: boolean } = {},
) =>
	Effect.gen(function* () {
		const counts = yield* Ref.make({
			aggregate: 0,
			affects: 0,
			conversation: 0,
			conflicts: 0,
			graph: 0,
			groups: 0,
			session: 0,
			surfaces: 0,
			transcript: 0,
			usage_scope: 0,
			work: 0,
		});
		const subscriptions = subscriptions_for(tags);
		/** Delivery only serves claimed subscriptions; every registered id shares one live claim here. */
		const claim = {
			cancelled: yield* Deferred.make<void>(),
			message_id: "subscribe_live_projection",
			publication: yield* Semaphore.make(1),
			released: yield* Deferred.make<void>(),
		};
		const state = yield* Ref.make<ConnectionState>({
			_tag: "Ready",
			acknowledged_cursors: {},
			acknowledged_journal_sequence: 0,
			connection_id: "connection_live_projection",
			delivered_cursors: {},
			delivered_journal_sequence: 0,
			last_activity_ms: 0,
			stream_ticket: "ticket_live_projection",
			subscription_claims: Object.fromEntries(
				Object.keys(subscriptions).map((subscription_id) => [subscription_id, claim]),
			),
			subscriptions,
		});
		const increment = (key: keyof typeof counts extends never ? never : string) =>
			Ref.update(counts, (current) => ({
				...current,
				[key as keyof typeof current]: current[key as keyof typeof current] + 1,
			}));
		const control = ConnectionSubscriptionControl.of({
			state,
			Enqueue: () => Effect.void,
			EnqueueError: () => Effect.void,
		});
		const runtime_metadata = RuntimeMetadata.of({
			MakeId: () => Effect.succeed("message_live_projection"),
			Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
		} as never);
		const conversation_delivery = yield* MakeConnectionConversationDelivery.pipe(
			Effect.provideService(
				ConversationReadModel,
				ConversationReadModel.of({
					ReadPatches: () => increment("conversation").pipe(Effect.as([])),
					ReadSnapshot: () => Effect.die("Conversation snapshot must not be read"),
				} as never),
			),
			Effect.provideService(ConnectionSubscriptionControl, control),
			Effect.provideService(RuntimeMetadata, runtime_metadata),
		);
		const delivery = yield* MakeLiveEventDelivery.pipe(
			Effect.provideService(ConnectionSubscriptionControl, control),
			Effect.provideService(ConnectionConversationDelivery, conversation_delivery),
			Effect.provideService(
				AgentGraphOrchestrator,
				AgentGraphOrchestrator.of({
					GetGraph: () => increment("graph").pipe(Effect.as({ journal_sequence: 1 })),
					ListGroups: () => increment("groups").pipe(Effect.as([])),
				} as never),
			),
			Effect.provideService(
				OrchestrationRepository,
				OrchestrationRepository.of({
					GetSession: () => increment("session").pipe(Effect.as({})),
					GetWork: () => increment("work").pipe(Effect.as(undefined)),
				} as never),
			),
			Effect.provideService(
				SurfaceService,
				SurfaceService.of({
					AggregateUsageSnapshot: () =>
						increment("aggregate").pipe(
							Effect.as({ aggregate: {}, journal_sequence: 1 }),
						),
					ListSnapshot: () =>
						increment("surfaces").pipe(Effect.as({ items: [], journal_sequence: 1 })),
					UsageEventAffects: () => increment("affects").pipe(Effect.as(true)),
					UsageScopeThread: () => increment("usage_scope").pipe(Effect.as(undefined)),
				} as never),
			),
			Effect.provideService(ThreadReadModel, ThreadReadModel.of({} as never)),
			Effect.provideService(
				TranscriptReadModel,
				TranscriptReadModel.of({
					Read: () =>
						increment("transcript").pipe(
							Effect.as({ entries: [{ journal_sequence: 1 }], status: "available" }),
						),
				} as never),
			),
			Effect.provideService(
				WorkspaceChangeRepository,
				WorkspaceChangeRepository.of({
					ListConflictSnapshot: () =>
						increment("conflicts").pipe(Effect.as({ journal_sequence: 1 })),
				} as never),
			),
			Effect.provideService(RuntimeMetadata, runtime_metadata),
		);
		yield* delivery.DeliverLiveEvents(events, options);
		return yield* Ref.get(counts);
	});

describe("live event delivery performance", () => {
	it("skips unrelated projection reads while relevant events retain one read per equivalent key", async () => {
		const unrelated = await Effect.runPromise(
			Deliver(
				[graph_event],
				[
					"conversation",
					"thread.transcript",
					"orchestration.group.list",
					"surface.list",
					"workspace.conflict.list",
					"surface.usage.aggregate",
					"orchestration.graph",
				],
			),
		);
		const surfaces = await Effect.runPromise(
			Deliver(
				[surface_event],
				["conversation", "thread.transcript", "surface.list", "surface.usage.aggregate"],
			),
		);
		const conflicts = await Effect.runPromise(
			Deliver([conflict_event], ["workspace.conflict.list"]),
		);
		const other_thread = await Effect.runPromise(
			Deliver(
				[{ ...surface_event, thread_id: "thread_other" }],
				["conversation", "thread.transcript", "surface.list", "workspace.conflict.list"],
			),
		);
		const later_relevant = await Effect.runPromise(
			Deliver(
				[graph_event, surface_event],
				["conversation", "thread.transcript", "surface.list"],
			),
		);
		const empty_wake = await Effect.runPromise(Deliver([], ["conversation"]));
		const projection_wake = await Effect.runPromise(
			Deliver([], ["conversation", "surface.list", "surface.usage.aggregate"], {
				projection_only: true,
			}),
		);
		const session = await Effect.runPromise(Deliver([session_event], ["thread.session"]));
		const work = await Effect.runPromise(Deliver([graph_event], ["thread.work"]));
		const other_thread_work = await Effect.runPromise(
			Deliver([{ ...graph_event, thread_id: "thread_other" }], ["thread.work"]),
		);

		/**
		 * Conversation is the one projection never skipped: its patches are
		 * written outside the journal, so no event shape can prove a wake
		 * carried none. The cursor read costs one indexed query that returns
		 * nothing when nothing is pending.
		 */
		expect(unrelated).toEqual({
			aggregate: 0,
			affects: 0,
			conversation: 1,
			conflicts: 0,
			graph: 1,
			groups: 1,
			session: 0,
			surfaces: 0,
			transcript: 0,
			usage_scope: 0,
			work: 0,
		});
		expect(surfaces).toMatchObject({
			aggregate: 1,
			affects: 1,
			conversation: 1,
			surfaces: 1,
			transcript: 1,
			usage_scope: 1,
		});
		expect(conflicts.conflicts).toBe(1);
		expect(other_thread).toMatchObject({
			conversation: 1,
			conflicts: 0,
			surfaces: 0,
			transcript: 0,
		});
		expect(later_relevant).toMatchObject({ conversation: 1, surfaces: 1, transcript: 1 });
		/**
		 * A positive notifier whose trusted tail is empty is a duplicate wake —
		 * and a duplicate wake still drains conversation cursors, because
		 * projection writers can publish already-delivered watermarks after
		 * writing patches that are owed exactly this drain.
		 */
		expect(empty_wake.conversation).toBe(1);
		expect(projection_wake).toMatchObject({
			aggregate: 1,
			conversation: 1,
			surfaces: 1,
			usage_scope: 1,
		});
		expect(session.session).toBe(1);
		expect(work.work).toBe(1);
		expect(other_thread_work.work).toBe(0);
	});

	it("refreshes durable projections for provider observations without widening graph commands", async () => {
		const [run_state, terminal, message, unrelated] = await Promise.all([
			Effect.runPromise(
				Deliver([provider_run_state_event], ["conversation", "surface.list"]),
			),
			Effect.runPromise(Deliver([provider_terminal_event], ["conversation", "surface.list"])),
			Effect.runPromise(Deliver([provider_message_event], ["conversation", "surface.list"])),
			Effect.runPromise(
				Deliver([unrelated_provider_graph_event], ["conversation", "surface.list"]),
			),
		]);

		for (const counts of [run_state, terminal, message]) {
			expect(counts).toMatchObject({ conversation: 1, surfaces: 1 });
		}
		/** Surfaces stay predicate-gated; the conversation cursor is always consulted. */
		expect(unrelated).toMatchObject({ conversation: 1, surfaces: 0 });
	});
});
