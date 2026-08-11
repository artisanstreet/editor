import { and, desc, eq, inArray, notInArray, or } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import {
	ThreadSessionPolicy,
	type CommandEnvelope,
	type EventEnvelope,
	type ThreadMessageRoutedEvent,
	type ThreadSessionSnapshot,
	type ThreadWorkItem,
} from "@artisan/protocol";

import { Database } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import { make_observation_recording } from "./observation-recording";
import { make_outbox_operations, ReadAuthoritativeSteeringPayload } from "./outbox";
import { ImageAttachmentsFor } from "./message-attachments";
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
	OrchestrationRuns,
} from "../tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import { CancelPendingInteractions } from "../../conversation/index.ts";
import type { IntakeAssessment } from "../../orchestration/intake-policy";
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

		const CancelInterruptedRun = (run_id: string) =>
			Effect.gen(function* () {
				const event = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const updated_at = yield* metadata.Now;
						const [run] = yield* transaction
							.update(OrchestrationRuns)
							.set({ status: "cancelled", updated_at })
							.where(
								and(
									eq(OrchestrationRuns.run_id, run_id),
									eq(OrchestrationRuns.status, "interrupted"),
								),
							)
							.returning();
						if (!run) return undefined;
						return yield* AppendEvent(transaction, {
							agent_id: run.agent_id,
							causation_id: `shutdown:${run_id}`,
							correlation_id: run_id,
							payload: {
								state: "cancelled",
								type: "run.lifecycle",
								working_directory: run.working_directory,
							},
							run_id,
							thread_id: run.thread_id,
						});
					}),
				);
				if (event === undefined) return false;
				yield* notifier.Publish(event.journal_sequence);
				return true;
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
						const [outbox] = yield* transaction
							.select({ payload_json: OrchestrationOutbox.payload_json })
							.from(OrchestrationOutbox)
							.where(eq(OrchestrationOutbox.command_id, command_id))
							.limit(1);
						if (!outbox) {
							return yield* new OrchestrationFailure({
								cause: new Error(`Missing steering outbox ${command_id}`),
							});
						}
						const { payload, raw_origin } = yield* ReadAuthoritativeSteeringPayload(
							transaction,
							command_id,
							outbox.payload_json,
						);

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
							working_directory: payload.working_directory,
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
									...(payload.attachments === undefined
										? {}
										: { attachments: ImageAttachmentsFor(payload) }),
									...(payload.content === undefined
										? {}
										: { content: payload.content }),
									...(payload.mentioned_projects === undefined
										? {}
										: { mentioned_projects: payload.mentioned_projects }),
									reason,
									text: payload.text,
									type: "thread.message_queued",
									working_directory: payload.working_directory,
								},
								...(raw_origin === undefined ? {} : { raw_origin }),
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
									working_directory: payload.working_directory,
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

		const { RecordObservation, RecordObservations } = make_observation_recording(
			database.client,
			metadata,
			notifier,
		);

		const ClaimNativeRecoveries = () =>
			Effect.gen(function* () {
				const recovery = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const updated_at = yield* metadata.Now;

						/**
						 * Heals interactions that predate terminal-run release: a
						 * requested approval whose run already ended can never be
						 * answered, and leaving it requested lets stale responses
						 * enter the outbox with no run to deliver them to.
						 */
						const released = yield* transaction
							.update(OrchestrationInteractions)
							.set({ state: "cancelled", updated_at })
							.where(
								and(
									eq(OrchestrationInteractions.state, "requested"),
									inArray(
										OrchestrationInteractions.run_id,
										transaction
											.select({ run_id: OrchestrationRuns.run_id })
											.from(OrchestrationRuns)
											.where(
												notInArray(OrchestrationRuns.status, [
													"queued",
													"running",
													"waiting",
													"interrupted",
												]),
											),
									),
								),
							)
							.returning({ run_id: OrchestrationInteractions.run_id });

						for (const released_run_id of new Set(
							released.map((interaction) => interaction.run_id),
						)) {
							const [released_run] = yield* transaction
								.select({ thread_id: OrchestrationRuns.thread_id })
								.from(OrchestrationRuns)
								.where(eq(OrchestrationRuns.run_id, released_run_id))
								.limit(1);

							if (!released_run) continue;

							yield* CancelPendingInteractions(
								transaction,
								{
									occurred_at: updated_at,
									run_id: released_run_id,
									thread_id: released_run.thread_id,
								},
								`run:${released_run_id}`,
								`recovery:${released_run_id}`,
							);
						}

						const stranded = yield* transaction
							.select({
								command_id: OrchestrationOutbox.command_id,
								kind: OrchestrationOutbox.kind,
								run_id: OrchestrationOutbox.run_id,
							})
							.from(OrchestrationOutbox)
							.where(eq(OrchestrationOutbox.status, "dispatching"));

						const stranded_steers = stranded.filter(
							(outbox) => outbox.kind === "steer",
						);
						const stranded_non_steers = stranded.filter(
							(outbox) => outbox.kind !== "steer",
						);
						if (stranded_non_steers.length > 0) {
							yield* transaction
								.update(OrchestrationOutbox)
								.set({ status: "undeliverable", updated_at })
								.where(
									inArray(
										OrchestrationOutbox.command_id,
										stranded_non_steers.map((outbox) => outbox.command_id),
									),
								);
						}

						const stranded_run_ids = stranded.map((outbox) => outbox.run_id);
						const stranded_steer_run_ids = new Set(
							stranded_steers.map((outbox) => outbox.run_id),
						);
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
							if (stranded_steer_run_ids.has(run.run_id)) continue;

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

						return {
							events,
							recoverable,
							stranded_steer_command_ids: stranded_steers.map(
								(outbox) => outbox.command_id,
							),
						};
					}),
				);

				const latest_event = recovery.events.at(-1);
				if (latest_event !== undefined)
					yield* notifier.Publish(latest_event.journal_sequence);
				yield* Effect.forEach(recovery.stranded_steer_command_ids, (command_id) =>
					FallbackSteering(command_id, "delivery_failed"),
				);

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
			CancelInterruptedRun,
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
			RecordObservations,
		};
	}),
);
