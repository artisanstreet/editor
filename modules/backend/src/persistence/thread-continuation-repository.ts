import { and, asc, desc, eq, isNotNull, lt, sql } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	normalize_engine_compaction_summary,
	type EngineObservation,
	type EngineResumeToken,
} from "@artisan/engines";
import { TranscriptEntry } from "@artisan/protocol";

import { Database } from "./database";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
} from "./schema";
import {
	ThreadContinuationLaunches,
	ThreadPortableHandoffs,
	ThreadRunContinuationState,
} from "./thread-continuation-schema";
import {
	encode_portable_checkpoint_content,
	PortableCheckpoint,
	PortableCheckpointSummary,
	type PortableCheckpoint as PortableCheckpointValue,
} from "../orchestration/thread-continuation-model";

const EngineResumeTokenSchema = Schema.Struct({
	native_thread_id: Schema.NonEmptyString,
	opaque_checkpoint: Schema.optional(Schema.String),
});

const NativeCompaction = Schema.Struct({
	boundary_id: Schema.NonEmptyString,
	method: Schema.Literal("claude_post_compact"),
	observation_id: Schema.NonEmptyString,
	source_native_thread_id: Schema.NonEmptyString,
	summary: PortableCheckpointSummary,
	summary_sha256: Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/)),
	trigger: Schema.Literals(["manual", "auto"]),
});
type NativeCompaction = typeof NativeCompaction.Type;

export type ThreadContinuationContext = {
	readonly first_target_journal_sequence: number;
	readonly source_cut_journal_sequence: number;
	readonly native_compaction: Option.Option<{
		readonly through_journal_sequence: number;
		readonly through_run_id: string;
		readonly value: NativeCompaction;
	}>;
	readonly request:
		| { readonly command_id: string; readonly message_id: string; readonly text: string }
		| undefined;
	readonly source: Option.Option<{
		readonly engine_id: string;
		readonly last_native_turn_id: string | null | undefined;
		readonly last_observation_sequence: number;
		readonly model_id: string | null | undefined;
		readonly native_thread_id: string | null;
		readonly resume_token: Option.Option<EngineResumeToken>;
		readonly status: string;
		readonly working_directory: string;
		readonly run_id: string;
	}>;
	readonly target: {
		readonly engine_id: string;
		readonly model_id: string | null;
		readonly run_id: string;
		readonly thread_id: string;
	};
};

const FailureCode = Schema.String.check(Schema.isPattern(/^[a-z][a-z0-9_]{0,63}$/));

export class ThreadContinuationFailure extends Data.TaggedError("ThreadContinuationFailure")<{
	readonly code: string;
}> {}

export class ThreadContinuationConflict extends Data.TaggedError("ThreadContinuationConflict")<{
	readonly target_run_id: string;
}> {}

type ContinuationError = ThreadContinuationFailure | ThreadContinuationConflict;
export type ThreadContinuationLaunchState = "prepared" | "opening" | "bound" | "failed";

export type CanonicalHistoryBoundary = {
	readonly through_journal_sequence: number;
	readonly through_run_id: string;
};

export type CanonicalHistoryEntry = {
	readonly journal_sequence: number;
	readonly logical_sequence: number;
	readonly role: "user" | "assistant";
	readonly run_id: string;
	readonly text: string;
};

export type CanonicalHistory = {
	readonly entries: ReadonlyArray<CanonicalHistoryEntry>;
	readonly first_user_objective: Option.Option<CanonicalHistoryEntry>;
	readonly total_entries: number;
};

export type ContinuationLaunch =
	| {
			readonly _tag: "fresh";
			readonly request_id: string;
			readonly target_model_id?: string;
	  }
	| {
			readonly _tag: "native";
			readonly request_id: string;
			readonly source_run_id: string;
			readonly target_model_id?: string;
	  }
	| {
			readonly _tag: "portable";
			readonly handoff_id: string;
			readonly request_id: string;
			readonly source_run_id: string;
			readonly target_model_id?: string;
			readonly checkpoint: PortableCheckpointValue;
			readonly lineage: typeof PortableHandoffLineage.Type;
	  };

export const PortableHandoffLineage = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("canonical") }),
	Schema.Struct({
		boundary_id: Schema.NonEmptyString,
		kind: Schema.Literal("claude"),
		observation_id: Schema.NonEmptyString,
		source_native_thread_id: Schema.NonEmptyString,
		source_native_turn_id: Schema.optional(Schema.NonEmptyString),
		through_run_id: Schema.NonEmptyString,
	}),
	Schema.Struct({
		export_native_item_id: Schema.NonEmptyString,
		export_native_thread_id: Schema.NonEmptyString,
		export_native_turn_id: Schema.NonEmptyString,
		kind: Schema.Literal("codex"),
		source_native_thread_id: Schema.NonEmptyString,
		source_native_turn_id: Schema.NonEmptyString,
	}),
]);

const lineage_matches_method = (
	method: PortableCheckpointValue["method"],
	lineage: typeof PortableHandoffLineage.Type,
) =>
	(method === "canonical_transcript_summary" && lineage.kind === "canonical") ||
	(method === "claude_post_compact" && lineage.kind === "claude") ||
	(method === "codex_fork_summary" && lineage.kind === "codex");

const DecodeResumeToken = (value: unknown) =>
	Schema.decodeUnknownOption(EngineResumeTokenSchema)(value).pipe(
		Option.map((token) => ({
			native_thread_id: token.native_thread_id,
			...(token.opaque_checkpoint === undefined
				? {}
				: { opaque_checkpoint: token.opaque_checkpoint }),
		})),
	);

const ParsePersistedJson = (value: string | null | undefined): unknown | undefined => {
	if (value === null || value === undefined) return undefined;
	try {
		return JSON.parse(value);
	} catch {
		return undefined;
	}
};

/** Private SQLite ownership for crash-safe engine continuation lineage. */
export class ThreadContinuationRepository extends Context.Service<
	ThreadContinuationRepository,
	{
		readonly IsDispatchReady: (
			target_run_id: string,
		) => Effect.Effect<boolean, ContinuationError>;
		readonly ReadContext: (
			target_run_id: string,
		) => Effect.Effect<ThreadContinuationContext, ContinuationError>;
		readonly ReadCanonicalHistory: (
			target_run_id: string,
			boundary?: CanonicalHistoryBoundary,
		) => Effect.Effect<CanonicalHistory, ContinuationError>;
		readonly RecordObservationMetadata: (
			observation: EngineObservation,
		) => Effect.Effect<void, ContinuationError>;
		readonly RecordNativeCompaction: (
			run_id: string,
			compaction: unknown,
		) => Effect.Effect<void, ContinuationError>;
		readonly PrepareLaunch: (
			target_run_id: string,
			launch: ContinuationLaunch,
		) => Effect.Effect<ThreadContinuationLaunchState, ContinuationError>;
		readonly MarkOpening: (target_run_id: string) => Effect.Effect<void, ContinuationError>;
		readonly BindTarget: (input: {
			readonly command_id: string;
			readonly model_id?: string;
			readonly native_thread_id: string;
			readonly resume_token: EngineResumeToken;
			readonly target_run_id: string;
		}) => Effect.Effect<void, ContinuationError>;
		readonly FailLaunch: (
			target_run_id: string,
			failure_code: string,
		) => Effect.Effect<void, ContinuationError>;
		readonly ReconcileStranded: () => Effect.Effect<ReadonlyArray<string>, ContinuationError>;
	}
>()("Artisan/ThreadContinuationRepository") {}

export const ThreadContinuationRepositoryLive = Layer.effect(
	ThreadContinuationRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const Fail = (code: string) => new ThreadContinuationFailure({ code });
		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [[thread], [claim], [tombstone]] = yield* Effect.all([
					transaction
						.select()
						.from(Threads)
						.where(eq(Threads.thread_id, thread_id))
						.limit(1),
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

		const ReadCanonicalHistory = (target_run_id: string, boundary?: CanonicalHistoryBoundary) =>
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
						const [boundary_start] =
							boundary === undefined
								? []
								: yield* transaction
										.select({ first_sequence: run_starts.first_sequence })
										.from(OrchestrationRuns)
										.innerJoin(
											run_starts,
											eq(run_starts.run_id, OrchestrationRuns.run_id),
										)
										.where(
											and(
												eq(
													OrchestrationRuns.run_id,
													boundary.through_run_id,
												),
												eq(OrchestrationRuns.thread_id, target.thread_id),
												eq(OrchestrationRuns.agent_id, target.agent_id),
												lt(
													run_starts.first_sequence,
													first_target_event.sequence,
												),
											),
										)
										.limit(1);
						if (boundary !== undefined && boundary_start === undefined)
							return yield* Effect.fail(Fail("canonical_boundary_invalid"));
						const boundary_filter =
							boundary === undefined
								? sql`1 = 1`
								: sql`(${run_starts.first_sequence} > ${boundary_start!.first_sequence} OR (${OrchestrationRuns.run_id} = ${boundary.through_run_id} AND ${JournalEvents.sequence} > ${boundary.through_journal_sequence}))`;
						const canonical_filter = and(
							eq(OrchestrationRuns.thread_id, target.thread_id),
							eq(OrchestrationRuns.agent_id, target.agent_id),
							lt(run_starts.first_sequence, first_target_event.sequence),
							sql`${JournalEvents.event_type} IN ('thread.message_queued', 'thread.message_steering', 'assistant.message_completed')`,
							boundary_filter,
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
							const payload = ParsePersistedJson(row.payload_json);
							const decoded =
								payload === undefined
									? Option.none()
									: Schema.decodeUnknownOption(TranscriptEntry)({
											event_id: row.event_id,
											journal_sequence: row.journal_sequence,
											occurred_at: row.occurred_at,
											payload,
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
							const decoded = decode_row(
								row,
								total_entries - rows.length + index + 1,
							);
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
						const [latest_native_compaction] =
							source?.engine_id !== "claude" || source.native_thread_id === null
								? []
								: yield* Effect.gen(function* () {
										if (
											source === undefined ||
											source.native_thread_id === null
										)
											return [];
										const source_native_thread_id = source.native_thread_id;
										const run_starts = transaction
											.select({
												first_sequence:
													sql<number>`min(${JournalEvents.sequence})`.as(
														"first_sequence",
													),
												run_id: JournalEvents.run_id,
											})
											.from(JournalEvents)
											.where(eq(JournalEvents.thread_id, target.thread_id))
											.groupBy(JournalEvents.run_id)
											.as("compaction_run_starts");
										const [source_start] = yield* transaction
											.select({ first_sequence: run_starts.first_sequence })
											.from(run_starts)
											.where(eq(run_starts.run_id, source.run_id))
											.limit(1);
										if (source_start === undefined) return [];
										return yield* transaction
											.select({
												run_id: ThreadRunContinuationState.run_id,
												native_compaction_boundary_journal_sequence:
													ThreadRunContinuationState.native_compaction_boundary_journal_sequence,
												native_compaction_json:
													ThreadRunContinuationState.native_compaction_json,
											})
											.from(ThreadRunContinuationState)
											.innerJoin(
												OrchestrationRuns,
												eq(
													ThreadRunContinuationState.run_id,
													OrchestrationRuns.run_id,
												),
											)
											.innerJoin(
												run_starts,
												eq(run_starts.run_id, OrchestrationRuns.run_id),
											)
											.where(
												and(
													eq(
														ThreadRunContinuationState.thread_id,
														target.thread_id,
													),
													eq(
														ThreadRunContinuationState.engine_id,
														"claude",
													),
													eq(
														OrchestrationRuns.thread_id,
														target.thread_id,
													),
													eq(OrchestrationRuns.agent_id, target.agent_id),
													eq(OrchestrationRuns.engine_id, "claude"),
													eq(
														OrchestrationRuns.native_thread_id,
														source_native_thread_id,
													),
													isNotNull(
														ThreadRunContinuationState.native_compaction_json,
													),
													isNotNull(
														ThreadRunContinuationState.native_compaction_boundary_journal_sequence,
													),
													sql`(${run_starts.first_sequence} < ${source_start.first_sequence} OR (${run_starts.run_id} = ${source.run_id} AND ${ThreadRunContinuationState.native_compaction_boundary_journal_sequence} <= ${source_cut_journal_sequence}))`,
												),
											)
											.orderBy(
												desc(run_starts.first_sequence),
												desc(
													ThreadRunContinuationState.native_compaction_boundary_journal_sequence,
												),
											)
											.limit(1);
									});
						const resume = DecodeResumeToken(
							ParsePersistedJson(source?.native_resume_json),
						);
						const parsed_native_compaction = ParsePersistedJson(
							latest_native_compaction?.native_compaction_json,
						);
						const decoded_native_compaction =
							parsed_native_compaction === undefined
								? Option.none<NativeCompaction>()
								: Schema.decodeUnknownOption(NativeCompaction)(
										parsed_native_compaction,
									);
						const verified_native_compaction = Option.isNone(decoded_native_compaction)
							? Option.none<NativeCompaction>()
							: (yield* crypto
										.digest(
											"SHA-256",
											new TextEncoder().encode(
												normalize_engine_compaction_summary(
													decoded_native_compaction.value.summary,
												),
											),
										)
										.pipe(Effect.map(Encoding.encodeHex))) ===
								  decoded_native_compaction.value.summary_sha256
								? decoded_native_compaction
								: Option.none<NativeCompaction>();
						const native_compaction =
							source &&
							latest_native_compaction?.native_compaction_boundary_journal_sequence !==
								null &&
							latest_native_compaction?.native_compaction_boundary_journal_sequence !==
								undefined
								? verified_native_compaction.pipe(
										Option.filter(
											(value) =>
												value.source_native_thread_id ===
												source.native_thread_id,
										),
										Option.map((value) => ({
											through_journal_sequence:
												latest_native_compaction.native_compaction_boundary_journal_sequence!,
											through_run_id: latest_native_compaction.run_id,
											value,
										})),
									)
								: Option.none<{
										readonly through_journal_sequence: number;
										readonly through_run_id: string;
										readonly value: NativeCompaction;
									}>();
						return {
							first_target_journal_sequence: first_target_event.sequence,
							source_cut_journal_sequence,
							native_compaction,
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
										engine_id: source.engine_id,
										last_native_turn_id: state?.last_native_turn_id,
										last_observation_sequence:
											state?.last_observation_sequence ?? 0,
										model_id: state?.model_id ?? source.model_id,
										native_thread_id: source.native_thread_id,
										resume_token: Option.filter(
											resume,
											(token) =>
												token.native_thread_id === source.native_thread_id,
										),
										run_id: source.run_id,
										status: source.status,
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

		const RecordObservationMetadata = (observation: EngineObservation) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [run] = yield* transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, observation.artisan_run_id))
							.limit(1);
						if (!run) return yield* Effect.fail(Fail("run_missing"));
						yield* EnsureLiveThread(transaction, run.thread_id);
						const [raw] = yield* transaction
							.select()
							.from(OrchestrationRawObservations)
							.where(
								and(
									eq(
										OrchestrationRawObservations.observation_id,
										observation.observation_id,
									),
									eq(
										OrchestrationRawObservations.run_id,
										observation.artisan_run_id,
									),
									eq(OrchestrationRawObservations.sequence, observation.sequence),
								),
							)
							.limit(1);
						if (!raw) return yield* Effect.fail(Fail("raw_observation_missing"));
						const sequence_owners = yield* transaction
							.select({
								observation_id: OrchestrationRawObservations.observation_id,
							})
							.from(OrchestrationRawObservations)
							.where(
								and(
									eq(
										OrchestrationRawObservations.run_id,
										observation.artisan_run_id,
									),
									eq(OrchestrationRawObservations.sequence, observation.sequence),
								),
							)
							.limit(2);
						if (
							sequence_owners.length !== 1 ||
							sequence_owners[0]?.observation_id !== observation.observation_id
						)
							return yield* Effect.fail(Fail("observation_sequence_conflict"));
						if (observation._tag === "compaction" && observation.summary !== undefined)
							return yield* Effect.fail(Fail("public_compaction_summary_rejected"));
						const [watermark] = yield* transaction
							.select({ sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.where(
								and(
									eq(JournalEvents.thread_id, run.thread_id),
									eq(JournalEvents.run_id, run.run_id),
								),
							)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const completed_turn =
							observation._tag === "turn_state" && observation.state === "completed"
								? observation.turn_id
								: undefined;
						const compaction =
							observation._tag === "compaction" && observation.state === "completed"
								? {
										native_compaction_boundary_journal_sequence:
											watermark?.sequence ?? 0,
										native_compaction_json: null,
										native_compaction_observation_id:
											observation.observation_id,
									}
								: {};
						const [existing_state] = yield* transaction
							.select()
							.from(ThreadRunContinuationState)
							.where(
								eq(ThreadRunContinuationState.run_id, observation.artisan_run_id),
							)
							.limit(1);
						if (
							existing_state !== undefined &&
							existing_state.last_observation_sequence > observation.sequence
						)
							return;
						if (
							existing_state !== undefined &&
							existing_state.last_observation_sequence === observation.sequence
						) {
							if (
								(observation._tag === "compaction" &&
									observation.state === "completed" &&
									existing_state.native_compaction_observation_id !==
										observation.observation_id) ||
								(completed_turn !== undefined &&
									existing_state.last_native_turn_id !== null &&
									existing_state.last_native_turn_id !== completed_turn)
							)
								return yield* Effect.fail(Fail("observation_metadata_conflict"));
							return;
						}
						yield* transaction
							.insert(ThreadRunContinuationState)
							.values({
								created_at: updated_at,
								engine_id: run.engine_id,
								last_native_turn_id: completed_turn ?? null,
								last_observation_sequence: observation.sequence,
								model_id: run.model_id,
								run_id: run.run_id,
								thread_id: run.thread_id,
								updated_at,
								...compaction,
							})
							.onConflictDoUpdate({
								target: ThreadRunContinuationState.run_id,
								set: {
									engine_id: run.engine_id,
									...(completed_turn === undefined
										? {}
										: { last_native_turn_id: completed_turn }),
									last_observation_sequence: sql`max(${ThreadRunContinuationState.last_observation_sequence}, ${observation.sequence})`,
									model_id: run.model_id,
									updated_at,
									...compaction,
								},
							});
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure
						? cause
						: Fail("observation_metadata_failed"),
				),
			);

		const RecordNativeCompaction = (run_id: string, value: unknown) =>
			Effect.gen(function* () {
				const compaction = yield* Schema.decodeUnknownEffect(NativeCompaction)(value).pipe(
					Effect.mapError(() => Fail("native_compaction_invalid")),
				);
				const digest = yield* crypto
					.digest(
						"SHA-256",
						new TextEncoder().encode(
							normalize_engine_compaction_summary(compaction.summary),
						),
					)
					.pipe(Effect.map(Encoding.encodeHex));
				if (digest !== compaction.summary_sha256)
					return yield* Effect.fail(Fail("native_compaction_hash_mismatch"));
				const updated_at = yield* metadata.Now;
				yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [[run], [state], [raw]] = yield* Effect.all([
							transaction
								.select()
								.from(OrchestrationRuns)
								.where(eq(OrchestrationRuns.run_id, run_id))
								.limit(1),
							transaction
								.select()
								.from(ThreadRunContinuationState)
								.where(eq(ThreadRunContinuationState.run_id, run_id))
								.limit(1),
							transaction
								.select()
								.from(OrchestrationRawObservations)
								.where(
									and(
										eq(
											OrchestrationRawObservations.observation_id,
											compaction.observation_id,
										),
										eq(OrchestrationRawObservations.run_id, run_id),
									),
								)
								.limit(1),
						]);
						if (
							!run ||
							!state ||
							!raw ||
							run.engine_id !== "claude" ||
							raw.engine_id !== run.engine_id ||
							run.native_thread_id !== compaction.source_native_thread_id ||
							raw.native_id !== compaction.boundary_id ||
							raw.sequence > state.last_observation_sequence ||
							state.native_compaction_observation_id !== compaction.observation_id ||
							state.native_compaction_boundary_journal_sequence === null
						)
							return yield* Effect.fail(Fail("native_compaction_unverified"));
						yield* EnsureLiveThread(transaction, run.thread_id);
						const parsed_frame = ParsePersistedJson(raw.frame_json);
						const frame =
							parsed_frame === undefined
								? Option.none<{
										readonly compactMetadata: {
											readonly trigger: "manual" | "auto";
										};
										readonly subtype: "compact_boundary";
										readonly type: "system";
										readonly uuid: string;
									}>()
								: Schema.decodeUnknownOption(
										Schema.Struct({
											compactMetadata: Schema.Struct({
												trigger: Schema.Literals(["manual", "auto"]),
											}),
											subtype: Schema.Literal("compact_boundary"),
											type: Schema.Literal("system"),
											uuid: Schema.NonEmptyString,
										}),
									)(parsed_frame);
						if (
							Option.isNone(frame) ||
							frame.value.uuid !== compaction.boundary_id ||
							frame.value.compactMetadata.trigger !== compaction.trigger ||
							raw.native_method !== "system.compact_boundary"
						)
							return yield* Effect.fail(Fail("native_compaction_raw_mismatch"));
						const persisted = JSON.stringify(compaction);
						if (
							state.native_compaction_json !== null &&
							state.native_compaction_json !== persisted
						)
							return yield* Effect.fail(
								new ThreadContinuationConflict({ target_run_id: run_id }),
							);
						yield* transaction
							.update(ThreadRunContinuationState)
							.set({ native_compaction_json: persisted, updated_at })
							.where(eq(ThreadRunContinuationState.run_id, run_id));
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure ||
					cause instanceof ThreadContinuationConflict
						? cause
						: Fail("native_compaction_record_failed"),
				),
			);

		const PrepareLaunch = (target_run_id: string, launch: ContinuationLaunch) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				return yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [target] = yield* transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, target_run_id))
							.limit(1);
						if (!target) return yield* Effect.fail(Fail("target_run_missing"));
						yield* EnsureLiveThread(transaction, target.thread_id);
						const [[request], [outbox]] = yield* Effect.all([
							transaction
								.select()
								.from(OrchestrationMessages)
								.where(
									and(
										eq(OrchestrationMessages.run_id, target_run_id),
										eq(OrchestrationMessages.command_id, launch.request_id),
									),
								)
								.limit(1),
							transaction
								.select()
								.from(OrchestrationOutbox)
								.where(
									and(
										eq(OrchestrationOutbox.run_id, target_run_id),
										eq(OrchestrationOutbox.command_id, launch.request_id),
									),
								)
								.limit(1),
						]);
						if (
							target.status !== "queued" ||
							!request ||
							!outbox ||
							outbox.kind !== "start" ||
							outbox.status !== "dispatching"
						)
							return yield* Effect.fail(Fail("launch_request_invalid"));
						if (
							target.model_id !== null &&
							launch.target_model_id !== undefined &&
							target.model_id !== launch.target_model_id
						)
							return yield* Effect.fail(Fail("launch_target_model_mismatch"));
						const intended_target_model = launch.target_model_id ?? target.model_id;
						const cut = yield* ReadSourceCut(transaction, target);
						const source = cut.source;
						if (
							(launch._tag === "fresh" && source !== undefined) ||
							(launch._tag !== "fresh" &&
								(source === undefined || source.run_id !== launch.source_run_id))
						)
							return yield* Effect.fail(Fail("launch_source_mismatch"));
						if (
							source !== undefined &&
							["queued", "running", "waiting"].includes(source.status)
						)
							return yield* Effect.fail(Fail("launch_source_active"));
						const [source_state] =
							source === undefined
								? []
								: yield* transaction
										.select()
										.from(ThreadRunContinuationState)
										.where(eq(ThreadRunContinuationState.run_id, source.run_id))
										.limit(1);
						const source_resume =
							source === undefined
								? Option.none<EngineResumeToken>()
								: DecodeResumeToken(
										ParsePersistedJson(source.native_resume_json),
									).pipe(
										Option.filter(
											(token) =>
												token.native_thread_id === source.native_thread_id,
										),
									);
						if (
							launch._tag === "native" &&
							(source?.status !== "completed" ||
								source.engine_id !== target.engine_id ||
								Option.isNone(source_resume))
						)
							return yield* Effect.fail(Fail("native_launch_invalid"));

						let decoded_checkpoint: PortableCheckpointValue | undefined;
						let decoded_lineage: typeof PortableHandoffLineage.Type | undefined;
						if (launch._tag === "portable") {
							const checkpoint = yield* Schema.decodeUnknownEffect(
								PortableCheckpoint,
							)(launch.checkpoint).pipe(
								Effect.mapError(() => Fail("portable_checkpoint_invalid")),
							);
							const lineage = yield* Schema.decodeUnknownEffect(
								PortableHandoffLineage,
							)(launch.lineage).pipe(
								Effect.mapError(() => Fail("portable_lineage_invalid")),
							);
							const source_model = source_state?.model_id ?? source?.model_id ?? null;
							if (
								source === undefined ||
								checkpoint.source.cut.thread_id !== target.thread_id ||
								checkpoint.source.cut.through_run_id !== source.run_id ||
								checkpoint.source.cut.through_journal_sequence !==
									cut.source_cut_journal_sequence ||
								checkpoint.source.cut.through_observation_sequence !==
									(source_state?.last_observation_sequence ?? 0) ||
								checkpoint.source.engine_id !== source.engine_id ||
								(checkpoint.source.model_id ?? null) !== source_model ||
								!lineage_matches_method(checkpoint.method, lineage)
							)
								return yield* Effect.fail(Fail("portable_checkpoint_cut_mismatch"));
							if (lineage.kind === "codex") {
								if (
									source.engine_id !== "codex" ||
									Option.isNone(source_resume) ||
									lineage.source_native_thread_id !==
										source_resume.value.native_thread_id ||
									lineage.source_native_turn_id !==
										source_state?.last_native_turn_id ||
									lineage.export_native_thread_id ===
										lineage.source_native_thread_id
								)
									return yield* Effect.fail(Fail("portable_lineage_mismatch"));
							} else if (lineage.kind === "claude") {
								if (source.native_thread_id === null)
									return yield* Effect.fail(Fail("portable_lineage_mismatch"));
								const run_starts = transaction
									.select({
										first_sequence:
											sql<number>`min(${JournalEvents.sequence})`.as(
												"first_sequence",
											),
										run_id: JournalEvents.run_id,
									})
									.from(JournalEvents)
									.where(eq(JournalEvents.thread_id, target.thread_id))
									.groupBy(JournalEvents.run_id)
									.as("launch_compaction_run_starts");
								const [[source_start], [selected_compaction], [compaction_state]] =
									yield* Effect.all([
										transaction
											.select({ first_sequence: run_starts.first_sequence })
											.from(run_starts)
											.where(eq(run_starts.run_id, source.run_id))
											.limit(1),
										transaction
											.select({
												boundary:
													ThreadRunContinuationState.native_compaction_boundary_journal_sequence,
												run_id: ThreadRunContinuationState.run_id,
											})
											.from(ThreadRunContinuationState)
											.innerJoin(
												OrchestrationRuns,
												eq(
													ThreadRunContinuationState.run_id,
													OrchestrationRuns.run_id,
												),
											)
											.innerJoin(
												run_starts,
												eq(run_starts.run_id, OrchestrationRuns.run_id),
											)
											.where(
												and(
													eq(
														OrchestrationRuns.thread_id,
														target.thread_id,
													),
													eq(OrchestrationRuns.agent_id, target.agent_id),
													eq(OrchestrationRuns.engine_id, "claude"),
													eq(
														OrchestrationRuns.native_thread_id,
														source.native_thread_id,
													),
													isNotNull(
														ThreadRunContinuationState.native_compaction_json,
													),
													isNotNull(
														ThreadRunContinuationState.native_compaction_boundary_journal_sequence,
													),
													sql`(${run_starts.first_sequence} < (SELECT min(${JournalEvents.sequence}) FROM ${JournalEvents} WHERE ${JournalEvents.run_id} = ${source.run_id}) OR (${run_starts.run_id} = ${source.run_id} AND ${ThreadRunContinuationState.native_compaction_boundary_journal_sequence} <= ${cut.source_cut_journal_sequence}))`,
												),
											)
											.orderBy(
												desc(run_starts.first_sequence),
												desc(
													ThreadRunContinuationState.native_compaction_boundary_journal_sequence,
												),
											)
											.limit(1),
										transaction
											.select()
											.from(ThreadRunContinuationState)
											.where(
												eq(
													ThreadRunContinuationState.run_id,
													lineage.through_run_id,
												),
											)
											.limit(1),
									]);
								const native = Schema.decodeUnknownOption(NativeCompaction)(
									ParsePersistedJson(compaction_state?.native_compaction_json),
								);
								const native_digest = Option.isNone(native)
									? undefined
									: yield* crypto
											.digest(
												"SHA-256",
												new TextEncoder().encode(
													normalize_engine_compaction_summary(
														native.value.summary,
													),
												),
											)
											.pipe(Effect.map(Encoding.encodeHex));
								if (
									source.engine_id !== "claude" ||
									source_start === undefined ||
									selected_compaction?.run_id !== lineage.through_run_id ||
									compaction_state?.native_compaction_boundary_journal_sequence !==
										selected_compaction?.boundary ||
									compaction_state?.native_compaction_observation_id !==
										lineage.observation_id ||
									Option.isNone(native) ||
									native_digest !== native.value.summary_sha256 ||
									checkpoint.summary !== native.value.summary ||
									native.value.boundary_id !== lineage.boundary_id ||
									native.value.observation_id !== lineage.observation_id ||
									native.value.source_native_thread_id !==
										lineage.source_native_thread_id ||
									lineage.source_native_thread_id !== source.native_thread_id ||
									(lineage.source_native_turn_id !== undefined &&
										lineage.source_native_turn_id !==
											source_state?.last_native_turn_id)
								)
									return yield* Effect.fail(Fail("portable_lineage_mismatch"));
							}
							const digest = yield* crypto
								.digest("SHA-256", encode_portable_checkpoint_content(checkpoint))
								.pipe(Effect.map(Encoding.encodeHex));
							if (digest !== checkpoint.sha256)
								return yield* Effect.fail(
									Fail("portable_checkpoint_hash_mismatch"),
								);
							decoded_checkpoint = checkpoint;
							decoded_lineage = lineage;
						}

						const [existing] = yield* transaction
							.select()
							.from(ThreadContinuationLaunches)
							.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
							.limit(1);
						const source_run_id = launch._tag === "fresh" ? null : launch.source_run_id;
						const handoff_id = launch._tag === "portable" ? launch.handoff_id : null;
						if (existing) {
							if (
								existing.request_id !== launch.request_id ||
								existing.source_kind !== launch._tag ||
								existing.source_run_id !== source_run_id ||
								existing.handoff_id !== handoff_id ||
								existing.target_engine_id !== target.engine_id ||
								existing.target_model_id !== intended_target_model
							)
								return yield* Effect.fail(
									new ThreadContinuationConflict({ target_run_id }),
								);
							if (
								launch._tag === "portable" &&
								decoded_checkpoint !== undefined &&
								decoded_lineage !== undefined
							) {
								const [handoff] = yield* transaction
									.select()
									.from(ThreadPortableHandoffs)
									.where(eq(ThreadPortableHandoffs.target_run_id, target_run_id))
									.limit(1);
								if (
									!handoff ||
									handoff.handoff_id !== launch.handoff_id ||
									handoff.content_sha256 !== decoded_checkpoint.sha256 ||
									handoff.created_at !== decoded_checkpoint.created_at ||
									handoff.method !== decoded_checkpoint.method ||
									handoff.omitted_entries !==
										decoded_checkpoint.omitted_entries ||
									handoff.source_engine_id !==
										decoded_checkpoint.source.engine_id ||
									handoff.source_model_id !==
										(decoded_checkpoint.source.model_id ?? null) ||
									handoff.through_journal_sequence !==
										decoded_checkpoint.source.cut.through_journal_sequence ||
									handoff.through_observation_sequence !==
										decoded_checkpoint.source.cut
											.through_observation_sequence ||
									handoff.summary !== decoded_checkpoint.summary ||
									handoff.tail_json !== JSON.stringify(decoded_checkpoint.tail) ||
									handoff.provider_lineage_json !==
										JSON.stringify(decoded_lineage)
								)
									return yield* Effect.fail(
										new ThreadContinuationConflict({ target_run_id }),
									);
							}
							return existing.state as ThreadContinuationLaunchState;
						}
						if (launch._tag === "portable") {
							const checkpoint = decoded_checkpoint!;
							const lineage = decoded_lineage!;
							yield* transaction.insert(ThreadPortableHandoffs).values({
								content_sha256: checkpoint.sha256,
								created_at: checkpoint.created_at,
								handoff_id: launch.handoff_id,
								method: checkpoint.method,
								omitted_entries: checkpoint.omitted_entries,
								source_engine_id: checkpoint.source.engine_id,
								source_model_id: checkpoint.source.model_id,
								source_run_id: checkpoint.source.cut.through_run_id,
								provider_lineage_json: JSON.stringify(lineage),
								summary: checkpoint.summary,
								tail_json: JSON.stringify(checkpoint.tail),
								target_run_id,
								thread_id: target.thread_id,
								through_journal_sequence:
									checkpoint.source.cut.through_journal_sequence,
								through_native_turn_id:
									lineage.kind === "codex"
										? lineage.source_native_turn_id
										: lineage.kind === "claude"
											? (lineage.source_native_turn_id ?? null)
											: null,
								through_observation_sequence:
									checkpoint.source.cut.through_observation_sequence,
							});
						}
						yield* transaction.insert(ThreadContinuationLaunches).values({
							created_at: updated_at,
							failure_code: null,
							handoff_id,
							request_id: launch.request_id,
							source_kind: launch._tag,
							source_run_id,
							state: "prepared",
							target_engine_id: target.engine_id,
							target_model_id: intended_target_model,
							target_run_id,
							thread_id: target.thread_id,
							updated_at,
						});
						return "prepared" as const;
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure ||
					cause instanceof ThreadContinuationConflict
						? cause
						: Fail("launch_prepare_failed"),
				),
			);

		const MarkOpening = (target_run_id: string) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [launch] = yield* transaction
							.select()
							.from(ThreadContinuationLaunches)
							.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
							.limit(1);
						if (!launch) return yield* Effect.fail(Fail("launch_missing"));
						yield* EnsureLiveThread(transaction, launch.thread_id);
						const [[run], [outbox]] = yield* Effect.all([
							transaction
								.select()
								.from(OrchestrationRuns)
								.where(eq(OrchestrationRuns.run_id, launch.target_run_id))
								.limit(1),
							transaction
								.select()
								.from(OrchestrationOutbox)
								.where(
									and(
										eq(OrchestrationOutbox.run_id, launch.target_run_id),
										eq(OrchestrationOutbox.command_id, launch.request_id),
									),
								)
								.limit(1),
						]);
						if (
							!run ||
							run.status !== "queued" ||
							!outbox ||
							outbox.kind !== "start" ||
							outbox.status !== "dispatching"
						)
							return yield* Effect.fail(Fail("launch_not_openable"));
						const changed = yield* transaction
							.update(ThreadContinuationLaunches)
							.set({ state: "opening", updated_at })
							.where(
								and(
									eq(ThreadContinuationLaunches.target_run_id, target_run_id),
									eq(ThreadContinuationLaunches.state, "prepared"),
								),
							)
							.returning();
						if (changed.length !== 1)
							return yield* Effect.fail(Fail("launch_not_prepared"));
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure
						? cause
						: Fail("launch_opening_failed"),
				),
			);

		const BindTarget = (input: {
			readonly command_id: string;
			readonly model_id?: string;
			readonly native_thread_id: string;
			readonly resume_token: EngineResumeToken;
			readonly target_run_id: string;
		}) =>
			Effect.gen(function* () {
				const resume_token = yield* Schema.decodeUnknownEffect(EngineResumeTokenSchema)(
					input.resume_token,
				).pipe(Effect.mapError(() => Fail("resume_token_invalid")));
				if (resume_token.native_thread_id !== input.native_thread_id)
					return yield* Effect.fail(Fail("resume_token_thread_mismatch"));
				const updated_at = yield* metadata.Now;
				yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [target] = yield* transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, input.target_run_id))
							.limit(1);
						if (!target) return yield* Effect.fail(Fail("target_run_missing"));
						const [[launch], [outbox], [coordinator]] = yield* Effect.all([
							transaction
								.select()
								.from(ThreadContinuationLaunches)
								.where(
									eq(
										ThreadContinuationLaunches.target_run_id,
										input.target_run_id,
									),
								)
								.limit(1),
							transaction
								.select()
								.from(OrchestrationOutbox)
								.where(
									and(
										eq(OrchestrationOutbox.run_id, input.target_run_id),
										eq(OrchestrationOutbox.command_id, input.command_id),
									),
								)
								.limit(1),
							transaction
								.select()
								.from(OrchestrationCoordinators)
								.where(eq(OrchestrationCoordinators.thread_id, target.thread_id))
								.limit(1),
						]);
						yield* EnsureLiveThread(transaction, target.thread_id);
						if (
							!launch ||
							!outbox ||
							!coordinator ||
							launch.request_id !== input.command_id ||
							launch.thread_id !== target.thread_id ||
							launch.target_engine_id !== target.engine_id ||
							launch.target_model_id !== (input.model_id ?? null) ||
							coordinator.thread_id !== target.thread_id ||
							(coordinator.active_run_id === target.run_id &&
								coordinator.engine_id !== target.engine_id)
						)
							return yield* Effect.fail(Fail("target_bind_intent_mismatch"));
						if (launch.state === "bound") {
							const persisted_resume = DecodeResumeToken(
								ParsePersistedJson(target.native_resume_json),
							);
							if (
								outbox.status !== "delivered" ||
								target.native_thread_id !== input.native_thread_id ||
								target.model_id !== (input.model_id ?? null) ||
								Option.isNone(persisted_resume) ||
								JSON.stringify(persisted_resume.value) !==
									JSON.stringify(resume_token) ||
								(coordinator.active_run_id === target.run_id &&
									(coordinator.native_thread_id !== input.native_thread_id ||
										coordinator.native_resume_json !==
											JSON.stringify(resume_token)))
							)
								return yield* Effect.fail(
									new ThreadContinuationConflict({
										target_run_id: input.target_run_id,
									}),
								);
							return;
						}
						if (launch.state !== "opening" || outbox.status !== "dispatching")
							return yield* Effect.fail(Fail("launch_not_opening"));
						const [changed_launch] = yield* transaction
							.update(ThreadContinuationLaunches)
							.set({ state: "bound", updated_at })
							.where(
								and(
									eq(
										ThreadContinuationLaunches.target_run_id,
										input.target_run_id,
									),
									eq(ThreadContinuationLaunches.state, "opening"),
								),
							)
							.returning();
						if (!changed_launch) return yield* Effect.fail(Fail("launch_not_opening"));
						const [run] = yield* transaction
							.update(OrchestrationRuns)
							.set({
								...(input.model_id === undefined
									? {}
									: { model_id: input.model_id }),
								native_resume_json: JSON.stringify(resume_token),
								native_thread_id: input.native_thread_id,
								updated_at,
							})
							.where(eq(OrchestrationRuns.run_id, input.target_run_id))
							.returning();
						if (!run) return yield* Effect.fail(Fail("target_bind_failed"));
						if (coordinator.active_run_id === target.run_id) {
							const [updated_coordinator] = yield* transaction
								.update(OrchestrationCoordinators)
								.set({
									native_resume_json: JSON.stringify(resume_token),
									native_thread_id: input.native_thread_id,
									updated_at,
								})
								.where(
									and(
										eq(OrchestrationCoordinators.thread_id, target.thread_id),
										eq(OrchestrationCoordinators.active_run_id, target.run_id),
									),
								)
								.returning();
							if (!updated_coordinator)
								return yield* Effect.fail(Fail("coordinator_bind_failed"));
						}
						yield* transaction
							.insert(ThreadRunContinuationState)
							.values({
								created_at: updated_at,
								engine_id: target.engine_id,
								last_observation_sequence: 0,
								model_id: input.model_id ?? target.model_id,
								run_id: input.target_run_id,
								thread_id: target.thread_id,
								updated_at,
							})
							.onConflictDoUpdate({
								target: ThreadRunContinuationState.run_id,
								set: {
									engine_id: target.engine_id,
									model_id: input.model_id ?? target.model_id,
									updated_at,
								},
							});
						const [updated_outbox] = yield* transaction
							.update(OrchestrationOutbox)
							.set({ status: "delivered", updated_at })
							.where(
								and(
									eq(OrchestrationOutbox.command_id, input.command_id),
									eq(OrchestrationOutbox.run_id, input.target_run_id),
									eq(OrchestrationOutbox.status, "dispatching"),
								),
							)
							.returning();
						if (!updated_outbox) return yield* Effect.fail(Fail("outbox_bind_failed"));
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure ||
					cause instanceof ThreadContinuationConflict
						? cause
						: Fail("target_bind_transaction_failed"),
				),
			);

		const FailLaunch = (target_run_id: string, failure_code: string) =>
			Effect.gen(function* () {
				const code = yield* Schema.decodeUnknownEffect(FailureCode)(failure_code).pipe(
					Effect.mapError(() => Fail("failure_code_invalid")),
				);
				const updated_at = yield* metadata.Now;
				yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [launch] = yield* transaction
							.select()
							.from(ThreadContinuationLaunches)
							.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
							.limit(1);
						if (!launch) return yield* Effect.fail(Fail("launch_missing"));
						yield* EnsureLiveThread(transaction, launch.thread_id);
						if (launch.state === "failed" && launch.failure_code === code) return;
						if (launch.state !== "prepared" && launch.state !== "opening")
							return yield* Effect.fail(Fail("launch_not_failable"));
						const changed = yield* transaction
							.update(ThreadContinuationLaunches)
							.set({ failure_code: code, state: "failed", updated_at })
							.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
							.returning();
						if (changed.length !== 1)
							return yield* Effect.fail(Fail("launch_not_failable"));
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure ? cause : Fail("launch_fail_failed"),
				),
			);

		const ReconcileStranded = () =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				return yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const launches = yield* transaction
							.select()
							.from(ThreadContinuationLaunches)
							.where(
								sql`${ThreadContinuationLaunches.state} IN ('prepared', 'opening')`,
							);
						const reconciled: Array<string> = [];
						for (const launch of launches) {
							const [[thread], [claim], [tombstone]] = yield* Effect.all([
								transaction
									.select()
									.from(Threads)
									.where(eq(Threads.thread_id, launch.thread_id))
									.limit(1),
								transaction
									.select()
									.from(ThreadErasureClaims)
									.where(eq(ThreadErasureClaims.thread_id, launch.thread_id))
									.limit(1),
								transaction
									.select()
									.from(ThreadTombstones)
									.where(eq(ThreadTombstones.thread_id, launch.thread_id))
									.limit(1),
							]);
							if (!thread || claim || tombstone) continue;
							/**
							 * Recovery invokes this only after proving that the process owns no
							 * live EngineRun. Any prepared/opening row is therefore ownerless
							 * even if its run and outbox still look active.
							 */
							yield* transaction
								.update(ThreadContinuationLaunches)
								.set({
									failure_code: "stranded_recovery",
									state: "failed",
									updated_at,
								})
								.where(
									eq(
										ThreadContinuationLaunches.target_run_id,
										launch.target_run_id,
									),
								);
							reconciled.push(launch.target_run_id);
						}
						return reconciled;
					}),
				);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ThreadContinuationFailure
						? cause
						: Fail("stranded_reconcile_failed"),
				),
			);

		return {
			BindTarget,
			FailLaunch,
			IsDispatchReady,
			MarkOpening,
			PrepareLaunch,
			ReadCanonicalHistory,
			ReadContext,
			RecordNativeCompaction,
			RecordObservationMetadata,
			ReconcileStranded,
		};
	}),
);
