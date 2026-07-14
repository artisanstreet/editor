import { and, asc, eq, gt, isNull, lte, ne, or } from "drizzle-orm";
import { Context, Data, DateTime, Effect, Layer, Option, Schema } from "effect";

import {
	Identifier,
	IsoDateTime,
	ProjectRef,
	EventEnvelope,
	WorkspaceGitFetchQueryResult,
	type EventEnvelope as EventEnvelopeValue,
	type WorkspaceGitFetchCompletedEvent,
	type WorkspaceGitFetchPolicyUpdatedEvent,
	type WorkspaceGitFetchRequestedEvent,
	WorkspaceGitFetchResult,
	type WorkspaceGitFetchQueryResult as WorkspaceGitFetchQueryResultValue,
	type WorkspaceGitFetchResult as WorkspaceGitFetchResultValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	Projects,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceGitCheckoutClaims,
	WorkspaceGitFetchOperations,
	WorkspaceGitFetchPolicies,
	WorkspaceGitFetchStates,
	WorkspaceGitMutationClaims,
	WorkspaceMutationAuthorities,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { settings_scope_id, settings_stream_id } from "../settings/internal-scope";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));

/** Identifies the private settings stream used for global fetch-policy receipts. */
export const workspace_git_fetch_thread_id = settings_scope_id("git-fetch");

const PolicyUpdate = Schema.Struct({
	enabled: Schema.Boolean,
	message_id: Identifier,
	request_fingerprint: RequestFingerprint,
	sent_at: IsoDateTime,
});

const ManualPreparation = Schema.Struct({
	attempt_id: Identifier,
	message_id: Identifier,
	request_fingerprint: RequestFingerprint,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const ManualClaim = Schema.Struct({
	lease_expires_at: IsoDateTime,
	lease_owner: Identifier,
	message_id: Identifier,
	now: IsoDateTime,
});

const AutomaticClaim = Schema.Struct({
	attempt_id: Identifier,
	due_before: IsoDateTime,
	lease_expires_at: IsoDateTime,
	lease_owner: Identifier,
	now: IsoDateTime,
	workspace_id: Identifier,
});

const ClaimVerification = Schema.Struct({
	attempt_id: Identifier,
	lease_expires_at: IsoDateTime,
	lease_owner: Identifier,
	now: IsoDateTime,
	workspace_id: Identifier,
});

const ClaimCompletion = Schema.Struct({
	attempt_id: Identifier,
	attempted_at: IsoDateTime,
	lease_owner: Identifier,
	result: WorkspaceGitFetchResult,
	workspace_id: Identifier,
});

const ClaimRelease = Schema.Struct({
	attempt_id: Identifier,
	lease_owner: Identifier,
	workspace_id: Identifier,
});

export type WorkspaceGitFetchPolicyUpdate = typeof PolicyUpdate.Type;
export type WorkspaceGitFetchManualPreparation = typeof ManualPreparation.Type;
export type WorkspaceGitFetchManualClaim = typeof ManualClaim.Type;
export type WorkspaceGitFetchAutomaticClaim = typeof AutomaticClaim.Type;
export type WorkspaceGitFetchClaimVerification = typeof ClaimVerification.Type;
export type WorkspaceGitFetchClaimCompletion = typeof ClaimCompletion.Type;
export type WorkspaceGitFetchClaimRelease = typeof ClaimRelease.Type;

export interface WorkspaceGitFetchPolicy {
	readonly enabled: boolean;
}

export interface WorkspaceGitFetchPolicyAcceptance {
	readonly event: EventEnvelopeValue;
	readonly policy: WorkspaceGitFetchPolicy;
	readonly status: "accepted" | "duplicate";
}

export interface WorkspaceGitFetchManualAcceptance {
	readonly event: EventEnvelopeValue;
	readonly operation: WorkspaceGitFetchManualOperation;
	readonly status: "accepted" | "duplicate";
}

export interface WorkspaceGitFetchManualOperation {
	readonly attempt_id: string;
	readonly attempted_at?: string;
	readonly message_id: string;
	readonly result?: WorkspaceGitFetchResultValue;
	readonly status: "pending" | "terminal";
	readonly thread_id: string;
	readonly workspace_id: string;
}

export interface WorkspaceGitFetchClaim {
	readonly attempt_id: string;
	readonly kind: "automatic" | "manual";
	readonly message_id?: string;
	readonly workspace_id: string;
}

export class WorkspaceGitFetchConflict extends Data.TaggedError("WorkspaceGitFetchConflict")<{
	readonly reason: "claim_conflict" | "request_conflict";
}> {}

export class WorkspaceGitFetchUnavailable extends Data.TaggedError("WorkspaceGitFetchUnavailable")<{
	readonly reason: "erased" | "missing" | "unattached";
}> {}

export class WorkspaceGitFetchInvariant extends Data.TaggedError("WorkspaceGitFetchInvariant")<{
	readonly message: string;
}> {}

export class WorkspaceGitFetchStorage extends Data.TaggedError("WorkspaceGitFetchStorage")<{
	readonly cause: unknown;
}> {}

export type WorkspaceGitFetchRepositoryError =
	| WorkspaceGitFetchConflict
	| WorkspaceGitFetchInvariant
	| WorkspaceGitFetchStorage
	| WorkspaceGitFetchUnavailable;

export class WorkspaceGitFetchRepository extends Context.Service<
	WorkspaceGitFetchRepository,
	{
		readonly ClaimAutomatic: (
			input: WorkspaceGitFetchAutomaticClaim,
		) => Effect.Effect<Option.Option<WorkspaceGitFetchClaim>, WorkspaceGitFetchRepositoryError>;
		readonly ClaimManual: (
			input: WorkspaceGitFetchManualClaim,
		) => Effect.Effect<Option.Option<WorkspaceGitFetchClaim>, WorkspaceGitFetchRepositoryError>;
		readonly CompleteClaim: (
			input: WorkspaceGitFetchClaimCompletion,
		) => Effect.Effect<void, WorkspaceGitFetchRepositoryError>;
		readonly ListPendingManual: Effect.Effect<
			ReadonlyArray<WorkspaceGitFetchManualOperation>,
			WorkspaceGitFetchRepositoryError
		>;
		readonly PrepareManual: (
			input: WorkspaceGitFetchManualPreparation,
		) => Effect.Effect<WorkspaceGitFetchManualAcceptance, WorkspaceGitFetchRepositoryError>;
		readonly Query: Effect.Effect<
			WorkspaceGitFetchQueryResultValue,
			WorkspaceGitFetchRepositoryError
		>;
		readonly ReadManual: (
			message_id: string,
		) => Effect.Effect<
			Option.Option<WorkspaceGitFetchManualOperation>,
			WorkspaceGitFetchRepositoryError
		>;
		readonly ReadPolicy: Effect.Effect<
			WorkspaceGitFetchPolicy,
			WorkspaceGitFetchRepositoryError
		>;
		readonly ReleaseClaim: (
			input: WorkspaceGitFetchClaimRelease,
		) => Effect.Effect<void, WorkspaceGitFetchRepositoryError>;
		readonly UpdatePolicy: (
			input: WorkspaceGitFetchPolicyUpdate,
		) => Effect.Effect<WorkspaceGitFetchPolicyAcceptance, WorkspaceGitFetchRepositoryError>;
		readonly VerifyClaim: (
			input: WorkspaceGitFetchClaimVerification,
		) => Effect.Effect<Option.Option<WorkspaceGitFetchClaim>, WorkspaceGitFetchRepositoryError>;
	}
>()("Artisan/WorkspaceGitFetchRepository") {}

type OperationRow = typeof WorkspaceGitFetchOperations.$inferSelect;
type StateRow = typeof WorkspaceGitFetchStates.$inferSelect;

function invariant(message: string) {
	return new WorkspaceGitFetchInvariant({ message });
}

function normalize_error(error: unknown): WorkspaceGitFetchRepositoryError {
	if (
		error instanceof WorkspaceGitFetchConflict ||
		error instanceof WorkspaceGitFetchInvariant ||
		error instanceof WorkspaceGitFetchUnavailable ||
		error instanceof WorkspaceGitFetchStorage
	) {
		return error;
	}

	return new WorkspaceGitFetchStorage({ cause: error });
}

const DecodeIdentifier = (input: unknown) =>
	Schema.decodeUnknownEffect(Identifier, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodePolicyUpdate = (input: unknown) =>
	Schema.decodeUnknownEffect(PolicyUpdate, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodeManualPreparation = (input: unknown) =>
	Schema.decodeUnknownEffect(ManualPreparation, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodeManualClaim = (input: unknown) =>
	Schema.decodeUnknownEffect(ManualClaim, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodeAutomaticClaim = (input: unknown) =>
	Schema.decodeUnknownEffect(AutomaticClaim, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodeClaimVerification = (input: unknown) =>
	Schema.decodeUnknownEffect(ClaimVerification, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodeClaimCompletion = (input: unknown) =>
	Schema.decodeUnknownEffect(ClaimCompletion, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

const DecodeClaimRelease = (input: unknown) =>
	Schema.decodeUnknownEffect(ClaimRelease, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new WorkspaceGitFetchUnavailable({ reason: "missing" })),
	);

function DecodeDateTime(value: string, label: string) {
	return Schema.decodeUnknownEffect(IsoDateTime)(value).pipe(
		Effect.flatMap((timestamp) =>
			Option.match(DateTime.make(timestamp), {
				onNone: () => Effect.fail(invariant(`${label} is not a valid timestamp`)),
				onSome: Effect.succeed,
			}),
		),
		Effect.mapError(() => invariant(`${label} is not a valid timestamp`)),
	);
}

function DecodeProjectRef(json: string, message: string) {
	return Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(ProjectRef, { onExcessProperty: "error" })),
		Effect.mapError(() => invariant(message)),
	);
}

function DecodeProjectRefs(json: string) {
	return Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
		Effect.flatMap(
			Schema.decodeUnknownEffect(Schema.Array(ProjectRef), { onExcessProperty: "error" }),
		),
		Effect.mapError(() => invariant("Stored thread project links are corrupt")),
	);
}

function DecodeOperation(row: OperationRow) {
	const attempt_id = row.attempt_id;
	const attempted_at = row.attempted_at;
	const status = row.status;
	const thread_id = row.thread_id;
	const workspace_id = row.workspace_id;

	if (
		row.kind !== "manual" ||
		thread_id === null ||
		workspace_id === null ||
		attempt_id === null ||
		(status !== "pending" && status !== "terminal") ||
		(status === "pending" && (row.result !== null || attempted_at !== null)) ||
		(status === "terminal" && (row.result === null || attempted_at === null))
	) {
		return Effect.fail(invariant(`Fetch operation ${row.message_id} is corrupt`));
	}

	return Effect.gen(function* () {
		const result =
			row.result === null
				? undefined
				: yield* Schema.decodeUnknownEffect(WorkspaceGitFetchResult)(row.result).pipe(
						Effect.mapError(() =>
							invariant(`Fetch operation ${row.message_id} has an invalid result`),
						),
					);

		if (attempted_at !== null) {
			yield* DecodeDateTime(attempted_at, `Fetch operation ${row.message_id} attempted_at`);
		}

		return {
			attempt_id,
			...(attempted_at === null ? {} : { attempted_at }),
			message_id: row.message_id,
			...(result === undefined ? {} : { result }),
			status,
			thread_id,
			workspace_id,
		} satisfies WorkspaceGitFetchManualOperation;
	});
}

function DecodeClaim(row: StateRow) {
	const active_attempt_id = row.active_attempt_id;
	const active_kind = row.active_kind;
	const active_message_id = row.active_message_id;
	const lease_expires_at = row.lease_expires_at;
	const started_at = row.started_at;

	if (
		active_attempt_id === null ||
		(active_kind !== "automatic" && active_kind !== "manual") ||
		row.lease_owner === null ||
		lease_expires_at === null ||
		started_at === null ||
		(active_kind === "automatic" && active_message_id !== null) ||
		(active_kind === "manual" && active_message_id === null)
	) {
		return Effect.fail(
			invariant(`Fetch state ${row.workspace_id} has an invalid active claim`),
		);
	}

	return Effect.gen(function* () {
		yield* DecodeDateTime(started_at, `Fetch state ${row.workspace_id} started_at`);
		yield* DecodeDateTime(lease_expires_at, `Fetch state ${row.workspace_id} lease_expires_at`);

		return {
			attempt_id: active_attempt_id,
			kind: active_kind,
			...(active_message_id === null ? {} : { message_id: active_message_id }),
			workspace_id: row.workspace_id,
		} satisfies WorkspaceGitFetchClaim;
	});
}

function command_matches(
	row: typeof JournalCommands.$inferSelect,
	input: {
		readonly message_id: string;
		readonly payload_json: string;
		readonly payload_type: string;
		readonly sent_at: string;
		readonly thread_id: string;
	},
) {
	return (
		row.message_id === input.message_id &&
		row.schema_version === 1 &&
		row.thread_id === input.thread_id &&
		row.run_id === null &&
		row.agent_id === null &&
		row.causation_id === null &&
		row.origin === "frontend" &&
		row.raw_origin_json === null &&
		row.sent_at === input.sent_at &&
		row.payload_type === input.payload_type &&
		row.payload_json === input.payload_json &&
		row.status === "accepted"
	);
}

function DecodeEventRow(row: typeof JournalEvents.$inferSelect) {
	return Effect.gen(function* () {
		const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
			row.payload_json,
		);
		const raw_origin =
			row.raw_origin_json === null
				? undefined
				: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
						row.raw_origin_json,
					);

		return yield* Schema.decodeUnknownEffect(EventEnvelope, {
			onExcessProperty: "error",
		})({
			causation_id: row.causation_id,
			correlation_id: row.correlation_id,
			journal_sequence: row.sequence,
			kind: "event",
			message_id: row.event_id,
			origin: row.origin,
			payload,
			protocol_version: 1,
			...(raw_origin === undefined ? {} : { raw_origin }),
			...(row.run_id === null ? {} : { run_id: row.run_id }),
			...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
			schema_version: row.schema_version,
			sequence: row.stream_sequence,
			sent_at: row.occurred_at,
			stream_id: row.stream_id,
			thread_id: row.thread_id,
		});
	}).pipe(Effect.mapError(() => invariant(`Fetch event ${row.event_id} is corrupt`)));
}

/** Supplies durable policy, exact replay, and leased local-fetch state. */
export const WorkspaceGitFetchRepositoryLive = Layer.effect(
	WorkspaceGitFetchRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ReadAcceptanceEvent = (
			transaction: typeof database.client,
			message_id: string,
			event_type: string,
		) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, message_id),
						eq(JournalEvents.event_type, event_type),
					),
				)
				.orderBy(asc(JournalEvents.sequence))
				.limit(2)
				.pipe(
					Effect.flatMap((rows) => {
						if (rows.length !== 1) {
							return Effect.fail(
								invariant(
									`Fetch command ${message_id} has an invalid event history`,
								),
							);
						}

						return DecodeEventRow(rows[0]!);
					}),
				);

		const AppendEvent = (
			transaction: typeof database.client,
			input: {
				readonly causation_id: string;
				readonly correlation_id: string;
				readonly event_type: string;
				readonly idempotency_key: string;
				readonly payload:
					| WorkspaceGitFetchCompletedEvent
					| WorkspaceGitFetchPolicyUpdatedEvent
					| WorkspaceGitFetchRequestedEvent;
				readonly require_stream: boolean;
				readonly stream_id: string;
				readonly thread_id: string;
			},
		) =>
			Effect.gen(function* () {
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, input.stream_id))
					.limit(1);

				if (!stream && input.require_stream) {
					return yield* invariant(`Fetch thread ${input.thread_id} has no event stream`);
				}

				const occurred_at = yield* metadata.Now;
				const event_id = yield* metadata.MakeId("event");
				const sequence = (stream?.last_sequence ?? 0) + 1;

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: sequence })
						.where(eq(EventStreams.stream_id, input.stream_id));
				} else {
					yield* transaction.insert(EventStreams).values({
						last_sequence: sequence,
						stream_id: input.stream_id,
					});
				}

				const [event_row] = yield* transaction
					.insert(JournalEvents)
					.values({
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: input.event_type,
						idempotency_key: input.idempotency_key,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(input.payload),
						schema_version: 1,
						stream_id: input.stream_id,
						stream_sequence: sequence,
						thread_id: input.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					journal_sequence: event_row!.journal_sequence,
					kind: "event",
					message_id: event_id,
					origin: "backend",
					payload: input.payload,
					protocol_version: 1,
					schema_version: 1,
					sequence,
					sent_at: occurred_at,
					stream_id: input.stream_id,
					thread_id: input.thread_id,
				}).pipe(Effect.mapError(() => invariant("Fetch event projection is invalid")));
			});

		const ReadPolicy = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [row] = yield* transaction
						.select()
						.from(WorkspaceGitFetchPolicies)
						.where(eq(WorkspaceGitFetchPolicies.policy_id, 1))
						.limit(1);

					if (!row) {
						return yield* invariant("Fetch policy row is missing");
					}

					return { enabled: row.enabled };
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		const HasWriter = (transaction: typeof database.client, workspace_id: string) =>
			Effect.gen(function* () {
				const [change] = yield* transaction
					.select({ message_id: WorkspaceChangeOperations.message_id })
					.from(WorkspaceChangeOperations)
					.innerJoin(
						WorkspaceMutationAuthorities,
						eq(
							WorkspaceMutationAuthorities.message_id,
							WorkspaceChangeOperations.message_id,
						),
					)
					.where(
						and(
							eq(WorkspaceMutationAuthorities.workspace_id, workspace_id),
							ne(WorkspaceChangeOperations.lifecycle, "committed"),
							ne(WorkspaceChangeOperations.lifecycle, "rejected"),
						),
					)
					.limit(1);
				const [checkout] = yield* transaction
					.select({ workspace_id: WorkspaceGitCheckoutClaims.workspace_id })
					.from(WorkspaceGitCheckoutClaims)
					.where(eq(WorkspaceGitCheckoutClaims.workspace_id, workspace_id))
					.limit(1);
				const [mutation] = yield* transaction
					.select({ workspace_id: WorkspaceGitMutationClaims.workspace_id })
					.from(WorkspaceGitMutationClaims)
					.where(eq(WorkspaceGitMutationClaims.workspace_id, workspace_id))
					.limit(1);

				return change !== undefined || checkout !== undefined || mutation !== undefined;
			});

		const ReadManual = (message_id: string) =>
			DecodeIdentifier(message_id).pipe(
				Effect.flatMap((decoded) =>
					database.client
						.transaction((transaction) =>
							transaction
								.select()
								.from(WorkspaceGitFetchOperations)
								.where(eq(WorkspaceGitFetchOperations.message_id, decoded))
								.limit(1)
								.pipe(
									Effect.flatMap(([row]) =>
										Effect.gen(function* () {
											if (row === undefined) {
												return Option.none<WorkspaceGitFetchManualOperation>();
											}

											if (row.kind !== "manual") {
												return yield* new WorkspaceGitFetchConflict({
													reason: "request_conflict",
												});
											}

											return Option.some(yield* DecodeOperation(row));
										}),
									),
								),
						)
						.pipe(Effect.mapError(normalize_error)),
				),
			);

		const UpdatePolicy = (input: WorkspaceGitFetchPolicyUpdate) =>
			DecodePolicyUpdate(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const thread_id = workspace_git_fetch_thread_id;
								const stream_id = settings_stream_id("git-fetch");
								const command_payload = {
									enabled: decoded.enabled,
									type: "workspace.git.fetch.policy.update",
								};
								const payload_json = JSON.stringify(command_payload);
								const [operation] = yield* transaction
									.select()
									.from(WorkspaceGitFetchOperations)
									.where(
										eq(
											WorkspaceGitFetchOperations.message_id,
											decoded.message_id,
										),
									)
									.limit(1);

								if (operation) {
									const matches =
										operation.kind === "policy" &&
										operation.request_fingerprint ===
											decoded.request_fingerprint &&
										operation.sent_at === decoded.sent_at &&
										operation.enabled === decoded.enabled;

									if (!matches) {
										return yield* new WorkspaceGitFetchConflict({
											reason: "request_conflict",
										});
									}

									const [command] = yield* transaction
										.select()
										.from(JournalCommands)
										.where(eq(JournalCommands.message_id, decoded.message_id))
										.limit(1);

									if (
										!command ||
										!command_matches(command, {
											message_id: decoded.message_id,
											payload_json,
											payload_type: command_payload.type,
											sent_at: decoded.sent_at,
											thread_id,
										})
									) {
										return yield* invariant(
											`Fetch policy command ${decoded.message_id} is corrupt`,
										);
									}

									return {
										event: yield* ReadAcceptanceEvent(
											transaction,
											decoded.message_id,
											"workspace.git.fetch.policy.updated",
										),
										policy: { enabled: decoded.enabled },
										status: "duplicate" as const,
									};
								}

								const updated_at = yield* metadata.Now;

								yield* transaction.insert(JournalCommands).values({
									accepted_at: updated_at,
									message_id: decoded.message_id,
									origin: "frontend",
									payload_json,
									payload_type: command_payload.type,
									schema_version: 1,
									sent_at: decoded.sent_at,
									status: "accepted",
									thread_id,
								});
								yield* transaction
									.insert(WorkspaceGitFetchPolicies)
									.values({ enabled: decoded.enabled, policy_id: 1, updated_at })
									.onConflictDoUpdate({
										target: WorkspaceGitFetchPolicies.policy_id,
										set: { enabled: decoded.enabled, updated_at },
									});
								yield* transaction.insert(WorkspaceGitFetchOperations).values({
									created_at: updated_at,
									enabled: decoded.enabled,
									kind: "policy",
									message_id: decoded.message_id,
									request_fingerprint: decoded.request_fingerprint,
									sent_at: decoded.sent_at,
									status: "terminal",
									updated_at,
								});

								const event = yield* AppendEvent(transaction, {
									causation_id: decoded.message_id,
									correlation_id: decoded.message_id,
									event_type: "workspace.git.fetch.policy.updated",
									idempotency_key: `workspace_git_fetch:policy:${decoded.message_id}`,
									payload: {
										enabled: decoded.enabled,
										type: "workspace.git.fetch.policy.updated",
									},
									require_stream: false,
									stream_id,
									thread_id,
								});

								return {
									event,
									policy: { enabled: decoded.enabled },
									status: "accepted" as const,
								};
							}),
						),
					).pipe(
						Effect.tap((acceptance) =>
							acceptance.status === "accepted"
								? notifier.Publish(acceptance.event.journal_sequence)
								: Effect.void,
						),
						Effect.mapError(normalize_error),
					),
				),
			);

		const PrepareManual = (input: WorkspaceGitFetchManualPreparation) =>
			DecodeManualPreparation(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const command_payload = {
									type: "workspace.git.fetch.request",
									workspace_id: decoded.workspace_id,
								};
								const payload_json = JSON.stringify(command_payload);
								const [existing] = yield* transaction
									.select()
									.from(WorkspaceGitFetchOperations)
									.where(
										eq(
											WorkspaceGitFetchOperations.message_id,
											decoded.message_id,
										),
									)
									.limit(1);

								if (existing) {
									const matches =
										existing.kind === "manual" &&
										existing.request_fingerprint ===
											decoded.request_fingerprint &&
										existing.sent_at === decoded.sent_at &&
										existing.thread_id === decoded.thread_id &&
										existing.workspace_id === decoded.workspace_id;

									if (!matches) {
										return yield* new WorkspaceGitFetchConflict({
											reason: "request_conflict",
										});
									}

									const [command] = yield* transaction
										.select()
										.from(JournalCommands)
										.where(eq(JournalCommands.message_id, decoded.message_id))
										.limit(1);

									if (
										!command ||
										!command_matches(command, {
											message_id: decoded.message_id,
											payload_json,
											payload_type: command_payload.type,
											sent_at: decoded.sent_at,
											thread_id: decoded.thread_id,
										})
									) {
										return yield* invariant(
											`Fetch request command ${decoded.message_id} is corrupt`,
										);
									}

									return {
										event: yield* ReadAcceptanceEvent(
											transaction,
											decoded.message_id,
											"workspace.git.fetch.requested",
										),
										operation: yield* DecodeOperation(existing),
										status: "duplicate" as const,
									};
								}

								const [thread] = yield* transaction
									.select()
									.from(Threads)
									.where(eq(Threads.thread_id, decoded.thread_id))
									.limit(1);
								const [erasure] = yield* transaction
									.select({ thread_id: ThreadErasureClaims.thread_id })
									.from(ThreadErasureClaims)
									.where(eq(ThreadErasureClaims.thread_id, decoded.thread_id))
									.limit(1);
								const [tombstone] = yield* transaction
									.select({ thread_id: ThreadTombstones.thread_id })
									.from(ThreadTombstones)
									.where(eq(ThreadTombstones.thread_id, decoded.thread_id))
									.limit(1);

								if (!thread || erasure || tombstone) {
									return yield* new WorkspaceGitFetchUnavailable({
										reason: "erased",
									});
								}

								const [project] = yield* transaction
									.select()
									.from(Projects)
									.where(eq(Projects.workspace_id, decoded.workspace_id))
									.limit(1);

								if (!project) {
									return yield* new WorkspaceGitFetchUnavailable({
										reason: "missing",
									});
								}

								const linked_projects = yield* DecodeProjectRefs(
									thread.linked_projects_json,
								);
								const primary_project =
									thread.primary_project_id === null
										? undefined
										: thread.primary_project_json === null
											? yield* invariant(
													"Stored primary thread project is missing",
												)
											: yield* DecodeProjectRef(
													thread.primary_project_json,
													"Stored primary thread project is corrupt",
												);
								const attached =
									primary_project?.project_id === project.project_id ||
									linked_projects.some(
										(linked) => linked.project_id === project.project_id,
									);

								if (!attached) {
									return yield* new WorkspaceGitFetchUnavailable({
										reason: "unattached",
									});
								}

								const created_at = yield* metadata.Now;

								yield* transaction
									.insert(WorkspaceGitFetchStates)
									.values({
										workspace_id: decoded.workspace_id,
									})
									.onConflictDoNothing();
								yield* transaction.insert(WorkspaceGitFetchOperations).values({
									attempt_id: decoded.attempt_id,
									created_at,
									kind: "manual",
									message_id: decoded.message_id,
									request_fingerprint: decoded.request_fingerprint,
									sent_at: decoded.sent_at,
									status: "pending",
									thread_id: decoded.thread_id,
									updated_at: created_at,
									workspace_id: decoded.workspace_id,
								});
								yield* transaction.insert(JournalCommands).values({
									accepted_at: created_at,
									message_id: decoded.message_id,
									origin: "frontend",
									payload_json,
									payload_type: command_payload.type,
									schema_version: 1,
									sent_at: decoded.sent_at,
									status: "accepted",
									thread_id: decoded.thread_id,
								});
								const event = yield* AppendEvent(transaction, {
									causation_id: decoded.message_id,
									correlation_id: decoded.message_id,
									event_type: "workspace.git.fetch.requested",
									idempotency_key: `workspace_git_fetch:requested:${decoded.message_id}`,
									payload: {
										type: "workspace.git.fetch.requested",
										workspace_id: decoded.workspace_id,
									},
									require_stream: true,
									stream_id: `thread:${decoded.thread_id}`,
									thread_id: decoded.thread_id,
								});

								return {
									event,
									operation: {
										attempt_id: decoded.attempt_id,
										message_id: decoded.message_id,
										status: "pending" as const,
										thread_id: decoded.thread_id,
										workspace_id: decoded.workspace_id,
									},
									status: "accepted" as const,
								};
							}),
						),
					).pipe(
						Effect.tap((acceptance) =>
							acceptance.status === "accepted"
								? notifier.Publish(acceptance.event.journal_sequence)
								: Effect.void,
						),
						Effect.mapError(normalize_error),
					),
				),
			);

		const ClaimManual = (input: WorkspaceGitFetchManualClaim) =>
			DecodeManualClaim(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [operation] = yield* transaction
									.select()
									.from(WorkspaceGitFetchOperations)
									.where(
										eq(
											WorkspaceGitFetchOperations.message_id,
											decoded.message_id,
										),
									)
									.limit(1);

								if (!operation || operation.kind !== "manual") {
									return Option.none();
								}

								const manual = yield* DecodeOperation(operation);

								if (
									manual.status !== "pending" ||
									(yield* HasWriter(transaction, manual.workspace_id))
								) {
									return Option.none();
								}

								const [oldest] = yield* transaction
									.select({ message_id: WorkspaceGitFetchOperations.message_id })
									.from(WorkspaceGitFetchOperations)
									.where(
										and(
											eq(WorkspaceGitFetchOperations.kind, "manual"),
											eq(WorkspaceGitFetchOperations.status, "pending"),
											eq(
												WorkspaceGitFetchOperations.workspace_id,
												manual.workspace_id,
											),
										),
									)
									.orderBy(asc(WorkspaceGitFetchOperations.created_at))
									.limit(1);

								if (!oldest || oldest.message_id !== manual.message_id) {
									return Option.none();
								}

								const [state] = yield* transaction
									.select()
									.from(WorkspaceGitFetchStates)
									.where(
										eq(
											WorkspaceGitFetchStates.workspace_id,
											manual.workspace_id,
										),
									)
									.limit(1);

								if (!state) {
									return yield* invariant(
										`Fetch state ${manual.workspace_id} is missing`,
									);
								}

								const active =
									state.active_attempt_id === null
										? undefined
										: yield* DecodeClaim(state);
								const expired =
									state.lease_expires_at !== null &&
									state.lease_expires_at <= decoded.now;
								const recoverable =
									active?.kind === "manual" &&
									active.message_id === manual.message_id &&
									expired;

								if (active && !recoverable) {
									return Option.none();
								}

								const [claimed] = yield* transaction
									.update(WorkspaceGitFetchStates)
									.set({
										active_attempt_id: manual.attempt_id,
										active_kind: "manual",
										active_message_id: manual.message_id,
										lease_expires_at: decoded.lease_expires_at,
										lease_owner: decoded.lease_owner,
										started_at: decoded.now,
									})
									.where(
										and(
											eq(
												WorkspaceGitFetchStates.workspace_id,
												manual.workspace_id,
											),
											or(
												isNull(WorkspaceGitFetchStates.active_attempt_id),
												and(
													eq(
														WorkspaceGitFetchStates.active_kind,
														"manual",
													),
													eq(
														WorkspaceGitFetchStates.active_message_id,
														manual.message_id,
													),
													lte(
														WorkspaceGitFetchStates.lease_expires_at,
														decoded.now,
													),
												),
											),
										),
									)
									.returning({
										workspace_id: WorkspaceGitFetchStates.workspace_id,
									});

								return claimed
									? Option.some({
											attempt_id: manual.attempt_id,
											kind: "manual" as const,
											message_id: manual.message_id,
											workspace_id: manual.workspace_id,
										})
									: Option.none();
							}),
						),
					).pipe(Effect.mapError(normalize_error)),
				),
			);

		const ClaimAutomatic = (input: WorkspaceGitFetchAutomaticClaim) =>
			DecodeAutomaticClaim(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								if (yield* HasWriter(transaction, decoded.workspace_id)) {
									return Option.none();
								}

								const [policy] = yield* transaction
									.select()
									.from(WorkspaceGitFetchPolicies)
									.where(eq(WorkspaceGitFetchPolicies.policy_id, 1))
									.limit(1);

								if (!policy?.enabled) {
									return Option.none();
								}

								const [pending] = yield* transaction
									.select({ message_id: WorkspaceGitFetchOperations.message_id })
									.from(WorkspaceGitFetchOperations)
									.where(
										and(
											eq(WorkspaceGitFetchOperations.kind, "manual"),
											eq(WorkspaceGitFetchOperations.status, "pending"),
											eq(
												WorkspaceGitFetchOperations.workspace_id,
												decoded.workspace_id,
											),
										),
									)
									.limit(1);

								if (pending) {
									return Option.none();
								}

								const [state] = yield* transaction
									.select()
									.from(WorkspaceGitFetchStates)
									.where(
										eq(
											WorkspaceGitFetchStates.workspace_id,
											decoded.workspace_id,
										),
									)
									.limit(1);

								if (
									state?.last_attempted_at !== null &&
									state?.last_attempted_at !== undefined &&
									state.last_attempted_at > decoded.due_before
								) {
									return Option.none();
								}

								const active =
									state?.active_attempt_id === null || state === undefined
										? undefined
										: yield* DecodeClaim(state);
								const expired =
									state?.lease_expires_at !== null &&
									state?.lease_expires_at !== undefined &&
									state.lease_expires_at <= decoded.now;
								const recoverable = active?.kind === "automatic" && expired;
								const attempt_id = recoverable
									? active.attempt_id
									: decoded.attempt_id;

								if (active && !recoverable) {
									return Option.none();
								}

								if (!state) {
									yield* transaction.insert(WorkspaceGitFetchStates).values({
										active_attempt_id: attempt_id,
										active_kind: "automatic",
										lease_expires_at: decoded.lease_expires_at,
										lease_owner: decoded.lease_owner,
										started_at: decoded.now,
										workspace_id: decoded.workspace_id,
									});
								} else {
									const [claimed] = yield* transaction
										.update(WorkspaceGitFetchStates)
										.set({
											active_attempt_id: attempt_id,
											active_kind: "automatic",
											active_message_id: null,
											lease_expires_at: decoded.lease_expires_at,
											lease_owner: decoded.lease_owner,
											started_at: decoded.now,
										})
										.where(
											and(
												eq(
													WorkspaceGitFetchStates.workspace_id,
													decoded.workspace_id,
												),
												or(
													isNull(
														WorkspaceGitFetchStates.active_attempt_id,
													),
													and(
														eq(
															WorkspaceGitFetchStates.active_kind,
															"automatic",
														),
														lte(
															WorkspaceGitFetchStates.lease_expires_at,
															decoded.now,
														),
													),
												),
											),
										)
										.returning({
											workspace_id: WorkspaceGitFetchStates.workspace_id,
										});

									if (!claimed) {
										return Option.none();
									}
								}

								return Option.some({
									attempt_id,
									kind: "automatic" as const,
									workspace_id: decoded.workspace_id,
								});
							}),
						),
					).pipe(Effect.mapError(normalize_error)),
				),
			);

		const VerifyClaim = (input: WorkspaceGitFetchClaimVerification) =>
			DecodeClaimVerification(input).pipe(
				Effect.flatMap((decoded) =>
					database.client
						.transaction((transaction) =>
							Effect.gen(function* () {
								if (yield* HasWriter(transaction, decoded.workspace_id)) {
									return Option.none();
								}

								const [state] = yield* transaction
									.update(WorkspaceGitFetchStates)
									.set({ lease_expires_at: decoded.lease_expires_at })
									.where(
										and(
											eq(
												WorkspaceGitFetchStates.workspace_id,
												decoded.workspace_id,
											),
											eq(
												WorkspaceGitFetchStates.active_attempt_id,
												decoded.attempt_id,
											),
											eq(
												WorkspaceGitFetchStates.lease_owner,
												decoded.lease_owner,
											),
											gt(
												WorkspaceGitFetchStates.lease_expires_at,
												decoded.now,
											),
										),
									)
									.returning();

								return state === undefined
									? Option.none()
									: Option.some(yield* DecodeClaim(state));
							}),
						)
						.pipe(Effect.mapError(normalize_error)),
				),
			);

		const CompleteClaim = (input: WorkspaceGitFetchClaimCompletion) =>
			DecodeClaimCompletion(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [state] = yield* transaction
									.select()
									.from(WorkspaceGitFetchStates)
									.where(
										eq(
											WorkspaceGitFetchStates.workspace_id,
											decoded.workspace_id,
										),
									)
									.limit(1);

								if (
									!state ||
									state.active_attempt_id !== decoded.attempt_id ||
									state.lease_owner !== decoded.lease_owner
								) {
									return yield* new WorkspaceGitFetchConflict({
										reason: "claim_conflict",
									});
								}

								const claim = yield* DecodeClaim(state);
								const [updated] = yield* transaction
									.update(WorkspaceGitFetchStates)
									.set({
										active_attempt_id: null,
										active_kind: null,
										active_message_id: null,
										last_attempted_at: decoded.attempted_at,
										last_result: decoded.result,
										lease_expires_at: null,
										lease_owner: null,
										started_at: null,
										version: state.version + 1,
									})
									.where(
										and(
											eq(
												WorkspaceGitFetchStates.workspace_id,
												decoded.workspace_id,
											),
											eq(
												WorkspaceGitFetchStates.active_attempt_id,
												decoded.attempt_id,
											),
											eq(
												WorkspaceGitFetchStates.lease_owner,
												decoded.lease_owner,
											),
										),
									)
									.returning({
										workspace_id: WorkspaceGitFetchStates.workspace_id,
									});

								if (!updated) {
									return yield* new WorkspaceGitFetchConflict({
										reason: "claim_conflict",
									});
								}

								if (claim.kind === "manual") {
									const [operation] = yield* transaction
										.update(WorkspaceGitFetchOperations)
										.set({
											attempted_at: decoded.attempted_at,
											result: decoded.result,
											status: "terminal",
											updated_at: decoded.attempted_at,
										})
										.where(
											and(
												eq(
													WorkspaceGitFetchOperations.message_id,
													claim.message_id ?? "",
												),
												eq(WorkspaceGitFetchOperations.kind, "manual"),
												eq(
													WorkspaceGitFetchOperations.attempt_id,
													decoded.attempt_id,
												),
												eq(WorkspaceGitFetchOperations.status, "pending"),
											),
										)
										.returning({
											message_id: WorkspaceGitFetchOperations.message_id,
											thread_id: WorkspaceGitFetchOperations.thread_id,
										});

									if (
										!operation ||
										operation.thread_id === null ||
										claim.message_id === undefined
									) {
										return yield* invariant(
											"Manual fetch operation disappeared during completion",
										);
									}

									return Option.some(
										yield* AppendEvent(transaction, {
											causation_id: claim.message_id,
											correlation_id: claim.message_id,
											event_type: "workspace.git.fetch.completed",
											idempotency_key: `workspace_git_fetch:completed:${claim.message_id}`,
											payload: {
												attempt: {
													attempted_at: decoded.attempted_at,
													result: decoded.result,
												},
												type: "workspace.git.fetch.completed",
												workspace_id: decoded.workspace_id,
											},
											require_stream: true,
											stream_id: `thread:${operation.thread_id}`,
											thread_id: operation.thread_id,
										}),
									);
								}

								return Option.none<EventEnvelopeValue>();
							}),
						),
					).pipe(
						Effect.tap((event) =>
							Option.match(event, {
								onNone: () => Effect.void,
								onSome: (completed) => notifier.Publish(completed.journal_sequence),
							}),
						),
						Effect.asVoid,
						Effect.mapError(normalize_error),
					),
				),
			);

		const ReleaseClaim = (input: WorkspaceGitFetchClaimRelease) =>
			DecodeClaimRelease(input).pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							transaction
								.update(WorkspaceGitFetchStates)
								.set({
									active_attempt_id: null,
									active_kind: null,
									active_message_id: null,
									lease_expires_at: null,
									lease_owner: null,
									started_at: null,
								})
								.where(
									and(
										eq(
											WorkspaceGitFetchStates.workspace_id,
											decoded.workspace_id,
										),
										eq(
											WorkspaceGitFetchStates.active_attempt_id,
											decoded.attempt_id,
										),
										eq(
											WorkspaceGitFetchStates.lease_owner,
											decoded.lease_owner,
										),
									),
								)
								.returning({ workspace_id: WorkspaceGitFetchStates.workspace_id })
								.pipe(
									Effect.flatMap(([released]) =>
										released
											? Effect.void
											: Effect.fail(
													new WorkspaceGitFetchConflict({
														reason: "claim_conflict",
													}),
												),
									),
								),
						),
					).pipe(Effect.mapError(normalize_error)),
				),
			);

		const ListPendingManual = database.client
			.transaction((transaction) =>
				transaction
					.select()
					.from(WorkspaceGitFetchOperations)
					.where(
						and(
							eq(WorkspaceGitFetchOperations.kind, "manual"),
							eq(WorkspaceGitFetchOperations.status, "pending"),
						),
					)
					.orderBy(asc(WorkspaceGitFetchOperations.created_at))
					.pipe(Effect.flatMap((rows) => Effect.forEach(rows, DecodeOperation))),
			)
			.pipe(Effect.mapError(normalize_error));

		const Query = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [policy] = yield* transaction
						.select()
						.from(WorkspaceGitFetchPolicies)
						.where(eq(WorkspaceGitFetchPolicies.policy_id, 1))
						.limit(1);
					const rows = yield* transaction
						.select()
						.from(WorkspaceGitFetchStates)
						.orderBy(asc(WorkspaceGitFetchStates.workspace_id));

					const workspaces = yield* Effect.forEach(rows, (row) =>
						Effect.gen(function* () {
							if ((row.last_attempted_at === null) !== (row.last_result === null)) {
								return yield* invariant(
									`Fetch state ${row.workspace_id} has an invalid result`,
								);
							}

							if (row.last_attempted_at === null || row.last_result === null) {
								return { workspace_id: row.workspace_id };
							}

							yield* DecodeDateTime(
								row.last_attempted_at,
								`Fetch state ${row.workspace_id} attempted_at`,
							);
							const result = yield* Schema.decodeUnknownEffect(
								WorkspaceGitFetchResult,
							)(row.last_result).pipe(
								Effect.mapError(() =>
									invariant(
										`Fetch state ${row.workspace_id} has an invalid result`,
									),
								),
							);

							return {
								last_attempt: { attempted_at: row.last_attempted_at, result },
								workspace_id: row.workspace_id,
							};
						}),
					);

					if (!policy) {
						return yield* invariant("Fetch policy row is missing");
					}

					return yield* Schema.decodeUnknownEffect(WorkspaceGitFetchQueryResult, {
						onExcessProperty: "error",
					})({ enabled: policy.enabled, workspaces }).pipe(
						Effect.mapError(() => invariant("Fetch query projection is invalid")),
					);
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		return {
			ClaimAutomatic,
			ClaimManual,
			CompleteClaim,
			ListPendingManual,
			PrepareManual,
			Query,
			ReadManual,
			ReadPolicy,
			ReleaseClaim,
			UpdatePolicy,
			VerifyClaim,
		};
	}),
);
