import { Cause, Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import { OrchestrationRepository, type PendingWork } from "../persistence/orchestration/repository";
import { ThreadContinuationRepository } from "../persistence/thread-continuation/repository";

/**
 * Builds the durable write path for one run's observation stream. Batches
 * commit in one transaction pair; a failed batch degrades to per-observation
 * writes so one poison observation cannot discard its siblings.
 */
export const MakeObservationPersistence = (input: {
	readonly continuation_repository: typeof ThreadContinuationRepository.Service;
	readonly repository: typeof OrchestrationRepository.Service;
}) => {
	const PersistIndividually = (
		work: Pick<PendingWork, "run_id">,
		batch: ReadonlyArray<EngineObservation>,
	) =>
		Effect.forEach(batch, (observation) =>
			input.repository.RecordObservation(observation).pipe(
				Effect.andThen(
					input.continuation_repository.RecordObservationMetadata(observation),
				),
				Effect.asVoid,
				Effect.catchCause((cause) =>
					Effect.sync(() => {
						console.error("Artisan continuation observation metadata failed", {
							failure_kind: Cause.hasInterruptsOnly(cause)
								? "interrupted"
								: "persistence",
							run_id: work.run_id,
						});
					}),
				),
			),
		);

	const PersistBatch = (
		work: Pick<PendingWork, "run_id">,
		batch: ReadonlyArray<EngineObservation>,
	) =>
		input.repository.RecordObservations(batch).pipe(
			Effect.andThen(input.continuation_repository.RecordObservationsMetadata(batch)),
			Effect.asVoid,
			Effect.catchCause((cause) =>
				Effect.gen(function* () {
					const interrupted = Cause.hasInterruptsOnly(cause);

					yield* Effect.sync(() => {
						console.error("Artisan observation batch persistence failed", {
							batch_size: batch.length,
							failure_kind: interrupted ? "interrupted" : "persistence",
							run_id: work.run_id,
						});
					});

					if (interrupted) return;

					yield* PersistIndividually(work, batch);
				}),
			),
		);

	return { PersistBatch };
};
