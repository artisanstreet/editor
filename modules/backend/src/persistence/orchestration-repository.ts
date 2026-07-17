import { and, asc, desc, eq, inArray, notExists, or, sql } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";

import {
	RawOrigin,
	EventPayload,
	CommandPayload,
	type EventEnvelope,
	type ThreadWorkItem,
	type CommandEnvelope,
} from "@artisan/protocol";
import { EngineResumeToken, type EngineObservation } from "@artisan/engines";

import { Database } from "./database";
import { JournalNotifier } from "./journal-notifier";
import { RetrySqliteWrite } from "./sqlite-write-retry";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationInteractions,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
	RunUsageSamples,
	ThreadErasureClaims,
	Threads,
} from "./schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";

type WorkStatus = ThreadWorkItem["status"];
type OutboxKind =
	| "start"
	| "resume"
	| "steer"
	| "cancel"
	| "close"
	| "respond_approval"
	| "respond_question";

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
	readonly open_mode: "resume" | "start";
	readonly payload: CommandEnvelope["payload"];
	readonly resume_token?: typeof EngineResumeToken.Type;
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
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly CompleteOutbox: (command_id: string) => Effect.Effect<void, OrchestrationError>;
		readonly ClaimOutbox: (command_id: string) => Effect.Effect<boolean, OrchestrationError>;
		readonly FallbackSteering: (
			command_id: string,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
		readonly GetPending: () => Effect.Effect<ReadonlyArray<PendingWork>, OrchestrationError>;
		readonly GetWork: (
			thread_id: string,
		) => Effect.Effect<ThreadWorkItem | undefined, OrchestrationError>;
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
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const text_encoder = new TextEncoder();
		const OpaqueIdentity = (kind: string, parts: ReadonlyArray<string>) =>
			crypto
				.digest("SHA-256", text_encoder.encode(JSON.stringify([kind, ...parts])))
				.pipe(Effect.map((digest) => `${kind}:${Encoding.encodeHex(digest)}`));

		const ParsePersistedJson = (json: string) =>
			Effect.try({
				try: () => JSON.parse(json),
				catch: (cause) => new OrchestrationFailure({ cause }),
			});

		const ReadUsage = (transaction: typeof database.client, run_id: string) =>
			Effect.gen(function* () {
				const samples = yield* transaction
					.select()
					.from(RunUsageSamples)
					.where(eq(RunUsageSamples.run_id, run_id));
				const run_total = samples.find((sample) => sample.sample_scope === "run_total");
				const totals = run_total
					? run_total
					: samples.reduce(
							(total, sample) => ({
								input_tokens: total.input_tokens + sample.input_tokens,
								output_tokens: total.output_tokens + sample.output_tokens,
							}),
							{ input_tokens: 0, output_tokens: 0 },
						);

				return samples.length === 0 ||
					!Number.isSafeInteger(totals.input_tokens) ||
					!Number.isSafeInteger(totals.output_tokens)
					? undefined
					: { input_tokens: totals.input_tokens, output_tokens: totals.output_tokens };
			});

		const HasSafeTurnAggregate = (
			samples: ReadonlyArray<{
				readonly input_tokens: number;
				readonly output_tokens: number;
				readonly sample_scope: string;
			}>,
		) => {
			const totals = samples
				.filter((sample) => sample.sample_scope === "turn_total")
				.reduce(
					(total, sample) => ({
						input_tokens: total.input_tokens + sample.input_tokens,
						output_tokens: total.output_tokens + sample.output_tokens,
					}),
					{ input_tokens: 0, output_tokens: 0 },
				);

			return (
				Number.isSafeInteger(totals.input_tokens) &&
				Number.isSafeInteger(totals.output_tokens)
			);
		};

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
			Effect.gen(function* () {
				const [work] = yield* database.client
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
					.limit(1);

				if (!work) {
					return undefined;
				}

				const usage = yield* ReadUsage(database.client, work.run_id);
				const { native_thread_id, ...without_native_thread } = work;

				return {
					...without_native_thread,
					...(native_thread_id ? { native_thread_id } : {}),
					...(usage ? { usage } : {}),
					status: work.status as WorkStatus,
				} satisfies ThreadWorkItem;
			}).pipe(Effect.mapError(normalize_error));

		const Accept = (command: CommandEnvelope, can_steer: boolean) =>
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
											current_run && is_active_status(current_run.status)
												? "unsupported"
												: "no_active_run",
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
					native_resume_json: OrchestrationRuns.native_resume_json,
					native_thread_id: OrchestrationRuns.native_thread_id,
					open_mode: OrchestrationRuns.open_mode,
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
							rows.filter(
								(row) =>
									!["resume", "start"].includes(row.kind) ||
									row.status === "queued",
							),
							(row) =>
								Effect.gen(function* () {
									const value = yield* ParsePersistedJson(row.payload_json);
									const payload = yield* Schema.decodeUnknownEffect(
										CommandPayload,
									)(value).pipe(
										Effect.mapError(
											(cause) => new OrchestrationFailure({ cause }),
										),
									);

									if (row.open_mode !== "resume" && row.open_mode !== "start") {
										return yield* new OrchestrationFailure({
											cause: new Error("Stored run has an invalid open mode"),
										});
									}

									const open_mode = row.open_mode;
									const opening = row.kind === "resume" || row.kind === "start";

									if (
										opening &&
										row.kind !== (open_mode === "resume" ? "resume" : "start")
									) {
										return yield* new OrchestrationFailure({
											cause: new Error(
												"Stored run open mode does not match its outbox",
											),
										});
									}

									if (
										opening &&
										open_mode === "start" &&
										(row.native_resume_json !== null ||
											row.native_thread_id !== null)
									) {
										return yield* new OrchestrationFailure({
											cause: new Error(
												"Stored start run has unexpected resume state",
											),
										});
									}

									const resume_token =
										opening && open_mode === "resume"
											? yield* Effect.gen(function* () {
													if (row.native_resume_json === null) {
														return yield* new OrchestrationFailure({
															cause: new Error(
																"Stored resume run has no token",
															),
														});
													}

													const stored = yield* ParsePersistedJson(
														row.native_resume_json,
													);
													const token = yield* Schema.decodeUnknownEffect(
														EngineResumeToken,
														{ onExcessProperty: "error" },
													)(stored).pipe(
														Effect.mapError(
															(cause) =>
																new OrchestrationFailure({ cause }),
														),
													);

													if (
														token.native_thread_id !==
														row.native_thread_id
													) {
														return yield* new OrchestrationFailure({
															cause: new Error(
																"Stored resume token does not match its native thread",
															),
														});
													}

													return token;
												})
											: undefined;

									return {
										agent_id: row.agent_id,
										command_id: row.command_id,
										engine_id: row.engine_id,
										kind: row.kind as OutboxKind,
										open_mode,
										payload,
										...(resume_token === undefined ? {} : { resume_token }),
										run_id: row.run_id,
										thread_id: row.thread_id,
										working_directory: row.working_directory,
									} satisfies PendingWork;
								}),
						),
					),
					Effect.mapError(normalize_error),
				);

		const CompleteOutbox = (command_id: string) =>
			Effect.gen(function* () {
				const updated_at = yield* metadata.Now;

				yield* database.client
					.update(OrchestrationOutbox)
					.set({ status: "delivered", updated_at })
					.where(eq(OrchestrationOutbox.command_id, command_id));
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
				const validated_resume_token = yield* Schema.decodeUnknownEffect(
					EngineResumeToken,
					{ onExcessProperty: "error" },
				)(resume_token).pipe(
					Effect.mapError((cause) => new OrchestrationFailure({ cause })),
				);

				if (validated_resume_token.native_thread_id !== native_thread_id) {
					return yield* new OrchestrationFailure({
						cause: new Error("Engine resume token does not match its native thread"),
					});
				}

				const native_resume_json = JSON.stringify(validated_resume_token);
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
						native_resume_json,
						native_thread_id,
						updated_at,
					})
					.where(eq(OrchestrationRuns.run_id, run_id));
				yield* database.client
					.update(OrchestrationCoordinators)
					.set({
						native_resume_json,
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

		const FallbackSteering = (command_id: string) =>
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
									reason: "steering_rejected",
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
				const write = database.client.transaction((transaction) =>
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

						if (observation._tag === "usage") {
							const { input_tokens, output_tokens } = observation;
							const scope_key =
								observation.sample_scope === "run_total"
									? "run"
									: observation.turn_id
										? `turn:${observation.turn_id}`
										: undefined;

							if (
								!run ||
								!scope_key ||
								input_tokens === undefined ||
								output_tokens === undefined ||
								!Number.isSafeInteger(input_tokens) ||
								!Number.isSafeInteger(output_tokens) ||
								input_tokens < 0 ||
								output_tokens < 0
							) {
								return [];
							}

							const samples = yield* transaction
								.select()
								.from(RunUsageSamples)
								.where(eq(RunUsageSamples.run_id, run.run_id));
							const existing = samples.find(
								(sample) => sample.scope_key === scope_key,
							);
							const changed =
								!existing ||
								input_tokens > existing.input_tokens ||
								output_tokens > existing.output_tokens;

							if (!changed) {
								return [];
							}
							const candidate_samples = existing
								? samples.map((sample) =>
										sample.scope_key === scope_key
											? {
													...sample,
													input_tokens: Math.max(
														sample.input_tokens,
														input_tokens,
													),
													output_tokens: Math.max(
														sample.output_tokens,
														output_tokens,
													),
												}
											: sample,
									)
								: [
										...samples,
										{
											input_tokens,
											output_tokens,
											run_id: run.run_id,
											sample_scope: observation.sample_scope,
											scope_key,
											updated_at: "",
										},
									];

							if (!HasSafeTurnAggregate(candidate_samples)) {
								return [];
							}

							const updated_at = yield* metadata.Now;

							yield* transaction
								.insert(RunUsageSamples)
								.values({
									input_tokens,
									output_tokens,
									run_id: run.run_id,
									sample_scope: observation.sample_scope,
									scope_key,
									updated_at,
								})
								.onConflictDoUpdate({
									target: [RunUsageSamples.run_id, RunUsageSamples.scope_key],
									set: {
										input_tokens: sql`MAX(${RunUsageSamples.input_tokens}, excluded.input_tokens)`,
										output_tokens: sql`MAX(${RunUsageSamples.output_tokens}, excluded.output_tokens)`,
										updated_at,
									},
								});

							const usage = yield* ReadUsage(transaction, run.run_id);

							if (!usage) {
								return [];
							}

							const observation_id = yield* OpaqueIdentity("engine_observation", [
								run.run_id,
								observation.observation_id,
							]);
							const raw_origin_provider = yield* OpaqueIdentity("engine", [
								run.engine_id,
							]);

							return [
								yield* AppendEvent(transaction, {
									agent_id: run.agent_id,
									causation_id: observation_id,
									correlation_id: run.run_id,
									payload: { type: "run.usage.updated", usage },
									raw_origin: {
										provider: raw_origin_provider,
										reference: observation_id,
									},
									run_id: run.run_id,
									thread_id: run.thread_id,
								}),
							];
						}

						if (
							!run ||
							observation._tag === "agent_message_delta" ||
							!is_projectable_status(run.status)
						) {
							return [];
						}

						const observation_id = yield* OpaqueIdentity("engine_observation", [
							observation.artisan_run_id,
							observation.observation_id,
						]);
						const raw_origin_provider = yield* OpaqueIdentity("engine", [
							run.engine_id,
						]);

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
												: observation._tag === "tool"
													? ({
															effect: "unknown",
															invocation_id: yield* OpaqueIdentity(
																"engine_tool",
																[
																	run.run_id,
																	run.engine_id,
																	observation.tool_id,
																],
															),
															label: "Engine tool",
															source: "engine",
															state: observation.action,
															type: "capability.invocation.updated",
														} satisfies EventPayload)
													: observation._tag === "native_action"
														? ({
																action_id: observation_id,
																effect: "unknown",
																label: "Engine native action",
																source: "engine",
																state: "observed",
																type: "engine.native_action.observed",
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
								causation_id: observation_id,
								correlation_id: run.run_id,
								payload,
								raw_origin: {
									provider: raw_origin_provider,
									reference: observation_id,
								},
								run_id: run.run_id,
								thread_id: run.thread_id,
							}),
						];
					}),
				);
				const result = yield* observation._tag === "usage"
					? RetrySqliteWrite(write)
					: write;

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
			GetWork,
			MarkInterrupted,
			MarkOutboxUndeliverable,
			MarkRunStarted,
			PersistNativeRun,
			RecordObservation,
		};
	}),
);
