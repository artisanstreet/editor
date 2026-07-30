import { Context, Data, Effect, Layer, Option, PubSub, Ref, Schedule, Semaphore } from "effect";

import type { EventEnvelope } from "@artisan/protocol";

import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalStore, type JournalStoreError } from "../persistence/journal-store";
import { ThreadReadModel, type ThreadReadModelError } from "../persistence/thread-read-model";
import { ThreadMetadataRefinementWorker } from "./thread-metadata-refinement-worker";
import { ThreadMetadataRepository, type ThreadMetadataError } from "./thread-metadata-repository";
import type {
	ThreadMetadataRefinementRequest,
	ThreadMetadataRefinementTrigger,
} from "./thread-metadata-refiner";

interface ThreadRefinementContext {
	readonly recent_activity: ReadonlyArray<string>;
	readonly recent_artifacts: ReadonlyArray<string>;
	readonly recent_files: ReadonlyArray<string>;
	readonly recent_user_text: ReadonlyArray<string>;
}

interface RefinementCoordinatorState {
	readonly contexts: ReadonlyMap<string, ThreadRefinementContext>;
	readonly journal_sequence: number;
}

interface RefinementTrigger {
	readonly source_event_id: string;
	readonly thread_id: string;
	readonly trigger: ThreadMetadataRefinementTrigger;
}

interface ReducedEvent {
	readonly context?: ThreadRefinementContext;
	readonly erase_context: boolean;
	readonly trigger?: ThreadMetadataRefinementTrigger;
}

/** Reports that a source event could not yet produce a durable refinement outcome. */
export class ThreadMetadataRefinementPending extends Data.TaggedError(
	"ThreadMetadataRefinementPending",
)<{
	readonly source_event_id: string;
	readonly thread_id: string;
}> {}

export type ThreadMetadataRefinementCoordinatorError =
	| JournalStoreError
	| ThreadMetadataError
	| ThreadMetadataRefinementPending
	| ThreadReadModelError;

/** Replays meaningful journal activity into the scoped latest-wins refinement worker. */
export class ThreadMetadataRefinementCoordinator extends Context.Service<
	ThreadMetadataRefinementCoordinator,
	{
		readonly CatchUp: Effect.Effect<number, ThreadMetadataRefinementCoordinatorError>;
		readonly WaitForIdle: Effect.Effect<void, ThreadMetadataRefinementCoordinatorError>;
	}
>()("Artisan/ThreadMetadataRefinementCoordinator") {}

/** Keeps the portable backend deterministic when no automatic refiner is composed. */
export const ThreadMetadataRefinementCoordinatorDisabled = Layer.succeed(
	ThreadMetadataRefinementCoordinator,
	{
		CatchUp: Effect.succeed(0),
		WaitForIdle: Effect.void,
	},
);

const max_remembered_context_items = 16;

function append_context(values: ReadonlyArray<string>, value: string) {
	return [...values, value].slice(-max_remembered_context_items);
}

function empty_context(): ThreadRefinementContext {
	return {
		recent_activity: [],
		recent_artifacts: [],
		recent_files: [],
		recent_user_text: [],
	};
}

function trigger_from_run_state(state: string): ThreadMetadataRefinementTrigger {
	if (state === "failed") {
		return "run_failed";
	}

	return state === "cancelled" ||
		state === "closed" ||
		state === "complete" ||
		state === "completed" ||
		state === "interrupted" ||
		state === "stopped" ||
		state === "summarized"
		? "run_completed"
		: "run_started";
}

function reduce_event(event: EventEnvelope, current: ThreadRefinementContext): ReducedEvent {
	const payload = event.payload;

	if (payload.type === "thread.content_erased" || payload.type === "thread.erased") {
		return { erase_context: true };
	}

	if (payload.type === "thread.message_queued" || payload.type === "thread.message_steering") {
		return {
			context: {
				...current,
				recent_activity: append_context(current.recent_activity, "User message"),
				recent_user_text: append_context(current.recent_user_text, payload.text),
			},
			erase_context: false,
			trigger: "user_message",
		};
	}

	if (payload.type === "run.lifecycle") {
		return {
			context: {
				...current,
				recent_activity: append_context(current.recent_activity, `Run ${payload.state}`),
			},
			erase_context: false,
			trigger: trigger_from_run_state(payload.state),
		};
	}

	if (payload.type === "orchestration.graph.lifecycle" && payload.node_type === "agent_run") {
		return {
			context: {
				...current,
				recent_activity: append_context(
					current.recent_activity,
					`${payload.state}: ${payload.action}`,
				),
			},
			erase_context: false,
			trigger: trigger_from_run_state(payload.state),
		};
	}

	if (payload.type === "artifact.recorded") {
		const artifact_label = `${payload.artifact.kind}: ${payload.artifact.label}`;
		const file_reference =
			payload.artifact.kind === "file" || payload.artifact.kind === "diff"
				? (payload.artifact.uri ?? payload.artifact.label)
				: undefined;

		return {
			context: {
				...current,
				recent_artifacts: append_context(current.recent_artifacts, artifact_label),
				recent_files:
					file_reference === undefined
						? current.recent_files
						: append_context(current.recent_files, file_reference),
			},
			erase_context: false,
		};
	}

	if (payload.type === "assistant.message_completed") {
		return {
			context: {
				...current,
				recent_activity: append_context(
					current.recent_activity,
					"Assistant response completed",
				),
			},
			erase_context: false,
		};
	}

	return { erase_context: false };
}

/** Builds the scoped journal consumer that activates automatic metadata refinement. */
export const ThreadMetadataRefinementCoordinatorLive = Layer.effect(
	ThreadMetadataRefinementCoordinator,
	Effect.gen(function* () {
		const journal = yield* JournalStore;
		const notifier = yield* JournalNotifier;
		const read_model = yield* ThreadReadModel;
		const repository = yield* ThreadMetadataRepository;
		const worker = yield* ThreadMetadataRefinementWorker;
		const subscription = yield* notifier.Subscribe;
		const lock = yield* Semaphore.make(1);
		const state = yield* Ref.make<RefinementCoordinatorState>({
			contexts: new Map(),
			journal_sequence: 0,
		});

		const SubmitUntilAccepted = (request: ThreadMetadataRefinementRequest) =>
			Effect.gen(function* () {
				while (true) {
					const result = yield* worker.Submit(request);

					if (result !== "dropped") {
						return;
					}

					yield* worker.WaitForIdle;
				}
			});

		const VerifyRefined = (trigger: RefinementTrigger) =>
			Effect.gen(function* () {
				const was_refined = yield* repository.WasRefined(
					trigger.source_event_id,
					trigger.thread_id,
				);

				if (was_refined) {
					return;
				}

				const thread = yield* read_model.Lookup(trigger.thread_id);

				if (Option.isNone(thread)) {
					return;
				}

				return yield* new ThreadMetadataRefinementPending({
					source_event_id: trigger.source_event_id,
					thread_id: trigger.thread_id,
				});
			});

		const CatchUpUnlocked = Effect.gen(function* () {
			const current = yield* Ref.get(state);
			const events = yield* journal.ReadReplay({
				after_journal_sequence: current.journal_sequence,
			});

			if (events.length === 0) {
				return 0;
			}

			const contexts = new Map(current.contexts);
			const latest_triggers = new Map<string, RefinementTrigger>();

			for (const event of events) {
				const reduced = reduce_event(
					event,
					contexts.get(event.thread_id) ?? empty_context(),
				);

				if (reduced.erase_context) {
					contexts.delete(event.thread_id);
					latest_triggers.delete(event.thread_id);
					continue;
				}

				if (reduced.context) {
					contexts.set(event.thread_id, reduced.context);
				}

				if (reduced.trigger) {
					latest_triggers.set(event.thread_id, {
						source_event_id: event.message_id,
						thread_id: event.thread_id,
						trigger: reduced.trigger,
					});
				}
			}

			const submitted_triggers: RefinementTrigger[] = [];

			yield* Effect.forEach(latest_triggers.values(), (trigger) =>
				Effect.gen(function* () {
					const thread = yield* read_model.Lookup(trigger.thread_id);

					if (Option.isNone(thread)) {
						return;
					}

					const context = contexts.get(trigger.thread_id) ?? empty_context();

					yield* SubmitUntilAccepted({
						...context,
						projection: thread.value,
						source_event_id: trigger.source_event_id,
						thread_id: trigger.thread_id,
						trigger: trigger.trigger,
					});
					submitted_triggers.push(trigger);
				}),
			);

			yield* worker.WaitForIdle;
			yield* Effect.forEach(submitted_triggers, VerifyRefined);

			const latest_event = events.at(-1);
			if (latest_event === undefined) return 0;
			const journal_sequence = latest_event.journal_sequence;

			yield* Ref.set(state, { contexts, journal_sequence });

			return events.length;
		});

		const CatchUp = Semaphore.withPermit(lock)(CatchUpUnlocked);
		const CatchUpReliably = CatchUp.pipe(
			Effect.retry({ schedule: Schedule.spaced("250 millis") }),
		);
		const Watch = Effect.gen(function* () {
			yield* CatchUpReliably;

			while (true) {
				yield* PubSub.take(subscription);
				yield* CatchUpReliably;
			}
		});

		yield* Watch.pipe(Effect.forkScoped);

		const WaitForIdle = Effect.gen(function* () {
			yield* CatchUp;
			yield* worker.WaitForIdle;
			yield* CatchUp;
			yield* worker.WaitForIdle;
		});

		return { CatchUp, WaitForIdle };
	}),
);
