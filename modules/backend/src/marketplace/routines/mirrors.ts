import { eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";
import { MarketplaceLedgerEvent, ProviderSyncState, RoutineDetail } from "@artisan/protocol";
import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	MarketplaceRoutineMirrors,
	MarketplaceRoutineOperations,
	MarketplaceRoutines,
} from "../../persistence/tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	marketplace_routine_stream_id,
	marketplace_routine_thread_id,
	mirror_operation_lease_milliseconds,
	RoutineRepositoryError,
	type RoutineMirrorOperation,
	type RoutineRepositoryApi,
} from "./contracts";

export class RoutineMirrors extends Context.Service<
	RoutineMirrors,
	Pick<
		RoutineRepositoryApi,
		"ClaimMirrorOperation" | "CommitMirrorOperation" | "ReleaseMirrorOperation"
	>
>()("Artisan/Marketplace/Routines/RoutineMirrors") {}

export const RoutineMirrorsLive = Layer.effect(
	RoutineMirrors,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ClaimMirrorOperation = (input: RoutineMirrorOperation) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const kind = input.kind === "sync" ? "mirror_sync" : "mirror_drift";
						const intent_json = JSON.stringify({ engine_id: input.engine_id });
						const [existing] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							)
							.limit(1);
						if (existing) {
							if (
								existing.kind !== kind ||
								existing.preview_json !== intent_json ||
								existing.request_fingerprint !== input.intent_fingerprint ||
								existing.routine_id !== input.routine_id
							)
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message:
										"Routine mirror operation id was reused with different intent",
								});
							if (existing.state === "claimed") {
								const now = yield* metadata.Now;
								const lease_expired =
									existing.dispatch_lease_expires_at === null ||
									Date.parse(existing.dispatch_lease_expires_at) <=
										Date.parse(now);
								if (!lease_expired) return { _tag: "InFlight" as const };
								yield* transaction
									.update(MarketplaceRoutineOperations)
									.set({
										dispatch_lease_expires_at: new Date(
											Date.parse(now) + mirror_operation_lease_milliseconds,
										).toISOString(),
										dispatch_owner_id: metadata.instance_id,
										updated_at: now,
									})
									.where(
										eq(
											MarketplaceRoutineOperations.operation_id,
											input.operation_id,
										),
									);
								return { _tag: "Claimed" as const };
							}
							if (existing.state !== "completed")
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message:
										"Routine mirror operation has an invalid durable state",
								});
							const [event] = yield* transaction
								.select({ journal_sequence: JournalEvents.sequence })
								.from(JournalEvents)
								.where(eq(JournalEvents.correlation_id, input.operation_id))
								.limit(1);
							if (!event)
								return yield* new RoutineRepositoryError({
									code: "invariant",
									message: "Completed routine mirror operation has no event",
								});
							return {
								_tag: "Completed" as const,
								journal_sequence: event.journal_sequence,
							};
						}
						const [routine] = yield* transaction
							.select({ id: MarketplaceRoutines.id })
							.from(MarketplaceRoutines)
							.where(eq(MarketplaceRoutines.id, input.routine_id))
							.limit(1);
						if (!routine)
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: `Routine ${input.routine_id} was not found`,
							});
						const occurred_at = yield* metadata.Now;
						yield* transaction.insert(MarketplaceRoutineOperations).values({
							created_at: occurred_at,
							kind,
							dispatch_lease_expires_at: new Date(
								Date.parse(occurred_at) + mirror_operation_lease_milliseconds,
							).toISOString(),
							dispatch_owner_id: metadata.instance_id,
							operation_id: input.operation_id,
							preview_json: intent_json,
							request_fingerprint: input.intent_fingerprint,
							routine_id: input.routine_id,
							state: "claimed",
							updated_at: occurred_at,
						});
						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: input.operation_id,
							origin: "backend",
							payload_json: JSON.stringify({
								intent_fingerprint: input.intent_fingerprint,
							}),
							payload_type: `marketplace.routine.${input.kind}`,
							schema_version: 1,
							sent_at: occurred_at,
							status: "accepted",
							thread_id: marketplace_routine_thread_id,
						});
						return { _tag: "Claimed" as const };
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine mirror claim could not be persisted",
								}),
					),
				);

		const CommitMirrorOperation = (input: {
			readonly imported?: RoutineDetail;
			readonly operation_id: string;
			readonly state: ProviderSyncState;
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
							(operation.kind !== "mirror_sync" &&
								operation.kind !== "mirror_drift") ||
							!operation.routine_id
						)
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: "Routine mirror claim was not found",
							});
						if (operation.state === "completed") {
							const [event] = yield* transaction
								.select({ journal_sequence: JournalEvents.sequence })
								.from(JournalEvents)
								.where(eq(JournalEvents.correlation_id, input.operation_id))
								.limit(1);
							if (!event)
								return yield* new RoutineRepositoryError({
									code: "invariant",
									message: "Completed mirror operation has no event",
								});
							return event.journal_sequence;
						}
						if (operation.state !== "claimed")
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine mirror operation is not claimed",
							});
						if (operation.dispatch_owner_id !== metadata.instance_id)
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Routine mirror operation lease is owned by another runtime",
							});
						const [routine] = yield* transaction
							.select()
							.from(MarketplaceRoutines)
							.where(eq(MarketplaceRoutines.id, operation.routine_id))
							.limit(1);
						if (!routine)
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: `Routine ${operation.routine_id} was not found`,
							});
						if (input.imported !== undefined) {
							yield* transaction
								.update(MarketplaceRoutines)
								.set({
									author: input.imported.author ?? null,
									commands_json: JSON.stringify(input.imported.exported_commands),
									compatibility_json: JSON.stringify(
										input.imported.compatibility,
									),
									description: input.imported.description,
									display_name: input.imported.display_name,
									enabled: input.imported.enabled,
									files_json: JSON.stringify(input.imported.files),
									instructions: input.imported.instructions,
									permissions_json: JSON.stringify(input.imported.permissions),
									scope_json: JSON.stringify(input.imported.scope),
									source_json: JSON.stringify(input.imported.source),
									status: input.imported.status,
									trust: input.imported.trust,
									updated_at: input.state.updated_at,
									version: input.imported.version,
								})
								.where(eq(MarketplaceRoutines.id, operation.routine_id));
						}
						yield* transaction
							.insert(MarketplaceRoutineMirrors)
							.values({
								engine_id: input.state.engine_id,
								last_error_code: input.state.last_error_code ?? null,
								observed_revision: input.state.observed_revision ?? null,
								routine_id: operation.routine_id,
								status: input.state.status,
								updated_at: input.state.updated_at,
							})
							.onConflictDoUpdate({
								target: [
									MarketplaceRoutineMirrors.routine_id,
									MarketplaceRoutineMirrors.engine_id,
								],
								set: {
									last_error_code: input.state.last_error_code ?? null,
									observed_revision: input.state.observed_revision ?? null,
									status: input.state.status,
									updated_at: input.state.updated_at,
								},
							});
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id))
							.limit(1);
						const stream_sequence = (stream?.last_sequence ?? 0) + 1;
						const payload = {
							item_id: operation.routine_id,
							item_kind: "routine" as const,
							operation:
								operation.kind === "mirror_sync"
									? ("synced" as const)
									: ("drift_resolved" as const),
							status: routine.status,
							sync_status: input.state.status,
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine mirror event is invalid",
									}),
							),
						);
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({
								dispatch_lease_expires_at: null,
								dispatch_owner_id: null,
								state: "completed",
								updated_at: input.state.updated_at,
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
								correlation_id: input.operation_id,
								event_id: `${yield* metadata.MakeId("event")}:mirror`,
								event_type: payload.type,
								occurred_at: input.state.updated_at,
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
								message: "Routine mirror event was not returned",
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
									message: "Routine mirror completion could not be persisted",
								}),
					),
				);

		const ReleaseMirrorOperation = (operation_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(eq(MarketplaceRoutineOperations.operation_id, operation_id))
							.limit(1);
						if (
							!operation ||
							(operation.kind !== "mirror_sync" &&
								operation.kind !== "mirror_drift") ||
							operation.state !== "claimed"
						)
							return;
						if (operation.dispatch_owner_id !== metadata.instance_id)
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Routine mirror operation lease is owned by another runtime",
							});
						const updated_at = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({
								dispatch_lease_expires_at: null,
								dispatch_owner_id: null,
								updated_at,
							})
							.where(eq(MarketplaceRoutineOperations.operation_id, operation_id));
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine mirror operation lease could not be released",
								}),
					),
				);

		return { ClaimMirrorOperation, CommitMirrorOperation, ReleaseMirrorOperation };
	}),
);
