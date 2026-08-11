import { and, asc, desc, eq, gt, inArray, isNull, lte } from "drizzle-orm";
import {
	Context,
	Data,
	Effect,
	Layer,
	Option,
	PubSub,
	Ref,
	Schedule,
	Schema,
	Semaphore,
} from "effect";

import { JournalSequence } from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalCommands, JournalEvents } from "../persistence/tables";
import { ThreadReadModel, type ThreadReadModelError } from "../persistence/thread-read-model";
import { ThreadMetadataRefinementWorker } from "./thread-metadata-refinement-worker";
import { ThreadMetadataRepository, type ThreadMetadataError } from "./thread-metadata-repository";
import type {
	ThreadMetadataRefinementRequest,
	ThreadMetadataRefinementTrigger,
} from "./thread-metadata-refiner";

interface ThreadRefinementContext {
	readonly recent_activity: ReadonlyArray<string>;
	readonly recent_assistant_text: ReadonlyArray<string>;
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

const ThreadMetadataEvidencePayload = Schema.Union([
	Schema.Struct({ type: Schema.Literal("thread.content_erased") }),
	Schema.Struct({ type: Schema.Literal("thread.erased") }),
	Schema.Struct({ text: Schema.NonEmptyString, type: Schema.Literal("thread.message_queued") }),
	Schema.Struct({ text: Schema.NonEmptyString, type: Schema.Literal("thread.message_steering") }),
	Schema.Struct({ state: Schema.NonEmptyString, type: Schema.Literal("run.lifecycle") }),
	Schema.Struct({
		action: Schema.NonEmptyString,
		node_type: Schema.NonEmptyString,
		state: Schema.NonEmptyString,
		type: Schema.Literal("orchestration.graph.lifecycle"),
	}),
	Schema.Struct({
		artifact: Schema.Struct({
			kind: Schema.NonEmptyString,
			label: Schema.NonEmptyString,
			uri: Schema.optional(Schema.String),
		}),
		type: Schema.Literal("artifact.recorded"),
	}),
	Schema.Struct({
		text: Schema.NonEmptyString,
		type: Schema.Literal("assistant.message_completed"),
	}),
]);

type ThreadMetadataEvidencePayload = typeof ThreadMetadataEvidencePayload.Type;

interface ThreadMetadataEvidence {
	readonly journal_sequence: number;
	readonly payload: ThreadMetadataEvidencePayload;
	readonly source_event_id: string;
	readonly thread_id: string;
}

const thread_metadata_evidence_types = [
	"thread.content_erased",
	"thread.erased",
	"thread.message_queued",
	"thread.message_steering",
	"run.lifecycle",
	"orchestration.graph.lifecycle",
	"artifact.recorded",
	"assistant.message_completed",
] as const;

/** Only these evidence kinds can require an automatic refinement on recovery. */
const thread_metadata_trigger_types = [
	"thread.message_queued",
	"thread.message_steering",
	"run.lifecycle",
	"orchestration.graph.lifecycle",
	"assistant.message_completed",
] as const;

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

/** Reports that the coordinator could not read its narrow journal evidence view. */
export class ThreadMetadataRefinementReadFailure extends Data.TaggedError(
	"ThreadMetadataRefinementReadFailure",
)<{ readonly cause: unknown }> {}

export type ThreadMetadataRefinementCoordinatorError =
	| ThreadMetadataError
	| ThreadMetadataRefinementPending
	| ThreadMetadataRefinementReadFailure
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
		recent_assistant_text: [],
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

function reduce_event(
	event: ThreadMetadataEvidence,
	current: ThreadRefinementContext,
): ReducedEvent {
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

	if (
		payload.type === "orchestration.graph.lifecycle" &&
		payload.node_type === "orchestration_group"
	) {
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
				recent_assistant_text: append_context(current.recent_assistant_text, payload.text),
			},
			erase_context: false,
			trigger: "assistant_message",
		};
	}

	return { erase_context: false };
}

/** Builds the scoped journal consumer that activates automatic metadata refinement. */
export const ThreadMetadataRefinementCoordinatorLive = Layer.effect(
	ThreadMetadataRefinementCoordinator,
	Effect.gen(function* () {
		const database = yield* Database;
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

		const DecodeEvidence = (row: {
			readonly event_id: string;
			readonly event_type: string;
			readonly journal_sequence: number;
			readonly payload_json: string;
			readonly thread_id: string;
		}) =>
			Effect.gen(function* () {
				const payload_json = Schema.decodeUnknownOption(Schema.UnknownFromJsonString)(
					row.payload_json,
				);
				const payload = Option.flatMap(payload_json, (value) =>
					Schema.decodeUnknownOption(ThreadMetadataEvidencePayload)(value),
				);

				if (Option.isNone(payload) || payload.value.type !== row.event_type) {
					yield* Effect.logWarning("Skipping incompatible thread metadata evidence", {
						event_id: row.event_id,
						event_type: row.event_type,
					});
					return Option.none<ThreadMetadataEvidence>();
				}

				const journal_sequence = yield* Schema.decodeUnknownEffect(JournalSequence)(
					row.journal_sequence,
				).pipe(
					Effect.mapError((cause) => new ThreadMetadataRefinementReadFailure({ cause })),
				);

				return Option.some({
					journal_sequence,
					payload: payload.value,
					source_event_id: row.event_id,
					thread_id: row.thread_id,
				});
			});

		const ReadEvidence = (after_journal_sequence: number) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [latest] = yield* transaction
							.select({ journal_sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const journal_sequence = latest?.journal_sequence ?? 0;

						if (journal_sequence <= after_journal_sequence) {
							return {
								events: [] as ReadonlyArray<ThreadMetadataEvidence>,
								journal_sequence,
							};
						}

						const rows = yield* transaction
							.select({
								event_id: JournalEvents.event_id,
								event_type: JournalEvents.event_type,
								journal_sequence: JournalEvents.sequence,
								payload_json: JournalEvents.payload_json,
								thread_id: JournalEvents.thread_id,
							})
							.from(JournalEvents)
							.where(
								and(
									gt(JournalEvents.sequence, after_journal_sequence),
									lte(JournalEvents.sequence, journal_sequence),
									inArray(
										JournalEvents.event_type,
										thread_metadata_evidence_types,
									),
								),
							)
							.orderBy(asc(JournalEvents.sequence));
						const decoded = yield* Effect.forEach(rows, DecodeEvidence);

						return {
							events: decoded.filter(Option.isSome).map((event) => event.value),
							journal_sequence,
						};
					}),
				)
				.pipe(
					Effect.mapError((cause) =>
						cause instanceof ThreadMetadataRefinementReadFailure
							? cause
							: new ThreadMetadataRefinementReadFailure({ cause }),
					),
				);

		/**
		 * Startup does not rebuild an in-memory cursor from every historical event.
		 * A source is recoverable precisely while the idempotent refinement command
		 * for it is absent, so this query scales with unfinished work rather than
		 * the lifetime of the journal.
		 */
		const ReadPendingRecoveryEvidence = (up_to_journal_sequence: number) =>
			database.client
				.select({
					event_id: JournalEvents.event_id,
					event_type: JournalEvents.event_type,
					journal_sequence: JournalEvents.sequence,
					payload_json: JournalEvents.payload_json,
					thread_id: JournalEvents.thread_id,
				})
				.from(JournalEvents)
				.leftJoin(
					JournalCommands,
					and(
						eq(JournalCommands.thread_id, JournalEvents.thread_id),
						eq(JournalCommands.causation_id, JournalEvents.event_id),
						eq(JournalCommands.origin, "backend"),
						eq(JournalCommands.payload_type, "thread.metadata.refine"),
					),
				)
				.where(
					and(
						lte(JournalEvents.sequence, up_to_journal_sequence),
						inArray(JournalEvents.event_type, thread_metadata_trigger_types),
						isNull(JournalCommands.message_id),
					),
				)
				.orderBy(asc(JournalEvents.sequence))
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeEvidence)),
					Effect.map((decoded) =>
						decoded.filter(Option.isSome).map((event) => event.value),
					),
					Effect.mapError((cause) => new ThreadMetadataRefinementReadFailure({ cause })),
				);
		const ReadCurrentJournalSequence = database.client
			.select({ journal_sequence: JournalEvents.sequence })
			.from(JournalEvents)
			.orderBy(desc(JournalEvents.sequence))
			.limit(1)
			.pipe(
				Effect.flatMap(([latest]) =>
					latest === undefined
						? Effect.succeed(0)
						: Schema.decodeUnknownEffect(JournalSequence)(latest.journal_sequence),
				),
				Effect.mapError((cause) => new ThreadMetadataRefinementReadFailure({ cause })),
			);

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

		const SubmitTriggers = (
			contexts: ReadonlyMap<string, ThreadRefinementContext>,
			triggers: Iterable<RefinementTrigger>,
		) =>
			Effect.gen(function* () {
				const submissions = yield* Effect.forEach(triggers, (trigger) =>
					Effect.gen(function* () {
						const thread = yield* read_model.Lookup(trigger.thread_id);

						if (Option.isNone(thread)) {
							return Option.none<{
								readonly request: ThreadMetadataRefinementRequest;
								readonly trigger: RefinementTrigger;
							}>();
						}

						return Option.some({
							request: {
								...(contexts.get(trigger.thread_id) ?? empty_context()),
								projection: thread.value,
								source_event_id: trigger.source_event_id,
								thread_id: trigger.thread_id,
								trigger: trigger.trigger,
							},
							trigger,
						});
					}),
				);
				const ready_submissions = submissions
					.filter(Option.isSome)
					.map((item) => item.value);
				yield* Effect.forEach(ready_submissions, ({ request }) =>
					SubmitUntilAccepted(request),
				);
				yield* worker.WaitForIdle;
				yield* Effect.forEach(ready_submissions, ({ trigger }) => VerifyRefined(trigger));
			});

		const RecoverPending = (up_to_journal_sequence: number) =>
			Effect.gen(function* () {
				const events = yield* ReadPendingRecoveryEvidence(up_to_journal_sequence);
				const contexts = new Map<string, ThreadRefinementContext>();
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
							source_event_id: event.source_event_id,
							thread_id: event.thread_id,
							trigger: reduced.trigger,
						});
					}
				}

				yield* SubmitTriggers(contexts, latest_triggers.values());
			});

		const CatchUpUnlocked = Effect.gen(function* () {
			const current = yield* Ref.get(state);
			const evidence = yield* ReadEvidence(current.journal_sequence);
			const events = evidence.events;

			if (evidence.journal_sequence === current.journal_sequence) {
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
						source_event_id: event.source_event_id,
						thread_id: event.thread_id,
						trigger: reduced.trigger,
					});
				}
			}

			const current_journal_sequence = yield* ReadCurrentJournalSequence;

			/**
			 * A newer append may have advanced both the title and metadata version after
			 * this evidence snapshot. Retry from the old cursor instead of submitting a
			 * stale context against that newer projection.
			 */
			if (current_journal_sequence !== evidence.journal_sequence) {
				return 0;
			}

			yield* SubmitTriggers(contexts, latest_triggers.values());

			yield* Ref.set(state, {
				contexts,
				journal_sequence: evidence.journal_sequence,
			});

			return events.length;
		});

		const CatchUp = Semaphore.withPermit(lock)(CatchUpUnlocked);
		const CatchUpReliably = CatchUp.pipe(
			Effect.retry({ schedule: Schedule.spaced("250 millis") }),
		);
		const startup_journal_sequence = yield* ReadCurrentJournalSequence;
		yield* Ref.set(state, {
			contexts: new Map(),
			journal_sequence: startup_journal_sequence,
		});
		const RecoverPendingReliably = Semaphore.withPermit(lock)(
			RecoverPending(startup_journal_sequence),
		).pipe(Effect.retry({ schedule: Schedule.spaced("250 millis") }));

		const Watch = Effect.gen(function* () {
			yield* RecoverPendingReliably;
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
