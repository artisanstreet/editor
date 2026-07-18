import { and, asc, eq } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	MarketplaceLedgerEvent,
	ProviderSyncState,
	RoutineDetail,
	RoutineDriftOverwriteRequest,
	RoutineInstallPreview,
	RoutineSummary,
} from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	MarketplaceRoutineOperations,
	MarketplaceRoutineMirrors,
	MarketplaceRoutines,
} from "../../persistence/schema";
import { RuntimeMetadata } from "../../runtime/runtime-metadata";
import { settings_scope_id, settings_stream_id } from "../../settings/internal-scope";

export const marketplace_routine_thread_id = settings_scope_id("marketplace-routines");
const marketplace_routine_stream_id = settings_stream_id("marketplace-routines");

export class RoutineRepositoryError extends Data.TaggedError("RoutineRepositoryError")<{
	readonly code: "conflict" | "invariant" | "not_found";
	readonly message: string;
}> {}

export interface RoutineOperation {
	readonly approval_id: string;
	readonly approval_fingerprint: string;
	readonly operation_id: string;
	readonly preview_json: string;
	readonly request_fingerprint: string;
	readonly routine_id: string;
}

export interface RoutineInstallRequestRecord extends RoutineOperation {}

export interface RoutineDriftOverwriteRequestRecord {
	readonly operation_id: string;
	readonly request: RoutineDriftOverwriteRequest;
}

export interface RoutineRecovery {
	readonly operation_id: string;
	readonly routine_id: string;
	readonly state: "approved" | "installed";
}

export interface RoutineRollbackRecovery {
	readonly operation_id: string;
	readonly rollback_id: string;
	readonly routine_id: string;
}

export interface RoutineMirrorOperation {
	readonly engine_id: string;
	readonly intent_fingerprint: string;
	readonly kind: "drift" | "sync";
	readonly operation_id: string;
	readonly routine_id: string;
}

export type RoutineOperationClaim =
	| { readonly _tag: "Claimed" }
	| { readonly _tag: "InFlight" }
	| { readonly _tag: "Completed"; readonly journal_sequence: number };

const mirror_operation_lease_milliseconds = 60_000;

export class RoutineRepository extends Context.Service<
	RoutineRepository,
	{
		readonly ReadSummaries: Effect.Effect<
			ReadonlyArray<RoutineSummary>,
			RoutineRepositoryError
		>;
		readonly ReadDetail: (
			routine_id: string,
		) => Effect.Effect<RoutineDetail, RoutineRepositoryError>;
		/** Records a preview request without granting authority to write. */
		readonly RecordPendingInstall: (
			operation: RoutineInstallRequestRecord,
		) => Effect.Effect<"accepted" | "duplicate", RoutineRepositoryError>;
		/** Resolves a decision from durable approval state, independent of a renderer connection. */
		readonly ReadPendingInstall: (
			approval_id: string,
		) => Effect.Effect<
			RoutineInstallRequestRecord & { readonly preview: RoutineInstallPreview },
			RoutineRepositoryError
		>;
		readonly RecordPendingDriftOverwrite: (
			input: RoutineDriftOverwriteRequestRecord,
		) => Effect.Effect<"accepted" | "duplicate", RoutineRepositoryError>;
		readonly ReadPendingDriftOverwrite: (
			approval_id: string,
		) => Effect.Effect<RoutineDriftOverwriteRequestRecord, RoutineRepositoryError>;
		readonly DecideDriftOverwrite: (input: {
			readonly approval_id: string;
			readonly approved: boolean;
			readonly intent_fingerprint: string;
		}) => Effect.Effect<"approved" | "denied" | "resume", RoutineRepositoryError>;
		/** The only transition that makes an installer eligible to run. */
		readonly DecideInstall: (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly approved: boolean;
			readonly operation_id: string;
		}) => Effect.Effect<"approved" | "denied" | "installed" | "resume", RoutineRepositoryError>;
		readonly CommitInstalled: (input: {
			readonly artifact_refs: ReadonlyArray<string>;
			readonly detail: RoutineDetail;
			readonly operation_id: string;
			readonly rollback_json?: string;
		}) => Effect.Effect<number, RoutineRepositoryError>;
		/** Records a terminal installer failure in the canonical lifecycle ledger. */
		readonly RecordInstallFailure: (input: {
			readonly code: "conflict" | "install_failed" | "rollback_failed";
			readonly operation_id: string;
		}) => Effect.Effect<number, RoutineRepositoryError>;
		readonly Transition: (input: {
			readonly enabled: boolean;
			readonly operation: "enabled" | "disabled" | "invoked" | "removed";
			readonly operation_id: string;
			readonly routine_id: string;
			readonly status: RoutineDetail["status"];
			readonly tool_name?: string;
		}) => Effect.Effect<number, RoutineRepositoryError>;
		readonly ReadRecovery: Effect.Effect<
			ReadonlyArray<RoutineRecovery>,
			RoutineRepositoryError
		>;
		readonly ClaimRollback: (
			input: RoutineRollbackRecovery,
		) => Effect.Effect<"claimed" | "completed", RoutineRepositoryError>;
		readonly CommitRollback: (
			operation_id: string,
		) => Effect.Effect<number, RoutineRepositoryError>;
		readonly ReadRollbackRecovery: Effect.Effect<
			ReadonlyArray<RoutineRollbackRecovery>,
			RoutineRepositoryError
		>;
		readonly ClaimMirrorOperation: (
			input: RoutineMirrorOperation,
		) => Effect.Effect<RoutineOperationClaim, RoutineRepositoryError>;
		readonly CommitMirrorOperation: (input: {
			readonly imported?: RoutineDetail;
			readonly operation_id: string;
			readonly state: ProviderSyncState;
		}) => Effect.Effect<number, RoutineRepositoryError>;
		/** Releases a failed adapter attempt without permitting another runtime's lease to be cleared. */
		readonly ReleaseMirrorOperation: (
			operation_id: string,
		) => Effect.Effect<void, RoutineRepositoryError>;
	}
>()("Artisan/Marketplace/RoutineRepository") {}

const DecodeSummary = (row: typeof MarketplaceRoutines.$inferSelect) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.scope_json).pipe(
		Effect.flatMap((scope) =>
			Schema.decodeUnknownEffect(RoutineSummary, {
				onExcessProperty: "error",
			})({
				description: row.description,
				display_name: row.display_name,
				enabled: row.enabled,
				id: row.id,
				scope,
				status: row.status,
				version: row.version,
			}),
		),
		Effect.mapError(
			() =>
				new RoutineRepositoryError({
					code: "invariant",
					message: `Routine ${row.id} contains invalid persisted metadata`,
				}),
		),
	);

const DecodeDetail = (
	row: typeof MarketplaceRoutines.$inferSelect,
	sync: ReadonlyArray<ProviderSyncState>,
) =>
	Effect.all({
		compatibility: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
			row.compatibility_json,
		).pipe(Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.compatibility))),
		commands: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.commands_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.exported_commands)),
		),
		files: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.files_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.files)),
		),
		permissions: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
			row.permissions_json,
		).pipe(Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.permissions))),
		scope: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.scope_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.scope)),
		),
		source: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.source_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.source)),
		),
	}).pipe(
		Effect.flatMap(({ compatibility, commands, files, permissions, scope, source }) =>
			Schema.decodeUnknownEffect(RoutineDetail, { onExcessProperty: "error" })({
				...(row.author === null ? {} : { author: row.author }),
				compatibility,
				description: row.description,
				display_name: row.display_name,
				enabled: row.enabled,
				exported_commands: commands,
				files,
				id: row.id,
				instructions: row.instructions,
				permissions,
				...(row.removed_at === null ? {} : { removed_at: row.removed_at }),
				scope,
				status: row.status,
				source,
				sync,
				trust: row.trust,
				version: row.version,
			}),
		),
		Effect.mapError(
			() =>
				new RoutineRepositoryError({
					code: "invariant",
					message: `Routine ${row.id} contains invalid persisted metadata`,
				}),
		),
	);

/** SQLite persistence for pre-write approval state. The installer is deliberately not a dependency. */
export const RoutineRepositoryLive = Layer.effect(
	RoutineRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ReadSummaries = database.client
			.select()
			.from(MarketplaceRoutines)
			.orderBy(asc(MarketplaceRoutines.display_name))
			.pipe(
				Effect.flatMap((rows) => Effect.forEach(rows, DecodeSummary)),
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message: "Routine registry could not be read",
							}),
				),
			);

		const ReadDetail = (routine_id: string) =>
			Effect.gen(function* () {
				const [row] = yield* database.client
					.select()
					.from(MarketplaceRoutines)
					.where(eq(MarketplaceRoutines.id, routine_id))
					.limit(1);
				if (!row) {
					return yield* new RoutineRepositoryError({
						code: "not_found",
						message: `Routine ${routine_id} was not found`,
					});
				}
				const mirrors = yield* database.client
					.select()
					.from(MarketplaceRoutineMirrors)
					.where(eq(MarketplaceRoutineMirrors.routine_id, routine_id));
				const sync = yield* Effect.forEach(mirrors, (mirror) =>
					Schema.decodeUnknownEffect(ProviderSyncState)({
						engine_id: mirror.engine_id,
						...(mirror.last_error_code === null
							? {}
							: { last_error_code: mirror.last_error_code }),
						...(mirror.observed_revision === null
							? {}
							: { observed_revision: mirror.observed_revision }),
						status: mirror.status,
						updated_at: mirror.updated_at,
					}),
				);
				return yield* DecodeDetail(row, sync);
			}).pipe(
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message: `Routine ${routine_id} contains invalid persisted metadata`,
							}),
				),
			);

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
				const preview = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					operation.preview_json,
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
					approval_fingerprint: operation.approval_fingerprint!,
					approval_id: operation.approval_id!,
					operation_id: operation.operation_id,
					preview,
					preview_json: operation.preview_json!,
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

		return {
			ClaimRollback,
			ClaimMirrorOperation,
			CommitInstalled,
			CommitRollback,
			CommitMirrorOperation,
			DecideInstall,
			DecideDriftOverwrite,
			ReadDetail,
			ReadPendingInstall,
			ReadPendingDriftOverwrite,
			ReadRecovery,
			ReadRollbackRecovery,
			ReadSummaries,
			RecordInstallFailure,
			RecordPendingInstall,
			RecordPendingDriftOverwrite,
			ReleaseMirrorOperation,
			Transition,
		};
	}),
);
