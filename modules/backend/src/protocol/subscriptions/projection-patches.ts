import { Cause, Effect, Option } from "effect";
import type { EventEnvelope, OutboundControlEnvelope } from "@artisan/protocol";
import { AgentGraphOrchestrator } from "../../orchestration/agent-graph-orchestrator";
import { OrchestrationRepository } from "../../persistence/orchestration/repository";
import { ThreadReadModel } from "../../persistence/thread-read-model";
import { TranscriptReadModel } from "../../persistence/transcript-read-model";
import { RuntimeMetadata } from "../../runtime/metadata";
import { SurfaceService } from "../../surfaces/service";
import {
	thread_activity_kind_from_event,
	thread_message_sent_from_event,
} from "../../threads/internal/thread-activity";
import { WorkspaceChangeRepository } from "../../workspace/changes/repository";
import type { PendingSubscription, ProjectionSubscription, ReadyState } from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";
import {
	DirectThreadListPatch,
	EventAffectsSurface,
	EventAffectsTranscript,
	EventAffectsWorkspaceConflicts,
	GraphGroupId,
} from "./patch-selection";

/**
 * Builds one connection's per-event projection patch delivery: every retained
 * non-conversation subscription reads its projection and enqueues its patch,
 * isolated so one poisoned read cannot starve its siblings.
 */
export const MakeConnectionProjectionPatches = Effect.gen(function* () {
	const { Enqueue: EnqueueUnsafe, PublishClaim: PublishClaimOption } =
		yield* ConnectionSubscriptionControl;
	const PublishClaim =
		PublishClaimOption ??
		(<A, E, R>(
			_subscription_id: string,
			_claim: PendingSubscription,
			publication: Effect.Effect<A, E, R>,
		) => publication.pipe(Effect.map(Option.some)));
	const PublishProjection = (
		current: ReadyState,
		subscription_id: string,
		publication: Effect.Effect<void>,
	) => {
		const claim = current.subscription_claims?.[subscription_id];
		return claim === undefined
			? Effect.succeed(Option.none<void>())
			: PublishClaim(subscription_id, claim, publication);
	};
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
			const Enqueue = (envelope: OutboundControlEnvelope) =>
				PublishProjection(
					current,
					"subscription_id" in envelope ? envelope.subscription_id : "",
					EnqueueUnsafe(envelope),
				).pipe(Effect.asVoid);
			/**
			 * A connection can legitimately carry several equivalent projections
			 * (for example while two renderer surfaces overlap during navigation).
			 * Projection reads are event snapshots, so cache the effect for this one
			 * delivery only: this preserves the next event's freshness while sharing
			 * both successes and failures among matching subscriptions.
			 */
			const cached_reads = new Map<string, Effect.Effect<unknown, unknown>>();
			const ProjectionKey = (
				...parts: ReadonlyArray<string | number | boolean | undefined>
			) => JSON.stringify(parts);
			const CachedRead = <A, E>(key: string, read: Effect.Effect<A, E>) =>
				Effect.gen(function* () {
					const existing = cached_reads.get(key) as Effect.Effect<A, E> | undefined;
					if (existing !== undefined) return yield* existing;
					const cached = yield* Effect.cached(read);
					cached_reads.set(key, cached);
					return yield* cached;
				});
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
					/**
					 * A question opening or closing flips the thread's live status
					 * between `Working` and `Waiting for answer` without being
					 * retention activity, so the list re-reads on it explicitly.
					 */
					event.payload.type === "interaction.question" ||
					thread_activity_kind_from_event(event.payload) !== undefined ||
					thread_message_sent_from_event(event.payload))
			) {
				const embedded = thread_patch;
				/** A failed coordinator join degrades to the embedded patch; it must not starve every projection this wake. */
				const thread = yield* thread_read_model.Lookup(event.thread_id).pipe(
					Effect.catchCause((cause) =>
						Cause.hasInterruptsOnly(cause)
							? Effect.failCause(cause)
							: Effect.logWarning("Thread list join failed during live delivery", {
									cause,
									thread_id: event.thread_id,
								}).pipe(Effect.as(Option.none())),
					),
				);

				thread_patch = Option.match(thread, {
					onNone: () => embedded,
					onSome: (item) => ({ _tag: "Upsert" as const, thread: item }),
				});
			}

			for (const [subscription_id, subscription] of Object.entries(current.subscriptions)) {
				if (subscription._tag === "conversation" || subscription._tag === "project.list")
					continue;
				/**
				 * Admission has already advanced the connection cursor for this
				 * event, so a failing projection read here is never retried by a
				 * later wake. Deliver each subscription independently: one
				 * poisoned read must not starve its siblings — or the
				 * conversation patch delivery that follows this loop — on every
				 * wake the failure persists. A skipped subscription keeps its own
				 * cursor and re-reads a fresh snapshot on the next event.
				 */
				const DeliverProjection = Effect.gen(function* () {
					const message_id = yield* metadata.MakeId("message");
					const sequence = subscription.sequence + 1;
					let next_journal_sequence = event.journal_sequence;
					let next_usage_thread_id =
						subscription._tag === "surface.usage.aggregate"
							? subscription.thread_id
							: undefined;

					if (subscription._tag === "thread.list") {
						if (!thread_patch) {
							return undefined;
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
						if (event.journal_sequence <= subscription.journal_sequence)
							return undefined;
						if (event.thread_id !== subscription.thread_id) return undefined;
						if (!EventAffectsTranscript(event)) return undefined;
						const snapshot = yield* CachedRead(
							ProjectionKey("transcript", event.thread_id, event.journal_sequence),
							transcript_read_model.Read({
								after_journal_sequence: Math.max(0, event.journal_sequence - 1),
								limit: 500,
								thread_id: event.thread_id,
							}),
						);
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
							if (entries.length === 0) return undefined;
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
						if (event.journal_sequence <= subscription.journal_sequence)
							return undefined;
						if (event.thread_id !== subscription.thread_id) return undefined;
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
							return {
								...subscription,
								journal_sequence: event.journal_sequence,
								sequence,
							};
						}
						const group_id = GraphGroupId(event);
						if (!group_id) return undefined;
						const groups = yield* CachedRead(
							ProjectionKey(
								"groups",
								subscription.thread_id,
								subscription.include_terminal,
							),
							graph.ListGroups(subscription.thread_id, subscription.include_terminal),
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
							return undefined;
						const snapshot = yield* CachedRead(
							ProjectionKey("session", subscription.thread_id),
							orchestration.GetSession(subscription.thread_id),
						);
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
					} else if (subscription._tag === "thread.work") {
						if (
							event.journal_sequence <= subscription.journal_sequence ||
							event.thread_id !== subscription.thread_id
						)
							return undefined;
						/**
						 * Work is cheap to read and too important to predicate on event
						 * taxonomy. Any fact for this thread may have moved coordinator
						 * ownership; an unchanged snapshot is harmless, a skipped one is
						 * the client/server split this subscription exists to prevent.
						 */
						const work = yield* CachedRead(
							ProjectionKey("work", subscription.thread_id),
							orchestration.GetWork(subscription.thread_id),
						);
						yield* Enqueue({
							journal_sequence: event.journal_sequence,
							kind: "thread.work.snapshot",
							message_id,
							origin: "backend",
							payload: {
								journal_sequence: event.journal_sequence,
								thread_id: subscription.thread_id,
								...(work === undefined ? {} : { work }),
							},
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
							event.thread_id !== subscription.query.thread_id ||
							!EventAffectsSurface(event)
						)
							return undefined;
						const snapshot = yield* CachedRead(
							ProjectionKey(
								"surface",
								subscription.query.thread_id,
								subscription.query.run_id,
								subscription.query.group_id,
							),
							surfaces.ListSnapshot({
								thread_id: subscription.query.thread_id,
								...(subscription.query.run_id === undefined
									? {}
									: { run_id: subscription.query.run_id }),
								...(subscription.query.group_id === undefined
									? {}
									: { group_id: subscription.query.group_id }),
							}),
						);
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
							event.thread_id !== subscription.thread_id ||
							!EventAffectsWorkspaceConflicts(event)
						)
							return undefined;
						const snapshot = yield* CachedRead(
							ProjectionKey("conflict", subscription.thread_id),
							ReadWorkspaceConflictSnapshot(subscription.thread_id),
						);
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
						if (event.journal_sequence <= subscription.journal_sequence)
							return undefined;
						if (!EventAffectsSurface(event)) return undefined;
						const erases_usage_scope =
							event.payload.type === "thread.erased" &&
							subscription.thread_id !== undefined &&
							event.thread_id === subscription.thread_id;
						if (
							!erases_usage_scope &&
							!(yield* CachedRead(
								ProjectionKey(
									"usage-affects",
									subscription.query.scope,
									subscription.query.scope_id,
									event.run_id,
								),
								surfaces.UsageEventAffects(subscription.query, event.run_id),
							))
						)
							return undefined;
						const snapshot = yield* CachedRead(
							ProjectionKey(
								"usage-aggregate",
								subscription.query.scope,
								subscription.query.scope_id,
							),
							surfaces.AggregateUsageSnapshot(subscription.query),
						);
						if (!erases_usage_scope) {
							next_usage_thread_id = yield* CachedRead(
								ProjectionKey(
									"usage-thread",
									subscription.query.scope,
									subscription.query.scope_id,
								),
								surfaces.UsageScopeThread(subscription.query),
							);
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
							return undefined;
						}

						const projection = yield* CachedRead(
							ProjectionKey("graph", group_id),
							graph.GetGraph(group_id),
						);

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
						subscription._tag === "thread.work" ||
						subscription._tag === "surface.list" ||
						subscription._tag === "surface.usage.aggregate" ||
						subscription._tag === "workspace.conflict.list"
							? {
									...subscription,
									journal_sequence: next_journal_sequence,
									sequence,
								}
							: { ...subscription, sequence };
					return subscription._tag === "surface.usage.aggregate"
						? {
								...next_subscription,
								...(next_usage_thread_id === undefined
									? {}
									: { thread_id: next_usage_thread_id }),
							}
						: next_subscription;
				});
				const delivered = yield* DeliverProjection.pipe(
					Effect.matchCauseEffect({
						onFailure: (cause) =>
							Cause.hasInterruptsOnly(cause)
								? Effect.failCause(cause)
								: Effect.logWarning(
										"Projection delivery failed for one subscription; live delivery continues",
										{
											cause,
											event_type: event.payload.type,
											subscription_id,
											subscription_tag: subscription._tag,
										},
									).pipe(Effect.as(undefined)),
						onSuccess: (outcome: ProjectionSubscription | undefined) =>
							Effect.succeed(outcome),
					}),
				);
				if (delivered !== undefined)
					subscriptions = { ...subscriptions, [subscription_id]: delivered };
			}

			return subscriptions;
		});

	return { EnqueueProjectionPatches };
});
