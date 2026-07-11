import { asc, eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";

import {
	ModelBehaviourProviderReconciledEvent,
	ModelBehaviourProviderState,
	ModelBehaviourSetting,
	ModelBehaviourSettingUpdatedEvent,
	type ModelBehaviourProviderState as ModelBehaviourProviderStateValue,
	type ModelBehaviourSetting as ModelBehaviourSettingValue,
	type ModelBehaviourUpdateRequest,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ModelBehaviourProviderStates,
	ModelBehaviourSettings,
} from "../persistence/schema";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
} from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { settings_scope_id, settings_stream_id } from "../settings/internal-scope";

/** Reserves the durable command scope for curated global model behaviour. */
export const model_behaviour_thread_id = settings_scope_id("model-behaviour");
const model_behaviour_stream_id = settings_stream_id("model-behaviour");

const ModelBehaviourEventPayload = Schema.Union([
	ModelBehaviourSettingUpdatedEvent,
	ModelBehaviourProviderReconciledEvent,
]);

const ModelBehaviourJournalEvent = Schema.Struct({
	causation_id: Schema.NonEmptyString,
	correlation_id: Schema.NonEmptyString,
	event_id: Schema.NonEmptyString,
	journal_sequence: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	occurred_at: Schema.NonEmptyString,
	payload: ModelBehaviourEventPayload,
	sequence: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});

const ModelBehaviourCommandIdentity = Schema.Struct({
	request_fingerprint: Schema.NonEmptyString,
	type: Schema.Literal("model_behaviour.commit"),
});

/** Describes the operation identity checked before any provider-side mutation. */
export interface ModelBehaviourOperation {
	readonly message_id: string;
	readonly origin: "backend" | "frontend";
	readonly request_fingerprint: string;
	readonly sent_at: string;
}

/** Describes a full, content-free provider reconciliation state to persist. */
export type ModelBehaviourProviderCommit = Omit<ModelBehaviourProviderStateValue, "updated_at">;

/** Describes one atomic canonical-setting and provider-reconciliation commit. */
export interface ModelBehaviourCommit {
	readonly operation: ModelBehaviourOperation;
	readonly provider_states: ReadonlyArray<ModelBehaviourProviderCommit>;
	readonly setting_update?: ModelBehaviourUpdateRequest;
}

/** Returns model settings and their provider-specific reconciliation metadata. */
export interface ModelBehaviourReadResult {
	readonly provider_states: ReadonlyArray<ModelBehaviourProviderStateValue>;
	readonly settings: ReadonlyArray<ModelBehaviourSettingValue>;
}

/** Carries a validated durable event emitted by this repository. */
export type ModelBehaviourEvent = typeof ModelBehaviourJournalEvent.Type;

/** Reports whether an operation may proceed or is an exact duplicate. */
export type ModelBehaviourPreflight =
	| { readonly _tag: "Proceed" }
	| { readonly _tag: "Duplicate"; readonly events: ReadonlyArray<ModelBehaviourEvent> };

/** Returns the persisted result of a fresh commit or an exact retry. */
export interface ModelBehaviourCommitResult {
	readonly events: ReadonlyArray<ModelBehaviourEvent>;
	readonly status: "accepted" | "duplicate";
}

/** Represents repository errors that preserve journal identity and invariant failures. */
export type ModelBehaviourRepositoryError =
	| CommandIdConflict
	| JournalInvariantError
	| JournalStoreFailure;

/** Owns content-safe SQLite persistence for curated model behaviour settings. */
export class ModelBehaviourRepository extends Context.Service<
	ModelBehaviourRepository,
	{
		readonly Commit: (
			input: ModelBehaviourCommit,
		) => Effect.Effect<ModelBehaviourCommitResult, ModelBehaviourRepositoryError>;
		readonly Preflight: (
			operation: ModelBehaviourOperation,
		) => Effect.Effect<ModelBehaviourPreflight, ModelBehaviourRepositoryError>;
		readonly Read: Effect.Effect<ModelBehaviourReadResult, ModelBehaviourRepositoryError>;
	}
>()("Artisan/ModelBehaviourRepository") {}

function normalize_error(error: unknown): ModelBehaviourRepositoryError {
	if (error instanceof CommandIdConflict || error instanceof JournalInvariantError) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function optional_fields<T extends Readonly<Record<string, unknown>>>(input: T) {
	return Object.fromEntries(Object.entries(input).filter(([, value]) => value !== null));
}

function command_payload_json(operation: ModelBehaviourOperation) {
	return JSON.stringify({
		request_fingerprint: operation.request_fingerprint,
		type: "model_behaviour.commit",
	});
}

const DecodeJson = Schema.decodeUnknownEffect(Schema.UnknownFromJsonString);

function DecodeSetting(row: typeof ModelBehaviourSettings.$inferSelect) {
	return DecodeJson(row.value_json).pipe(
		Effect.flatMap((value) =>
			Schema.decodeUnknownEffect(ModelBehaviourSetting, {
				onExcessProperty: "error",
			})({
				setting_id: row.setting_id,
				updated_at: row.updated_at,
				value,
				version: row.version,
			}),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored model behaviour setting ${row.setting_id} is invalid`,
				}),
		),
	);
}

function DecodeProviderState(row: typeof ModelBehaviourProviderStates.$inferSelect) {
	return Schema.decodeUnknownEffect(ModelBehaviourProviderState, {
		onExcessProperty: "error",
	})(
		optional_fields({
			applied_hash: row.applied_hash,
			backup_path: row.backup_path,
			ignored_drift_hash: row.ignored_drift_hash,
			last_error_code: row.last_error_code,
			native_key: row.native_key,
			observed_hash: row.observed_hash,
			provider_id: row.provider_id,
			setting_id: row.setting_id,
			status: row.status,
			target_path: row.target_path,
			updated_at: row.updated_at,
		}),
	).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored model behaviour provider ${row.provider_id} is invalid`,
				}),
		),
	);
}

function DecodeEvent(row: typeof JournalEvents.$inferSelect) {
	return DecodeJson(row.payload_json).pipe(
		Effect.flatMap((payload) =>
			Schema.decodeUnknownEffect(ModelBehaviourJournalEvent, {
				onExcessProperty: "error",
			})({
				causation_id: row.causation_id,
				correlation_id: row.correlation_id,
				event_id: row.event_id,
				journal_sequence: row.sequence,
				occurred_at: row.occurred_at,
				payload,
				sequence: row.stream_sequence,
			}),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored model behaviour event ${row.event_id} is invalid`,
				}),
		),
	);
}

function DecodeCommandIdentity(payload_json: string) {
	return DecodeJson(payload_json).pipe(
		Effect.flatMap((payload) =>
			Schema.decodeUnknownEffect(ModelBehaviourCommandIdentity, {
				onExcessProperty: "error",
			})(payload),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Stored model behaviour command identity is invalid",
				}),
		),
	);
}

/** Supplies the SQLite-backed, metadata-only model behaviour repository. */
export const ModelBehaviourRepositoryLive = Layer.effect(
	ModelBehaviourRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const Read = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const settings = yield* transaction
						.select()
						.from(ModelBehaviourSettings)
						.orderBy(asc(ModelBehaviourSettings.setting_id));
					const provider_rows = yield* transaction
						.select()
						.from(ModelBehaviourProviderStates)
						.orderBy(
							asc(ModelBehaviourProviderStates.setting_id),
							asc(ModelBehaviourProviderStates.provider_id),
						);

					if (settings.length === 0) {
						return yield* new JournalInvariantError({
							message: "The model behaviour default setting row is missing",
						});
					}

					return {
						provider_states: yield* Effect.forEach(provider_rows, DecodeProviderState),
						settings: yield* Effect.forEach(settings, DecodeSetting),
					};
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		const ReadDuplicate = (message_id: string) =>
			database.client
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.correlation_id, message_id))
				.orderBy(asc(JournalEvents.sequence))
				.pipe(
					Effect.flatMap((events) => Effect.forEach(events, DecodeEvent)),
					Effect.mapError(normalize_error),
				);

		const Preflight = (operation: ModelBehaviourOperation) =>
			Effect.gen(function* () {
				const [existing] = yield* database.client
					.select({
						origin: JournalCommands.origin,
						payload_json: JournalCommands.payload_json,
						thread_id: JournalCommands.thread_id,
					})
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, operation.message_id))
					.limit(1);

				if (!existing) {
					return { _tag: "Proceed" } as const;
				}

				const identity = yield* DecodeCommandIdentity(existing.payload_json);

				if (
					existing.origin !== operation.origin ||
					existing.thread_id !== model_behaviour_thread_id ||
					identity.request_fingerprint !== operation.request_fingerprint
				) {
					return yield* new CommandIdConflict({ message_id: operation.message_id });
				}

				return {
					_tag: "Duplicate" as const,
					events: yield* ReadDuplicate(operation.message_id),
				};
			}).pipe(Effect.mapError(normalize_error));

		const Commit = (input: ModelBehaviourCommit) =>
			Effect.gen(function* () {
				const payload_json = command_payload_json(input.operation);
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select({
								origin: JournalCommands.origin,
								payload_json: JournalCommands.payload_json,
								thread_id: JournalCommands.thread_id,
							})
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, input.operation.message_id))
							.limit(1);

						if (existing) {
							const identity = yield* DecodeCommandIdentity(existing.payload_json);

							if (
								existing.origin !== input.operation.origin ||
								existing.thread_id !== model_behaviour_thread_id ||
								identity.request_fingerprint !== input.operation.request_fingerprint
							) {
								return yield* new CommandIdConflict({
									message_id: input.operation.message_id,
								});
							}

							return { _tag: "Duplicate" as const };
						}

						const occurred_at = yield* metadata.Now;
						const payloads: Array<typeof ModelBehaviourEventPayload.Type> = [];

						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							message_id: input.operation.message_id,
							origin: input.operation.origin,
							payload_json,
							payload_type: "model_behaviour.commit",
							schema_version: 1,
							sent_at: input.operation.sent_at,
							status: "accepted",
							thread_id: model_behaviour_thread_id,
						});

						if (input.setting_update) {
							const [stored_setting] = yield* transaction
								.select()
								.from(ModelBehaviourSettings)
								.where(
									eq(
										ModelBehaviourSettings.setting_id,
										input.setting_update.setting_id,
									),
								)
								.limit(1);

							if (!stored_setting) {
								return yield* new JournalInvariantError({
									message: `Model behaviour setting ${input.setting_update.setting_id} is missing`,
								});
							}

							const stored = yield* DecodeSetting(stored_setting);
							const version = stored.version + 1;

							yield* transaction
								.update(ModelBehaviourSettings)
								.set({
									updated_at: occurred_at,
									value_json: JSON.stringify(input.setting_update.value),
									version,
								})
								.where(
									eq(
										ModelBehaviourSettings.setting_id,
										input.setting_update.setting_id,
									),
								);

							payloads.push({
								setting_id: input.setting_update.setting_id,
								type: "model_behaviour.setting.updated",
								value: input.setting_update.value,
								version,
							});
						}

						for (const provider_state of input.provider_states) {
							yield* transaction
								.insert(ModelBehaviourProviderStates)
								.values({
									applied_hash: provider_state.applied_hash ?? null,
									backup_path: provider_state.backup_path ?? null,
									ignored_drift_hash: provider_state.ignored_drift_hash ?? null,
									last_error_code: provider_state.last_error_code ?? null,
									native_key: provider_state.native_key ?? null,
									observed_hash: provider_state.observed_hash ?? null,
									provider_id: provider_state.provider_id,
									setting_id: provider_state.setting_id,
									status: provider_state.status,
									target_path: provider_state.target_path ?? null,
									updated_at: occurred_at,
								})
								.onConflictDoUpdate({
									target: [
										ModelBehaviourProviderStates.provider_id,
										ModelBehaviourProviderStates.setting_id,
									],
									set: {
										applied_hash: provider_state.applied_hash ?? null,
										backup_path: provider_state.backup_path ?? null,
										ignored_drift_hash:
											provider_state.ignored_drift_hash ?? null,
										last_error_code: provider_state.last_error_code ?? null,
										native_key: provider_state.native_key ?? null,
										observed_hash: provider_state.observed_hash ?? null,
										status: provider_state.status,
										target_path: provider_state.target_path ?? null,
										updated_at: occurred_at,
									},
								});

							payloads.push({
								...(provider_state.applied_hash === undefined
									? {}
									: { applied_hash: provider_state.applied_hash }),
								...(provider_state.ignored_drift_hash === undefined
									? {}
									: { ignored_drift_hash: provider_state.ignored_drift_hash }),
								...(provider_state.last_error_code === undefined
									? {}
									: { last_error_code: provider_state.last_error_code }),
								...(provider_state.observed_hash === undefined
									? {}
									: { observed_hash: provider_state.observed_hash }),
								provider_id: provider_state.provider_id,
								setting_id: provider_state.setting_id,
								status: provider_state.status,
								type: "model_behaviour.provider.reconciled",
							});
						}

						if (payloads.length === 0) {
							return { _tag: "Accepted" as const, events: [] };
						}

						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, model_behaviour_stream_id))
							.limit(1);
						const first_sequence = (stream?.last_sequence ?? 0) + 1;
						const last_sequence = first_sequence + payloads.length - 1;

						if (stream) {
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence })
								.where(eq(EventStreams.stream_id, model_behaviour_stream_id));
						} else {
							yield* transaction.insert(EventStreams).values({
								last_sequence,
								stream_id: model_behaviour_stream_id,
							});
						}

						const events = yield* Effect.forEach(payloads, (payload, index) =>
							Effect.gen(function* () {
								const event_id = yield* metadata.MakeId("event");
								const sequence = first_sequence + index;
								const [event_row] = yield* transaction
									.insert(JournalEvents)
									.values({
										causation_id: input.operation.message_id,
										correlation_id: input.operation.message_id,
										event_id,
										event_type: payload.type,
										occurred_at,
										origin: "backend",
										payload_json: JSON.stringify(payload),
										schema_version: 1,
										stream_id: model_behaviour_stream_id,
										stream_sequence: sequence,
										thread_id: model_behaviour_thread_id,
									})
									.returning();

								if (!event_row) {
									return yield* new JournalInvariantError({
										message: "Model behaviour journal event was not persisted",
									});
								}

								return yield* DecodeEvent(event_row);
							}),
						);

						return { _tag: "Accepted" as const, events };
					}),
				);

				if (result._tag === "Duplicate") {
					return {
						events: yield* ReadDuplicate(input.operation.message_id),
						status: "duplicate" as const,
					};
				}

				const watermark = result.events.at(-1)?.journal_sequence;

				if (watermark !== undefined) {
					yield* notifier.Publish(watermark);
				}

				return { events: result.events, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		return { Commit, Preflight, Read };
	}),
);
