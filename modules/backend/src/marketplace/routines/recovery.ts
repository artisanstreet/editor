import { and, asc, eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";
import { MarketplaceLedgerEvent } from "@artisan/protocol";
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
	type RoutineRollbackRecovery,
} from "./contracts";

export class RoutineRecoveryOperations extends Context.Service<
	RoutineRecoveryOperations,
	Pick<
		RoutineRepositoryApi,
		"ReadRecovery" | "ClaimRollback" | "CommitRollback" | "ReadRollbackRecovery"
	>
>()("Artisan/Marketplace/Routines/RoutineRecoveryOperations") {}

export const RoutineRecoveryOperationsLive = Layer.effect(
	RoutineRecoveryOperations,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ReadRecovery = Effect.gen(function* () {
			const rows = yield* database.client
				.select()
				.from(MarketplaceRoutineOperations)
				.orderBy(asc(MarketplaceRoutineOperations.created_at));
			return yield* Effect.forEach(
				rows.filter(
					(row) =>
						(row.state === "approved" || row.state === "installed") &&
						row.routine_id !== null,
				),
				(row) =>
					Schema.decodeUnknownEffect(
						Schema.Struct({
							operation_id: Schema.NonEmptyString,
							routine_id: Schema.NonEmptyString,
							state: Schema.Literals(["approved", "installed"]),
						}),
					)({
						operation_id: row.operation_id,
						routine_id: row.routine_id,
						state: row.state,
					}),
			);
		}).pipe(
			Effect.mapError(
				() =>
					new RoutineRepositoryError({
						code: "invariant",
						message: "Routine operation recovery rows are corrupt",
					}),
			),
		);

		const ClaimRollback = (input: RoutineRollbackRecovery) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const rollback_json = JSON.stringify({ rollback_id: input.rollback_id });
						const request_fingerprint = `${input.routine_id}:${input.rollback_id}`;
						const [existing] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							)
							.limit(1);
						if (existing) {
							if (
								existing.kind !== "rollback" ||
								existing.routine_id !== input.routine_id ||
								existing.rollback_json !== rollback_json ||
								existing.request_fingerprint !== request_fingerprint
							)
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message: "Routine rollback id was reused with different intent",
								});
							if (existing.state === "rolled_back") return "completed" as const;
							if (existing.state === "claimed") return "claimed" as const;
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine rollback claim has an invalid durable state",
							});
						}
						const [routine] = yield* transaction
							.select({
								id: MarketplaceRoutines.id,
								status: MarketplaceRoutines.status,
							})
							.from(MarketplaceRoutines)
							.where(eq(MarketplaceRoutines.id, input.routine_id))
							.limit(1);
						if (!routine)
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: `Routine ${input.routine_id} was not found`,
							});
						if (routine.status === "removed" || routine.status === "rolled_back")
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Terminal routines cannot start a new rollback",
							});
						const prior_rollbacks = yield* transaction
							.select({ rollback_json: MarketplaceRoutineOperations.rollback_json })
							.from(MarketplaceRoutineOperations)
							.where(
								and(
									eq(MarketplaceRoutineOperations.kind, "rollback"),
									eq(MarketplaceRoutineOperations.routine_id, input.routine_id),
								),
							);
						if (
							prior_rollbacks.some(
								(operation) => operation.rollback_json === rollback_json,
							)
						)
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Routine rollback receipt is already claimed by another operation",
							});
						const occurred_at = yield* metadata.Now;
						yield* transaction.insert(MarketplaceRoutineOperations).values({
							approval_decision: "approved",
							created_at: occurred_at,
							kind: "rollback",
							operation_id: input.operation_id,
							request_fingerprint,
							rollback_json,
							routine_id: input.routine_id,
							state: "claimed",
							updated_at: occurred_at,
						});
						const install_operations = yield* transaction
							.select({ rollback_json: MarketplaceRoutineOperations.rollback_json })
							.from(MarketplaceRoutineOperations)
							.where(
								and(
									eq(MarketplaceRoutineOperations.kind, "install"),
									eq(MarketplaceRoutineOperations.routine_id, input.routine_id),
									eq(MarketplaceRoutineOperations.state, "installed"),
								),
							);
						const persisted_receipts = yield* Effect.forEach(
							install_operations.filter(
								(operation) => operation.rollback_json !== null,
							),
							(operation) =>
								Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
									operation.rollback_json,
								).pipe(
									Effect.flatMap(
										Schema.decodeUnknownEffect(
											Schema.Struct({ rollback_id: Schema.NonEmptyString }),
										),
									),
									Effect.mapError(
										() =>
											new RoutineRepositoryError({
												code: "invariant",
												message:
													"Installed routine rollback receipt is corrupt",
											}),
									),
								),
						);
						if (
							!persisted_receipts.some(
								(receipt) => receipt.rollback_id === input.rollback_id,
							)
						)
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Rollback request does not match the installed routine receipt",
							});
						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: input.operation_id,
							origin: "backend",
							payload_json: JSON.stringify({ request_fingerprint }),
							payload_type: "marketplace.routine.rollback",
							schema_version: 1,
							sent_at: occurred_at,
							status: "accepted",
							thread_id: marketplace_routine_thread_id,
						});
						return "claimed" as const;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine rollback claim could not be persisted",
								}),
					),
				);

		const CommitRollback = (operation_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(eq(MarketplaceRoutineOperations.operation_id, operation_id))
							.limit(1);
						if (!operation || operation.kind !== "rollback" || !operation.routine_id)
							return yield* new RoutineRepositoryError({
								code: "not_found",
								message: "Routine rollback claim was not found",
							});
						if (operation.state === "rolled_back") {
							const [event] = yield* transaction
								.select({ journal_sequence: JournalEvents.sequence })
								.from(JournalEvents)
								.where(eq(JournalEvents.correlation_id, operation_id))
								.limit(1);
							if (!event)
								return yield* new RoutineRepositoryError({
									code: "invariant",
									message: "Completed rollback has no lifecycle event",
								});
							return event.journal_sequence;
						}
						if (operation.state !== "claimed")
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine rollback claim is not recoverable",
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
							operation: "rolled_back" as const,
							status: "rolled_back" as const,
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine rollback event is invalid",
									}),
							),
						);
						yield* transaction
							.update(MarketplaceRoutines)
							.set({
								enabled: false,
								removed_at: occurred_at,
								status: "rolled_back",
								updated_at: occurred_at,
							})
							.where(eq(MarketplaceRoutines.id, operation.routine_id));
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({ state: "rolled_back", updated_at: occurred_at })
							.where(eq(MarketplaceRoutineOperations.operation_id, operation_id));
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
								causation_id: operation_id,
								correlation_id: operation_id,
								event_id: `${yield* metadata.MakeId("event")}:rolled-back`,
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
								message: "Routine rollback event was not returned",
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
									message: "Routine rollback completion could not be persisted",
								}),
					),
				);

		const ReadRollbackRecovery = database.client
			.select()
			.from(MarketplaceRoutineOperations)
			.orderBy(asc(MarketplaceRoutineOperations.created_at))
			.pipe(
				Effect.flatMap((rows) =>
					Effect.forEach(
						rows.filter((row) => row.kind === "rollback" && row.state === "claimed"),
						(row) => {
							if (!row.routine_id || !row.rollback_json)
								return new RoutineRepositoryError({
									code: "invariant",
									message: "Routine rollback recovery row is incomplete",
								});
							const routine_id = row.routine_id;
							return Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.rollback_json,
							).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(
										Schema.Struct({ rollback_id: Schema.NonEmptyString }),
									),
								),
								Effect.map((receipt) => ({
									operation_id: row.operation_id,
									rollback_id: receipt.rollback_id,
									routine_id,
								})),
								Effect.mapError(
									() =>
										new RoutineRepositoryError({
											code: "invariant",
											message: "Routine rollback recovery row is corrupt",
										}),
								),
							);
						},
					),
				),
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message: "Routine rollback recovery could not be read",
							}),
				),
			);

		return { ReadRecovery, ClaimRollback, CommitRollback, ReadRollbackRecovery };
	}),
);
