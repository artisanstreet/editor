import { and, asc, desc, eq, inArray, notExists, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventPayload,
	ThreadSessionPolicy,
	RawOrigin,
	CommandPayload,
	type CommandEnvelope,
	type EventEnvelope,
	type ThreadMessageRoutedEvent,
	type ThreadSessionSnapshot,
	type ThreadWorkItem,
} from "@artisan/protocol";
import type { EngineObservation } from "@artisan/engines";

import { Database } from "./database";
import { JournalNotifier } from "./journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationInteractions,
	OrchestrationIntake,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
	ThreadErasureClaims,
	Threads,
} from "./schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import type { IntakeAssessment } from "../orchestration/intake-policy";
import { RecordThreadActivity } from "../threads/internal/thread-activity";
import { PersistSurfaceProjection } from "../surfaces/surface-projection";

type WorkStatus = ThreadWorkItem["status"];
type OutboxKind = "start" | "steer" | "cancel" | "close" | "respond_approval" | "respond_question";

export class OrchestrationCommandConflict extends Data.TaggedError("OrchestrationCommandConflict")<{
	readonly message_id: string;
}> {}

export class OrchestrationNotFound extends Data.TaggedError("OrchestrationNotFound")<{
	readonly resource: "run" | "thread";
	readonly id: string;
}> {}

export class OrchestrationFailure extends Data.TaggedError("OrchestrationFailure")<{
	readonly cause: unknown;
}> {}

export type OrchestrationError =
	| OrchestrationCommandConflict
	| OrchestrationFailure
	| OrchestrationNotFound;

export interface AcceptedOrchestrationCommand {
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly journal_sequence: number;
	readonly run_id: string;
	readonly status: "accepted" | "duplicate";
}

export interface PendingWork {
	readonly agent_id: string;
	readonly command_id: string;
	readonly engine_id: string;
	readonly kind: OutboxKind;
	readonly payload: CommandEnvelope["payload"];
	readonly run_id: string;
	readonly thread_id: string;
	readonly working_directory: string;
}

export class OrchestrationRepository extends Context.Service<
	OrchestrationRepository,
	{
		readonly Accept: (
			command: CommandEnvelope,
			can_steer: boolean,
			intake?: IntakeAssessment,
			routing_reason?: ThreadMessageRoutedEvent["reason"],
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly CompleteOutbox: (command_id: string) => Effect.Effect<void, OrchestrationError>;
		readonly ClaimOutbox: (command_id: string) => Effect.Effect<boolean, OrchestrationError>;
		readonly FallbackSteering: (
			command_id: string,
			reason?: "delivery_failed" | "rejected",
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
		readonly GetPending: () => Effect.Effect<ReadonlyArray<PendingWork>, OrchestrationError>;
		readonly GetWork: (
			thread_id: string,
		) => Effect.Effect<ThreadWorkItem | undefined, OrchestrationError>;
		readonly GetAutoSteer: (thread_id: string) => Effect.Effect<boolean, OrchestrationError>;
		readonly GetSessionPolicy: (
			thread_id: string,
		) => Effect.Effect<ThreadSessionPolicy, OrchestrationError>;
		readonly GetSession: (
			thread_id: string,
		) => Effect.Effect<ThreadSessionSnapshot, OrchestrationError>;
		readonly MarkInterrupted: () => Effect.Effect<void, OrchestrationError>;
		readonly MarkOutboxUndeliverable: (
			command_id: string,
		) => Effect.Effect<void, OrchestrationError>;
		readonly MarkRunStarted: (
			run_id: string,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
		readonly PersistNativeRun: (
			run_id: string,
			native_thread_id: string,
			resume_token: unknown,
		) => Effect.Effect<void, OrchestrationError>;
		readonly RecordObservation: (
			observation: EngineObservation,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
	}
>()("Artisan/OrchestrationRepository") {}

function normalize_error(error: unknown): OrchestrationError {
	if (error instanceof OrchestrationCommandConflict || error instanceof OrchestrationNotFound) {
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

export const OrchestrationRepositoryLive = Layer.effect(
	OrchestrationRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ParsePersistedJson = (json: string) =>
			Effect.try({
				try: () => JSON.parse(json),
				catch: (cause) => new OrchestrationFailure({ cause }),
			});

		const GetJournalSequence = (transaction: typeof database.client) =>
			transaction
				.select({ journal_sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1)
				.pipe(Effect.map(([event]) => event?.journal_sequence ?? 0));

		const AppendEvent = (
			transaction: typeof database.client,
			input: {
				readonly agent_id: string;
				readonly causation_id: string;
				readonly correlation_id: string;
				readonly payload: EventPayload;
				readonly raw_origin?: { readonly provider: string; readonly reference: string };
				readonly run_id?: string;
				readonly thread_id: string;
			},
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const occurred_at = yield* metadata.Now;

				yield* RecordThreadActivity(
					transaction,
					input.thread_id,
					occurred_at,
					input.payload,
				);

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction.insert(EventStreams).values({
						last_sequence: sequence,
						stream_id,
					});
				}

				const [inserted] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: input.payload.type,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(input.payload),
						raw_origin_json: input.raw_origin ? JSON.stringify(input.raw_origin) : null,
						...(input.run_id ? { run_id: input.run_id } : {}),
						schema_version: 1,
						stream_id,
						stream_sequence: sequence,
						thread_id: input.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				return {
					agent_id: input.agent_id,
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					journal_sequence: inserted!.journal_sequence,
					kind: "event" as const,
					message_id: event_id,
					origin: "backend" as const,
					payload: input.payload,
					protocol_version: 1 as const,
					...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
					...(input.run_id ? { run_id: input.run_id } : {}),
					schema_version: 1 as const,
					sequence,
					sent_at: occurred_at,
					stream_id,
					thread_id: input.thread_id,
				} satisfies EventEnvelope;
			});

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
			permission_mode: "on_request",
			sandbox_mode: "workspace_write",
			web_search_enabled: false,
			strict_clarification: false,
		} as const satisfies ThreadSessionPolicy;
		const DecodeSessionPolicy = (
			row:
				| {
						readonly policy_model: string | null;
						readonly policy_reasoning_effort: string;
						readonly policy_permission_mode: string;
						readonly policy_sandbox_mode: string;
						readonly policy_web_search_enabled: boolean;
						readonly policy_strict_clarification: boolean;
				  }
				| undefined,
		) =>
			row
				? Schema.decodeUnknownEffect(ThreadSessionPolicy, {
						onExcessProperty: "error",
					})({
						engine_id: "codex",
						...(row.policy_model === null ? {} : { model: row.policy_model }),
						reasoning_effort: row.policy_reasoning_effort,
						permission_mode: row.policy_permission_mode,
						sandbox_mode: row.policy_sandbox_mode,
						web_search_enabled: row.policy_web_search_enabled,
						strict_clarification: row.policy_strict_clarification,
					}).pipe(Effect.mapError((cause) => new OrchestrationFailure({ cause })))
				: Effect.succeed(DefaultSessionPolicy);
		const GetSessionPolicy = (thread_id: string) =>
			database.client
				.select({
					policy_model: OrchestrationCoordinators.policy_model,
					policy_reasoning_effort: OrchestrationCoordinators.policy_reasoning_effort,
					policy_permission_mode: OrchestrationCoordinators.policy_permission_mode,
					policy_sandbox_mode: OrchestrationCoordinators.policy_sandbox_mode,
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
								policy_model: OrchestrationCoordinators.policy_model,
								policy_reasoning_effort:
									OrchestrationCoordinators.policy_reasoning_effort,
								policy_permission_mode:
									OrchestrationCoordinators.policy_permission_mode,
								policy_sandbox_mode: OrchestrationCoordinators.policy_sandbox_mode,
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
							? yield* Schema.decodeUnknownEffect(EventPayload)(
									JSON.parse(routing_row.payload_json),
								).pipe(Effect.option)
							: undefined;
						const last_routing =
							decoded_routing &&
							decoded_routing._tag === "Some" &&
							decoded_routing.value.type === "thread.message_routed"
								? decoded_routing.value
								: undefined;
						const assumptions = intake
							? (JSON.parse(intake.assumptions_json) as ReadonlyArray<string>).map(
									(assumption) => ({ assumption, message_id: intake.message_id }),
								)
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

		const Accept = (
			command: CommandEnvelope,
			can_steer: boolean,
			intake?: IntakeAssessment,
			routing_reason?: ThreadMessageRoutedEvent["reason"],
		) =>
			Effect.gen(function* () {
				const payload_json = JSON.stringify(command.payload);
				const raw_origin_json = command.raw_origin
					? JSON.stringify(command.raw_origin)
					: null;

				const acceptance = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing_command] = yield* transaction
							.select({
								agent_id: JournalCommands.agent_id,
								causation_id: JournalCommands.causation_id,
								origin: JournalCommands.origin,
								payload_json: JournalCommands.payload_json,
								raw_origin_json: JournalCommands.raw_origin_json,
								run_id: JournalCommands.run_id,
								schema_version: JournalCommands.schema_version,
								sent_at: JournalCommands.sent_at,
								thread_id: JournalCommands.thread_id,
							})
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, command.message_id))
							.limit(1);

						if (existing_command) {
							const matches =
								existing_command.agent_id === (command.agent_id ?? null) &&
								existing_command.causation_id === (command.causation_id ?? null) &&
								existing_command.origin === command.origin &&
								existing_command.payload_json === payload_json &&
								existing_command.raw_origin_json === raw_origin_json &&
								existing_command.run_id === (command.run_id ?? null) &&
								existing_command.schema_version === command.schema_version &&
								existing_command.sent_at === command.sent_at &&
								existing_command.thread_id === command.thread_id;

							if (!matches) {
								return yield* new OrchestrationCommandConflict({
									message_id: command.message_id,
								});
							}

							const event_rows = yield* transaction
								.select({
									agent_id: JournalEvents.agent_id,
									causation_id: JournalEvents.causation_id,
									correlation_id: JournalEvents.correlation_id,
									event_id: JournalEvents.event_id,
									journal_sequence: JournalEvents.sequence,
									occurred_at: JournalEvents.occurred_at,
									payload_json: JournalEvents.payload_json,
									raw_origin_json: JournalEvents.raw_origin_json,
									run_id: JournalEvents.run_id,
									sequence: JournalEvents.stream_sequence,
									stream_id: JournalEvents.stream_id,
									thread_id: JournalEvents.thread_id,
								})
								.from(JournalEvents)
								.where(eq(JournalEvents.correlation_id, command.message_id));
							const events = yield* Effect.forEach(
								event_rows.sort(
									(left, right) => left.journal_sequence - right.journal_sequence,
								),
								(event) =>
									ParsePersistedJson(event.payload_json).pipe(
										Effect.flatMap((value) =>
											Schema.decodeUnknownEffect(EventPayload)(value).pipe(
												Effect.mapError(
													(cause) => new OrchestrationFailure({ cause }),
												),
											),
										),
										Effect.bindTo("payload"),
										Effect.bind("raw_origin", () =>
											event.raw_origin_json
												? ParsePersistedJson(event.raw_origin_json).pipe(
														Effect.flatMap((value) =>
															Schema.decodeUnknownEffect(RawOrigin)(
																value,
															).pipe(
																Effect.mapError(
																	(cause) =>
																		new OrchestrationFailure({
																			cause,
																		}),
																),
															),
														),
													)
												: Effect.succeed(undefined),
										),
										Effect.map(
											({ payload, raw_origin }) =>
												({
													agent_id: event.agent_id!,
													causation_id: event.causation_id,
													correlation_id: event.correlation_id,
													journal_sequence: event.journal_sequence,
													kind: "event" as const,
													message_id: event.event_id,
													origin: "backend" as const,
													payload,
													protocol_version: 1 as const,
													...(raw_origin ? { raw_origin } : {}),
													...(event.run_id
														? { run_id: event.run_id }
														: {}),
													schema_version: 1 as const,
													sequence: event.sequence,
													sent_at: event.occurred_at,
													stream_id: event.stream_id,
													thread_id: event.thread_id,
												}) satisfies EventEnvelope,
										),
									),
							);
							const [outbox] = yield* transaction
								.select({ run_id: OrchestrationOutbox.run_id })
								.from(OrchestrationOutbox)
								.where(eq(OrchestrationOutbox.command_id, command.message_id))
								.limit(1);

							return {
								events,
								journal_sequence:
									events.at(-1)?.journal_sequence ??
									(yield* GetJournalSequence(transaction)),
								run_id: outbox?.run_id ?? command.run_id ?? "unknown",
								status: "duplicate" as const,
							};
						}

						const [thread] = yield* transaction
							.select({ thread_id: Threads.thread_id })
							.from(Threads)
							.where(eq(Threads.thread_id, command.thread_id))
							.limit(1);

						if (!thread) {
							return yield* new OrchestrationNotFound({
								id: command.thread_id,
								resource: "thread",
							});
						}

						const [coordinator] = yield* transaction
							.select()
							.from(OrchestrationCoordinators)
							.where(eq(OrchestrationCoordinators.thread_id, command.thread_id))
							.limit(1);
						const payload = command.payload;
						const accepted_at = yield* metadata.Now;
						if (payload.type === "thread.auto_steer.update") {
							if (!coordinator) {
								return yield* new OrchestrationNotFound({
									id: command.thread_id,
									resource: "run",
								});
							}
							yield* transaction.insert(JournalCommands).values({
								accepted_at,
								agent_id: command.agent_id ?? null,
								causation_id: command.causation_id ?? null,
								message_id: command.message_id,
								origin: command.origin,
								payload_json,
								payload_type: payload.type,
								raw_origin_json,
								assigned_run_id: coordinator.active_run_id,
								run_id: command.run_id ?? null,
								schema_version: command.schema_version,
								sent_at: command.sent_at,
								status: "accepted",
								thread_id: command.thread_id,
							});
							yield* transaction
								.update(OrchestrationCoordinators)
								.set({
									auto_steer_follow_ups: payload.enabled,
									updated_at: accepted_at,
								})
								.where(eq(OrchestrationCoordinators.thread_id, command.thread_id));
							const event = yield* AppendEvent(transaction, {
								agent_id: coordinator.agent_id,
								causation_id: command.message_id,
								correlation_id: command.message_id,
								payload: {
									type: "thread.auto_steer.updated",
									enabled: payload.enabled,
								},
								thread_id: command.thread_id,
							});
							return {
								events: [event],
								journal_sequence: event.journal_sequence,
								run_id: coordinator.active_run_id ?? "none",
								status: "accepted" as const,
							};
						}
						if (payload.type === "thread.session_policy.update") {
							const agent_id =
								coordinator?.agent_id ?? (yield* metadata.MakeId("agent"));
							yield* transaction.insert(JournalCommands).values({
								accepted_at,
								agent_id: command.agent_id ?? null,
								causation_id: command.causation_id ?? null,
								message_id: command.message_id,
								origin: command.origin,
								payload_json,
								payload_type: payload.type,
								raw_origin_json,
								assigned_run_id: coordinator?.active_run_id ?? null,
								run_id: command.run_id ?? null,
								schema_version: command.schema_version,
								sent_at: command.sent_at,
								status: "accepted",
								thread_id: command.thread_id,
							});
							const policy_columns = {
								engine_id: "codex",
								policy_model: payload.policy.model ?? null,
								policy_reasoning_effort: payload.policy.reasoning_effort,
								policy_permission_mode: payload.policy.permission_mode,
								policy_sandbox_mode: payload.policy.sandbox_mode,
								policy_web_search_enabled: payload.policy.web_search_enabled,
								policy_strict_clarification: payload.policy.strict_clarification,
								updated_at: accepted_at,
							};
							if (coordinator) {
								yield* transaction
									.update(OrchestrationCoordinators)
									.set(policy_columns)
									.where(
										eq(OrchestrationCoordinators.thread_id, command.thread_id),
									);
							} else {
								yield* transaction.insert(OrchestrationCoordinators).values({
									active_run_id: null,
									agent_id,
									auto_steer_follow_ups: true,
									created_at: accepted_at,
									display_name: "Primary coordinator",
									native_resume_json: null,
									native_thread_id: null,
									role: "primary",
									thread_id: command.thread_id,
									...policy_columns,
								});
							}
							const event = yield* AppendEvent(transaction, {
								agent_id,
								causation_id: command.message_id,
								correlation_id: command.message_id,
								payload: {
									type: "thread.session_policy.updated",
									policy: payload.policy,
								},
								thread_id: command.thread_id,
							});
							return {
								events: [event],
								journal_sequence: event.journal_sequence,
								run_id: coordinator?.active_run_id ?? "none",
								status: "accepted" as const,
							};
						}
						if (payload.type === "intake.respond_question") {
							const [pending] = yield* transaction
								.select()
								.from(OrchestrationIntake)
								.where(
									and(
										eq(OrchestrationIntake.thread_id, command.thread_id),
										eq(OrchestrationIntake.question_id, payload.question_id),
										eq(OrchestrationIntake.state, "pending"),
									),
								)
								.limit(1);
							if (
								!pending ||
								Object.keys(payload.answers).length !== 1 ||
								!Object.hasOwn(payload.answers, payload.question_id)
							) {
								return yield* new OrchestrationNotFound({
									id: payload.question_id,
									resource: "run",
								});
							}
							const [active_coordinator_run] = coordinator?.active_run_id
								? yield* transaction
										.select({ status: OrchestrationRuns.status })
										.from(OrchestrationRuns)
										.where(
											eq(OrchestrationRuns.run_id, coordinator.active_run_id),
										)
										.limit(1)
								: [];
							if (
								active_coordinator_run &&
								is_projectable_status(active_coordinator_run.status)
							) {
								return yield* new OrchestrationFailure({
									cause: new Error(
										"Cannot resolve intake while a coordinator run is active",
									),
								});
							}
							const run_id = yield* metadata.MakeId("run");
							const intake_raw_origin = pending.raw_origin_json
								? JSON.parse(pending.raw_origin_json)
								: undefined;
							const resolved_agent_id =
								coordinator?.agent_id ??
								command.agent_id ??
								(yield* metadata.MakeId("agent"));
							yield* transaction.insert(JournalCommands).values({
								accepted_at,
								agent_id: command.agent_id ?? null,
								causation_id: command.causation_id ?? null,
								message_id: command.message_id,
								origin: command.origin,
								payload_json,
								payload_type: payload.type,
								raw_origin_json,
								assigned_run_id: run_id,
								run_id: command.run_id ?? null,
								schema_version: command.schema_version,
								sent_at: command.sent_at,
								status: "accepted",
								thread_id: command.thread_id,
							});
							if (!coordinator)
								yield* transaction.insert(OrchestrationCoordinators).values({
									active_run_id: run_id,
									agent_id: resolved_agent_id,
									auto_steer_follow_ups: true,
									created_at: accepted_at,
									display_name: "Primary coordinator",
									engine_id: pending.engine_id,
									native_resume_json: null,
									native_thread_id: null,
									role: "primary",
									thread_id: command.thread_id,
									updated_at: accepted_at,
								});
							else
								yield* transaction
									.update(OrchestrationCoordinators)
									.set({
										active_run_id: run_id,
										engine_id: pending.engine_id,
										updated_at: accepted_at,
									})
									.where(
										eq(OrchestrationCoordinators.thread_id, command.thread_id),
									);
							yield* transaction.insert(OrchestrationRuns).values({
								agent_id: resolved_agent_id,
								created_at: accepted_at,
								engine_id: pending.engine_id,
								native_resume_json: null,
								native_thread_id: null,
								run_id,
								status: "queued",
								thread_id: command.thread_id,
								updated_at: accepted_at,
								working_directory: pending.working_directory,
							});
							yield* transaction.insert(OrchestrationMessages).values({
								agent_id: resolved_agent_id,
								command_id: command.message_id,
								created_at: accepted_at,
								delivery: "queued",
								message_id: pending.message_id,
								run_id,
								text: pending.text,
								thread_id: command.thread_id,
							});
							yield* transaction
								.update(OrchestrationIntake)
								.set({ state: "resolved", updated_at: accepted_at })
								.where(eq(OrchestrationIntake.message_id, pending.message_id));
							const start_payload = {
								type: "thread.send_message",
								engine_id: pending.engine_id,
								text: pending.text,
								working_directory: pending.working_directory,
								...(pending.mentioned_projects_json
									? {
											mentioned_projects: JSON.parse(
												pending.mentioned_projects_json,
											),
										}
									: {}),
							};
							yield* transaction.insert(OrchestrationOutbox).values({
								agent_id: resolved_agent_id,
								command_id: command.message_id,
								created_at: accepted_at,
								kind: "start",
								payload_json: JSON.stringify(start_payload),
								run_id,
								status: "pending",
								thread_id: command.thread_id,
								updated_at: accepted_at,
							});
							const events = [
								yield* AppendEvent(transaction, {
									agent_id: resolved_agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										type: "interaction.question",
										question_id: payload.question_id,
										answers: payload.answers,
										state: "resolved",
										text: pending.question ?? "Clarified",
										source: "intake",
									},
									...(intake_raw_origin === undefined
										? {}
										: { raw_origin: intake_raw_origin }),
									run_id,
									thread_id: command.thread_id,
								}),
								yield* AppendEvent(transaction, {
									agent_id: resolved_agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										type: "thread.message_queued",
										message_id: pending.message_id,
										reason: "no_active_run",
										text: pending.text,
										working_directory: pending.working_directory,
									},
									...(intake_raw_origin === undefined
										? {}
										: { raw_origin: intake_raw_origin }),
									run_id,
									thread_id: command.thread_id,
								}),
								yield* AppendEvent(transaction, {
									agent_id: resolved_agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										message_id: pending.message_id,
										outcome: "queued",
										reason: "no_active_run",
										run_id,
										type: "thread.message_routed",
									},
									...(intake_raw_origin === undefined
										? {}
										: { raw_origin: intake_raw_origin }),
									run_id,
									thread_id: command.thread_id,
								}),
								yield* AppendEvent(transaction, {
									agent_id: resolved_agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										type: "run.lifecycle",
										state: "queued",
										working_directory: pending.working_directory,
									},
									...(intake_raw_origin === undefined
										? {}
										: { raw_origin: intake_raw_origin }),
									run_id,
									thread_id: command.thread_id,
								}),
							];
							return {
								events,
								journal_sequence: events.at(-1)!.journal_sequence,
								run_id,
								status: "accepted" as const,
							};
						}
						const agent_id =
							coordinator?.agent_id ??
							command.agent_id ??
							(yield* metadata.MakeId("agent"));
						const active_run = coordinator?.active_run_id
							? yield* transaction
									.select()
									.from(OrchestrationRuns)
									.where(eq(OrchestrationRuns.run_id, coordinator.active_run_id))
									.limit(1)
							: [];
						const current_run = active_run[0];

						if (command.run_id && current_run?.run_id !== command.run_id) {
							return yield* new OrchestrationNotFound({
								id: command.run_id,
								resource: "run",
							});
						}

						const send_message = payload.type === "thread.send_message";
						if (send_message && intake?.resolution === "question") {
							const question_id = yield* metadata.MakeId("message");
							const intake_id = yield* metadata.MakeId("run");
							yield* transaction.insert(JournalCommands).values({
								accepted_at,
								agent_id: command.agent_id ?? null,
								causation_id: command.causation_id ?? null,
								message_id: command.message_id,
								origin: command.origin,
								payload_json,
								payload_type: payload.type,
								raw_origin_json,
								assigned_run_id: intake_id,
								run_id: command.run_id ?? null,
								schema_version: command.schema_version,
								sent_at: command.sent_at,
								status: "accepted",
								thread_id: command.thread_id,
							});
							yield* transaction.insert(OrchestrationIntake).values({
								assumptions_json: JSON.stringify(intake.assumptions),
								created_at: accepted_at,
								engine_id: payload.engine_id,
								message_id: command.message_id,
								question: intake.question ?? "Please clarify the request.",
								question_id,
								risk: intake.risk,
								state: "pending",
								text: payload.text,
								mentioned_projects_json:
									payload.mentioned_projects === undefined
										? null
										: JSON.stringify(payload.mentioned_projects),
								thread_id: command.thread_id,
								raw_origin_json,
								updated_at: accepted_at,
								working_directory: payload.working_directory,
							});
							const events = [
								yield* AppendEvent(transaction, {
									agent_id: command.agent_id ?? "intake",
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										type: "intake.assessed",
										message_id: command.message_id,
										risk: intake.risk,
										resolution: "question",
									},
									...(command.raw_origin === undefined
										? {}
										: { raw_origin: command.raw_origin }),
									thread_id: command.thread_id,
								}),
								yield* AppendEvent(transaction, {
									agent_id: command.agent_id ?? "intake",
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										type: "interaction.question",
										question_id,
										state: "requested",
										text: intake.question ?? "Please clarify the request.",
										source: "intake",
									},
									...(command.raw_origin === undefined
										? {}
										: { raw_origin: command.raw_origin }),
									thread_id: command.thread_id,
								}),
							];
							return {
								events,
								journal_sequence: events.at(-1)!.journal_sequence,
								run_id: intake_id,
								status: "accepted" as const,
							};
						}
						const requested_engine_id =
							"engine_id" in payload ? payload.engine_id : undefined;
						const steer =
							send_message &&
							current_run &&
							is_active_status(current_run.status) &&
							requested_engine_id === current_run.engine_id &&
							can_steer;
						const engine_id = steer
							? current_run.engine_id
							: send_message
								? requested_engine_id
								: (current_run?.engine_id ?? coordinator?.engine_id);

						if (!engine_id) {
							return yield* new OrchestrationNotFound({
								id: command.thread_id,
								resource: "run",
							});
						}

						const run_id =
							steer || !send_message
								? (command.run_id ?? current_run?.run_id)
								: yield* metadata.MakeId("run");

						if (!run_id) {
							return yield* new OrchestrationNotFound({
								id: command.thread_id,
								resource: "run",
							});
						}

						yield* transaction.insert(JournalCommands).values({
							accepted_at,
							agent_id: command.agent_id ?? null,
							causation_id: command.causation_id ?? null,
							message_id: command.message_id,
							origin: command.origin,
							payload_json,
							payload_type: payload.type,
							raw_origin_json,
							assigned_run_id: run_id,
							run_id: command.run_id ?? null,
							schema_version: command.schema_version,
							sent_at: command.sent_at,
							status: "accepted",
							thread_id: command.thread_id,
						});

						if (!coordinator) {
							yield* transaction.insert(OrchestrationCoordinators).values({
								active_run_id: send_message && !steer ? run_id : null,
								agent_id,
								created_at: accepted_at,
								display_name: "Primary coordinator",
								engine_id,
								native_resume_json: null,
								native_thread_id: null,
								role: "primary",
								thread_id: command.thread_id,
								updated_at: accepted_at,
							});
						}

						const events: EventEnvelope[] = [];

						if (send_message && !steer) {
							yield* transaction.insert(OrchestrationRuns).values({
								agent_id,
								created_at: accepted_at,
								engine_id,
								native_resume_json: null,
								native_thread_id: null,
								run_id,
								status: "queued",
								thread_id: command.thread_id,
								updated_at: accepted_at,
								working_directory: payload.working_directory,
							});
							yield* transaction
								.update(OrchestrationCoordinators)
								.set({ active_run_id: run_id, engine_id, updated_at: accepted_at })
								.where(eq(OrchestrationCoordinators.thread_id, command.thread_id));
							yield* transaction.insert(OrchestrationMessages).values({
								agent_id,
								command_id: command.message_id,
								created_at: accepted_at,
								delivery: "queued",
								message_id: command.message_id,
								run_id,
								text: payload.text,
								thread_id: command.thread_id,
							});
							events.push(
								yield* AppendEvent(transaction, {
									agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										message_id: command.message_id,
										...(payload.mentioned_projects === undefined
											? {}
											: { mentioned_projects: payload.mentioned_projects }),
										reason:
											routing_reason ??
											(current_run && is_active_status(current_run.status)
												? "unsupported"
												: "no_active_run"),
										text: payload.text,
										type: "thread.message_queued",
										working_directory: payload.working_directory,
									},
									...(command.raw_origin
										? { raw_origin: command.raw_origin }
										: {}),
									run_id,
									thread_id: command.thread_id,
								}),
							);
							events.push(
								yield* AppendEvent(transaction, {
									agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										message_id: command.message_id,
										outcome: "queued",
										reason:
											routing_reason ??
											(current_run && is_active_status(current_run.status)
												? "unsupported"
												: "no_active_run"),
										run_id,
										type: "thread.message_routed",
									},
									run_id,
									thread_id: command.thread_id,
								}),
							);
							events.push(
								yield* AppendEvent(transaction, {
									agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										state: "queued",
										type: "run.lifecycle",
										working_directory: payload.working_directory,
									},
									...(command.raw_origin
										? { raw_origin: command.raw_origin }
										: {}),
									run_id,
									thread_id: command.thread_id,
								}),
							);
						} else if (send_message) {
							yield* transaction.insert(OrchestrationMessages).values({
								agent_id,
								command_id: command.message_id,
								created_at: accepted_at,
								delivery: "steering",
								message_id: command.message_id,
								run_id,
								text: payload.text,
								thread_id: command.thread_id,
							});
							events.push(
								yield* AppendEvent(transaction, {
									agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										message_id: command.message_id,
										...(payload.mentioned_projects === undefined
											? {}
											: { mentioned_projects: payload.mentioned_projects }),
										text: payload.text,
										type: "thread.message_steering",
										working_directory: payload.working_directory,
									},
									...(command.raw_origin
										? { raw_origin: command.raw_origin }
										: {}),
									run_id,
									thread_id: command.thread_id,
								}),
							);
						}

						if (payload.type === "run.respond_approval") {
							const [interaction] = yield* transaction
								.select()
								.from(OrchestrationInteractions)
								.where(
									and(
										eq(
											OrchestrationInteractions.interaction_id,
											payload.approval_id,
										),
										eq(OrchestrationInteractions.kind, "approval"),
										eq(OrchestrationInteractions.run_id, run_id),
										eq(OrchestrationInteractions.state, "requested"),
									),
								)
								.limit(1);

							if (!interaction) {
								return yield* new OrchestrationNotFound({
									id: payload.approval_id,
									resource: "run",
								});
							}
						}

						if (payload.type === "run.respond_question") {
							for (const question_id of Object.keys(payload.answers)) {
								const [interaction] = yield* transaction
									.select()
									.from(OrchestrationInteractions)
									.where(
										and(
											eq(
												OrchestrationInteractions.interaction_id,
												question_id,
											),
											eq(OrchestrationInteractions.kind, "question"),
											eq(OrchestrationInteractions.run_id, run_id),
											eq(OrchestrationInteractions.state, "requested"),
										),
									)
									.limit(1);

								if (!interaction) {
									return yield* new OrchestrationNotFound({
										id: question_id,
										resource: "run",
									});
								}
							}
						}

						if (send_message && intake) {
							yield* transaction.insert(OrchestrationIntake).values({
								assumptions_json: JSON.stringify(intake.assumptions),
								created_at: accepted_at,
								engine_id: payload.engine_id,
								message_id: command.message_id,
								mentioned_projects_json:
									payload.mentioned_projects === undefined
										? null
										: JSON.stringify(payload.mentioned_projects),
								question: null,
								question_id: null,
								raw_origin_json,
								risk: intake.risk,
								state: "resolved",
								text: payload.text,
								thread_id: command.thread_id,
								updated_at: accepted_at,
								working_directory: payload.working_directory,
							});
							events.push(
								yield* AppendEvent(transaction, {
									agent_id,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									payload: {
										type: "intake.assessed",
										message_id: command.message_id,
										risk: intake.risk,
										resolution: intake.resolution,
									},
									...(command.raw_origin === undefined
										? {}
										: { raw_origin: command.raw_origin }),
									thread_id: command.thread_id,
								}),
							);
							for (const assumption of intake.assumptions) {
								events.push(
									yield* AppendEvent(transaction, {
										agent_id,
										causation_id: command.message_id,
										correlation_id: command.message_id,
										payload: {
											type: "intake.assumption_recorded",
											message_id: command.message_id,
											assumption,
										},
										...(command.raw_origin === undefined
											? {}
											: { raw_origin: command.raw_origin }),
										thread_id: command.thread_id,
									}),
								);
							}
						}

						const kind: OutboxKind =
							payload.type === "thread.send_message"
								? steer
									? "steer"
									: "start"
								: (payload.type.replace("run.", "") as OutboxKind);
						yield* transaction.insert(OrchestrationOutbox).values({
							agent_id,
							command_id: command.message_id,
							created_at: accepted_at,
							kind,
							payload_json,
							run_id,
							status: "pending",
							thread_id: command.thread_id,
							updated_at: accepted_at,
						});

						return {
							events,
							journal_sequence:
								events.at(-1)?.journal_sequence ??
								(yield* GetJournalSequence(transaction)),
							run_id,
							status: "accepted" as const,
						};
					}),
				);

				if (acceptance.status === "accepted" && acceptance.journal_sequence > 0) {
					yield* notifier.Publish(acceptance.journal_sequence);
				}

				return acceptance;
			}).pipe(Effect.mapError(normalize_error));

		const GetPending = () =>
			database.client
				.select({
					agent_id: OrchestrationOutbox.agent_id,
					command_id: OrchestrationOutbox.command_id,
					engine_id: OrchestrationRuns.engine_id,
					kind: OrchestrationOutbox.kind,
					payload_json: OrchestrationOutbox.payload_json,
					run_id: OrchestrationOutbox.run_id,
					status: OrchestrationRuns.status,
					thread_id: OrchestrationOutbox.thread_id,
					working_directory: OrchestrationRuns.working_directory,
				})
				.from(OrchestrationOutbox)
				.innerJoin(
					OrchestrationRuns,
					eq(OrchestrationOutbox.run_id, OrchestrationRuns.run_id),
				)
				.where(
					and(
						eq(OrchestrationOutbox.status, "pending"),
						notExists(
							database.client
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(
									eq(
										ThreadErasureClaims.thread_id,
										OrchestrationOutbox.thread_id,
									),
								),
						),
					),
				)
				.orderBy(asc(OrchestrationOutbox.created_at), asc(OrchestrationOutbox.command_id))
				.pipe(
					Effect.flatMap((rows) =>
						Effect.forEach(
							rows.filter((row) => row.kind !== "start" || row.status === "queued"),
							(row) =>
								ParsePersistedJson(row.payload_json).pipe(
									Effect.flatMap((value) =>
										Schema.decodeUnknownEffect(CommandPayload)(value).pipe(
											Effect.mapError(
												(cause) => new OrchestrationFailure({ cause }),
											),
										),
									),
									Effect.map(
										(payload) =>
											({
												agent_id: row.agent_id,
												command_id: row.command_id,
												engine_id: row.engine_id,
												kind: row.kind as OutboxKind,
												payload,
												run_id: row.run_id,
												thread_id: row.thread_id,
												working_directory: row.working_directory,
											}) satisfies PendingWork,
									),
								),
						),
					),
					Effect.mapError(normalize_error),
				);

		const CompleteOutbox = (command_id: string) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				const events = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [outbox] = yield* transaction
							.update(OrchestrationOutbox)
							.set({ status: "delivered", updated_at })
							.where(
								and(
									eq(OrchestrationOutbox.command_id, command_id),
									eq(OrchestrationOutbox.status, "dispatching"),
								),
							)
							.returning();
						if (!outbox || outbox.kind !== "steer") return [];
						const [message] = yield* transaction
							.select()
							.from(OrchestrationMessages)
							.where(eq(OrchestrationMessages.command_id, command_id))
							.limit(1);
						if (
							!message ||
							message.delivery !== "steering" ||
							message.run_id === null ||
							message.agent_id === null
						)
							return [];
						return [
							yield* AppendEvent(transaction, {
								agent_id: message.agent_id,
								causation_id: command_id,
								correlation_id: command_id,
								payload: {
									message_id: message.message_id,
									outcome: "steered",
									run_id: message.run_id,
									type: "thread.message_routed",
								},
								run_id: message.run_id,
								thread_id: message.thread_id,
							}),
						];
					}),
				);
				if (events.length > 0) yield* notifier.Publish(events.at(-1)!.journal_sequence);
			}).pipe(Effect.mapError(normalize_error));

		const ClaimOutbox = (command_id: string) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;
				const claimed = yield* database.client
					.update(OrchestrationOutbox)
					.set({ status: "dispatching", updated_at })
					.where(
						and(
							eq(OrchestrationOutbox.command_id, command_id),
							eq(OrchestrationOutbox.status, "pending"),
							notExists(
								database.client
									.select({ thread_id: ThreadErasureClaims.thread_id })
									.from(ThreadErasureClaims)
									.where(
										eq(
											ThreadErasureClaims.thread_id,
											OrchestrationOutbox.thread_id,
										),
									),
							),
						),
					)
					.returning({ command_id: OrchestrationOutbox.command_id });

				return claimed.length === 1;
			}).pipe(Effect.mapError(normalize_error));

		const MarkOutboxUndeliverable = (command_id: string) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;

				yield* database.client
					.update(OrchestrationOutbox)
					.set({ status: "undeliverable", updated_at })
					.where(eq(OrchestrationOutbox.command_id, command_id));
			}).pipe(Effect.mapError(normalize_error));

		const PersistNativeRun = (
			run_id: string,
			native_thread_id: string,
			resume_token: unknown,
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

				if (result.length > 0) {
					yield* notifier.Publish(result.at(-1)!.journal_sequence);
				}

				return result;
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

						if (!message || message.delivery !== "steering") {
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
							.where(eq(OrchestrationRuns.run_id, message.run_id!))
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

				if (result.length > 0) {
					yield* notifier.Publish(result.at(-1)!.journal_sequence);
				}

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const RecordObservation = (observation: EngineObservation) =>
			Effect.gen(function* () {
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const inserted_observation = yield* transaction
							.insert(OrchestrationRawObservations)
							.values({
								engine_id: observation.raw.engine_id,
								frame_json: JSON.stringify(observation.raw.frame) ?? "null",
								native_id:
									observation.raw.native_id === undefined
										? null
										: String(observation.raw.native_id),
								native_method: observation.raw.native_method ?? null,
								observation_id: observation.observation_id,
								protocol_version: observation.raw.protocol_version ?? null,
								raw_frame_base64: observation.raw.raw_frame_base64 ?? null,
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
												...(observation.answers?.length
													? {
															answers: {
																[observation.question_id]: [
																	observation.answers[0]!,
																	...observation.answers.slice(1),
																],
															},
														}
													: {}),
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

				if (result.length > 0) {
					yield* notifier.Publish(result.at(-1)!.journal_sequence);
				}

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const MarkInterrupted = () =>
			Effect.gen(function* () {
				const events = yield* database.client.transaction((transaction) =>
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
						const recovered: EventEnvelope[] = [];

						for (const run of live_runs) {
							yield* transaction
								.update(OrchestrationRuns)
								.set({ status: "interrupted", updated_at })
								.where(eq(OrchestrationRuns.run_id, run.run_id));
							recovered.push(
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
						}

						return recovered;
					}),
				);

				if (events.length > 0) {
					yield* notifier.Publish(events.at(-1)!.journal_sequence);
				}
			}).pipe(Effect.mapError(normalize_error));

		return {
			Accept,
			ClaimOutbox,
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
			PersistNativeRun,
			RecordObservation,
		};
	}),
);
