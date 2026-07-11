import { Cause, Deferred, Effect, Option, Queue, Ref, Scope, Stream } from "effect";

import type {
	EventEnvelope,
	OutboundControlEnvelope,
	ProtocolErrorDetail,
	SubscribeEnvelope,
	UnsubscribeEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanClientCursors,
	ArtisanClientError,
	OrchestrationGraphUpdate,
	ThreadListUpdate,
} from "../client-contract";
import {
	client_error,
	protocol_client_error,
	record_to_cursors,
	type MakeTrace,
	type SendCurrent,
} from "./client-common";

interface ProjectionSubscriptionBase {
	readonly envelope: SubscribeEnvelope;
	readonly expected_sequence: number;
	readonly started: Deferred.Deferred<void, ArtisanClientError>;
	readonly stream_id: Option.Option<string>;
}

interface ThreadListSubscription extends ProjectionSubscriptionBase {
	readonly _tag: "thread.list";
	readonly queue: Queue.Queue<ThreadListUpdate, ArtisanClientError | Cause.Done<void>>;
}

interface OrchestrationGraphSubscription extends ProjectionSubscriptionBase {
	readonly _tag: "orchestration.graph";
	readonly queue: Queue.Queue<OrchestrationGraphUpdate, ArtisanClientError | Cause.Done<void>>;
}

type ProjectionSubscription = OrchestrationGraphSubscription | ThreadListSubscription;

type ProjectionEnvelope = Extract<
	OutboundControlEnvelope,
	{
		readonly kind:
			| "orchestration.graph.patch"
			| "orchestration.graph.snapshot"
			| "thread.list.snapshot"
			| "thread.list.upsert"
			| "thread.list.remove";
	}
>;

interface SubscriptionState {
	readonly disposed: boolean;
	readonly event_cursors: Readonly<Record<string, number>>;
	readonly ignored_correlations: ReadonlySet<string>;
	readonly last_journal_sequence: number;
	readonly subscriptions: ReadonlyMap<string, ProjectionSubscription>;
}

type EventApplication =
	| { readonly _tag: "Applied"; readonly cursors: ArtisanClientCursors }
	| { readonly _tag: "Duplicate" }
	| { readonly _tag: "Gap" }
	| { readonly _tag: "Overflow" };

type SubscriptionDelivery =
	| { readonly _tag: "Delivered" }
	| { readonly _tag: "Ignored" }
	| {
			readonly _tag: "Gap" | "Overflow";
			readonly subscription: ProjectionSubscription;
	  };

type SubscriptionStart =
	| { readonly _tag: "Duplicate" }
	| { readonly _tag: "Found"; readonly subscription: ProjectionSubscription }
	| { readonly _tag: "Ignored" }
	| { readonly _tag: "Missing" };

type SubscriptionRejection = Exclude<SubscriptionStart, { readonly _tag: "Duplicate" }>;

type ProjectionOffer = "mismatch" | "offered" | "overflow";

/** Owns applied durable cursors and connection-local thread-list subscriptions. */
export interface ClientSubscriptionCoordinator {
	readonly ApplyEvent: (
		event: EventEnvelope,
	) => Effect.Effect<ArtisanClientCursors, ArtisanClientError>;
	readonly Cursors: Effect.Effect<ArtisanClientCursors>;
	readonly Dispose: (error: Option.Option<ArtisanClientError>) => Effect.Effect<void>;
	readonly Events: Stream.Stream<EventEnvelope, ArtisanClientError>;
	readonly HandleStarted: (
		envelope: Extract<OutboundControlEnvelope, { kind: "subscription.started" }>,
	) => Effect.Effect<void, ArtisanClientError>;
	readonly HandleUpdate: (envelope: ProjectionEnvelope) => Effect.Effect<void>;
	readonly Reject: (
		correlation_id: string,
		detail: ProtocolErrorDetail,
	) => Effect.Effect<boolean, ArtisanClientError>;
	readonly ResetConnection: Effect.Effect<void>;
	readonly ResumeCursors: Effect.Effect<ArtisanClientCursors>;
	readonly Retry: Effect.Effect<void>;
	readonly SubscribeOrchestrationGraph: (
		group_id: string,
	) => Effect.Effect<
		Stream.Stream<OrchestrationGraphUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeThreadList: Effect.Effect<
		Stream.Stream<ThreadListUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
}

/** Builds bounded subscription delivery and ACK cursor state. */
export const make_client_subscription_coordinator = (
	event_capacity: number,
	subscription_capacity: number,
	make_trace: MakeTrace,
	make_id: (prefix: string) => Effect.Effect<string>,
	send_current: SendCurrent,
	publish_error: (error: ArtisanClientError) => Effect.Effect<void>,
) =>
	Effect.gen(function* () {
		const events = yield* Effect.acquireRelease(
			Queue.dropping<EventEnvelope, ArtisanClientError | Cause.Done<void>>(event_capacity),
			Queue.shutdown,
		);
		const state = yield* Ref.make<SubscriptionState>({
			disposed: false,
			event_cursors: {},
			ignored_correlations: new Set<string>(),
			last_journal_sequence: 0,
			subscriptions: new Map(),
		});

		const cursors = Ref.get(state).pipe(
			Effect.map((current) => ({
				event_cursors: record_to_cursors(current.event_cursors),
				last_journal_sequence: current.last_journal_sequence,
			})),
		);

		const apply_event = (event: EventEnvelope) =>
			Ref.modify<SubscriptionState, EventApplication>(state, (current) => {
				const stream_sequence = current.event_cursors[event.stream_id] ?? 0;

				if (
					event.journal_sequence <= current.last_journal_sequence &&
					event.sequence <= stream_sequence
				) {
					return [{ _tag: "Duplicate" }, current];
				}

				if (event.sequence !== stream_sequence + 1) {
					return [{ _tag: "Gap" }, current];
				}

				if (event.journal_sequence !== current.last_journal_sequence + 1) {
					return [{ _tag: "Gap" }, current];
				}

				if (!Queue.offerUnsafe(events, event)) {
					return [{ _tag: "Overflow" }, current];
				}

				const event_cursors = {
					...current.event_cursors,
					[event.stream_id]: event.sequence,
				};
				const last_journal_sequence = Math.max(
					current.last_journal_sequence,
					event.journal_sequence,
				);

				return [
					{
						_tag: "Applied",
						cursors: {
							event_cursors: record_to_cursors(event_cursors),
							last_journal_sequence,
						},
					},
					{ ...current, event_cursors, last_journal_sequence },
				];
			}).pipe(
				Effect.flatMap((outcome) => {
					switch (outcome._tag) {
						case "Applied":
							return Effect.succeed(outcome.cursors);
						case "Duplicate":
							return cursors;
						case "Gap":
							return Effect.fail(
								client_error(
									"stream_gap",
									"A durable event stream sequence contained a gap.",
									new Error("durable event cursor gap"),
									true,
								),
							);
						case "Overflow": {
							const error = client_error(
								"event_overflow",
								"The frontend event queue overflowed before the event was applied.",
								new Error("event delivery queue overflow"),
							);

							return Queue.fail(events, error).pipe(
								Effect.andThen(Effect.fail(error)),
							);
						}
					}
				}),
			);

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

				yield* send_current(unsubscribe);
			});

		const fail_projection = (subscription: ProjectionSubscription, error: ArtisanClientError) =>
			subscription._tag === "thread.list"
				? Queue.fail(subscription.queue, error)
				: Queue.fail(subscription.queue, error);
		const end_projection = (subscription: ProjectionSubscription) =>
			subscription._tag === "thread.list"
				? Queue.end(subscription.queue)
				: Queue.end(subscription.queue);
		const offer_projection_update = (
			subscription: ProjectionSubscription,
			envelope: ProjectionEnvelope,
		): ProjectionOffer => {
			if (
				subscription._tag === "thread.list" &&
				(envelope.kind === "thread.list.snapshot" ||
					envelope.kind === "thread.list.upsert" ||
					envelope.kind === "thread.list.remove")
			) {
				const update: ThreadListUpdate =
					envelope.kind === "thread.list.snapshot"
						? {
								journal_sequence: envelope.journal_sequence,
								threads: envelope.payload.threads,
								type: "snapshot",
							}
						: envelope.kind === "thread.list.upsert"
							? {
									journal_sequence: envelope.journal_sequence,
									thread: envelope.payload,
									type: "upsert",
								}
							: {
									journal_sequence: envelope.journal_sequence,
									thread_id: envelope.payload.thread_id,
									type: "remove",
								};

				return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
			}

			if (
				subscription._tag === "orchestration.graph" &&
				(envelope.kind === "orchestration.graph.snapshot" ||
					envelope.kind === "orchestration.graph.patch")
			) {
				const update: OrchestrationGraphUpdate = {
					graph: envelope.payload.graph,
					journal_sequence: envelope.journal_sequence,
					type: envelope.kind === "orchestration.graph.snapshot" ? "snapshot" : "patch",
				};

				return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
			}

			return "mismatch";
		};

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
					(
						current,
					): readonly [Option.Option<ProjectionSubscription>, SubscriptionState] => {
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

		const subscribe_thread_list = Effect.gen(function* () {
			const trace = yield* make_trace;
			const subscription_id = yield* make_id("thread_list_subscription");
			const queue = yield* Effect.acquireRelease(
				Queue.dropping<ThreadListUpdate, ArtisanClientError | Cause.Done<void>>(
					subscription_capacity,
				),
				Queue.shutdown,
			);
			const started = yield* Deferred.make<void, ArtisanClientError>();
			const envelope: SubscribeEnvelope = {
				...trace,
				kind: "subscribe",
				payload: { type: "thread.list" },
				subscription_id,
			};
			const subscription: ThreadListSubscription = {
				_tag: "thread.list",
				envelope,
				expected_sequence: -1,
				queue,
				started,
				stream_id: Option.none(),
			};

			yield* start_subscription(subscription);

			return Stream.fromQueue(queue);
		});

		const subscribe_orchestration_graph = (group_id: string) =>
			Effect.gen(function* () {
				const trace = yield* make_trace;
				const subscription_id = yield* make_id("orchestration_graph_subscription");
				const queue = yield* Effect.acquireRelease(
					Queue.dropping<OrchestrationGraphUpdate, ArtisanClientError | Cause.Done<void>>(
						subscription_capacity,
					),
					Queue.shutdown,
				);
				const started = yield* Deferred.make<void, ArtisanClientError>();
				const envelope: SubscribeEnvelope = {
					...trace,
					kind: "subscribe",
					payload: { group_id, type: "orchestration.graph" },
					subscription_id,
				};
				const subscription: OrchestrationGraphSubscription = {
					_tag: "orchestration.graph",
					envelope,
					expected_sequence: -1,
					queue,
					started,
					stream_id: Option.none(),
				};

				yield* start_subscription(subscription);

				return Stream.fromQueue(queue);
			});

		const reset_connection = Ref.update(state, (current) => ({
			...current,
			ignored_correlations: new Set<string>(),
			subscriptions: new Map(
				[...current.subscriptions].map(
					([id, subscription]) =>
						[
							id,
							{
								...subscription,
								expected_sequence: -1,
								stream_id: Option.none<string>(),
							},
						] as const,
				),
			),
		}));

		const retry = Ref.modify(
			state,
			(current): readonly [ReadonlyArray<ProjectionSubscription>, SubscriptionState] => [
				[...current.subscriptions.values()],
				{ ...current, ignored_correlations: new Set<string>() },
			],
		).pipe(
			Effect.flatMap((subscriptions) =>
				Effect.forEach(
					subscriptions,
					(subscription) => send_current(subscription.envelope),
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

				yield* Option.match(error, {
					onNone: () => Queue.end(events),
					onSome: (failure) => Queue.fail(events, failure),
				});
			});

		return {
			ApplyEvent: apply_event,
			Cursors: cursors,
			Dispose: dispose,
			Events: Stream.fromQueue(events),
			HandleStarted: handle_started,
			HandleUpdate: handle_update,
			Reject: reject,
			ResetConnection: reset_connection,
			ResumeCursors: cursors,
			Retry: retry,
			SubscribeOrchestrationGraph: subscribe_orchestration_graph,
			SubscribeThreadList: subscribe_thread_list,
		} satisfies ClientSubscriptionCoordinator;
	});
