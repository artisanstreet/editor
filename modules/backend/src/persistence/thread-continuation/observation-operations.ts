import { eq, sql } from "drizzle-orm";
import { Effect } from "effect";
import type { EngineObservation } from "@artisan/engines";
import { Database } from "../database";
import { OrchestrationRuns, ThreadErasureClaims, ThreadTombstones, Threads } from "../tables";
import { ThreadRunContinuationState } from "../thread-continuation-schema";
import { RuntimeMetadata } from "../../runtime/metadata";
import { ThreadContinuationFailure } from "./contracts";

export const MakeObservationOperations = Effect.gen(function* () {
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

	const ApplyObservationMetadata = (
		transaction: typeof database.client,
		observation: EngineObservation,
		run: {
			readonly engine_id: string;
			readonly model_id: string | null;
			readonly run_id: string;
			readonly thread_id: string;
		},
		updated_at: string,
	) =>
		Effect.gen(function* () {
			if (observation._tag === "compaction" && observation.summary !== undefined)
				return yield* Effect.fail(Fail("public_compaction_summary_rejected"));
			const completed_turn =
				observation._tag === "turn_state" && observation.state === "completed"
					? observation.turn_id
					: undefined;
			const [existing_state] = yield* transaction
				.select()
				.from(ThreadRunContinuationState)
				.where(eq(ThreadRunContinuationState.run_id, observation.artisan_run_id))
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
					completed_turn !== undefined &&
					existing_state.last_native_turn_id !== null &&
					existing_state.last_native_turn_id !== completed_turn
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
					},
				});
		});

	/**
	 * Records continuation metadata for a run-ordered batch in one transaction.
	 * Run and thread liveness are re-verified only when the run changes inside
	 * the batch, so a streaming burst pays those guards once instead of per
	 * observation.
	 */
	const RecordObservationsMetadata = (observations: ReadonlyArray<EngineObservation>) =>
		Effect.gen(function* () {
			if (observations.length === 0) return;
			const updated_at = yield* metadata.Now;
			yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					let verified_run:
						| {
								readonly engine_id: string;
								readonly model_id: string | null;
								readonly run_id: string;
								readonly thread_id: string;
						  }
						| undefined;
					for (const observation of observations) {
						if (verified_run?.run_id !== observation.artisan_run_id) {
							const [run] = yield* transaction
								.select()
								.from(OrchestrationRuns)
								.where(eq(OrchestrationRuns.run_id, observation.artisan_run_id))
								.limit(1);
							if (!run) return yield* Effect.fail(Fail("run_missing"));
							yield* EnsureLiveThread(transaction, run.thread_id);
							verified_run = run;
						}
						yield* ApplyObservationMetadata(
							transaction,
							observation,
							verified_run,
							updated_at,
						);
					}
				}),
			);
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof ThreadContinuationFailure
					? cause
					: Fail("observation_metadata_failed"),
			),
		);

	const RecordObservationMetadata = (observation: EngineObservation) =>
		RecordObservationsMetadata([observation]);

	return { RecordObservationMetadata, RecordObservationsMetadata };
});
