import { and, desc, eq, lt, sql } from "drizzle-orm";
import { Crypto, Effect, Encoding, Option, Schema } from "effect";
import type { EngineResumeToken } from "@artisan/engines";
import { Database } from "../database";
import {
	JournalEvents,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
} from "../tables";
import {
	ThreadContinuationLaunches,
	ThreadPortableHandoffs,
	ThreadRunContinuationState,
} from "../thread-continuation-schema";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	encode_portable_checkpoint_content,
	PortableCheckpoint,
	type PortableCheckpoint as PortableCheckpointValue,
} from "../../orchestration/thread-continuation-model";
import { DecodePersistedJson, DecodeResumeToken } from "./codec";
import {
	type ContinuationLaunch,
	lineage_matches_method,
	PortableHandoffLineage,
	ThreadContinuationConflict,
	ThreadContinuationFailure,
	ThreadContinuationLaunchState,
} from "./contracts";

export const MakePreparationOperations = Effect.gen(function* () {
	const crypto = yield* Crypto.Crypto;
	const database = yield* Database;
	const metadata = yield* RuntimeMetadata;
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
									DecodePersistedJson(source.native_resume_json).pipe(
										Option.getOrUndefined,
									),
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
						const checkpoint = yield* Schema.decodeUnknownEffect(PortableCheckpoint)(
							launch.checkpoint,
						).pipe(Effect.mapError(() => Fail("portable_checkpoint_invalid")));
						const lineage = yield* Schema.decodeUnknownEffect(PortableHandoffLineage)(
							launch.lineage,
						).pipe(Effect.mapError(() => Fail("portable_lineage_invalid")));
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
						const digest = yield* crypto
							.digest("SHA-256", encode_portable_checkpoint_content(checkpoint))
							.pipe(Effect.map(Encoding.encodeHex));
						if (digest !== checkpoint.sha256)
							return yield* Effect.fail(Fail("portable_checkpoint_hash_mismatch"));
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
								handoff.omitted_entries !== decoded_checkpoint.omitted_entries ||
								handoff.source_engine_id !== decoded_checkpoint.source.engine_id ||
								handoff.source_model_id !==
									(decoded_checkpoint.source.model_id ?? null) ||
								handoff.through_journal_sequence !==
									decoded_checkpoint.source.cut.through_journal_sequence ||
								handoff.through_observation_sequence !==
									decoded_checkpoint.source.cut.through_observation_sequence ||
								handoff.summary !== decoded_checkpoint.summary ||
								handoff.tail_json !== JSON.stringify(decoded_checkpoint.tail) ||
								handoff.provider_lineage_json !== JSON.stringify(decoded_lineage)
							)
								return yield* Effect.fail(
									new ThreadContinuationConflict({ target_run_id }),
								);
						}
						return yield* Schema.decodeUnknownEffect(ThreadContinuationLaunchState)(
							existing.state,
						).pipe(Effect.mapError(() => Fail("launch_state_invalid")));
					}
					if (launch._tag === "portable") {
						if (decoded_checkpoint === undefined || decoded_lineage === undefined)
							return yield* Effect.fail(Fail("portable_checkpoint_invalid"));
						const checkpoint = decoded_checkpoint;
						const lineage = decoded_lineage;
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
							through_native_turn_id: null,
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

	return { PrepareLaunch };
});
