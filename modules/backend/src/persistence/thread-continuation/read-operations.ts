import { and, asc, desc, eq, lt, sql } from "drizzle-orm";
import { Effect, Option, Schema } from "effect";
import { TranscriptEntry } from "@artisan/protocol";
import { Database } from "../database";
import {
	JournalEvents,
	OrchestrationMessages,
	OrchestrationRuns,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
	UsageInterruptions,
} from "../tables";
import { ThreadRunContinuationState } from "../thread-continuation-schema";
import { DecodePersistedJson, DecodeResumeToken } from "./codec";
import { type CanonicalHistoryEntry, ThreadContinuationFailure } from "./contracts";

export const MakeReadOperations = Effect.gen(function* () {
	const database = yield* Database;
	const Fail = (code: string) => new ThreadContinuationFailure({ code });
	const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
		Effect.gen(function* () {
			const [[thread], [claim], [tombstone]] = yield* Effect.all([
				transaction.select().from(Threads).where(eq(Threads.thread_id, thread_id)).limit(1),
				transaction
					.select()
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1),
				transaction
					.select()
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1),
			]);
			if (!thread || claim || tombstone)
				return yield* Effect.fail(Fail("thread_unavailable"));
		});

	const ReadSourceCut = (
		transaction: typeof database.client,
		target: {
			readonly agent_id: string;
			readonly run_id: string;
			readonly thread_id: string;
		},
	) =>
		Effect.gen(function* () {
			const [first_target_event] = yield* transaction
				.select({ sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.thread_id, target.thread_id),
						eq(JournalEvents.run_id, target.run_id),
					),
				)
				.orderBy(JournalEvents.sequence)
				.limit(1);
			if (!first_target_event) return yield* Effect.fail(Fail("target_journal_missing"));
			const run_starts = transaction
				.select({
					first_sequence: sql<number>`min(${JournalEvents.sequence})`.as(
						"first_sequence",
					),
					run_id: JournalEvents.run_id,
				})
				.from(JournalEvents)
				.where(eq(JournalEvents.thread_id, target.thread_id))
				.groupBy(JournalEvents.run_id)
				.as("source_run_starts");
			const [source_candidate] = yield* transaction
				.select({ run_id: OrchestrationRuns.run_id })
				.from(OrchestrationRuns)
				.innerJoin(run_starts, eq(run_starts.run_id, OrchestrationRuns.run_id))
				.where(
					and(
						eq(OrchestrationRuns.thread_id, target.thread_id),
						eq(OrchestrationRuns.agent_id, target.agent_id),
						lt(run_starts.first_sequence, first_target_event.sequence),
					),
				)
				.orderBy(desc(run_starts.first_sequence))
				.limit(1);
			const [source] =
				source_candidate === undefined
					? []
					: yield* transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, source_candidate.run_id))
							.limit(1);
			if (source === undefined)
				return {
					first_target_event,
					source: undefined,
					source_cut_journal_sequence: 0,
				};
			const [cut_event] = yield* transaction
				.select({ sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.thread_id, target.thread_id),
						eq(JournalEvents.run_id, source.run_id),
					),
				)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1);
			return {
				first_target_event,
				source,
				source_cut_journal_sequence: cut_event?.sequence ?? 0,
			};
		});

	const IsDispatchReady = (target_run_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [target] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, target_run_id))
						.limit(1);
					if (!target) return yield* Effect.fail(Fail("target_run_missing"));
					yield* EnsureLiveThread(transaction, target.thread_id);
					const { source } = yield* ReadSourceCut(transaction, target);
					const prior = source;
					return (
						prior === undefined ||
						!["queued", "running", "waiting"].includes(prior.status)
					);
				}),
			)
			.pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure
						? cause
						: Fail("dispatch_read_failed"),
				),
			);

	const ReadCanonicalHistory = (target_run_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [target] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, target_run_id))
						.limit(1);
					if (!target) return yield* Effect.fail(Fail("target_run_missing"));
					yield* EnsureLiveThread(transaction, target.thread_id);
					const [first_target_event] = yield* transaction
						.select({ sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.where(
							and(
								eq(JournalEvents.thread_id, target.thread_id),
								eq(JournalEvents.run_id, target.run_id),
							),
						)
						.orderBy(asc(JournalEvents.sequence))
						.limit(1);
					if (!first_target_event)
						return yield* Effect.fail(Fail("target_journal_missing"));
					const run_starts = transaction
						.select({
							first_sequence: sql<number>`min(${JournalEvents.sequence})`.as(
								"first_sequence",
							),
							run_id: JournalEvents.run_id,
						})
						.from(JournalEvents)
						.where(eq(JournalEvents.thread_id, target.thread_id))
						.groupBy(JournalEvents.run_id)
						.as("canonical_run_starts");
					const canonical_filter = and(
						eq(OrchestrationRuns.thread_id, target.thread_id),
						eq(OrchestrationRuns.agent_id, target.agent_id),
						lt(run_starts.first_sequence, first_target_event.sequence),
						sql`${JournalEvents.event_type} IN ('thread.message_queued', 'thread.message_steering', 'assistant.message_completed')`,
					);
					const [count_row] = yield* transaction
						.select({ total: sql<number>`count(*)` })
						.from(JournalEvents)
						.innerJoin(
							OrchestrationRuns,
							eq(JournalEvents.run_id, OrchestrationRuns.run_id),
						)
						.innerJoin(run_starts, eq(run_starts.run_id, OrchestrationRuns.run_id))
						.where(canonical_filter);
					const rows = yield* transaction
						.select({
							event_id: JournalEvents.event_id,
							event_type: JournalEvents.event_type,
							journal_sequence: JournalEvents.sequence,
							occurred_at: JournalEvents.occurred_at,
							payload_json: JournalEvents.payload_json,
							run_id: OrchestrationRuns.run_id,
						})
						.from(JournalEvents)
						.innerJoin(
							OrchestrationRuns,
							eq(JournalEvents.run_id, OrchestrationRuns.run_id),
						)
						.innerJoin(run_starts, eq(run_starts.run_id, OrchestrationRuns.run_id))
						.where(canonical_filter)
						.orderBy(desc(run_starts.first_sequence), desc(JournalEvents.sequence))
						.limit(500);
					const [first_user_row] = yield* transaction
						.select({
							event_id: JournalEvents.event_id,
							event_type: JournalEvents.event_type,
							journal_sequence: JournalEvents.sequence,
							occurred_at: JournalEvents.occurred_at,
							payload_json: JournalEvents.payload_json,
							run_first_sequence: run_starts.first_sequence,
							run_id: OrchestrationRuns.run_id,
						})
						.from(JournalEvents)
						.innerJoin(
							OrchestrationRuns,
							eq(JournalEvents.run_id, OrchestrationRuns.run_id),
						)
						.innerJoin(run_starts, eq(run_starts.run_id, OrchestrationRuns.run_id))
						.where(
							and(
								canonical_filter,
								sql`${JournalEvents.event_type} IN ('thread.message_queued', 'thread.message_steering')`,
							),
						)
						.orderBy(asc(run_starts.first_sequence), asc(JournalEvents.sequence))
						.limit(1);
					const [first_user_position] =
						first_user_row === undefined
							? []
							: yield* transaction
									.select({ logical_sequence: sql<number>`count(*)` })
									.from(JournalEvents)
									.innerJoin(
										OrchestrationRuns,
										eq(JournalEvents.run_id, OrchestrationRuns.run_id),
									)
									.innerJoin(
										run_starts,
										eq(run_starts.run_id, OrchestrationRuns.run_id),
									)
									.where(
										and(
											canonical_filter,
											sql`(${run_starts.first_sequence} < ${first_user_row.run_first_sequence} OR (${run_starts.first_sequence} = ${first_user_row.run_first_sequence} AND ${JournalEvents.sequence} <= ${first_user_row.journal_sequence}))`,
										),
									);
					const total_entries = Number(count_row?.total ?? 0);
					const decode_row = (
						row: NonNullable<(typeof rows)[number]>,
						logical_sequence: number,
					) => {
						const payload = DecodePersistedJson(row.payload_json);
						const decoded = Option.isNone(payload)
							? Option.none()
							: Schema.decodeUnknownOption(TranscriptEntry)({
									event_id: row.event_id,
									journal_sequence: row.journal_sequence,
									occurred_at: row.occurred_at,
									payload: payload.value,
								});
						if (
							Option.isNone(decoded) ||
							(decoded.value.payload.type !== "thread.message_queued" &&
								decoded.value.payload.type !== "thread.message_steering" &&
								decoded.value.payload.type !== "assistant.message_completed") ||
							decoded.value.payload.type !== row.event_type
						)
							return Option.none<CanonicalHistoryEntry>();
						return Option.some({
							journal_sequence: row.journal_sequence,
							logical_sequence,
							role:
								decoded.value.payload.type === "assistant.message_completed"
									? "assistant"
									: "user",
							run_id: row.run_id,
							text: decoded.value.payload.text,
						} satisfies CanonicalHistoryEntry);
					};
					const entries: Array<CanonicalHistoryEntry> = [];
					for (const [index, row] of rows.reverse().entries()) {
						const decoded = decode_row(row, total_entries - rows.length + index + 1);
						if (Option.isNone(decoded))
							return yield* Effect.fail(Fail("canonical_history_invalid"));
						entries.push(decoded.value);
					}
					const first_user_objective =
						first_user_row === undefined
							? Option.none<CanonicalHistoryEntry>()
							: decode_row(
									first_user_row,
									Number(first_user_position?.logical_sequence ?? 1),
								);
					if (first_user_row !== undefined && Option.isNone(first_user_objective))
						return yield* Effect.fail(Fail("canonical_history_invalid"));
					return { entries, first_user_objective, total_entries };
				}),
			)
			.pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure
						? cause
						: Fail("canonical_history_read_failed"),
				),
			);

	const ReadContext = (target_run_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [target] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, target_run_id))
						.limit(1);
					if (!target) return yield* Effect.fail(Fail("target_run_missing"));
					yield* EnsureLiveThread(transaction, target.thread_id);
					const [request] = yield* transaction
						.select()
						.from(OrchestrationMessages)
						.where(eq(OrchestrationMessages.run_id, target_run_id))
						.limit(1);
					if (!request) return yield* Effect.fail(Fail("target_request_missing"));
					const { first_target_event, source, source_cut_journal_sequence } =
						yield* ReadSourceCut(transaction, target);
					if (source && ["queued", "running", "waiting"].includes(source.status))
						return yield* Effect.fail(Fail("source_run_active"));
					const [state] =
						source === undefined
							? []
							: yield* transaction
									.select()
									.from(ThreadRunContinuationState)
									.where(eq(ThreadRunContinuationState.run_id, source.run_id))
									.limit(1);
					const resume = DecodeResumeToken(
						DecodePersistedJson(source?.native_resume_json).pipe(Option.getOrUndefined),
					);
					/** Mirrors exactly the authority PrepareLaunch demands for a failed source. */
					const [usage_interruption_authority] =
						source === undefined
							? []
							: yield* transaction
									.select({ interruption_id: UsageInterruptions.interruption_id })
									.from(UsageInterruptions)
									.where(
										and(
											eq(UsageInterruptions.source_run_id, source.run_id),
											eq(UsageInterruptions.target_run_id, target.run_id),
											eq(UsageInterruptions.state, "launching"),
										),
									)
									.limit(1);
					return {
						first_target_journal_sequence: first_target_event.sequence,
						source_cut_journal_sequence,
						request:
							request === undefined
								? undefined
								: {
										command_id: request.command_id,
										message_id: request.message_id,
										text: request.text,
									},
						source: source
							? Option.some({
									catalog_revision: source.catalog_revision,
									engine_id: source.engine_id,
									last_native_turn_id: state?.last_native_turn_id,
									last_observation_sequence:
										state?.last_observation_sequence ?? 0,
									model_id: state?.model_id ?? source.model_id,
									native_thread_id: source.native_thread_id,
									profile_id: source.profile_id,
									provider_route_id: source.provider_route_id,
									resume_token: Option.filter(
										resume,
										(token) =>
											token.native_thread_id === source.native_thread_id,
									),
									run_id: source.run_id,
									status: source.status,
									usage_interruption_resume:
										usage_interruption_authority !== undefined,
									variant_id: source.variant_id,
									working_directory: source.working_directory,
								})
							: Option.none(),
						target: {
							engine_id: target.engine_id,
							model_id: target.model_id,
							run_id: target.run_id,
							thread_id: target.thread_id,
						},
					};
				}),
			)
			.pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure
						? cause
						: Fail("context_read_failed"),
				),
			);

	return { IsDispatchReady, ReadCanonicalHistory, ReadContext };
});
