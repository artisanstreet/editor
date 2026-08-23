import { asc, eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";

import type {
	CommandEnvelope,
	EventEnvelope,
	SessionDefaults as SessionDefaultsSnapshot,
	SessionDefaultsUpdatedEvent,
} from "@artisan/protocol";
import {
	AgentNameDataset,
	DefaultAgentNameDatasetId,
	DefaultThreadTitleMode,
	NormalizePermissionId,
	ThreadTitleMode,
} from "@artisan/protocol";

import { settings_scope_id, settings_stream_id } from "./internal-scope";

export const session_defaults_thread_id = settings_scope_id("session-defaults");
const session_defaults_stream_id = settings_stream_id("session-defaults");

/** The singleton row's key; there is one set of defaults per Forge. */
const defaults_row_id = 1;

/** Applied until the operator chooses otherwise; the manifest's own default. */
const initial_permission = "autonomous";

import { Database } from "../persistence/database";
import {
	DisabledEngines,
	EventStreams,
	JournalCommands,
	JournalEvents,
	SessionDefaults,
	SessionModelDefaults,
} from "../persistence/tables";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStore,
	JournalStoreFailure,
	type JournalStoreError,
} from "../persistence/journal-store";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/metadata";

/** Returns one durable defaults change and its canonical event. */
export interface SessionDefaultsAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/**
 * Owns the defaults a new draft inherits.
 *
 * Forge is the authority rather than any one browser: a second client, or the
 * same client after clearing storage, must see the same permission mode. The
 * draft's in-flight policy stays client-side until its first send, so reading
 * these defaults never creates a durable thread.
 *
 * @since 0.8.0
 */
export class SessionDefaultsService extends Context.Service<
	SessionDefaultsService,
	{
		readonly Read: Effect.Effect<SessionDefaultsSnapshot, JournalStoreError>;
		readonly Update: (
			command: CommandEnvelope,
		) => Effect.Effect<SessionDefaultsAcceptance, JournalStoreError>;
	}
>()("Artisan/SessionDefaultsService") {}

function command_matches(
	command: CommandEnvelope,
	existing: {
		readonly agent_id: string | null;
		readonly causation_id: string | null;
		readonly origin: string;
		readonly payload_json: string;
		readonly raw_origin_json: string | null;
		readonly run_id: string | null;
		readonly schema_version: number;
		readonly sent_at: string;
		readonly thread_id: string;
	},
) {
	return (
		existing.payload_json === JSON.stringify(command.payload) &&
		existing.thread_id === command.thread_id &&
		existing.run_id === (command.run_id ?? null) &&
		existing.agent_id === (command.agent_id ?? null) &&
		existing.causation_id === (command.causation_id ?? null) &&
		existing.origin === command.origin &&
		existing.raw_origin_json ===
			(command.raw_origin ? JSON.stringify(command.raw_origin) : null) &&
		existing.schema_version === command.schema_version &&
		existing.sent_at === command.sent_at
	);
}

function normalize_error(error: unknown): JournalStoreError {
	if (error instanceof CommandIdConflict || error instanceof JournalInvariantError) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

const ReasoningEfforts = new Set(["low", "medium", "high", "xhigh", "max", "ultra"]);
const is_service_tier = Schema.is(Schema.NonEmptyString);
const is_thread_title_mode = Schema.is(ThreadTitleMode);

/** Rows are decoded defensively: retired or malformed controls are dropped. */
const ModelRow = (row: {
	readonly context_window: string | null;
	readonly model_id: string;
	readonly reasoning_effort: string | null;
	readonly service_tier: string | null;
}) => ({
	...(row.context_window === null ? {} : { context_window: row.context_window }),
	model_id: row.model_id,
	...(row.reasoning_effort === null || !ReasoningEfforts.has(row.reasoning_effort)
		? {}
		: {
				reasoning_effort:
					row.reasoning_effort as SessionDefaultsSnapshot["models"][number]["reasoning_effort"],
			}),
	...(row.service_tier === null || !is_service_tier(row.service_tier)
		? {}
		: { service_tier: row.service_tier }),
});

export const SessionDefaultsServiceLive = Layer.effect(
	SessionDefaultsService,
	Effect.gen(function* () {
		const database = yield* Database;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ReadSnapshot = <Client extends typeof database.client>(client: Client) =>
			Effect.gen(function* () {
				const [shared] = yield* client
					.select({
						agent_name_dataset: SessionDefaults.agent_name_dataset,
						auto_continue_usage_limits: SessionDefaults.auto_continue_usage_limits,
						compaction_model_id: SessionDefaults.compaction_model_id,
						last_model_id: SessionDefaults.last_model_id,
						onboarding_completed: SessionDefaults.onboarding_completed,
						permission: SessionDefaults.permission,
						thread_title_mode: SessionDefaults.thread_title_mode,
					})
					.from(SessionDefaults)
					.where(eq(SessionDefaults.defaults_id, defaults_row_id))
					.limit(1);
				const models = yield* client
					.select({
						context_window: SessionModelDefaults.context_window,
						model_id: SessionModelDefaults.model_id,
						reasoning_effort: SessionModelDefaults.reasoning_effort,
						service_tier: SessionModelDefaults.service_tier,
					})
					.from(SessionModelDefaults)
					.orderBy(asc(SessionModelDefaults.model_id));
				const disabled = yield* client
					.select({ engine_id: DisabledEngines.engine_id })
					.from(DisabledEngines)
					.orderBy(asc(DisabledEngines.engine_id));
				const stored_thread_title_mode = shared?.thread_title_mode;

				return {
					agent_name_dataset: yield* Schema.decodeUnknownEffect(AgentNameDataset)(
						shared?.agent_name_dataset ?? DefaultAgentNameDatasetId,
					),
					auto_continue_usage_limits: shared?.auto_continue_usage_limits ?? true,
					...(disabled.length > 0
						? { disabled_engines: disabled.map((row) => row.engine_id) }
						: {}),
					...(shared?.compaction_model_id
						? { compaction_model: shared.compaction_model_id }
						: {}),
					...(shared?.last_model_id ? { last_model_id: shared.last_model_id } : {}),
					onboarding_completed: shared?.onboarding_completed ?? false,
					models: models.map(ModelRow),
					permission: NormalizePermissionId(shared?.permission ?? initial_permission),
					thread_title_mode: is_thread_title_mode(stored_thread_title_mode)
						? stored_thread_title_mode
						: DefaultThreadTitleMode,
				} satisfies SessionDefaultsSnapshot;
			});

		const Read = ReadSnapshot(database.client).pipe(Effect.mapError(normalize_error));

		const Update = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				if (
					command.payload.type !== "session.defaults.update" ||
					command.thread_id !== session_defaults_thread_id
				) {
					return yield* new JournalInvariantError({
						message: "Session defaults updates require the canonical defaults scope",
					});
				}

				const payload = command.payload;
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
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

						if (existing) {
							if (!command_matches(command, existing)) {
								return yield* new CommandIdConflict({
									message_id: command.message_id,
								});
							}

							return { _tag: "Duplicate" as const };
						}

						const accepted_at = yield* metadata.Now;
						const event_id = yield* metadata.MakeId("event");
						const stream_id = session_defaults_stream_id;
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;

						/**
						 * Every field is a patch, so a composer that only moved one
						 * control cannot overwrite another client's change to the rest.
						 */
						if (
							payload.agent_name_dataset !== undefined ||
							payload.auto_continue_usage_limits !== undefined ||
							payload.permission !== undefined ||
							payload.last_model_id !== undefined ||
							payload.onboarding_completed !== undefined ||
							payload.compaction_model !== undefined ||
							payload.thread_title_mode !== undefined
						) {
							const [current] = yield* transaction
								.select({
									agent_name_dataset: SessionDefaults.agent_name_dataset,
									auto_continue_usage_limits:
										SessionDefaults.auto_continue_usage_limits,
									compaction_model_id: SessionDefaults.compaction_model_id,
									last_model_id: SessionDefaults.last_model_id,
									onboarding_completed: SessionDefaults.onboarding_completed,
									permission: SessionDefaults.permission,
									thread_title_mode: SessionDefaults.thread_title_mode,
								})
								.from(SessionDefaults)
								.where(eq(SessionDefaults.defaults_id, defaults_row_id))
								.limit(1);
							/** An explicit `null` restores the curated default. */
							const compaction_model_id =
								payload.compaction_model === undefined
									? (current?.compaction_model_id ?? null)
									: payload.compaction_model;
							const shared = {
								agent_name_dataset:
									payload.agent_name_dataset ??
									current?.agent_name_dataset ??
									DefaultAgentNameDatasetId,
								auto_continue_usage_limits:
									payload.auto_continue_usage_limits ??
									current?.auto_continue_usage_limits ??
									true,
								compaction_model_id,
								last_model_id:
									payload.last_model_id ?? current?.last_model_id ?? null,
								onboarding_completed:
									payload.onboarding_completed ??
									current?.onboarding_completed ??
									false,
								permission: NormalizePermissionId(
									payload.permission ?? current?.permission ?? initial_permission,
								),
								thread_title_mode:
									payload.thread_title_mode ??
									current?.thread_title_mode ??
									DefaultThreadTitleMode,
								updated_at: accepted_at,
							};

							yield* transaction
								.insert(SessionDefaults)
								.values({ defaults_id: defaults_row_id, ...shared })
								.onConflictDoUpdate({
									set: shared,
									target: SessionDefaults.defaults_id,
								});
						}

						/**
						 * Availability is row-per-engine like the favorites: switching
						 * one engine never touches another's row, and re-enabling is a
						 * plain delete rather than a rewritten set.
						 */
						if (payload.engine !== undefined) {
							if (payload.engine.enabled) {
								yield* transaction
									.delete(DisabledEngines)
									.where(eq(DisabledEngines.engine_id, payload.engine.engine_id));
							} else {
								yield* transaction
									.insert(DisabledEngines)
									.values({
										disabled_at: accepted_at,
										engine_id: payload.engine.engine_id,
									})
									.onConflictDoNothing();
							}
						}

						if (payload.model !== undefined) {
							const model = payload.model;
							const [current] = yield* transaction
								.select({
									context_window: SessionModelDefaults.context_window,
									reasoning_effort: SessionModelDefaults.reasoning_effort,
									service_tier: SessionModelDefaults.service_tier,
								})
								.from(SessionModelDefaults)
								.where(eq(SessionModelDefaults.model_id, model.model_id))
								.limit(1);
							/** Model fields are independently patchable, just like shared defaults. */
							const context_window =
								model.context_window === undefined
									? (current?.context_window ?? null)
									: model.context_window;
							const reasoning_effort =
								model.reasoning_effort === undefined
									? (current?.reasoning_effort ?? null)
									: model.reasoning_effort;
							const service_tier =
								model.service_tier === undefined
									? (current?.service_tier ?? null)
									: model.service_tier;
							yield* transaction
								.insert(SessionModelDefaults)
								.values({
									context_window,
									model_id: model.model_id,
									reasoning_effort,
									service_tier,
									updated_at: accepted_at,
								})
								.onConflictDoUpdate({
									set: {
										context_window,
										reasoning_effort,
										service_tier,
										updated_at: accepted_at,
									},
									target: SessionModelDefaults.model_id,
								});
						}

						const defaults = yield* ReadSnapshot(transaction);
						const event_payload = {
							defaults,
							type: "session.defaults.updated" as const,
						} satisfies SessionDefaultsUpdatedEvent;

						yield* transaction.insert(JournalCommands).values({
							accepted_at,
							agent_id: command.agent_id ?? null,
							causation_id: command.causation_id ?? null,
							message_id: command.message_id,
							origin: command.origin,
							payload_json: JSON.stringify(command.payload),
							payload_type: command.payload.type,
							raw_origin_json: command.raw_origin
								? JSON.stringify(command.raw_origin)
								: null,
							run_id: command.run_id ?? null,
							schema_version: command.schema_version,
							sent_at: command.sent_at,
							status: "accepted",
							thread_id: command.thread_id,
						});

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

						const [event_row] = yield* transaction
							.insert(JournalEvents)
							.values({
								agent_id: command.agent_id ?? null,
								causation_id: command.message_id,
								correlation_id: command.message_id,
								event_id,
								event_type: event_payload.type,
								occurred_at: accepted_at,
								origin: "backend",
								payload_json: JSON.stringify(event_payload),
								raw_origin_json: command.raw_origin
									? JSON.stringify(command.raw_origin)
									: null,
								run_id: command.run_id ?? null,
								schema_version: 1,
								stream_id,
								stream_sequence: sequence,
								thread_id: command.thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						if (event_row === undefined)
							return yield* new JournalInvariantError({
								message: `Session defaults event ${event_id} returned no inserted row`,
							});
						const event: EventEnvelope = {
							...(command.agent_id ? { agent_id: command.agent_id } : {}),
							causation_id: command.message_id,
							correlation_id: command.message_id,
							journal_sequence: event_row.journal_sequence,
							kind: "event",
							message_id: event_id,
							origin: "backend",
							payload: event_payload,
							protocol_version: 1,
							...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
							...(command.run_id ? { run_id: command.run_id } : {}),
							schema_version: 1,
							sequence,
							sent_at: accepted_at,
							stream_id,
							thread_id: command.thread_id,
						};

						return { _tag: "Accepted" as const, event };
					}),
				);

				if (result._tag === "Duplicate") {
					const [event] = yield* journal.ReadCorrelatedEvents(command.message_id);

					if (!event) {
						return yield* new JournalInvariantError({
							message: `Session defaults command ${command.message_id} has no event`,
						});
					}

					return { event, status: "duplicate" as const };
				}

				yield* notifier.Publish(result.event.journal_sequence);

				return { event: result.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		return { Read, Update };
	}),
);
