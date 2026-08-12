import { and, eq, inArray, lt, sql } from "drizzle-orm";
import { Effect } from "effect";

import { type EventEnvelope, type EventPayload } from "@artisan/protocol";
import type { EngineObservation } from "@artisan/engines";

import type { DatabaseClient } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import {
	OrchestrationCommandConflict,
	OrchestrationFailure,
	OrchestrationNotFound,
	OrchestrationProjectAuthorityError,
	type OrchestrationError,
} from "./contracts";
import {
	NativeSubagentObservationInbox,
	NativeSubagentTranscriptInbox,
	ConversationSources,
	OrchestrationInteractions,
	OrchestrationRuns,
} from "../tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import { ApplyEngineObservation } from "../../conversation/index.ts";
import { PersistSurfaceProjection } from "../../surfaces/surface-projection";
import { ReconcileRootThreadLiveStatus } from "./thread-lifecycle-status";

interface JournalNotifier {
	readonly Publish: (journal_sequence: number) => Effect.Effect<void>;
}

function normalize_error(error: unknown): OrchestrationError {
	if (
		error instanceof OrchestrationCommandConflict ||
		error instanceof OrchestrationNotFound ||
		error instanceof OrchestrationProjectAuthorityError
	) {
		return error;
	}

	return new OrchestrationFailure({ cause: error });
}

function is_projectable_status(status: string): status is "queued" | "running" | "waiting" {
	return status === "queued" || status === "running" || status === "waiting";
}

const question_answers = (
	observation: Extract<EngineObservation, { readonly _tag: "question" }>,
) => {
	const first_answer = observation.answers?.at(0);
	return first_answer === undefined
		? {}
		: {
				answers: {
					[observation.question_id]: [
						first_answer,
						...(observation.answers?.slice(1) ?? []),
					] satisfies readonly [string, ...string[]],
				},
			};
};

/**
 * Builds the durable observation write path for the orchestration repository.
 * A batch of run-ordered observations commits in one transaction; streaming
 * engines emit deltas faster than one commit per observation can absorb, so
 * batching keeps the durable record identical while the write path keeps
 * pace with the notification stream.
 */
export function make_observation_recording(
	client: DatabaseClient,
	metadata: typeof RuntimeMetadata.Service,
	notifier: JournalNotifier,
) {
	const AppendEvent = (
		transaction: DatabaseClient,
		input: Parameters<typeof AppendJournalEventInTransaction>[2],
	) => AppendJournalEventInTransaction(transaction, metadata, input);

	const PersistRecordedObservation = (
		transaction: DatabaseClient,
		observation: EngineObservation,
	) =>
		Effect.gen(function* () {
			if (observation._tag === "compaction" && observation.summary !== undefined) {
				return [];
			}

			const [run] = yield* transaction
				.select()
				.from(OrchestrationRuns)
				.where(eq(OrchestrationRuns.run_id, observation.artisan_run_id))
				.limit(1);
			if (!run || !is_projectable_status(run.status)) {
				return [];
			}

			/**
			 * The run watermark is the durable idempotency receipt. Its initial value is
			 * negative so a legitimate sequence-zero observation is admitted exactly once.
			 */
			const advanced = yield* transaction
				.update(OrchestrationRuns)
				.set({
					last_observation_sequence: sql`max(${OrchestrationRuns.last_observation_sequence}, ${observation.sequence})`,
				})
				.where(
					and(
						eq(OrchestrationRuns.run_id, observation.artisan_run_id),
						lt(OrchestrationRuns.last_observation_sequence, observation.sequence),
					),
				)
				.returning({ run_id: OrchestrationRuns.run_id });
			if (advanced.length === 0) return [];

			if (observation._tag === "subagent") {
				yield* transaction.insert(NativeSubagentObservationInbox).values({
					activity: observation.activity ?? null,
					agent_native_thread_id: observation.agent_native_thread_id,
					agent_path: observation.agent_path ?? null,
					created_at: yield* metadata.Now,
					engine_id: observation.raw.engine_id,
					native_id:
						observation.raw.native_id === undefined
							? null
							: String(observation.raw.native_id),
					observation_id: observation.observation_id,
					parent_native_thread_id: observation.parent_native_thread_id,
					processed_at: null,
					root_run_id: observation.artisan_run_id,
					sequence: observation.sequence,
					state: observation.state,
					turn_id: observation.turn_id ?? null,
				});
			}
			if (observation._tag === "subagent_transcript") {
				yield* transaction.insert(NativeSubagentTranscriptInbox).values({
					agent_native_thread_id: observation.agent_native_thread_id,
					content_json: JSON.stringify(observation.content),
					created_at: yield* metadata.Now,
					engine_id: observation.raw.engine_id,
					observation_id: observation.observation_id,
					parent_native_thread_id: observation.parent_native_thread_id,
					processed_at: null,
					root_run_id: observation.artisan_run_id,
					sequence: observation.sequence,
				});
				/** Child content is replayed only with its resolved child run context. */
				return [];
			}

			const projected_at = yield* metadata.Now;
			yield* PersistSurfaceProjection(transaction, observation, {
				agent_id: run.agent_id,
				occurred_at: projected_at,
				run_id: run.run_id,
				thread_id: run.thread_id,
			});
			yield* ApplyEngineObservation(transaction, observation, {
				agent_id: run.agent_id,
				occurred_at: projected_at,
				run_id: run.run_id,
				thread_id: run.thread_id,
			}) as Effect.Effect<unknown, unknown, never>;
			yield* transaction
				.delete(ConversationSources)
				.where(
					eq(ConversationSources.source_id, `observation:${observation.observation_id}`),
				);

			const payload =
				observation._tag === "agent_message_completed"
					? ({
							message_id: observation.observation_id,
							text: observation.message,
							type: "assistant.message_completed",
						} satisfies EventPayload)
					: observation._tag === "approval"
						? ({
								approval_id: observation.approval_id,
								...(observation.approved === undefined
									? {}
									: { approved: observation.approved }),
								description: observation.description,
								state: observation.state,
								type: "interaction.approval",
							} satisfies EventPayload)
						: observation._tag === "question"
							? ({
									...question_answers(observation),
									question_id: observation.question_id,
									state: observation.state,
									text: observation.text,
									type: "interaction.question",
								} satisfies EventPayload)
							: observation._tag === "run_state"
								? ({
										state:
											observation.state === "waiting" ? "waiting" : "running",
										type: "run.lifecycle",
										working_directory: run.working_directory,
									} satisfies EventPayload)
								: observation._tag === "run_terminal"
									? ({
											state: observation.state,
											type: "run.lifecycle",
											working_directory: run.working_directory,
										} satisfies EventPayload)
									: undefined;

			if (!payload) {
				return [];
			}

			const status = payload.type === "run.lifecycle" ? payload.state : undefined;

			if (status) {
				const updated_at = yield* metadata.Now;

				yield* transaction
					.update(OrchestrationRuns)
					.set({ status, updated_at })
					.where(
						and(
							eq(OrchestrationRuns.run_id, run.run_id),
							inArray(OrchestrationRuns.status, ["queued", "running", "waiting"]),
						),
					);
				yield* ReconcileRootThreadLiveStatus(transaction, run.thread_id, updated_at);
			}

			if (observation._tag === "run_terminal") {
				const updated_at = yield* metadata.Now;

				/**
				 * A terminal run can never answer its pending interactions.
				 * Releasing them lets a late approval response fail command
				 * dispatch as stale instead of queueing an outbox delivery
				 * that nothing will ever pick up.
				 */
				yield* transaction
					.update(OrchestrationInteractions)
					.set({ state: "cancelled", updated_at })
					.where(
						and(
							eq(OrchestrationInteractions.run_id, run.run_id),
							eq(OrchestrationInteractions.state, "requested"),
						),
					);
			}

			if (observation._tag === "approval" || observation._tag === "question") {
				const interaction_id =
					observation._tag === "approval"
						? observation.approval_id
						: observation.question_id;
				const description =
					observation._tag === "approval" ? observation.description : observation.text;
				const state = observation.state;
				const updated_at = yield* metadata.Now;

				if (state === "requested") {
					const [existing_interaction] = yield* transaction
						.select({
							interaction_id: OrchestrationInteractions.interaction_id,
						})
						.from(OrchestrationInteractions)
						.where(
							and(
								eq(OrchestrationInteractions.interaction_id, interaction_id),
								eq(OrchestrationInteractions.kind, observation._tag),
								eq(OrchestrationInteractions.run_id, run.run_id),
							),
						)
						.limit(1);

					if (!existing_interaction) {
						yield* transaction.insert(OrchestrationInteractions).values({
							created_at: updated_at,
							description,
							interaction_id,
							kind: observation._tag,
							run_id: run.run_id,
							state,
							updated_at,
						});
					}
				} else {
					const resolved = yield* transaction
						.update(OrchestrationInteractions)
						.set({ state, updated_at })
						.where(
							and(
								eq(OrchestrationInteractions.interaction_id, interaction_id),
								eq(OrchestrationInteractions.kind, observation._tag),
								eq(OrchestrationInteractions.run_id, run.run_id),
								eq(OrchestrationInteractions.state, "requested"),
							),
						)
						.returning({
							interaction_id: OrchestrationInteractions.interaction_id,
						});

					if (resolved.length === 0) {
						return [];
					}
				}
			}

			return [
				yield* AppendEvent(transaction, {
					agent_id: run.agent_id,
					causation_id: observation.observation_id,
					correlation_id: run.run_id,
					payload,
					raw_origin: {
						provider: observation.raw.engine_id,
						reference: String(observation.raw.native_id ?? observation.observation_id),
					},
					run_id: run.run_id,
					thread_id: run.thread_id,
				}),
			];
		});

	const RecordObservations = (observations: ReadonlyArray<EngineObservation>) =>
		Effect.gen(function* () {
			if (observations.length === 0) {
				return [] as ReadonlyArray<EventEnvelope>;
			}

			const result = yield* client.transaction((transaction) =>
				Effect.gen(function* () {
					const events: Array<EventEnvelope> = [];

					for (const observation of observations) {
						events.push(
							...(yield* PersistRecordedObservation(transaction, observation)),
						);
					}

					return events;
				}),
			);

			/** Delta-only batches commit no journal event; nothing woke up. */
			const latest_event = result.at(-1);
			if (latest_event !== undefined) yield* notifier.Publish(latest_event.journal_sequence);

			return result;
		}).pipe(Effect.mapError(normalize_error));

	const RecordObservation = (observation: EngineObservation) => RecordObservations([observation]);

	return { RecordObservation, RecordObservations };
}
