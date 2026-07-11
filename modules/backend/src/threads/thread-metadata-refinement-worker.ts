import { Cause, Context, Deferred, Effect, Layer, Option, Queue, Ref } from "effect";

import {
	ThreadMetadataRepository,
	type ThreadMetadataRefinementIntent,
} from "./thread-metadata-repository";
import {
	bound_thread_metadata_refiner_input,
	ThreadMetadataRefiner,
	type ThreadMetadataRefinementRequest,
	type ThreadMetadataRefinement,
} from "./thread-metadata-refiner";

/** Configures the bounded latest-wins metadata refinement worker. */
export interface ThreadMetadataRefinementWorkerOptions {
	readonly max_pending?: number;
}

/** Reports whether a refinement request was queued, coalesced, or rejected at capacity. */
export type ThreadMetadataRefinementSubmission = "queued" | "coalesced" | "dropped";

/** Schedules automatic metadata refinement without subscribing to its own emitted events. */
export class ThreadMetadataRefinementWorker extends Context.Service<
	ThreadMetadataRefinementWorker,
	{
		readonly Submit: (
			request: ThreadMetadataRefinementRequest,
		) => Effect.Effect<ThreadMetadataRefinementSubmission>;
		readonly WaitForIdle: Effect.Effect<void>;
	}
>()("Artisan/ThreadMetadataRefinementWorker") {}

interface WorkerState {
	readonly active: ReadonlySet<string>;
	readonly pending: ReadonlyMap<string, ThreadMetadataRefinementRequest>;
	readonly waiters: ReadonlyArray<Deferred.Deferred<void>>;
}

const make_intent = (
	request: ThreadMetadataRefinementRequest,
	refinement: ThreadMetadataRefinement,
): ThreadMetadataRefinementIntent => ({
	operation_id: `metadata-refine:${request.source_event_id}`,
	payload: {
		...refinement,
		basis_activity_version: request.projection.activity_version,
		basis_metadata_version: request.projection.metadata_version,
		type: "thread.metadata.refine",
	},
	source_event_id: request.source_event_id,
	thread_id: request.thread_id,
});

/** Creates a scoped, bounded, latest-wins refinement worker. */
export const make_thread_metadata_refinement_worker_layer = (
	options: ThreadMetadataRefinementWorkerOptions = {},
) =>
	Layer.effect(
		ThreadMetadataRefinementWorker,
		Effect.gen(function* () {
			const repository = yield* ThreadMetadataRepository;
			const refiner = yield* ThreadMetadataRefiner;
			const max_pending = Math.max(1, options.max_pending ?? 32);
			const queue = yield* Queue.bounded<string>(max_pending);
			const state = yield* Ref.make<WorkerState>({
				active: new Set(),
				pending: new Map(),
				waiters: [],
			});

			const signal_idle = Effect.gen(function* () {
				const waiters = yield* Ref.modify<
					WorkerState,
					ReadonlyArray<Deferred.Deferred<void>>
				>(state, (current) =>
					current.active.size === 0 && current.pending.size === 0
						? [current.waiters, { ...current, waiters: [] }]
						: [[], current],
				);

				yield* Effect.forEach(waiters, (waiter) => Deferred.succeed(waiter, undefined));
			});

			const process = (request: ThreadMetadataRefinementRequest) =>
				Effect.gen(function* () {
					const was_refined = yield* repository.WasRefined(
						request.source_event_id,
						request.thread_id,
					);

					if (was_refined) {
						return;
					}

					const refinement = yield* refiner.Refine(
						bound_thread_metadata_refiner_input(request),
					);

					yield* repository.Refine(make_intent(request, refinement));
				}).pipe(
					Effect.catchCause((cause) =>
						Cause.hasInterruptsOnly(cause) ? Effect.failCause(cause) : Effect.void,
					),
				);

			const process_thread = (thread_id: string) =>
				Effect.gen(function* () {
					while (true) {
						const request = yield* Ref.modify(state, (current) => {
							const next_pending = new Map(current.pending);
							const next_request = next_pending.get(thread_id);

							if (!next_request) {
								return [Option.none<ThreadMetadataRefinementRequest>(), current];
							}

							next_pending.delete(thread_id);
							return [
								Option.some(next_request),
								{
									...current,
									active: new Set([...current.active, thread_id]),
									pending: next_pending,
								},
							];
						});

						if (Option.isNone(request)) {
							return;
						}

						yield* process(request.value);
					}
				}).pipe(
					Effect.ensuring(
						Ref.update(state, (current) => ({
							...current,
							active: new Set([...current.active].filter((id) => id !== thread_id)),
						})).pipe(Effect.andThen(signal_idle)),
					),
				);

			const drain = Effect.forever(
				Effect.gen(function* () {
					const thread_id = yield* Queue.take(queue);
					yield* process_thread(thread_id);
				}),
			);
			yield* drain.pipe(Effect.forkScoped);

			yield* Effect.addFinalizer(() => Queue.shutdown(queue));

			const submit = (request: ThreadMetadataRefinementRequest) =>
				Effect.gen(function* () {
					const bounded_request = {
						...bound_thread_metadata_refiner_input(request),
						source_event_id: request.source_event_id,
						thread_id: request.thread_id,
					};
					const decision = yield* Ref.modify(state, (current) => {
						const already_pending = current.pending.has(request.thread_id);
						const already_active = current.active.has(request.thread_id);

						if (already_pending || already_active) {
							const pending = new Map(current.pending);
							pending.set(request.thread_id, bounded_request);
							return ["coalesced" as const, { ...current, pending }];
						}

						if (current.pending.size >= max_pending) {
							return ["dropped" as const, current];
						}

						const pending = new Map(current.pending);
						pending.set(request.thread_id, bounded_request);
						return ["queued" as const, { ...current, pending }];
					});

					if (decision !== "queued") {
						return decision;
					}

					yield* Queue.offer(queue, request.thread_id);
					return decision;
				});

			const wait_for_idle = Effect.gen(function* () {
				const deferred = yield* Deferred.make<void>();
				const should_wait = yield* Ref.modify(state, (current) =>
					current.active.size === 0 && current.pending.size === 0
						? [false, current]
						: [true, { ...current, waiters: [...current.waiters, deferred] }],
				);

				if (should_wait) {
					yield* Deferred.await(deferred);
				}
			});

			return { Submit: submit, WaitForIdle: wait_for_idle };
		}),
	);
