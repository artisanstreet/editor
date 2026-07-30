import { Cause, Deferred, Effect, Option, Queue, Ref, Scope, Stream } from "effect";

import type {
	EventEnvelope,
	OutboundControlEnvelope,
	ProtocolErrorDetail,
	SubscribeEnvelope,
	UnsubscribeEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanClientError,
	ProjectCatalogUpdate,
	OrchestrationGraphUpdate,
	OrchestrationGroupListUpdate,
	SurfaceListUpdate,
	SurfaceUsageAggregateUpdate,
	WorkspaceConflictListUpdate,
	ThreadSessionUpdate,
	ThreadTranscriptUpdate,
	ThreadListUpdate,
	ConversationUpdate,
} from "../../client-api/service";
import { client_error, protocol_client_error, type SendCurrent } from "../client-common";
import { SubscriptionContext } from "./context";
import { MakeEventIngress } from "./ingress";
import type { ClientSubscriptionCoordinator } from "./contract";
import type {
	ConversationSubscription,
	EventTerminal,
	OrchestrationGraphSubscription,
	OrchestrationGroupListSubscription,
	ProjectCatalogSubscription,
	ProjectionEnvelope,
	ProjectionSubscription,
	SubscriptionDelivery,
	SubscriptionRejection,
	SubscriptionStart,
	SubscriptionState,
	SurfaceListSubscription,
	SurfaceUsageAggregateSubscription,
	ThreadListSubscription,
	ThreadSessionSubscription,
	ThreadTranscriptSubscription,
	WorkspaceConflictListSubscription,
} from "./model";
import { offer_projection_update } from "./offers";

/** Builds bounded subscription delivery and ACK cursor state. */
export const MakeClientSubscriptionCoordinator = Effect.gen(function* () {
	const {
		event_capacity,
		make_id,
		make_trace,
		publish_error,
		send_current,
		state,
		subscription_capacity,
	} = yield* SubscriptionContext;

	const ingress = yield* MakeEventIngress;
	const handle_started = (
		envelope: Extract<OutboundControlEnvelope, { kind: "subscription.started" }>,
	) =>
		Effect.gen(function* () {
			const match = yield* Ref.modify<SubscriptionState, SubscriptionStart>(
				state,
				(current) => {
					const existing = current.subscriptions.get(envelope.subscription_id);

					if (!existing || existing.envelope.message_id !== envelope.correlation_id) {
						if (current.ignored_correlations.has(envelope.correlation_id)) {
							const ignored_correlations = new Set(current.ignored_correlations);

							ignored_correlations.delete(envelope.correlation_id);

							return [{ _tag: "Ignored" }, { ...current, ignored_correlations }];
						}

						return [{ _tag: "Missing" }, current];
					}

					if (Option.isSome(existing.stream_id)) {
						return [{ _tag: "Duplicate" }, current];
					}

					const updated: ProjectionSubscription = {
						...existing,
						expected_sequence: -1,
						stream_id: Option.some(envelope.payload.stream_id),
					};

					return [
						{ _tag: "Found", subscription: updated },
						{
							...current,
							subscriptions: new Map(current.subscriptions).set(
								envelope.subscription_id,
								updated,
							),
						},
					];
				},
			);

			if (match._tag === "Missing" || match._tag === "Duplicate") {
				return yield* Effect.fail(
					client_error(
						"correlation_conflict",
						match._tag === "Duplicate"
							? "The backend reused a subscription correlation id."
							: "The backend returned an unknown subscription correlation id.",
						new Error(
							match._tag === "Duplicate"
								? "duplicate subscription response"
								: "unknown subscription response",
						),
					),
				);
			}

			if (match._tag === "Found") {
				yield* Deferred.succeed(match.subscription.started, undefined);
			}
		});

	const send_unsubscribe = (subscription_id: string) =>
		Effect.gen(function* () {
			const trace = yield* make_trace;
			const unsubscribe: UnsubscribeEnvelope = {
				...trace,
				kind: "unsubscribe",
				payload: {},
				subscription_id,
			};

			yield* send_current(unsubscribe).pipe(Effect.ignore);
		});

	/**
	 * Queue is invariant in its element type, so the union of per-kind
	 * queues cannot be addressed directly. Failing and ending touch only
	 * the error and done channels — never the element — so narrowing the
	 * element to `never` is the safe direction of the cast.
	 */
	const fail_projection = (subscription: ProjectionSubscription, error: ArtisanClientError) =>
		Effect.gen(function* () {
			switch (subscription._tag) {
				case "thread.list":
					return yield* Queue.fail(subscription.queue, error);
				case "project.list":
					return yield* Queue.fail(subscription.queue, error);
				case "orchestration.graph":
					return yield* Queue.fail(subscription.queue, error);
				case "thread.transcript":
					return yield* Queue.fail(subscription.queue, error);
				case "conversation":
					return yield* Queue.fail(subscription.queue, error);
				case "orchestration.group.list":
					return yield* Queue.fail(subscription.queue, error);
				case "thread.session":
					return yield* Queue.fail(subscription.queue, error);
				case "surface.list":
					return yield* Queue.fail(subscription.queue, error);
				case "surface.usage.aggregate":
					return yield* Queue.fail(subscription.queue, error);
				case "workspace.conflict.list":
					return yield* Queue.fail(subscription.queue, error);
			}
		});
	const end_projection = (subscription: ProjectionSubscription) =>
		Effect.gen(function* () {
			switch (subscription._tag) {
				case "thread.list":
					return yield* Queue.end(subscription.queue);
				case "project.list":
					return yield* Queue.end(subscription.queue);
				case "orchestration.graph":
					return yield* Queue.end(subscription.queue);
				case "thread.transcript":
					return yield* Queue.end(subscription.queue);
				case "conversation":
					return yield* Queue.end(subscription.queue);
				case "orchestration.group.list":
					return yield* Queue.end(subscription.queue);
				case "thread.session":
					return yield* Queue.end(subscription.queue);
				case "surface.list":
					return yield* Queue.end(subscription.queue);
				case "surface.usage.aggregate":
					return yield* Queue.end(subscription.queue);
				case "workspace.conflict.list":
					return yield* Queue.end(subscription.queue);
			}
		});
	const handle_update = (envelope: ProjectionEnvelope) =>
		Ref.modify<SubscriptionState, SubscriptionDelivery>(state, (current) => {
			const subscription = current.subscriptions.get(envelope.subscription_id);

			if (
				!subscription ||
				Option.isNone(subscription.stream_id) ||
				subscription.stream_id.value !== envelope.stream_id
			) {
				return [{ _tag: "Ignored" }, current];
			}

			if (envelope.sequence !== subscription.expected_sequence + 1) {
				const subscriptions = new Map(current.subscriptions);

				subscriptions.delete(envelope.subscription_id);

				return [
					{ _tag: "Gap", subscription },
					{ ...current, subscriptions },
				];
			}

			const offer = offer_projection_update(subscription, envelope);

			if (offer !== "offered") {
				const subscriptions = new Map(current.subscriptions);

				subscriptions.delete(envelope.subscription_id);

				return [
					{ _tag: offer === "overflow" ? "Overflow" : "Gap", subscription },
					{ ...current, subscriptions },
				];
			}

			const updated = { ...subscription, expected_sequence: envelope.sequence };

			return [
				{ _tag: "Delivered" },
				{
					...current,
					subscriptions: new Map(current.subscriptions).set(
						envelope.subscription_id,
						updated,
					),
				},
			];
		}).pipe(
			Effect.flatMap((delivery) => {
				if (delivery._tag !== "Gap" && delivery._tag !== "Overflow") {
					return Effect.void;
				}

				const error = client_error(
					delivery._tag === "Gap" ? "stream_gap" : "subscription_overflow",
					delivery._tag === "Gap"
						? "The projection subscription sequence contained a gap."
						: "The projection subscription queue overflowed.",
					new Error("projection subscription could not remain contiguous"),
				);

				return fail_projection(delivery.subscription, error).pipe(
					Effect.andThen(send_unsubscribe(envelope.subscription_id)),
					Effect.andThen(publish_error(error)),
				);
			}),
		);

	const reject = (correlation_id: string, detail: ProtocolErrorDetail) =>
		Effect.gen(function* () {
			const match = yield* Ref.modify(
				state,
				(current): readonly [SubscriptionRejection, SubscriptionState] => {
					const found = [...current.subscriptions.values()].find(
						(candidate) => candidate.envelope.message_id === correlation_id,
					);

					if (!found) {
						if (current.ignored_correlations.has(correlation_id)) {
							const ignored_correlations = new Set(current.ignored_correlations);

							ignored_correlations.delete(correlation_id);

							return [{ _tag: "Ignored" }, { ...current, ignored_correlations }];
						}

						return [{ _tag: "Missing" }, current];
					}

					const subscriptions = new Map(current.subscriptions);

					subscriptions.delete(found.envelope.subscription_id);

					return [
						{ _tag: "Found", subscription: found },
						{ ...current, subscriptions },
					];
				},
			);

			if (match._tag === "Missing") {
				return false;
			}

			if (match._tag === "Ignored") {
				return true;
			}

			const error = protocol_client_error(detail);

			yield* Deferred.fail(match.subscription.started, error);
			yield* fail_projection(match.subscription, error);

			return true;
		});

	const remove = (subscription_id: string) =>
		Effect.gen(function* () {
			const removed = yield* Ref.modify(
				state,
				(current): readonly [Option.Option<ProjectionSubscription>, SubscriptionState] => {
					const subscription = current.subscriptions.get(subscription_id);

					if (!subscription) {
						return [Option.none(), current];
					}

					const subscriptions = new Map(current.subscriptions);
					const ignored_correlations = new Set(current.ignored_correlations);

					subscriptions.delete(subscription_id);
					ignored_correlations.add(subscription.envelope.message_id);

					return [
						Option.some(subscription),
						{ ...current, ignored_correlations, subscriptions },
					];
				},
			);

			if (Option.isNone(removed)) {
				return;
			}

			yield* end_projection(removed.value);
			yield* send_unsubscribe(subscription_id);
		});

	const start_subscription = (subscription: ProjectionSubscription) =>
		Effect.gen(function* () {
			const inserted = yield* Ref.modify(
				state,
				(current): readonly [boolean, SubscriptionState] =>
					current.disposed
						? [false, current]
						: [
								true,
								{
									...current,
									subscriptions: new Map(current.subscriptions).set(
										subscription.envelope.subscription_id,
										subscription,
									),
								},
							],
			);

			if (!inserted) {
				return yield* Effect.fail(
					client_error(
						"disposed",
						"The Artisan client was disposed.",
						new Error("client disposed"),
					),
				);
			}

			yield* Effect.addFinalizer(() => remove(subscription.envelope.subscription_id));
			yield* send_current(subscription.envelope);
			yield* Deferred.await(subscription.started).pipe(
				Effect.onInterrupt(() => remove(subscription.envelope.subscription_id)),
			);
		});

	/**
	 * One factory owns every projection subscription lifecycle: trace, id,
	 * bounded queue, start registration, and stream exposure. Each kind supplies
	 * only its typed identity through the builder, whose annotation ties the
	 * queue element type to the subscription tag — the pairing the previous
	 * hand-written copies re-stated six times and the old generic asserted with
	 * a cast.
	 */
	const subscribe_projection = <Update, S extends ProjectionSubscription>(
		id_prefix: string,
		payload: SubscribeEnvelope["payload"],
		build: (parts: {
			readonly envelope: SubscribeEnvelope;
			readonly expected_sequence: number;
			readonly queue: Queue.Queue<Update, ArtisanClientError | Cause.Done<void>>;
			readonly started: Deferred.Deferred<void, ArtisanClientError>;
			readonly stream_id: Option.Option<string>;
		}) => S,
	): Effect.Effect<Stream.Stream<Update, ArtisanClientError>, ArtisanClientError, Scope.Scope> =>
		Effect.gen(function* () {
			const trace = yield* make_trace;
			const subscription_id = yield* make_id(id_prefix);
			const queue = yield* Effect.acquireRelease(
				Queue.dropping<Update, ArtisanClientError | Cause.Done<void>>(
					subscription_capacity,
				),
				Queue.shutdown,
			);
			const started = yield* Deferred.make<void, ArtisanClientError>();
			yield* start_subscription(
				build({
					envelope: { ...trace, kind: "subscribe", payload, subscription_id },
					expected_sequence: -1,
					queue,
					started,
					stream_id: Option.none(),
				}),
			);
			return Stream.fromQueue(queue);
		});

	const subscribe_thread_list = subscribe_projection<ThreadListUpdate, ThreadListSubscription>(
		"thread_list_subscription",
		{ type: "thread.list" },
		(parts) => ({
			_tag: "thread.list",
			...parts,
		}),
	);
	const subscribe_projects = subscribe_projection<
		ProjectCatalogUpdate,
		ProjectCatalogSubscription
	>("project_list_subscription", { type: "project.list" }, (parts) => ({
		_tag: "project.list",
		...parts,
	}));
	const subscribe_orchestration_graph = (group_id: string) =>
		subscribe_projection<OrchestrationGraphUpdate, OrchestrationGraphSubscription>(
			"orchestration_graph_subscription",
			{ group_id, type: "orchestration.graph" },
			(parts) => ({ _tag: "orchestration.graph", ...parts }),
		);
	const reset_connection = Effect.gen(function* () {
		const current = yield* Ref.get(state);
		const starts = yield* Effect.forEach([...current.subscriptions.entries()], ([id]) =>
			Deferred.make<void, ArtisanClientError>().pipe(
				Effect.map((started) => [id, started] as const),
			),
		);
		const by_id = new Map(starts);

		yield* Ref.update(state, (latest) => ({
			...latest,
			ignored_correlations: new Set<string>(),
			subscriptions: new Map(
				[...latest.subscriptions].map(
					([id, subscription]) =>
						[
							id,
							{
								...subscription,
								expected_sequence: -1,
								started: by_id.get(id) ?? subscription.started,
								stream_id: Option.none<string>(),
							},
						] as const,
				),
			),
		}));
	});

	const subscribe_thread_transcript = (thread_id: string) =>
		subscribe_projection<ThreadTranscriptUpdate, ThreadTranscriptSubscription>(
			"thread_transcript_subscription",
			{ type: "thread.transcript", thread_id },
			(parts) => ({ _tag: "thread.transcript", ...parts }),
		);
	const subscribe_conversation = (thread_id: string) =>
		subscribe_projection<ConversationUpdate, ConversationSubscription>(
			"conversation_subscription",
			{ type: "conversation", thread_id },
			(parts) => ({ _tag: "conversation", ...parts }),
		);
	const subscribe_orchestration_groups = (thread_id: string, include_terminal: boolean) =>
		subscribe_projection<OrchestrationGroupListUpdate, OrchestrationGroupListSubscription>(
			"orchestration_group_list_subscription",
			{ type: "orchestration.group.list", thread_id, include_terminal },
			(parts) => ({ _tag: "orchestration.group.list", ...parts }),
		);
	const retry = (send: SendCurrent = send_current) =>
		Ref.modify(
			state,
			(current): readonly [ReadonlyArray<ProjectionSubscription>, SubscriptionState] => [
				[...current.subscriptions.values()],
				{ ...current, ignored_correlations: new Set<string>() },
			],
		).pipe(
			Effect.flatMap((subscriptions) =>
				Effect.forEach(subscriptions, (subscription) => send(subscription.envelope), {
					discard: true,
				}),
			),
		);

	const await_ready = Ref.get(state).pipe(
		Effect.flatMap((current) =>
			Effect.forEach(
				current.subscriptions.values(),
				(subscription) => Deferred.await(subscription.started),
				{ discard: true },
			),
		),
	);

	const dispose = (error: Option.Option<ArtisanClientError>) =>
		Effect.gen(function* () {
			const current = yield* Ref.getAndUpdate(state, (value) => ({
				...value,
				disposed: true,
				subscriptions: new Map(),
			}));

			yield* Effect.forEach(
				current.subscriptions.values(),
				(subscription) =>
					Option.match(error, {
						onNone: () => end_projection(subscription),
						onSome: (failure) =>
							Effect.all(
								[
									Deferred.fail(subscription.started, failure),
									fail_projection(subscription, failure),
								],
								{ discard: true },
							),
					}),
				{ discard: true },
			);

			const observers = yield* Ref.modify(state, (current) => [
				[...current.event_observers.values()],
				{
					...current,
					event_observers: new Map(),
					event_terminal: Option.match(error, {
						onNone: () => ({ _tag: "ended" }) as const,
						onSome: (failure) => ({ _tag: "failed", error: failure }) as const,
					}),
				},
			]);
			yield* Effect.forEach(
				observers,
				(observer) =>
					Option.match(error, {
						onNone: () => Queue.end(observer),
						onSome: (failure) => Queue.fail(observer, failure),
					}),
				{ discard: true },
			);
		});

	return {
		ApplyEvent: ingress.ApplyEvent,
		ApplyReplayComplete: ingress.ApplyReplayComplete,
		AwaitReady: await_ready,
		Cursors: ingress.Cursors,
		Dispose: dispose,
		Events: Stream.scoped(
			Stream.unwrap(
				Effect.gen(function* () {
					const observer_id = yield* make_id("event_observer");
					const observer = yield* Effect.acquireRelease(
						Queue.dropping<EventEnvelope, ArtisanClientError | Cause.Done<void>>(
							event_capacity,
						),
						Queue.shutdown,
					);
					const terminal = yield* Ref.modify<SubscriptionState, EventTerminal>(
						state,
						(current) => {
							if (current.event_terminal._tag !== "active") {
								return [current.event_terminal, current];
							}
							return [
								current.event_terminal,
								{
									...current,
									event_observers: new Map(current.event_observers).set(
										observer_id,
										observer,
									),
								},
							];
						},
					);
					yield* Effect.addFinalizer(() =>
						Ref.update(state, (current) => {
							const event_observers = new Map(current.event_observers);
							event_observers.delete(observer_id);
							return { ...current, event_observers };
						}),
					);
					return terminal._tag === "active"
						? Stream.fromQueue(observer)
						: terminal._tag === "ended"
							? Stream.empty
							: Stream.fail(terminal.error);
				}),
			),
		),
		HandleStarted: handle_started,
		HandleUpdate: handle_update,
		Reject: reject,
		ResetConnection: reset_connection,
		ResumeCursors: ingress.Cursors,
		Retry: retry,
		SubscribeOrchestrationGraph: subscribe_orchestration_graph,
		SubscribeOrchestrationGroups: subscribe_orchestration_groups,
		SubscribeThreadList: subscribe_thread_list,
		SubscribeProjects: subscribe_projects,
		SubscribeThreadTranscript: subscribe_thread_transcript,
		SubscribeConversation: subscribe_conversation,
		SubscribeThreadSession: (thread_id) =>
			subscribe_projection<ThreadSessionUpdate, ThreadSessionSubscription>(
				"thread.session_subscription",
				{ type: "thread.session", thread_id },
				(parts) => ({ _tag: "thread.session", ...parts }),
			),
		SubscribeSurfaceItems: (query) =>
			subscribe_projection<SurfaceListUpdate, SurfaceListSubscription>(
				"surface.list_subscription",
				{ type: "surface.list", query },
				(parts) => ({ _tag: "surface.list", ...parts }),
			),
		SubscribeSurfaceUsageAggregate: (query) =>
			subscribe_projection<SurfaceUsageAggregateUpdate, SurfaceUsageAggregateSubscription>(
				"surface.usage.aggregate_subscription",
				{ type: "surface.usage.aggregate", query },
				(parts) => ({ _tag: "surface.usage.aggregate", ...parts }),
			),
		SubscribeWorkspaceConflicts: (thread_id) =>
			subscribe_projection<WorkspaceConflictListUpdate, WorkspaceConflictListSubscription>(
				"workspace.conflict.list_subscription",
				{ type: "workspace.conflict.list", thread_id },
				(parts) => ({ _tag: "workspace.conflict.list", ...parts }),
			),
	} satisfies ClientSubscriptionCoordinator;
});
