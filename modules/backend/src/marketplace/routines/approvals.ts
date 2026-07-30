import { eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";
import {
	MarketplaceLedgerEvent,
	RoutineDriftOverwriteRequest,
	RoutineInstallPreview,
} from "@artisan/protocol";
import { Database } from "../../persistence/database";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	MarketplaceRoutineOperations,
} from "../../persistence/tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	marketplace_routine_stream_id,
	marketplace_routine_thread_id,
	RoutineRepositoryError,
	type RoutineDriftOverwriteRequestRecord,
	type RoutineInstallRequestRecord,
	type RoutineRepositoryApi,
} from "./contracts";

export class RoutineApprovals extends Context.Service<
	RoutineApprovals,
	Pick<
		RoutineRepositoryApi,
		| "RecordPendingInstall"
		| "DecideInstall"
		| "ReadPendingInstall"
		| "RecordPendingDriftOverwrite"
		| "ReadPendingDriftOverwrite"
		| "DecideDriftOverwrite"
	>
>()("Artisan/Marketplace/Routines/RoutineApprovals") {}

export const RoutineApprovalsLive = Layer.effect(
	RoutineApprovals,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const RecordPendingInstall = (operation: RoutineInstallRequestRecord) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(
								eq(
									MarketplaceRoutineOperations.operation_id,
									operation.operation_id,
								),
							)
							.limit(1);
						if (existing) {
							if (
								existing.request_fingerprint !== operation.request_fingerprint ||
								existing.preview_json !== operation.preview_json ||
								existing.routine_id !== operation.routine_id
							) {
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message:
										"Routine operation id was reused with different intent",
								});
							}
							return "duplicate" as const;
						}
						const occurred_at = yield* metadata.Now;
						yield* transaction.insert(MarketplaceRoutineOperations).values({
							approval_decision: null,
							approval_fingerprint: operation.approval_fingerprint,
							approval_id: operation.approval_id,
							created_at: occurred_at,
							kind: "install",
							operation_id: operation.operation_id,
							preview_json: operation.preview_json,
							request_fingerprint: operation.request_fingerprint,
							routine_id: operation.routine_id,
							state: "awaiting_approval",
							updated_at: occurred_at,
						});
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id))
							.limit(1);
						const stream_sequence = (stream?.last_sequence ?? 0) + 1;
						const payload = {
							approval_id: operation.approval_id,
							item_id: operation.routine_id,
							item_kind: "routine" as const,
							operation: "install_requested" as const,
							status: "awaiting_approval" as const,
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine install request event is invalid",
									}),
							),
						);
						if (stream) {
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: stream_sequence })
								.where(eq(EventStreams.stream_id, marketplace_routine_stream_id));
						} else {
							yield* transaction.insert(EventStreams).values({
								last_sequence: stream_sequence,
								stream_id: marketplace_routine_stream_id,
							});
						}
						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: operation.operation_id,
							origin: "backend",
							payload_json: JSON.stringify({
								request_fingerprint: operation.request_fingerprint,
							}),
							payload_type: "marketplace.routine.install",
							schema_version: 1,
							sent_at: occurred_at,
							status: "accepted",
							thread_id: marketplace_routine_thread_id,
						});
						yield* transaction.insert(JournalEvents).values({
							causation_id: operation.operation_id,
							correlation_id: operation.operation_id,
							event_id: `${yield* metadata.MakeId("event")}:requested`,
							event_type: payload.type,
							occurred_at,
							origin: "backend",
							payload_json: JSON.stringify(payload),
							schema_version: 1,
							stream_id: marketplace_routine_stream_id,
							stream_sequence,
							thread_id: marketplace_routine_thread_id,
						});
						return "accepted" as const;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine install preview could not be persisted",
								}),
					),
				);

		const DecideInstall = (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly approved: boolean;
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
						if (
							!operation ||
							operation.kind !== "install" ||
							operation.approval_id !== input.approval_id ||
							operation.approval_fingerprint !== input.approval_fingerprint
						) {
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine approval does not match its pending preview",
							});
						}
						const decision = input.approved ? "approved" : "denied";
						if (operation.state === "installed" && input.approved)
							return "installed" as const;
						if (operation.state === "approved" && input.approved)
							return "resume" as const;
						if (operation.state === "denied" && !input.approved)
							return "denied" as const;
						if (operation.state !== "awaiting_approval")
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine approval was already decided",
							});
						const occurred_at = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({
								approval_decision: decision,
								state: decision,
								updated_at: occurred_at,
							})
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							);
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id))
							.limit(1);
						if (!stream) {
							return yield* new RoutineRepositoryError({
								code: "invariant",
								message: "Routine approval request has no event stream",
							});
						}
						const stream_sequence = stream.last_sequence + 1;
						const payload = {
							approval_id: input.approval_id,
							item_id: operation.routine_id,
							item_kind: "routine" as const,
							operation: "approval_resolved" as const,
							status: input.approved
								? ("installing" as const)
								: ("approval_denied" as const),
							type: "marketplace.lifecycle" as const,
						};
						yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
							Effect.mapError(
								() =>
									new RoutineRepositoryError({
										code: "invariant",
										message: "Routine approval decision event is invalid",
									}),
							),
						);
						yield* transaction
							.update(EventStreams)
							.set({ last_sequence: stream_sequence })
							.where(eq(EventStreams.stream_id, marketplace_routine_stream_id));
						yield* transaction.insert(JournalEvents).values({
							causation_id: input.operation_id,
							correlation_id: `${input.operation_id}:decision`,
							event_id: `${yield* metadata.MakeId("event")}:decision`,
							event_type: payload.type,
							occurred_at,
							origin: "backend",
							payload_json: JSON.stringify(payload),
							schema_version: 1,
							stream_id: marketplace_routine_stream_id,
							stream_sequence,
							thread_id: marketplace_routine_thread_id,
						});
						return decision;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message: "Routine approval decision could not be persisted",
								}),
					),
				);

		const ReadPendingInstall = (approval_id: string) =>
			Effect.gen(function* () {
				const [operation] = yield* database.client
					.select()
					.from(MarketplaceRoutineOperations)
					.where(eq(MarketplaceRoutineOperations.approval_id, approval_id))
					.limit(1);
				if (
					!operation ||
					operation.kind !== "install" ||
					operation.approval_id === null ||
					operation.approval_fingerprint === null ||
					operation.preview_json === null ||
					operation.routine_id === null
				)
					return yield* Effect.fail(
						new RoutineRepositoryError({
							code: "not_found",
							message: "Routine approval request was not found",
						}),
					);
				const persisted_approval_fingerprint = operation.approval_fingerprint;
				const persisted_approval_id = operation.approval_id;
				const preview_json = operation.preview_json;
				const preview = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					preview_json,
				).pipe(
					Effect.flatMap(Schema.decodeUnknownEffect(RoutineInstallPreview)),
					Effect.mapError(
						() =>
							new RoutineRepositoryError({
								code: "invariant",
								message:
									"Routine approval request contains invalid persisted metadata",
							}),
					),
				);
				return {
					approval_fingerprint: persisted_approval_fingerprint,
					approval_id: persisted_approval_id,
					operation_id: operation.operation_id,
					preview,
					preview_json,
					request_fingerprint: operation.request_fingerprint,
					routine_id: operation.routine_id,
				};
			}).pipe(
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message:
									"Routine approval request contains invalid persisted metadata",
							}),
				),
			);

		const RecordPendingDriftOverwrite = (input: RoutineDriftOverwriteRequestRecord) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const intent_json = JSON.stringify(input.request);
						const [existing] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(
								eq(MarketplaceRoutineOperations.operation_id, input.operation_id),
							)
							.limit(1);
						if (existing) {
							if (
								existing.kind !== "drift_overwrite_approval" ||
								existing.preview_json !== intent_json ||
								existing.approval_id !== input.request.approval_id ||
								existing.approval_fingerprint !==
									input.request.intent_fingerprint ||
								existing.routine_id !== input.request.routine_id
							)
								return yield* new RoutineRepositoryError({
									code: "conflict",
									message:
										"Routine drift overwrite operation id was reused with different intent",
								});
							return "duplicate" as const;
						}
						const occurred_at = yield* metadata.Now;
						yield* transaction.insert(MarketplaceRoutineOperations).values({
							approval_decision: null,
							approval_fingerprint: input.request.intent_fingerprint,
							approval_id: input.request.approval_id,
							created_at: occurred_at,
							kind: "drift_overwrite_approval",
							operation_id: input.operation_id,
							preview_json: intent_json,
							request_fingerprint: input.request.intent_fingerprint,
							routine_id: input.request.routine_id,
							state: "awaiting_approval",
							updated_at: occurred_at,
						});
						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: input.operation_id,
							origin: "backend",
							payload_json: intent_json,
							payload_type: "marketplace.routine.drift.overwrite.request",
							schema_version: 1,
							sent_at: occurred_at,
							status: "accepted",
							thread_id: marketplace_routine_thread_id,
						});
						return "accepted" as const;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message:
										"Routine drift overwrite request could not be persisted",
								}),
					),
				);

		const ReadPendingDriftOverwrite = (approval_id: string) =>
			Effect.gen(function* () {
				const [operation] = yield* database.client
					.select()
					.from(MarketplaceRoutineOperations)
					.where(eq(MarketplaceRoutineOperations.approval_id, approval_id))
					.limit(1);
				if (
					!operation ||
					operation.kind !== "drift_overwrite_approval" ||
					operation.preview_json === null
				)
					return yield* new RoutineRepositoryError({
						code: "not_found",
						message: "Routine drift overwrite approval was not found",
					});
				const request = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					operation.preview_json,
				).pipe(
					Effect.flatMap(Schema.decodeUnknownEffect(RoutineDriftOverwriteRequest)),
					Effect.mapError(
						() =>
							new RoutineRepositoryError({
								code: "invariant",
								message: "Routine drift overwrite approval contains invalid intent",
							}),
					),
				);
				return { operation_id: operation.operation_id, request };
			}).pipe(
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message: "Routine drift overwrite approval could not be read",
							}),
				),
			);

		const DecideDriftOverwrite = (input: {
			readonly approval_id: string;
			readonly approved: boolean;
			readonly intent_fingerprint: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceRoutineOperations)
							.where(eq(MarketplaceRoutineOperations.approval_id, input.approval_id))
							.limit(1);
						if (
							!operation ||
							operation.kind !== "drift_overwrite_approval" ||
							operation.approval_fingerprint !== input.intent_fingerprint
						)
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message:
									"Routine drift overwrite decision does not match its request",
							});
						const decision = input.approved ? "approved" : "denied";
						if (operation.state === decision)
							return input.approved ? ("resume" as const) : ("denied" as const);
						if (operation.state !== "awaiting_approval")
							return yield* new RoutineRepositoryError({
								code: "conflict",
								message: "Routine drift overwrite approval was already decided",
							});
						yield* transaction
							.update(MarketplaceRoutineOperations)
							.set({
								approval_decision: decision,
								state: decision,
								updated_at: yield* metadata.Now,
							})
							.where(
								eq(
									MarketplaceRoutineOperations.operation_id,
									operation.operation_id,
								),
							);
						return decision;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof RoutineRepositoryError
							? error
							: new RoutineRepositoryError({
									code: "invariant",
									message:
										"Routine drift overwrite decision could not be persisted",
								}),
					),
				);

		return {
			RecordPendingInstall,
			DecideInstall,
			ReadPendingInstall,
			RecordPendingDriftOverwrite,
			ReadPendingDriftOverwrite,
			DecideDriftOverwrite,
		};
	}),
);
