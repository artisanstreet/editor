import { eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";
import { MarketplaceLedgerEvent, RoutineDetail } from "@artisan/protocol";
import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	MarketplaceRoutineOperations,
	MarketplaceRoutines,
} from "../../persistence/tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	marketplace_routine_stream_id,
	marketplace_routine_thread_id,
	RoutineRepositoryError,
	type RoutineRepositoryApi,
} from "./contracts";

export class RoutineInstallation extends Context.Service<
	RoutineInstallation,
	Pick<RoutineRepositoryApi, "CommitInstalled" | "Transition" | "RecordInstallFailure">
>()("Artisan/Marketplace/Routines/RoutineInstallation") {}

export const RoutineInstallationLive = Layer.effect(
	RoutineInstallation,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const CommitInstalled = (input: {
			readonly artifact_refs: ReadonlyArray<string>;
			readonly detail: RoutineDetail;
			readonly operation_id: string;
			readonly rollback_json?: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							)
							.limit(1);
						if (
							!operation ||
							(operation.state !== "approved" && operation.state !== "installed")
						) {
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine install is not durably approved",
							});
						}
						if (operation.routine_id !== input.detail.id) {
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Approved routine identity does not match installed routine",
							});
						}
						if (operation.state === "installed") {
							const [event] = yield* transaction
								.select({ journal_sequence: JournalEvents.sequence })
								.from(JournalEvents)
								.where(
									eq(
										JournalEvents.correlation_id,
										`${input.operation_id}:installed`,
									),
								)
								.limit(1);
							if (!event)
								return yield* new RoutineRepositoryError({
									code: "invariant",
									message: "Installed routine has no canonical event",
								});
							return event.journal_sequence;
						}
						const occurred_at = yield* metadata.Now;
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;
						const event_id = yield* metadata.MakeId("event");
						const payload = {
							approval_id: operation.approval_id ?? undefined,
							artifact_id: input.artifact_refs.at(0),
							item_id: input.detail.id,
							item_kind: "routine" as const,
							operation: "installed" as const,
							status: input.detail.status,
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine installed event is invalid",
									}),
							),
						);
						yield* transaction.insert(MarketplaceRoutines).values({
							artifact_refs_json: JSON.stringify(input.artifact_refs),
							author: input.detail.author ?? null,
							commands_json: JSON.stringify(input.detail.exported_commands),
							compatibility_json: JSON.stringify(input.detail.compatibility),
							created_at: occurred_at,
							description: input.detail.description,
							display_name: input.detail.display_name,
							enabled: input.detail.enabled,
							files_json: JSON.stringify(input.detail.files),
							id: input.detail.id,
							instructions: input.detail.instructions,
							permissions_json: JSON.stringify(input.detail.permissions),
							removed_at: input.detail.removed_at ?? null,
							scope_json: JSON.stringify(input.detail.scope),
							source_json: JSON.stringify(input.detail.source),
							status: input.detail.status,
							trust: input.detail.trust,
							updated_at: occurred_at,
							version: input.detail.version,
						});
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({
								rollback_json: input.rollback_json ?? null,
								state: "installed",
								updated_at: occurred_at,
							})
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							);
						if (stream) {
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: sequence })
								.where(eq(EventStreams.stream_id, marketplace_routine_stream_id));
						} else {
							yield* transaction.insert(EventStreams).values({
								last_sequence: sequence,
								stream_id: marketplace_routine_stream_id,
							});
						}
						const [event] = yield* transaction
							.insert(JournalEvents)
							.values({
								causation_id: input.operation_id,
								correlation_id: `${input.operation_id}:installed`,
								event_id,
								event_type: payload.type,
								occurred_at,
								origin: "backend",
								payload_json: JSON.stringify(payload),
								schema_version: 1,
								stream_id: marketplace_routine_stream_id,
								stream_sequence: sequence,
								thread_id: marketplace_routine_thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						if (!event)
							return yield* new RoutineRepositoryError({
								code: "invariant",
								message: "Routine install event was not returned",
							});
						return event.journal_sequence;
					}),
				)
				.pipe(
					Effect.tap((journal_sequence) => notifier.Publish(journal_sequence)),
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine installation could not be committed",
								}),
					),
				);

		const Transition = (input: {
			readonly enabled: boolean;
			readonly operation: "enabled" | "disabled" | "invoked" | "removed";
			readonly operation_id: string;
			readonly routine_id: string;
			readonly status: RoutineDetail["status"];
			readonly tool_name?: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [routine] = yield* transaction
							.select()
							.from(MarketplaceRoutines)
							.where(eq(MarketplaceRoutines.id, input.routine_id))
							.limit(1);
						if (!routine) {
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: `Routine ${input.routine_id} was not found`,
							});
						}
						const occurred_at = yield* metadata.Now;
						const event_id = yield* metadata.MakeId("event");
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;
						const payload = {
							item_id: input.routine_id,
							item_kind: "routine" as const,
							operation: input.operation,
							status: input.status,
							...(input.tool_name === undefined
								? {}
								: { tool_name: input.tool_name }),
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine lifecycle event is invalid",
									}),
							),
						);
						const payload_json = JSON.stringify(payload);
						const [existing_command] = yield* transaction
							.select()
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, input.operation_id))
							.limit(1);
						if (existing_command) {
							if (
								existing_command.thread_id !== marketplace_routine_thread_id ||
								existing_command.payload_json !== payload_json
							) {
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message:
										"Routine lifecycle operation id was reused with different intent",
								});
							}
							const [existing_event] = yield* transaction
								.select({ journal_sequence: JournalEvents.sequence })
								.from(JournalEvents)
								.where(eq(JournalEvents.correlation_id, input.operation_id))
								.limit(1);
							if (!existing_event) {
								return yield* new RoutineRepositoryError({
									code: "invariant",
									message: "Routine lifecycle command has no canonical event",
								});
							}
							return existing_event.journal_sequence;
						}
						yield* transaction
							.update(MarketplaceRoutines)
							.set({
								enabled: input.enabled,
								removed_at:
									input.operation === "removed"
										? occurred_at
										: routine.removed_at,
								status: input.status,
								updated_at: occurred_at,
							})
							.where(eq(MarketplaceRoutines.id, input.routine_id));
						if (stream) {
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: sequence })
								.where(eq(EventStreams.stream_id, marketplace_routine_stream_id));
						} else {
							yield* transaction.insert(EventStreams).values({
								last_sequence: sequence,
								stream_id: marketplace_routine_stream_id,
							});
						}
						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: input.operation_id,
							origin: "backend",
							payload_json,
							payload_type: "marketplace.routine.lifecycle",
							schema_version: 1,
							sent_at: occurred_at,
							status: "accepted",
							thread_id: marketplace_routine_thread_id,
						});
						const [event] = yield* transaction
							.insert(JournalEvents)
							.values({
								causation_id: input.operation_id,
								correlation_id: input.operation_id,
								event_id,
								event_type: payload.type,
								occurred_at,
								origin: "backend",
								payload_json: JSON.stringify(payload),
								schema_version: 1,
								stream_id: marketplace_routine_stream_id,
								stream_sequence: sequence,
								thread_id: marketplace_routine_thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						if (!event)
							return yield* new RoutineRepositoryError({
								code: "invariant",
								message: "Routine lifecycle event was not returned",
							});
						return event.journal_sequence;
					}),
				)
				.pipe(
					Effect.tap((journal_sequence) => notifier.Publish(journal_sequence)),
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine lifecycle could not be committed",
								}),
					),
				);

		const RecordInstallFailure = (input: {
			readonly code: "conflict" | "install_failed" | "rollback_failed";
			readonly operation_id: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							)
							.limit(1);
						if (!operation || operation.kind !== "install" || !operation.routine_id)
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: "Routine install operation was not found",
							});
						if (operation.state === "failed") {
							if (operation.failure_code !== input.code)
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message:
										"Routine install failure was retried with a different cause",
								});
							const [event] = yield* transaction
								.select({ journal_sequence: JournalEvents.sequence })
								.from(JournalEvents)
								.where(
									eq(
										JournalEvents.correlation_id,
										`${input.operation_id}:failed`,
									),
								)
								.limit(1);
							if (!event)
								return yield* new RoutineRepositoryError({
									code: "invariant",
									message: "Failed routine install has no lifecycle event",
								});
							return event.journal_sequence;
						}
						if (operation.state !== "approved")
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Routine install cannot transition to failure from its current state",
							});
						const occurred_at = yield* metadata.Now;
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id))
							.limit(1);
						const stream_sequence = (stream?.last_sequence ?? 0) + 1;
						const payload = {
							item_id: operation.routine_id,
							item_kind: "routine" as const,
							operation: "install_failed" as const,
							status: "failed" as const,
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine failure event is invalid",
									}),
							),
						);
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({
								failure_code: input.code,
								state: "failed",
								updated_at: occurred_at,
							})
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							);
						if (stream)
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: stream_sequence })
								.where(eq(EventStreams.stream_id, marketplace_routine_stream_id));
						else
							yield* transaction.insert(EventStreams).values({
								last_sequence: stream_sequence,
								stream_id: marketplace_routine_stream_id,
							});
						const [event] = yield* transaction
							.insert(JournalEvents)
							.values({
								causation_id: input.operation_id,
								correlation_id: `${input.operation_id}:failed`,
								event_id: `${yield* metadata.MakeId("event")}:failed`,
								event_type: payload.type,
								occurred_at,
								origin: "backend",
								payload_json: JSON.stringify(payload),
								schema_version: 1,
								stream_id: marketplace_routine_stream_id,
								stream_sequence,
								thread_id: marketplace_routine_thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						if (!event)
							return yield* new RoutineRepositoryError({
								code: "invariant",
								message: "Routine failure event was not returned",
							});
						return event.journal_sequence;
					}),
				)
				.pipe(
					Effect.tap((sequence) => notifier.Publish(sequence)),
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine install failure could not be committed",
								}),
					),
				);

		return { CommitInstalled, Transition, RecordInstallFailure };
	}),
);
