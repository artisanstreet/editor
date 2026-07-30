import { desc, eq } from "drizzle-orm";
import { Effect, Schema } from "effect";
import {
	type CommandEnvelope,
	type EventEnvelope,
	Project,
	type ProjectRef,
	type ThreadMessageRoutedEvent,
} from "@artisan/protocol";
import { RuntimeMetadata } from "../../runtime/metadata";
import { Database } from "../database";
import { JournalNotifier } from "../journal-notifier";
import { JournalCommands, JournalEvents, OrchestrationOutbox, Projects, Threads } from "../tables";
import {
	CanonicaliseClientMessageIntent,
	SanitisePayload,
	ValidateImageAttachments,
} from "./message-attachments";
import {
	OrchestrationCommandConflict,
	OrchestrationFailure,
	OrchestrationNotFound,
	OrchestrationProjectAuthorityError,
	type OrchestrationError,
} from "./contracts";
import {
	AuthoritativeThreadSendMessageCommand,
	type InboundOrAuthoritativeCommandEnvelope,
} from "./message-command";
import { DecodePersistedJson, PersistedEventPayload, PersistedRawOrigin } from "./storage-codec";
import type { IntakeAssessment } from "../../orchestration/intake-policy";
import { CommandTransaction, MakeCommandDispatcher } from "./command-dispatch";

function normalize_error(error: unknown): OrchestrationError {
	if (
		error instanceof OrchestrationCommandConflict ||
		error instanceof OrchestrationNotFound ||
		error instanceof OrchestrationProjectAuthorityError
	)
		return error;
	return new OrchestrationFailure({ cause: error });
}
type AuthoritativePayload =
	| (CommandEnvelope["payload"] & {
			readonly type: Exclude<CommandEnvelope["payload"]["type"], "thread.send_message">;
	  })
	| AuthoritativeThreadSendMessageCommand;

export const MakeCommandAcceptor = Effect.gen(function* () {
	const database = yield* Database;
	const metadata = yield* RuntimeMetadata;
	const notifier = yield* JournalNotifier;
	const DispatchCommand = yield* MakeCommandDispatcher;
	const GetJournalSequence = (transaction: typeof database.client) =>
		transaction
			.select({ journal_sequence: JournalEvents.sequence })
			.from(JournalEvents)
			.orderBy(desc(JournalEvents.sequence))
			.limit(1)
			.pipe(Effect.map(([event]) => event?.journal_sequence ?? 0));
	const AcceptCommand = (
		command: InboundOrAuthoritativeCommandEnvelope,
		can_steer: boolean,
		intake?: IntakeAssessment,
		routing_reason?: ThreadMessageRoutedEvent["reason"],
		client_intent = false,
	) =>
		Effect.gen(function* () {
			const payload_json = yield* Effect.try({
				try: () => {
					ValidateImageAttachments(command.payload);
					return JSON.stringify(
						client_intent
							? CanonicaliseClientMessageIntent(
									command.payload as CommandEnvelope["payload"],
								)
							: SanitisePayload(command.payload),
					);
				},
				catch: (cause) => new OrchestrationFailure({ cause }),
			});
			const raw_origin_json = command.raw_origin ? JSON.stringify(command.raw_origin) : null;

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
								DecodePersistedJson(PersistedEventPayload, event.payload_json).pipe(
									Effect.bindTo("payload"),
									Effect.bind("raw_origin", () =>
										event.raw_origin_json
											? DecodePersistedJson(
													PersistedRawOrigin,
													event.raw_origin_json,
												)
											: Effect.succeed(undefined),
									),
									Effect.flatMap(({ payload, raw_origin }) =>
										event.agent_id === null
											? Effect.fail(
													new OrchestrationFailure({
														cause: new Error(
															`Accepted event ${event.event_id} has no agent identity`,
														),
													}),
												)
											: Effect.succeed({
													agent_id: event.agent_id,
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
												} satisfies EventEnvelope),
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
						.select({
							primary_project_id: Threads.primary_project_id,
							thread_id: Threads.thread_id,
						})
						.from(Threads)
						.where(eq(Threads.thread_id, command.thread_id))
						.limit(1);

					if (!thread) {
						return yield* new OrchestrationNotFound({
							id: command.thread_id,
							resource: "thread",
						});
					}

					const incoming_payload = command.payload;
					const payload: AuthoritativePayload =
						incoming_payload.type !== "thread.send_message"
							? incoming_payload
							: "working_directory" in incoming_payload
								? yield* Schema.decodeUnknownEffect(
										AuthoritativeThreadSendMessageCommand,
									)(incoming_payload).pipe(
										Effect.mapError(
											(cause) => new OrchestrationFailure({ cause }),
										),
									)
								: yield* Effect.gen(function* () {
										if (thread.primary_project_id === null) {
											return yield* new OrchestrationProjectAuthorityError({
												reason: "thread_unassigned",
												thread_id: command.thread_id,
											});
										}

										const [project_row] = yield* transaction
											.select()
											.from(Projects)
											.where(
												eq(Projects.project_id, thread.primary_project_id),
											)
											.limit(1);
										if (!project_row) {
											return yield* new OrchestrationProjectAuthorityError({
												project_id: thread.primary_project_id,
												reason: "project_detached",
												thread_id: command.thread_id,
											});
										}

										const project = yield* Schema.decodeUnknownEffect(Project)(
											project_row,
										).pipe(
											Effect.mapError(
												(cause) => new OrchestrationFailure({ cause }),
											),
										);
										const project_ref: ProjectRef = {
											display_name: project.display_name,
											project_id: project.project_id,
											root_path: project.root_path,
										};
										const attachments = yield* Effect.forEach(
											incoming_payload.attachments ?? [],
											(attachment) =>
												metadata.MakeId("attachment").pipe(
													Effect.map((id) => ({
														bytes: attachment.bytes,
														id,
														media_type: attachment.media_type,
														name: attachment.name,
														token: attachment.client_token,
													})),
												),
										);
										const attachment_ids = new Map(
											attachments.map((attachment) => [
												attachment.token,
												attachment.id,
											]),
										);
										const content =
											incoming_payload.content === undefined
												? undefined
												: yield* Effect.forEach(
														incoming_payload.content,
														(part) =>
															Effect.gen(function* () {
																if (part.type === "text") {
																	return part;
																}
																const attachment_id =
																	attachment_ids.get(
																		part.client_token,
																	);
																if (attachment_id === undefined) {
																	return yield* new OrchestrationFailure(
																		{
																			cause: new Error(
																				`Content references unknown attachment ${part.client_token}`,
																			),
																		},
																	);
																}
																return {
																	attachment_id,
																	type: "image" as const,
																};
															}),
													);

										return yield* Schema.decodeUnknownEffect(
											AuthoritativeThreadSendMessageCommand,
										)({
											...incoming_payload,
											...(attachments.length === 0
												? {}
												: {
														attachments: attachments.map(
															({ token: _token, ...attachment }) =>
																attachment,
														),
													}),
											...(content === undefined ? {} : { content }),
											mentioned_projects: [project_ref],
											working_directory: project.root_path,
										}).pipe(
											Effect.mapError(
												(cause) => new OrchestrationFailure({ cause }),
											),
										);
									});

					return yield* DispatchCommand({
						can_steer,
						command,
						intake,
						payload,
						payload_json,
						raw_origin_json,
						routing_reason,
					}).pipe(Effect.provideService(CommandTransaction, { client: transaction }));
				}),
			);

			if (acceptance.status === "accepted" && acceptance.journal_sequence > 0) {
				yield* notifier.Publish(acceptance.journal_sequence);
			}

			return acceptance;
		}).pipe(Effect.mapError(normalize_error));

	return AcceptCommand;
});
