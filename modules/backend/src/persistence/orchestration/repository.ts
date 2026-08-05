import { and, desc, eq, inArray, or } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import {
	EventPayload,
	ThreadSessionPolicy,
	type CommandEnvelope,
	type EventEnvelope,
	type ThreadMessageRoutedEvent,
	type ThreadSessionSnapshot,
	type ThreadWorkItem,
} from "@artisan/protocol";
import type { EngineObservation } from "@artisan/engines";

import { Database } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import { make_outbox_operations } from "./outbox";
import {
	OrchestrationCommandConflict,
	OrchestrationFailure,
	OrchestrationNotFound,
	OrchestrationProjectAuthorityError,
	OrchestrationRepository,
	type OrchestrationError,
	type RecoverableNativeRun,
	type WorkStatus,
} from "./contracts";
import {
	DecodePersistedJson,
	PersistedAssumptions,
	PersistedEventPayload,
	PersistedResumeToken,
} from "./storage-codec";
import { JournalNotifier } from "../journal-notifier";
import {
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationInteractions,
	OrchestrationIntake,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
} from "../tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import { ApplyEngineObservation } from "../../conversation/index.ts";
import type { IntakeAssessment } from "../../orchestration/intake-policy";
import { PersistSurfaceProjection } from "../../surfaces/surface-projection";
import { MakeCommandAcceptor } from "./acceptance";

export {
	OrchestrationCommandConflict,
	OrchestrationFailure,
	OrchestrationNotFound,
	OrchestrationRepository,
	type AcceptedOrchestrationCommand,
	type OrchestrationError,
	type PendingWork,
	type RecoverableNativeRun,
} from "./contracts";

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

function is_active_status(status: string): status is "running" | "waiting" {
	return status === "running" || status === "waiting";
}

function is_projectable_status(status: string): status is "queued" | "running" | "waiting" {
	return status === "queued" || is_active_status(status);
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

export const OrchestrationRepositoryLive = Layer.effect(
	OrchestrationRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const AppendEvent = (
			transaction: typeof database.client,
			input: Parameters<typeof AppendJournalEventInTransaction>[2],
		) => AppendJournalEventInTransaction(transaction, metadata, input);

		const GetWork = (thread_id: string) =>
			database.client
				.select({
					agent_id: OrchestrationCoordinators.agent_id,
					display_name: OrchestrationCoordinators.display_name,
					engine_id: OrchestrationCoordinators.engine_id,
					native_thread_id: OrchestrationRuns.native_thread_id,
					role: OrchestrationCoordinators.role,
					run_id: OrchestrationRuns.run_id,
					status: OrchestrationRuns.status,
				})
				.from(OrchestrationCoordinators)
				.innerJoin(
					OrchestrationRuns,
					eq(OrchestrationCoordinators.active_run_id, OrchestrationRuns.run_id),
				)
				.where(eq(OrchestrationCoordinators.thread_id, thread_id))
				.limit(1)
				.pipe(
					Effect.map(([work]) => {
						if (!work) {
							return undefined;
						}

						const { native_thread_id, ...without_native_thread } = work;

						return {
							...without_native_thread,
							...(native_thread_id ? { native_thread_id } : {}),
							status: work.status as WorkStatus,
						} satisfies ThreadWorkItem;
					}),
					Effect.mapError(normalize_error),
				);
		const GetAutoSteer = (thread_id: string) =>
			database.client
				.select({ enabled: OrchestrationCoordinators.auto_steer_follow_ups })
				.from(OrchestrationCoordinators)
				.where(eq(OrchestrationCoordinators.thread_id, thread_id))
				.limit(1)
				.pipe(
					Effect.map(([row]) => row?.enabled ?? true),
					Effect.mapError(normalize_error),
				);
		const DefaultSessionPolicy = {
			engine_id: "codex",
			reasoning_effort: "medium",
			permission: "supervised",
			permission_mode: "on_request",
			sandbox_mode: "workspace_write",
			service_tier: "standard",
			web_search_enabled: false,
			strict_clarification: false,
		} as const satisfies ThreadSessionPolicy;
		const DecodeSessionPolicy = (
			row:
				| {
						readonly engine_id: string;
						readonly policy_model: string | null;
						readonly policy_context_window: string | null;
						readonly policy_reasoning_effort: string;
						readonly policy_permission: string;
						readonly policy_permission_mode: string;
						readonly policy_sandbox_mode: string;
						readonly policy_service_tier: string;
						readonly policy_web_search_enabled: boolean;
						readonly policy_strict_clarification: boolean;
				  }
				| undefined,
		) =>
			row
				? Schema.decodeUnknownEffect(ThreadSessionPolicy, {
						onExcessProperty: "error",
					})({
						engine_id: row.engine_id,
						...(row.policy_model === null ? {} : { model: row.policy_model }),
						...(row.policy_context_window === null
							? {}
							: { context_window: row.policy_context_window }),
						reasoning_effort: row.policy_reasoning_effort,
						permission: row.policy_permission,
						permission_mode: row.policy_permission_mode,
						sandbox_mode: row.policy_sandbox_mode,
						service_tier: row.policy_service_tier,
						web_search_enabled: row.policy_web_search_enabled,
						strict_clarification: row.policy_strict_clarification,
					}).pipe(Effect.mapError((cause) => new OrchestrationFailure({ cause })))
				: Effect.succeed(DefaultSessionPolicy);
		const GetSessionPolicy = (thread_id: string) =>
			database.client
				.select({
					engine_id: OrchestrationCoordinators.engine_id,
					policy_model: OrchestrationCoordinators.policy_model,
					policy_context_window: OrchestrationCoordinators.policy_context_window,
					policy_reasoning_effort: OrchestrationCoordinators.policy_reasoning_effort,
					policy_permission: OrchestrationCoordinators.policy_permission,
					policy_permission_mode: OrchestrationCoordinators.policy_permission_mode,
					policy_sandbox_mode: OrchestrationCoordinators.policy_sandbox_mode,
					policy_service_tier: OrchestrationCoordinators.policy_service_tier,
					policy_web_search_enabled: OrchestrationCoordinators.policy_web_search_enabled,
					policy_strict_clarification:
						OrchestrationCoordinators.policy_strict_clarification,
				})
				.from(OrchestrationCoordinators)
				.where(eq(OrchestrationCoordinators.thread_id, thread_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) => DecodeSessionPolicy(row)),
					Effect.mapError(normalize_error),
				);

		const GetSession = (thread_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [coordinator] = yield* transaction
							.select({
								enabled: OrchestrationCoordinators.auto_steer_follow_ups,
								engine_id: OrchestrationCoordinators.engine_id,
								policy_model: OrchestrationCoordinators.policy_model,
								policy_context_window:
									OrchestrationCoordinators.policy_context_window,
								policy_reasoning_effort:
									OrchestrationCoordinators.policy_reasoning_effort,
								policy_permission: OrchestrationCoordinators.policy_permission,
								policy_permission_mode:
									OrchestrationCoordinators.policy_permission_mode,
								policy_sandbox_mode: OrchestrationCoordinators.policy_sandbox_mode,
								policy_service_tier: OrchestrationCoordinators.policy_service_tier,
								policy_web_search_enabled:
									OrchestrationCoordinators.policy_web_search_enabled,
								policy_strict_clarification:
									OrchestrationCoordinators.policy_strict_clarification,
							})
							.from(OrchestrationCoordinators)
							.where(eq(OrchestrationCoordinators.thread_id, thread_id))
							.limit(1);
						const [intake] = yield* transaction
							.select()
							.from(OrchestrationIntake)
							.where(eq(OrchestrationIntake.thread_id, thread_id))
							.orderBy(
								desc(OrchestrationIntake.updated_at),
								desc(OrchestrationIntake.message_id),
							)
							.limit(1);
						const [routing_row] = yield* transaction
							.select({ payload_json: JournalEvents.payload_json })
							.from(JournalEvents)
							.where(
								and(
									eq(JournalEvents.thread_id, thread_id),
									eq(JournalEvents.event_type, "thread.message_routed"),
								),
							)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const [watermark] = yield* transaction
							.select({ journal_sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const decoded_routing = routing_row
							? yield* DecodePersistedJson(
									PersistedEventPayload,
									routing_row.payload_json,
								)
							: undefined;
						const last_routing =
							decoded_routing?.type === "thread.message_routed"
								? decoded_routing
								: undefined;
						const assumptions = intake
							? (yield* DecodePersistedJson(
									PersistedAssumptions,
									intake.assumptions_json,
								)).map((assumption) => ({
									assumption,
									message_id: intake.message_id,
								}))
							: [];
						return {
							thread_id,
							journal_sequence: watermark?.journal_sequence ?? 0,
							auto_steer_enabled: coordinator?.enabled ?? true,
							policy: yield* DecodeSessionPolicy(coordinator),
							...(intake
								? {
										latest_intake: {
											message_id: intake.message_id,
											risk: intake.risk as NonNullable<
												ThreadSessionSnapshot["latest_intake"]
											>["risk"],
											resolution: intake.question_id
												? ("question" as const)
												: ("proceed" as const),
										},
									}
								: {}),
							assumptions,
							...(intake?.question_id && intake.question
								? {
										pending_question: {
											question_id: intake.question_id,
											state: intake.state as "pending" | "resolved",
											text: intake.question,
										},
									}
								: {}),
							...(last_routing ? { last_routing } : {}),
						} satisfies ThreadSessionSnapshot;
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		const AcceptCommand = yield* MakeCommandAcceptor;
		const { ClaimOutbox, CompleteOutbox, GetPending, MarkOutboxUndeliverable } =
			make_outbox_operations(database.client, metadata, notifier);

		const PersistNativeRun = (
			run_id: string,
			native_thread_id: string,
			resume_token: unknown,
			model_id?: string,
		) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				const [run] = yield* database.client
					.select()
					.from(OrchestrationRuns)
					.where(eq(OrchestrationRuns.run_id, run_id))
					.limit(1);

				if (!run) {
					return yield* new OrchestrationNotFound({ id: run_id, resource: "run" });
				}

				yield* database.client
					.update(OrchestrationRuns)
					.set({
						...(model_id === undefined ? {} : { model_id }),
						native_resume_json: JSON.stringify(resume_token),
						native_thread_id,
						updated_at,
					})
					.where(eq(OrchestrationRuns.run_id, run_id));
				yield* database.client
					.update(OrchestrationCoordinators)
					.set({
						native_resume_json: JSON.stringify(resume_token),
						native_thread_id,
						updated_at,
					})
					.where(eq(OrchestrationCoordinators.thread_id, run.thread_id));
			}).pipe(Effect.mapError(normalize_error));

		const MarkRunStarted = (run_id: string) =>
			Effect.gen(function* () {
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const updated_at = yield* metadata.Now;
						const [run] = yield* transaction
							.update(OrchestrationRuns)
							.set({ status: "running", updated_at })
							.where(
								and(
									eq(OrchestrationRuns.run_id, run_id),
									eq(OrchestrationRuns.status, "queued"),
								),
							)
							.returning();

						if (!run) {
							return [];
						}

						return [
							yield* AppendEvent(transaction, {
								agent_id: run.agent_id,
								causation_id: run_id,
								correlation_id: run_id,
								payload: {
									state: "running",
									type: "run.lifecycle",
									working_directory: run.working_directory,
								},
								run_id,
								thread_id: run.thread_id,
							}),
						];
					}),
				);

				const latest_event = result.at(-1);
				if (latest_event !== undefined)
					yield* notifier.Publish(latest_event.journal_sequence);

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const MarkRunResumed = (run_id: string) =>
			Effect.gen(function* () {
				const events = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const updated_at = yield* metadata.Now;
						const [run] = yield* transaction
							.update(OrchestrationRuns)
							.set({ status: "running", updated_at })
							.where(
								and(
									eq(OrchestrationRuns.run_id, run_id),
									eq(OrchestrationRuns.status, "interrupted"),
								),
							)
							.returning();

						if (!run) {
							return [];
						}

						return [
							yield* AppendEvent(transaction, {
								agent_id: run.agent_id,
								causation_id: `resume:${run_id}`,
								correlation_id: run_id,
								payload: {
									state: "running",
									type: "run.lifecycle",
									working_directory: run.working_directory,
								},
								run_id,
								thread_id: run.thread_id,
							}),
						];
					}),
				);

				const latest_event = events.at(-1);
				if (latest_event !== undefined)
					yield* notifier.Publish(latest_event.journal_sequence);

				return events.length === 1;
			}).pipe(Effect.mapError(normalize_error));

		const FallbackSteering = (
			command_id: string,
			reason: "delivery_failed" | "rejected" = "rejected",
		) =>
			Effect.gen(function* () {
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [message] = yield* transaction
							.select()
							.from(OrchestrationMessages)
							.where(eq(OrchestrationMessages.command_id, command_id))
							.limit(1);

						if (
							!message ||
							message.delivery !== "steering" ||
							message.run_id === null
						) {
							return [];
						}

						const [coordinator] = yield* transaction
							.select()
							.from(OrchestrationCoordinators)
							.where(eq(OrchestrationCoordinators.thread_id, message.thread_id))
							.limit(1);
						const [prior_run] = yield* transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, message.run_id))
							.limit(1);

						if (!coordinator || !prior_run) {
							return yield* new OrchestrationNotFound({
								id: message.thread_id,
								resource: "run",
							});
						}

						const run_id = yield* metadata.MakeId("run");
						const updated_at = yield* metadata.Now;

						yield* transaction.insert(OrchestrationRuns).values({
							agent_id: coordinator.agent_id,
							created_at: updated_at,
							engine_id: coordinator.engine_id,
							native_resume_json: null,
							native_thread_id: null,
							run_id,
							status: "queued",
							thread_id: message.thread_id,
							updated_at,
							working_directory: prior_run.working_directory,
						});
						yield* transaction
							.update(OrchestrationMessages)
							.set({ delivery: "queued", run_id })
							.where(eq(OrchestrationMessages.command_id, command_id));
						yield* transaction
							.update(OrchestrationOutbox)
							.set({ kind: "start", run_id, status: "pending", updated_at })
							.where(eq(OrchestrationOutbox.command_id, command_id));
						yield* transaction
							.update(OrchestrationCoordinators)
							.set({ active_run_id: run_id, updated_at })
							.where(eq(OrchestrationCoordinators.thread_id, message.thread_id));

						return [
							yield* AppendEvent(transaction, {
								agent_id: coordinator.agent_id,
								causation_id: command_id,
								correlation_id: command_id,
								payload: {
									message_id: message.message_id,
									reason,
									text: message.text,
									type: "thread.message_queued",
									working_directory: prior_run.working_directory,
								},
								run_id,
								thread_id: message.thread_id,
							}),
							yield* AppendEvent(transaction, {
								agent_id: coordinator.agent_id,
								causation_id: command_id,
								correlation_id: command_id,
								payload: {
									message_id: message.message_id,
									outcome: "queued",
									reason,
									run_id,
									type: "thread.message_routed",
								},
								run_id,
								thread_id: message.thread_id,
							}),
							yield* AppendEvent(transaction, {
								agent_id: coordinator.agent_id,
								causation_id: command_id,
								correlation_id: command_id,
								payload: {
									state: "queued",
									type: "run.lifecycle",
									working_directory: prior_run.working_directory,
								},
								run_id,
								thread_id: message.thread_id,
							}),
						];
					}),
				);

				const latest_event = result.at(-1);
				if (latest_event !== undefined)
					yield* notifier.Publish(latest_event.journal_sequence);

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const RecordObservation = (observation: EngineObservation) =>
			Effect.gen(function* () {
				const frame_json = JSON.stringify(observation.raw.frame) ?? "null";
				/**
				 * UTF-8 JSON can reconstruct an identical raw frame byte-for-byte. Retaining
				 * its base64 twin duplicates the full payload, but base64 stays for binary or
				 * otherwise non-identical native frames where it is the exact record.
				 */
				const raw_frame_base64 =
					observation.raw.raw_frame_base64 !== undefined &&
					Buffer.from(frame_json, "utf8").toString("base64") ===
						observation.raw.raw_frame_base64
						? null
						: (observation.raw.raw_frame_base64 ?? null);
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const inserted_observation = yield* transaction
							.insert(OrchestrationRawObservations)
							.values({
								engine_id: observation.raw.engine_id,
								frame_json,
								native_id:
									observation.raw.native_id === undefined
										? null
										: String(observation.raw.native_id),
								native_method: observation.raw.native_method ?? null,
								observation_id: observation.observation_id,
								protocol_version: observation.raw.protocol_version ?? null,
								raw_frame_base64,
								run_id: observation.artisan_run_id,
								sequence: observation.sequence,
								transport: observation.raw.transport,
							})
							.onConflictDoNothing()
							.returning({
								observation_id: OrchestrationRawObservations.observation_id,
							});

						if (inserted_observation.length === 0) {
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
														observation.state === "waiting"
															? "waiting"
															: "running",
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
										inArray(OrchestrationRuns.status, [
											"queued",
											"running",
											"waiting",
										]),
									),
								);
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
								observation._tag === "approval"
									? observation.description
									: observation.text;
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
											eq(
												OrchestrationInteractions.interaction_id,
												interaction_id,
											),
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
											eq(
												OrchestrationInteractions.interaction_id,
												interaction_id,
											),
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
									reference: String(
										observation.raw.native_id ?? observation.observation_id,
									),
								},
								run_id: run.run_id,
								thread_id: run.thread_id,
							}),
						];
					}),
				);

				yield* notifier.Publish(result.at(-1)?.journal_sequence ?? 0);

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const ClaimNativeRecoveries = () =>
			Effect.gen(function* () {
				const recovery = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const updated_at = yield* metadata.Now;
						const stranded = yield* transaction
							.select({ run_id: OrchestrationOutbox.run_id })
							.from(OrchestrationOutbox)
							.where(eq(OrchestrationOutbox.status, "dispatching"));

						if (stranded.length > 0) {
							yield* transaction
								.update(OrchestrationOutbox)
								.set({ status: "undeliverable", updated_at })
								.where(eq(OrchestrationOutbox.status, "dispatching"));
						}

						const stranded_run_ids = stranded.map((outbox) => outbox.run_id);
						const live_runs = yield* transaction
							.select()
							.from(OrchestrationRuns)
							.where(
								stranded_run_ids.length > 0
									? or(
											inArray(OrchestrationRuns.status, [
												"running",
												"waiting",
											]),
											and(
												eq(OrchestrationRuns.status, "queued"),
												inArray(OrchestrationRuns.run_id, stranded_run_ids),
											),
										)
									: inArray(OrchestrationRuns.status, ["running", "waiting"]),
							);
						const events: EventEnvelope[] = [];
						const recoverable: RecoverableNativeRun[] = [];

						for (const run of live_runs) {
							yield* transaction
								.update(OrchestrationRuns)
								.set({ status: "interrupted", updated_at })
								.where(eq(OrchestrationRuns.run_id, run.run_id));
							events.push(
								yield* AppendEvent(transaction, {
									agent_id: run.agent_id,
									causation_id: `recovery:${run.run_id}`,
									correlation_id: run.run_id,
									payload: {
										state: "interrupted",
										type: "run.lifecycle",
										working_directory: run.working_directory,
									},
									run_id: run.run_id,
									thread_id: run.thread_id,
								}),
							);

							if (
								!is_active_status(run.status) ||
								!run.native_thread_id ||
								!run.native_resume_json
							) {
								continue;
							}

							const decoded = yield* DecodePersistedJson(
								PersistedResumeToken,
								run.native_resume_json,
							);

							if (decoded.native_thread_id === run.native_thread_id) {
								recoverable.push({
									agent_id: run.agent_id,
									engine_id: run.engine_id,
									resume_token: {
										native_thread_id: decoded.native_thread_id,
										...(decoded.opaque_checkpoint !== undefined
											? { opaque_checkpoint: decoded.opaque_checkpoint }
											: {}),
									},
									run_id: run.run_id,
									thread_id: run.thread_id,
									working_directory: run.working_directory,
								});
							}
						}

						return { events, recoverable };
					}),
				);

				const latest_event = recovery.events.at(-1);
				if (latest_event !== undefined)
					yield* notifier.Publish(latest_event.journal_sequence);

				return recovery.recoverable;
			}).pipe(Effect.mapError(normalize_error));

		const MarkInterrupted = () => ClaimNativeRecoveries().pipe(Effect.asVoid);
		const Accept = (
			command: import("./message-command").AuthoritativeCommandEnvelope,
			can_steer: boolean,
			intake?: IntakeAssessment,
			routing_reason?: ThreadMessageRoutedEvent["reason"],
		) => AcceptCommand(command, can_steer, intake, routing_reason);
		const AcceptInbound = (
			command: CommandEnvelope,
			can_steer: boolean,
			intake?: IntakeAssessment,
			routing_reason?: ThreadMessageRoutedEvent["reason"],
		) => AcceptCommand(command, can_steer, intake, routing_reason, true);

		return {
			Accept,
			AcceptInbound,
			ClaimOutbox,
			ClaimNativeRecoveries,
			CompleteOutbox,
			FallbackSteering,
			GetPending,
			GetAutoSteer,
			GetSessionPolicy,
			GetSession,
			GetWork,
			MarkInterrupted,
			MarkOutboxUndeliverable,
			MarkRunStarted,
			MarkRunResumed,
			PersistNativeRun,
			RecordObservation,
		};
	}),
);
