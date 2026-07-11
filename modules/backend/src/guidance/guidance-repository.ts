import { asc, eq } from "drizzle-orm";
import { Context, Effect, Layer, Option, Schema } from "effect";

import {
	GuidanceHash,
	GlobalGuidanceMetadata,
	GlobalGuidanceProviderReconciledEvent,
	type EventEnvelope,
	type EventPayload,
	type GlobalGuidanceCanonicalCommitIntent,
	type GlobalGuidanceProviderReconciliation,
	type GlobalGuidanceSelectionRequiredIntent,
} from "@artisan/protocol";

import { settings_scope_id, settings_stream_id } from "../settings/internal-scope";

import { Database } from "../persistence/database";
import {
	EventStreams,
	GlobalGuidanceCanonical,
	GlobalGuidanceProviderSync,
	JournalCommands,
	JournalEvents,
} from "../persistence/schema";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStore,
	JournalStoreFailure,
	type JournalStoreError,
} from "../persistence/journal-store";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

/** Reserves the durable command scope for the global guidance singleton. */
export const global_guidance_thread_id = settings_scope_id("guidance");
const global_guidance_stream_id = settings_stream_id("guidance");

type GlobalGuidanceCanonicalIntent =
	| GlobalGuidanceCanonicalCommitIntent
	| GlobalGuidanceSelectionRequiredIntent;

/** Supplies a backend-private, content-free canonical metadata transition. */
export interface GlobalGuidanceCommandInput {
	readonly intent: GlobalGuidanceCanonicalIntent;
	readonly message_id: string;
	readonly origin: "backend" | "frontend";
	readonly request_fingerprint?: string;
	readonly sent_at: string;
}

/** Returns an accepted guidance transition or the canonical result of an exact retry. */
export interface GlobalGuidanceAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Records an external reconciliation operation under a stable, content-free id. */
export interface GlobalGuidanceReconciliationInput extends GlobalGuidanceProviderReconciliation {
	readonly operation_id: string;
	readonly request_fingerprint?: string;
}

/** Identifies one direct provider mutation before any reconciliation side effect. */
export interface GlobalGuidanceProviderMutationInput {
	readonly operation_id: string;
	readonly request_fingerprint: string;
}

/** Identifies one user-visible guidance request before consulting mutable provider state. */
export interface GlobalGuidanceRequestInput {
	readonly message_id: string;
	readonly origin: "backend" | "frontend";
	readonly request_fingerprint: string;
}

/** Owns durable, content-free global guidance metadata and journal transitions. */
export class GlobalGuidanceRepository extends Context.Service<
	GlobalGuidanceRepository,
	{
		readonly Accept: (
			input: GlobalGuidanceCommandInput,
		) => Effect.Effect<GlobalGuidanceAcceptance, JournalStoreError>;
		readonly PreflightAccept: (
			input: GlobalGuidanceCommandInput,
		) => Effect.Effect<Option.Option<GlobalGuidanceAcceptance>, JournalStoreError>;
		readonly PreflightProviderMutation: (
			input: GlobalGuidanceProviderMutationInput,
		) => Effect.Effect<Option.Option<GlobalGuidanceAcceptance>, JournalStoreError>;
		readonly PreflightRequest: (
			input: GlobalGuidanceRequestInput,
		) => Effect.Effect<Option.Option<GlobalGuidanceAcceptance>, JournalStoreError>;
		readonly Read: Effect.Effect<GlobalGuidanceMetadata, JournalStoreError>;
		readonly RecordProviderReconciliation: (
			input: GlobalGuidanceReconciliationInput,
		) => Effect.Effect<GlobalGuidanceAcceptance, JournalStoreError>;
	}
>()("Artisan/GlobalGuidanceRepository") {}

function normalize_error(error: unknown): JournalStoreError {
	if (error instanceof CommandIdConflict || error instanceof JournalInvariantError) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

const DecodeGuidanceMetadata = (input: unknown) =>
	Schema.decodeUnknownEffect(GlobalGuidanceMetadata, {
		onExcessProperty: "error",
	})(input).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Stored global guidance metadata is invalid",
				}),
		),
	);

function optional_fields<T extends Readonly<Record<string, unknown>>>(input: T) {
	return Object.fromEntries(Object.entries(input).filter(([, value]) => value !== null));
}

const DecodeStoredJson = Schema.decodeUnknownEffect(Schema.UnknownFromJsonString);
const DecodeProviderRequestFingerprint = Schema.decodeUnknownOption(
	Schema.Struct({
		request_fingerprint: GuidanceHash,
	}),
);

function command_payload_json(input: GlobalGuidanceCommandInput) {
	return JSON.stringify({
		...input.intent,
		...(input.request_fingerprint === undefined
			? {}
			: { request_fingerprint: input.request_fingerprint }),
	});
}

function parse_request_fingerprint(payload_json: string) {
	return DecodeStoredJson(payload_json).pipe(
		Effect.map((payload) =>
			Option.map(
				DecodeProviderRequestFingerprint(payload),
				({ request_fingerprint }) => request_fingerprint,
			),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Stored provider reconciliation intent contains invalid JSON",
				}),
		),
	);
}

/** Supplies the SQLite-backed metadata repository while keeping guidance content in files. */
export const GlobalGuidanceRepositoryLive = Layer.effect(
	GlobalGuidanceRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const Read = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [canonical] = yield* transaction
						.select()
						.from(GlobalGuidanceCanonical)
						.where(eq(GlobalGuidanceCanonical.canonical_id, 1))
						.limit(1);
					const providers = yield* transaction
						.select()
						.from(GlobalGuidanceProviderSync)
						.orderBy(asc(GlobalGuidanceProviderSync.provider));

					if (!canonical) {
						return yield* new JournalInvariantError({
							message: "The global guidance canonical metadata row is missing",
						});
					}

					return yield* DecodeGuidanceMetadata({
						canonical: optional_fields({
							byte_count: canonical.byte_count,
							content_hash: canonical.content_hash,
							selected_provider: canonical.selected_provider,
							status: canonical.status,
							updated_at: canonical.updated_at,
						}),
						providers: providers.map((provider) =>
							optional_fields({
								applied_byte_count: provider.applied_byte_count,
								applied_hash: provider.applied_hash,
								backup_path: provider.backup_path,
								ignored_drift_hash: provider.ignored_drift_hash,
								last_error_code: provider.last_error_code,
								modified_at: provider.modified_at,
								observed_byte_count: provider.observed_byte_count,
								observed_hash: provider.observed_hash,
								path: provider.path,
								provider: provider.provider,
								status: provider.status,
								updated_at: provider.updated_at,
							}),
						),
					});
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		const ReadDuplicate = (message_id: string) =>
			journal.ReadCorrelatedEvents(message_id).pipe(
				Effect.flatMap(([event]) =>
					event
						? Effect.succeed({ event, status: "duplicate" as const })
						: Effect.fail(
								new JournalInvariantError({
									message: `Guidance operation ${message_id} has no event`,
								}),
							),
				),
			);

		const Preflight = (
			message_id: string,
			payload_json: string,
			origin: "backend" | "frontend",
		) =>
			Effect.gen(function* () {
				const [existing] = yield* database.client
					.select({
						origin: JournalCommands.origin,
						payload_json: JournalCommands.payload_json,
						thread_id: JournalCommands.thread_id,
					})
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, message_id))
					.limit(1);

				if (!existing) {
					return Option.none<GlobalGuidanceAcceptance>();
				}

				if (
					existing.payload_json !== payload_json ||
					existing.thread_id !== global_guidance_thread_id ||
					existing.origin !== origin
				) {
					return yield* new CommandIdConflict({ message_id });
				}

				return Option.some(yield* ReadDuplicate(message_id));
			}).pipe(Effect.mapError(normalize_error));

		const PreflightAccept = (input: GlobalGuidanceCommandInput) =>
			Preflight(input.message_id, command_payload_json(input), input.origin);

		const PreflightRequest = (input: GlobalGuidanceRequestInput) =>
			Effect.gen(function* () {
				const [existing] = yield* database.client
					.select({
						origin: JournalCommands.origin,
						payload_json: JournalCommands.payload_json,
						thread_id: JournalCommands.thread_id,
					})
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, input.message_id))
					.limit(1);

				if (!existing) {
					return Option.none<GlobalGuidanceAcceptance>();
				}

				const existing_fingerprint = yield* parse_request_fingerprint(
					existing.payload_json,
				);

				if (
					existing.thread_id !== global_guidance_thread_id ||
					existing.origin !== input.origin ||
					Option.isNone(existing_fingerprint) ||
					existing_fingerprint.value !== input.request_fingerprint
				) {
					return yield* new CommandIdConflict({ message_id: input.message_id });
				}

				return Option.some(yield* ReadDuplicate(input.message_id));
			}).pipe(Effect.mapError(normalize_error));

		const PreflightProviderMutation = (input: GlobalGuidanceProviderMutationInput) =>
			Effect.gen(function* () {
				const [existing] = yield* database.client
					.select({
						origin: JournalCommands.origin,
						payload_json: JournalCommands.payload_json,
						thread_id: JournalCommands.thread_id,
					})
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, input.operation_id))
					.limit(1);

				if (!existing) {
					return Option.none<GlobalGuidanceAcceptance>();
				}

				const existing_fingerprint = yield* parse_request_fingerprint(
					existing.payload_json,
				);

				if (
					existing.thread_id !== global_guidance_thread_id ||
					existing.origin !== "backend" ||
					Option.isNone(existing_fingerprint) ||
					existing_fingerprint.value !== input.request_fingerprint
				) {
					return yield* new CommandIdConflict({ message_id: input.operation_id });
				}

				return Option.some(yield* ReadDuplicate(input.operation_id));
			}).pipe(Effect.mapError(normalize_error));

		const Accept = (input: GlobalGuidanceCommandInput) =>
			Effect.gen(function* () {
				const payload_json = command_payload_json(input);
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select({
								origin: JournalCommands.origin,
								payload_json: JournalCommands.payload_json,
								sent_at: JournalCommands.sent_at,
								thread_id: JournalCommands.thread_id,
							})
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, input.message_id))
							.limit(1);

						if (existing) {
							if (
								existing.payload_json !== payload_json ||
								existing.thread_id !== global_guidance_thread_id ||
								existing.origin !== input.origin
							) {
								return yield* new CommandIdConflict({
									message_id: input.message_id,
								});
							}

							return { _tag: "Duplicate" as const };
						}

						const accepted_at = yield* metadata.Now;
						const event_id = yield* metadata.MakeId("event");
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, global_guidance_stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;
						const event_payload: EventPayload =
							input.intent.type === "guidance.canonical.commit"
								? {
										byte_count: input.intent.byte_count,
										content_hash: input.intent.content_hash,
										...(input.intent.selected_provider === undefined
											? {}
											: {
													selected_provider:
														input.intent.selected_provider,
												}),
										type: "guidance.canonical.updated",
									}
								: {
										candidate_hashes: input.intent.candidate_hashes,
										type: "guidance.selection.required",
									};

						yield* transaction.insert(JournalCommands).values({
							accepted_at,
							message_id: input.message_id,
							origin: input.origin,
							payload_json,
							payload_type: input.intent.type,
							schema_version: 1,
							sent_at: input.sent_at,
							status: "accepted",
							thread_id: global_guidance_thread_id,
						});

						if (input.intent.type === "guidance.canonical.commit") {
							yield* transaction
								.update(GlobalGuidanceCanonical)
								.set({
									byte_count: input.intent.byte_count,
									content_hash: input.intent.content_hash,
									selected_provider: input.intent.selected_provider ?? null,
									status: "ready",
									updated_at: accepted_at,
								})
								.where(eq(GlobalGuidanceCanonical.canonical_id, 1));
						} else {
							yield* transaction
								.update(GlobalGuidanceCanonical)
								.set({ status: "selection_required", updated_at: accepted_at })
								.where(eq(GlobalGuidanceCanonical.canonical_id, 1));
						}

						if (stream) {
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: sequence })
								.where(eq(EventStreams.stream_id, global_guidance_stream_id));
						} else {
							yield* transaction.insert(EventStreams).values({
								last_sequence: sequence,
								stream_id: global_guidance_stream_id,
							});
						}

						const [event_row] = yield* transaction
							.insert(JournalEvents)
							.values({
								causation_id: input.message_id,
								correlation_id: input.message_id,
								event_id,
								event_type: event_payload.type,
								occurred_at: accepted_at,
								origin: "backend",
								payload_json: JSON.stringify(event_payload),
								schema_version: 1,
								stream_id: global_guidance_stream_id,
								stream_sequence: sequence,
								thread_id: global_guidance_thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						const event = {
							causation_id: input.message_id,
							correlation_id: input.message_id,
							journal_sequence: event_row!.journal_sequence,
							kind: "event" as const,
							message_id: event_id,
							origin: "backend" as const,
							payload: event_payload,
							protocol_version: 1 as const,
							schema_version: 1 as const,
							sequence,
							sent_at: accepted_at,
							stream_id: global_guidance_stream_id,
							thread_id: global_guidance_thread_id,
						} satisfies EventEnvelope;

						return { _tag: "Accepted" as const, event };
					}),
				);

				if (result._tag === "Duplicate") {
					return yield* ReadDuplicate(input.message_id);
				}

				yield* notifier.Publish(result.event.journal_sequence);

				return { event: result.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		const RecordProviderReconciliation = (input: GlobalGuidanceReconciliationInput) =>
			Effect.gen(function* () {
				const { operation_id, ...reconciliation } = input;
				const intent = {
					...reconciliation,
					type: "guidance.provider.reconcile" as const,
				};
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select({
								origin: JournalCommands.origin,
								payload_json: JournalCommands.payload_json,
								thread_id: JournalCommands.thread_id,
							})
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, operation_id))
							.limit(1);

						if (existing) {
							const existing_fingerprint = yield* parse_request_fingerprint(
								existing.payload_json,
							);
							const same_intent =
								input.request_fingerprint === undefined
									? existing.payload_json === JSON.stringify(intent)
									: Option.isSome(existing_fingerprint) &&
										existing_fingerprint.value === input.request_fingerprint;

							if (
								!same_intent ||
								existing.thread_id !== global_guidance_thread_id ||
								existing.origin !== "backend"
							) {
								return yield* new CommandIdConflict({ message_id: operation_id });
							}

							return { _tag: "Duplicate" as const };
						}

						const occurred_at = yield* metadata.Now;
						const event_id = yield* metadata.MakeId("event");
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, global_guidance_stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;
						const event_payload = {
							...(input.applied_byte_count === undefined
								? {}
								: { applied_byte_count: input.applied_byte_count }),
							...(input.applied_hash === undefined
								? {}
								: { applied_hash: input.applied_hash }),
							...(input.ignored_drift_hash === undefined
								? {}
								: { ignored_drift_hash: input.ignored_drift_hash }),
							...(input.last_error_code === undefined
								? {}
								: { last_error_code: input.last_error_code }),
							...(input.observed_byte_count === undefined
								? {}
								: { observed_byte_count: input.observed_byte_count }),
							...(input.observed_hash === undefined
								? {}
								: { observed_hash: input.observed_hash }),
							provider: input.provider,
							status: input.status,
							type: "guidance.provider.reconciled" as const,
						} satisfies GlobalGuidanceProviderReconciledEvent;

						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: operation_id,
							origin: "backend",
							payload_json: JSON.stringify(intent),
							payload_type: intent.type,
							schema_version: 1,
							sent_at: occurred_at,
							status: "accepted",
							thread_id: global_guidance_thread_id,
						});
						yield* transaction
							.insert(GlobalGuidanceProviderSync)
							.values({
								applied_byte_count: input.applied_byte_count ?? null,
								applied_hash: input.applied_hash ?? null,
								backup_path: input.backup_path ?? null,
								ignored_drift_hash: input.ignored_drift_hash ?? null,
								last_error_code: input.last_error_code ?? null,
								modified_at: input.modified_at ?? null,
								observed_byte_count: input.observed_byte_count ?? null,
								observed_hash: input.observed_hash ?? null,
								path: input.path ?? null,
								provider: input.provider,
								status: input.status,
								updated_at: occurred_at,
							})
							.onConflictDoUpdate({
								target: GlobalGuidanceProviderSync.provider,
								set: {
									applied_byte_count: input.applied_byte_count ?? null,
									applied_hash: input.applied_hash ?? null,
									backup_path: input.backup_path ?? null,
									ignored_drift_hash: input.ignored_drift_hash ?? null,
									last_error_code: input.last_error_code ?? null,
									modified_at: input.modified_at ?? null,
									observed_byte_count: input.observed_byte_count ?? null,
									observed_hash: input.observed_hash ?? null,
									path: input.path ?? null,
									status: input.status,
									updated_at: occurred_at,
								},
							});

						if (stream) {
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: sequence })
								.where(eq(EventStreams.stream_id, global_guidance_stream_id));
						} else {
							yield* transaction.insert(EventStreams).values({
								last_sequence: sequence,
								stream_id: global_guidance_stream_id,
							});
						}

						const [event_row] = yield* transaction
							.insert(JournalEvents)
							.values({
								causation_id: operation_id,
								correlation_id: operation_id,
								event_id,
								event_type: event_payload.type,
								occurred_at,
								origin: "backend",
								payload_json: JSON.stringify(event_payload),
								schema_version: 1,
								stream_id: global_guidance_stream_id,
								stream_sequence: sequence,
								thread_id: global_guidance_thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						const event = {
							causation_id: operation_id,
							correlation_id: operation_id,
							journal_sequence: event_row!.journal_sequence,
							kind: "event" as const,
							message_id: event_id,
							origin: "backend" as const,
							payload: event_payload,
							protocol_version: 1 as const,
							schema_version: 1 as const,
							sequence,
							sent_at: occurred_at,
							stream_id: global_guidance_stream_id,
							thread_id: global_guidance_thread_id,
						} satisfies EventEnvelope;

						return { _tag: "Accepted" as const, event };
					}),
				);

				if (result._tag === "Duplicate") {
					return yield* ReadDuplicate(operation_id);
				}

				yield* notifier.Publish(result.event.journal_sequence);

				return { event: result.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		return {
			Accept,
			PreflightAccept,
			PreflightProviderMutation,
			PreflightRequest,
			Read,
			RecordProviderReconciliation,
		};
	}),
);
