import { Context, Effect, Layer, Option, PubSub, Ref, Stream } from "effect";

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

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	global_guidance: Option.none(),
	model_behaviour: Option.none(),
	phase: "connecting",
	selected_thread_id: Option.none(),
	thread_work: Option.none(),
	threads: [],
};

export const ToLiveWorkspacePhase = (phase: FrontendConnectionPhase): LiveWorkspacePhase => {
	if (phase === "ready") return "ready";
	if (phase === "reconnecting") return "reconnecting";
	if (phase === "stale") return "stale";
	if (phase === "error" || phase === "unavailable") return "error";
	return "connecting";
};

export const ApplyThreadListUpdate = (
	snapshot: LiveWorkspaceSnapshot,
	update:
		| { readonly type: "snapshot"; readonly threads: ReadonlyArray<ThreadListItem> }
		| { readonly type: "upsert"; readonly thread: ThreadListItem }
		| { readonly type: "remove"; readonly thread_id: string },
): LiveWorkspaceSnapshot => {
	if (update.type === "snapshot") return { ...snapshot, threads: update.threads };
	if (update.type === "remove") {
		return {
			...snapshot,
			selected_thread_id:
				Option.getOrUndefined(snapshot.selected_thread_id) === update.thread_id
					? Option.none()
					: snapshot.selected_thread_id,
			threads: snapshot.threads.filter((thread) => thread.thread_id !== update.thread_id),
		};
	}

	const threads = snapshot.threads.filter(
		(thread) => thread.thread_id !== update.thread.thread_id,
	);
	return { ...snapshot, threads: [update.thread, ...threads] };
};

/** Owns renderer-only live projection state; durable records remain backend projections. */
export class LiveWorkspaceStore extends Context.Service<
	LiveWorkspaceStore,
	{
		readonly Changes: Stream.Stream<LiveWorkspaceSnapshot>;
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
		const state = yield* Ref.make<LiveWorkspaceSnapshot>(EmptySnapshot);
		const changes = yield* PubSub.sliding<LiveWorkspaceSnapshot>(16);

		const Update = (update: (snapshot: LiveWorkspaceSnapshot) => LiveWorkspaceSnapshot) =>
			Ref.modify(state, (snapshot) => {
				const next = update(snapshot);
				return [next, next] as const;
			}).pipe(
				Effect.tap((snapshot) => PubSub.publish(changes, snapshot)),
				Effect.asVoid,
			);

		const LoadSelectedThread = (thread_id: string) =>
			client.GetThreadWork(thread_id).pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((snapshot) => ({ ...snapshot, error: Option.some(error.message) })),
					onSuccess: (thread_work) =>
						Update((snapshot) => ({ ...snapshot, thread_work })),
				}),
			);

		const Refresh = Effect.gen(function* () {
			yield* client.ListThreads.pipe(
				Effect.matchEffect({
					onFailure: (error) =>
						Update((snapshot) => ({
							...snapshot,
							error: Option.some(error.message),
							phase: "error",
						})),
					onSuccess: (threads) =>
						Update((snapshot) => {
							const selected_thread_id = Option.isSome(snapshot.selected_thread_id)
								? snapshot.selected_thread_id
								: threads[0] === undefined
									? Option.none<string>()
									: Option.some(threads[0].thread_id);
							return {
								...snapshot,
								phase: threads.length === 0 ? "empty" : "ready",
								selected_thread_id,
								threads,
							};
						}),
				}),
			);
			const selected_thread_id = Option.getOrUndefined(
				(yield* Ref.get(state)).selected_thread_id,
			);
			if (selected_thread_id !== undefined) yield* LoadSelectedThread(selected_thread_id);
			yield* client.GetGlobalGuidance.pipe(
				Effect.tap((global_guidance) =>
					Update((snapshot) => ({
						...snapshot,
						global_guidance: Option.some(global_guidance),
					})),
				),
				Effect.ignore,
			);
			yield* client.GetModelBehaviour.pipe(
				Effect.tap((model_behaviour) =>
					Update((snapshot) => ({
						...snapshot,
						model_behaviour: Option.some(model_behaviour),
					})),
				),
				Effect.ignore,
			);
		});

		const SelectThread = (thread_id: string) =>
			Effect.gen(function* () {
				yield* Update((snapshot) => ({
					...snapshot,
					error: Option.none(),
					selected_thread_id: Option.some(thread_id),
					thread_work: Option.none(),
				}));
				yield* LoadSelectedThread(thread_id);
			});

		const SendMessage = (text: string) =>
			Effect.gen(function* () {
				const trimmed = text.trim();
				const snapshot = yield* Ref.get(state);
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
						error: Option.some(
							"A selected thread with active work and a project is required to send a message.",
						),
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
									error: Option.some(error.message),
								})),
							onSuccess: () =>
								Update((current) => ({ ...current, error: Option.none() })),
						}),
					);
			});

		yield* Stream.runForEach(lifecycle.Changes, (connection) =>
			Update((snapshot) => ({
				...snapshot,
				error:
					connection.message === undefined
						? snapshot.error
						: Option.some(connection.message),
				phase: ToLiveWorkspacePhase(connection.phase),
			})),
		).pipe(Effect.forkScoped);
		yield* client.SubscribeThreadList.pipe(
			Effect.flatMap((updates) =>
				Stream.runForEach(updates, (update) =>
					Update((snapshot) => ApplyThreadListUpdate(snapshot, update)),
				),
			),
			Effect.ignore,
			Effect.forkScoped,
		);
		yield* Refresh;

		return LiveWorkspaceStore.of({
			Changes: Stream.fromPubSub(changes),
			Refresh,
			SendMessage,
			SelectThread,
			Snapshot: Ref.get(state),
		});
	}),
);
