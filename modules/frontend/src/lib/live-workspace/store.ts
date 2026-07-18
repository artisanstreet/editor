import {
	Context,
	Data,
	Effect,
	Fiber,
	Layer,
	Option,
	Ref,
	Schedule,
	Scope,
	Stream,
	SubscriptionRef,
} from "effect";

import type {
	GlobalGuidanceSnapshot,
	ModelBehaviourSnapshot,
	ThreadListItem,
	ThreadWorkItem,
} from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

import {
	FrontendConnectionLifecycle,
	type FrontendConnectionPhase,
} from "../runtime/desktop-message-port-connector";

export type LiveWorkspacePhase =
	| "connecting"
	| "ready"
	| "reconnecting"
	| "stale"
	| "error"
	| "empty";

export interface LiveWorkspaceSnapshot {
	readonly error: Option.Option<string>;
	readonly global_guidance: Option.Option<GlobalGuidanceSnapshot>;
	readonly model_behaviour: Option.Option<ModelBehaviourSnapshot>;
	readonly phase: LiveWorkspacePhase;
	readonly selected_thread_id: Option.Option<string>;
	readonly thread_work: Option.Option<ThreadWorkItem>;
	readonly threads: ReadonlyArray<ThreadListItem>;
}

interface LiveWorkspaceState {
	readonly list_generation: number;
	readonly refresh_generation: number;
	readonly selection_generation: number;
	readonly subscription_generation: number;
	readonly snapshot: LiveWorkspaceSnapshot;
}

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	global_guidance: Option.none(),
	model_behaviour: Option.none(),
	phase: "connecting",
	selected_thread_id: Option.none(),
	thread_work: Option.none(),
	threads: [],
};

const EmptyState: LiveWorkspaceState = {
	list_generation: 0,
	refresh_generation: 0,
	selection_generation: 0,
	subscription_generation: 0,
	snapshot: EmptySnapshot,
};

class ThreadListSubscriptionLost extends Data.TaggedError("ThreadListSubscriptionLost")<{
	readonly message: string;
}> {}

const ThreadListSubscriptionRetrySchedule = Schedule.exponential("100 millis").pipe(
	Schedule.upTo({ duration: "1 second", times: 3 }),
);

export const ToLiveWorkspacePhase = (phase: FrontendConnectionPhase): LiveWorkspacePhase => {
	if (phase === "ready") return "ready";
	if (phase === "reconnecting") return "reconnecting";
	if (phase === "stale") return "stale";
	if (phase === "error" || phase === "unavailable") return "error";
	return "connecting";
};

/** A new ready generation is the only lifecycle state that may reload projections. */
export const ShouldRefreshForConnection = (phase: FrontendConnectionPhase) => phase === "ready";

const selected_thread_exists = (
	selected_thread_id: Option.Option<string>,
	threads: ReadonlyArray<ThreadListItem>,
) =>
	Option.isNone(selected_thread_id) ||
	threads.some((thread) => thread.thread_id === selected_thread_id.value);

const reconcile_selection = (
	snapshot: LiveWorkspaceSnapshot,
	threads: ReadonlyArray<ThreadListItem>,
): LiveWorkspaceSnapshot =>
	selected_thread_exists(snapshot.selected_thread_id, threads)
		? { ...snapshot, threads }
		: {
				...snapshot,
				selected_thread_id: Option.none(),
				thread_work: Option.none(),
				threads,
			};

export const ApplyThreadListUpdate = (
	snapshot: LiveWorkspaceSnapshot,
	update:
		| { readonly type: "snapshot"; readonly threads: ReadonlyArray<ThreadListItem> }
		| { readonly type: "upsert"; readonly thread: ThreadListItem }
		| { readonly type: "remove"; readonly thread_id: string },
): LiveWorkspaceSnapshot => {
	if (update.type === "snapshot") return reconcile_selection(snapshot, update.threads);
	if (update.type === "remove") {
		return reconcile_selection(
			snapshot,
			snapshot.threads.filter((thread) => thread.thread_id !== update.thread_id),
		);
	}

	const threads = snapshot.threads.filter(
		(thread) => thread.thread_id !== update.thread.thread_id,
	);
	return reconcile_selection(snapshot, [update.thread, ...threads]);
};

/** Applies a complete backend list as the current authoritative renderer projection. */
export const ApplyAuthoritativeThreadRefresh = (
	snapshot: LiveWorkspaceSnapshot,
	threads: ReadonlyArray<ThreadListItem>,
): LiveWorkspaceSnapshot => {
	const reconciled = reconcile_selection(snapshot, threads);
	const selected_thread_id = Option.isSome(reconciled.selected_thread_id)
		? reconciled.selected_thread_id
		: threads[0] === undefined
			? Option.none<string>()
			: Option.some(threads[0].thread_id);
	const selection_changed =
		Option.getOrUndefined(reconciled.selected_thread_id) !==
		Option.getOrUndefined(selected_thread_id);

	return {
		...reconciled,
		error: Option.none(),
		phase: threads.length === 0 ? "empty" : "ready",
		selected_thread_id,
		thread_work: selection_changed ? Option.none() : reconciled.thread_work,
	};
};

/** Makes subscription loss actionable while retaining the last known backend projection. */
export const ApplyThreadListSubscriptionFailure = (
	snapshot: LiveWorkspaceSnapshot,
	error: string,
): LiveWorkspaceSnapshot => ({
	...snapshot,
	error: Option.some(error),
	phase: "error",
});

/** Retries a dropped authoritative stream with a bounded backoff before reporting its final loss. */
export const RunThreadListSubscription = <E extends { readonly message: string }>(
	subscribe: Effect.Effect<Stream.Stream<ThreadListUpdate>, E, Scope.Scope>,
	on_update: (update: ThreadListUpdate) => Effect.Effect<void>,
	on_failure: (message: string) => Effect.Effect<void>,
) =>
	subscribe.pipe(
		Effect.flatMap((updates) =>
			Stream.runForEach(updates, on_update).pipe(
				Effect.flatMap(() =>
					Effect.fail(
						new ThreadListSubscriptionLost({
							message: "Thread-list subscription ended unexpectedly.",
						}),
					),
				),
			),
		),
		Effect.retry({ schedule: ThreadListSubscriptionRetrySchedule }),
		Effect.catch((error) => on_failure(error.message)),
		Effect.asVoid,
	);

const SelectThreadSnapshot = (snapshot: LiveWorkspaceSnapshot, thread_id: string) => ({
	...snapshot,
	error: Option.none(),
	selected_thread_id: Option.some(thread_id),
	thread_work: Option.none(),
});

/** Rejects a late work query when the renderer has selected another thread. */
export const IsCurrentThreadSelection = (
	snapshot: LiveWorkspaceSnapshot,
	selection_generation: number,
	expected_thread_id: string,
	expected_selection_generation: number,
) =>
	selection_generation === expected_selection_generation &&
	Option.getOrUndefined(snapshot.selected_thread_id) === expected_thread_id;

/** Owns renderer-only live projection state; durable records remain backend projections. */
export class LiveWorkspaceStore extends Context.Service<
	LiveWorkspaceStore,
	{
		readonly Changes: Stream.Stream<LiveWorkspaceSnapshot>;
		readonly CreateThread: (title: string) => Effect.Effect<void>;
		readonly Refresh: Effect.Effect<void>;
		readonly SendMessage: (text: string) => Effect.Effect<void>;
		readonly SelectThread: (thread_id: string) => Effect.Effect<void>;
		readonly Snapshot: Effect.Effect<LiveWorkspaceSnapshot>;
	}
>()("Artisan/LiveWorkspaceStore") {}

export const LiveWorkspaceStoreLive = Layer.effect(
	LiveWorkspaceStore,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const lifecycle = yield* FrontendConnectionLifecycle;
		const state = yield* SubscriptionRef.make(EmptyState);
		const subscription_fiber = yield* Ref.make<Option.Option<Fiber.RuntimeFiber<void, never>>>(
			Option.none(),
		);

		const Update = (update: (current: LiveWorkspaceState) => LiveWorkspaceState) =>
			SubscriptionRef.update(state, update);

		const UpdateAndGet = (update: (current: LiveWorkspaceState) => LiveWorkspaceState) =>
			SubscriptionRef.modify(state, (current) => {
				const next = update(current);
				return [next, next] as const;
			});

		const LoadSelectedThread = (thread_id: string, selection_generation: number) =>
			client.GetThreadWork(thread_id).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) =>
							IsCurrentThreadSelection(
								current.snapshot,
								current.selection_generation,
								thread_id,
								selection_generation,
							)
								? {
										...current,
										snapshot: {
											...current.snapshot,
											error: Option.some(error.message),
										},
									}
								: current,
						),
					onSuccess: (thread_work) =>
						Update((current) =>
							IsCurrentThreadSelection(
								current.snapshot,
								current.selection_generation,
								thread_id,
								selection_generation,
							)
								? {
										...current,
										snapshot: { ...current.snapshot, thread_work },
									}
								: current,
						),
				}),
			);

		const Refresh = Effect.gen(function* () {
			const started = yield* UpdateAndGet((current) => ({
				...current,
				refresh_generation: current.refresh_generation + 1,
			}));
			const refresh_generation = started.refresh_generation;
			const list_generation = started.list_generation;

			yield* client.ListThreads.pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((current) =>
							current.refresh_generation === refresh_generation
								? {
										...current,
										snapshot: {
											...current.snapshot,
											error: Option.some(error.message),
											phase: "error",
										},
									}
								: current,
						),
					onSuccess: (threads) =>
						Update((current) => {
							if (
								current.refresh_generation !== refresh_generation ||
								current.list_generation !== list_generation
							) {
								return current;
							}

							const refreshed = ApplyAuthoritativeThreadRefresh(
								current.snapshot,
								threads,
							);
							const selection_changed =
								Option.getOrUndefined(current.snapshot.selected_thread_id) !==
								Option.getOrUndefined(refreshed.selected_thread_id);

							return {
								...current,
								selection_generation: selection_changed
									? current.selection_generation + 1
									: current.selection_generation,
								snapshot: refreshed,
							};
						}),
				}),
			);

			const current = yield* SubscriptionRef.get(state);
			const selected_thread_id = Option.getOrUndefined(current.snapshot.selected_thread_id);
			if (
				selected_thread_id !== undefined &&
				current.refresh_generation === refresh_generation
			) {
				yield* LoadSelectedThread(selected_thread_id, current.selection_generation);
			}
			yield* client.GetGlobalGuidance.pipe(
				Effect.tap((global_guidance) =>
					Update((current) =>
						current.refresh_generation === refresh_generation
							? {
									...current,
									snapshot: {
										...current.snapshot,
										global_guidance: Option.some(global_guidance),
									},
								}
							: current,
					),
				),
				Effect.ignore,
			);
			yield* client.GetModelBehaviour.pipe(
				Effect.tap((model_behaviour) =>
					Update((current) =>
						current.refresh_generation === refresh_generation
							? {
									...current,
									snapshot: {
										...current.snapshot,
										model_behaviour: Option.some(model_behaviour),
									},
								}
							: current,
					),
				),
				Effect.ignore,
			);
		});

		const SelectThread = (thread_id: string) =>
			Effect.gen(function* () {
				const selected = yield* UpdateAndGet((current) => ({
					...current,
					selection_generation: current.selection_generation + 1,
					snapshot: SelectThreadSnapshot(current.snapshot, thread_id),
				}));
				yield* LoadSelectedThread(thread_id, selected.selection_generation);
			});

		const SendMessage = (text: string) =>
			Effect.gen(function* () {
				const snapshot = (yield* SubscriptionRef.get(state)).snapshot;
				const trimmed = text.trim();
				const thread_id = Option.getOrUndefined(snapshot.selected_thread_id);
				const thread_work = Option.getOrUndefined(snapshot.thread_work);
				const thread = snapshot.threads.find(
					(candidate) => candidate.thread_id === thread_id,
				);
				const project = thread?.primary_project;

				if (
					trimmed.length === 0 ||
					thread_id === undefined ||
					thread_work === undefined ||
					project === undefined
				) {
					yield* Update((current) => ({
						...current,
						snapshot: {
							...current.snapshot,
							error: Option.some(
								"A selected thread with active work and a project is required to send a message.",
							),
						},
					}));
					return;
				}

				yield* client
					.Command({
						agent_id: thread_work.agent_id,
						payload: {
							engine_id: thread_work.engine_id,
							mentioned_projects: [project],
							text: trimmed,
							type: "thread.send_message",
							working_directory: project.root_path,
						},
						run_id: thread_work.run_id,
						thread_id,
					})
					.pipe(
						Effect.matchEffect({
							onFailure: (error) =>
								Update((current) => ({
									...current,
									snapshot: {
										...current.snapshot,
										error: Option.some(error.message),
									},
								})),
							onSuccess: () =>
								Update((current) => ({
									...current,
									snapshot: { ...current.snapshot, error: Option.none() },
								})),
						}),
					);
			});

		const CreateThread = (title: string) =>
			Effect.gen(function* () {
				const trimmed = title.trim();
				const thread_id = globalThis.crypto?.randomUUID?.();

				if (trimmed.length === 0 || thread_id === undefined) {
					yield* Update((current) => ({
						...current,
						snapshot: {
							...current.snapshot,
							error: Option.some(
								"A secure thread identifier and non-empty title are required.",
							),
						},
					}));
					return;
				}

				yield* client
					.Command({
						payload: { title: trimmed, type: "thread.create" },
						thread_id: `thread_${thread_id}`,
					})
					.pipe(
						Effect.matchEffect({
							onFailure: (error) =>
								Update((current) => ({
									...current,
									snapshot: {
										...current.snapshot,
										error: Option.some(error.message),
									},
								})),
							onSuccess: () =>
								Update((current) => ({
									...current,
									snapshot: { ...current.snapshot, error: Option.none() },
								})),
						}),
					);
			});

		const StartThreadListSubscription = Effect.gen(function* () {
			const previous = yield* Ref.get(subscription_fiber);
			if (Option.isSome(previous)) yield* Fiber.interrupt(previous.value);

			const started = yield* UpdateAndGet((current) => ({
				...current,
				subscription_generation: current.subscription_generation + 1,
			}));
			const subscription_generation = started.subscription_generation;
			const Subscribe = RunThreadListSubscription(
				client.SubscribeThreadList,
				(update) =>
					Update((current) => {
						if (current.subscription_generation !== subscription_generation) {
							return current;
						}

						const snapshot = ApplyThreadListUpdate(current.snapshot, update);
						return {
							...current,
							list_generation: current.list_generation + 1,
							selection_generation:
								Option.getOrUndefined(snapshot.selected_thread_id) !==
								Option.getOrUndefined(current.snapshot.selected_thread_id)
									? current.selection_generation + 1
									: current.selection_generation,
							snapshot,
						};
					}),
				(message) =>
					Update((current) =>
						current.subscription_generation === subscription_generation
							? {
									...current,
									snapshot: ApplyThreadListSubscriptionFailure(
										current.snapshot,
										message,
									),
								}
							: current,
					),
			);
			const fiber = yield* Effect.forkScoped(Subscribe);
			yield* Ref.set(subscription_fiber, Option.some(fiber));
		});

		const StopThreadListSubscription = Effect.gen(function* () {
			const previous = yield* Ref.get(subscription_fiber);
			if (Option.isSome(previous)) yield* Fiber.interrupt(previous.value);
			yield* Ref.set(subscription_fiber, Option.none());
			yield* Update((current) => ({
				...current,
				subscription_generation: current.subscription_generation + 1,
			}));
		});

		yield* Stream.runForEach(lifecycle.Changes, (connection) =>
			Effect.gen(function* () {
				yield* Update((current) => ({
					...current,
					snapshot: {
						...current.snapshot,
						error:
							connection.message === undefined
								? current.snapshot.error
								: Option.some(connection.message),
						phase: ToLiveWorkspacePhase(connection.phase),
					},
				}));
				if (ShouldRefreshForConnection(connection.phase)) {
					yield* StartThreadListSubscription;
					yield* Refresh;
				} else {
					yield* StopThreadListSubscription;
				}
			}),
		).pipe(Effect.forkScoped);
		yield* StartThreadListSubscription;
		yield* Refresh;

		return LiveWorkspaceStore.of({
			Changes: SubscriptionRef.changes(state).pipe(Stream.map((current) => current.snapshot)),
			CreateThread,
			Refresh,
			SendMessage,
			SelectThread,
			Snapshot: SubscriptionRef.get(state).pipe(Effect.map((current) => current.snapshot)),
		});
	}),
);
