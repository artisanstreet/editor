import { and, asc, eq, isNull, ne, sql } from "drizzle-orm";
import { Context, Crypto, Data, DateTime, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	Identifier,
	IsoDateTime,
	EventPayload,
	InvokeRequest,
	ToolApprovalProjection,
	ToolArguments,
	ToolDescriptor,
	ToolInvocationProjection,
	ToolResult,
	type ToolInvocationProjection as ToolInvocationProjectionValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	ToolExecutionClaims,
	ToolInvocationPrivate,
	ToolInvocations,
	ToolThreadDispatchState,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";

const execution_lease_seconds = 30;
const text_encoder = new TextEncoder();

export const ToolExecutionIdentity = Schema.Struct({
	claim_token: Identifier,
	invocation_id: Identifier,
});

export class ToolExecutionConflict extends Data.TaggedError("ToolExecutionConflict")<{
	readonly reason: "invalid_transition" | "lease_conflict";
}> {}

export class ToolExecutionUnavailable extends Data.TaggedError("ToolExecutionUnavailable")<{
	readonly reason: "erased" | "missing";
}> {}

export class ToolExecutionInvariant extends Data.TaggedError("ToolExecutionInvariant")<{
	readonly message: string;
}> {}

export class ToolExecutionPersistenceFailure extends Data.TaggedError(
	"ToolExecutionPersistenceFailure",
)<{
	readonly reason: "unexpected";
}> {}

export type ToolExecutionRepositoryError =
	| ToolExecutionConflict
	| ToolExecutionInvariant
	| ToolExecutionPersistenceFailure
	| ToolExecutionUnavailable;

export interface ToolExecution {
	readonly arguments: typeof ToolArguments.Type;
	readonly claim_token: string;
	readonly invocation: ToolInvocationProjectionValue;
	readonly launch_started: boolean;
	readonly result?: typeof ToolResult.Type;
}

export interface CompletedToolExecutionRead {
	readonly invocation: ToolInvocationProjectionValue;
	readonly result: typeof ToolResult.Type;
}

export interface ToolExecutionDispatch {
	readonly identity?: typeof ToolExecutionIdentity.Type;
	readonly invocation_id: string;
	readonly recovery: "owned" | "quarantine" | "recoverable" | "waiting";
	readonly thread_id: string;
}

/** Owns durable phase-two claims, execution fencing, recovery, and source-safe settlement journals. */
export class ToolExecutionRepository extends Context.Service<
	ToolExecutionRepository,
	{
		readonly AbandonExecution: (
			identity: typeof ToolExecutionIdentity.Type,
		) => Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly AbandonOwnedExecutions: Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly ActiveClaimsForThread: (
			thread_id: string,
		) => Effect.Effect<boolean, ToolExecutionRepositoryError>;
		readonly BeginThreadQuiescence: (
			thread_id: string,
		) => Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly ClaimPending: (
			invocation_id: string,
		) => Effect.Effect<Option.Option<ToolExecution>, ToolExecutionRepositoryError>;
		readonly ClaimRecovery: (
			invocation_id: string,
		) => Effect.Effect<Option.Option<ToolExecution>, ToolExecutionRepositoryError>;
		readonly ListPending: Effect.Effect<
			ReadonlyArray<{ readonly invocation_id: string; readonly thread_id: string }>,
			ToolExecutionRepositoryError
		>;
		readonly ListRunning: Effect.Effect<
			ReadonlyArray<ToolExecutionDispatch>,
			ToolExecutionRepositoryError
		>;
		readonly MarkLaunchStarted: (
			identity: typeof ToolExecutionIdentity.Type,
		) => Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly QuarantineInterrupted: (
			invocation_id: string,
		) => Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly ReadCompleted: (
			request: typeof InvokeRequest.Type,
		) => Effect.Effect<Option.Option<CompletedToolExecutionRead>, ToolExecutionRepositoryError>;
		readonly ReadExecution: (
			identity: typeof ToolExecutionIdentity.Type,
		) => Effect.Effect<ToolExecution, ToolExecutionRepositoryError>;
		readonly RenewLease: (
			identity: typeof ToolExecutionIdentity.Type,
		) => Effect.Effect<void, ToolExecutionRepositoryError>;
		readonly SettleCompleted: (
			identity: typeof ToolExecutionIdentity.Type,
			result: unknown,
		) => Effect.Effect<ToolInvocationProjectionValue, ToolExecutionRepositoryError>;
		readonly SettleFailed: (
			identity: typeof ToolExecutionIdentity.Type,
		) => Effect.Effect<ToolInvocationProjectionValue, ToolExecutionRepositoryError>;
		readonly ThreadQuiescencePending: (
			thread_id: string,
		) => Effect.Effect<boolean, ToolExecutionRepositoryError>;
	}
>()("Artisan/ToolExecutionRepository") {}

type InvocationRow = typeof ToolInvocations.$inferSelect;
type ClaimRow = typeof ToolExecutionClaims.$inferSelect;

function invariant(message: string) {
	return new ToolExecutionInvariant({ message });
}

function conflict(reason: ToolExecutionConflict["reason"]) {
	return new ToolExecutionConflict({ reason });
}

function normalize_error(error: unknown): ToolExecutionRepositoryError {
	if (
		error instanceof ToolExecutionConflict ||
		error instanceof ToolExecutionInvariant ||
		error instanceof ToolExecutionPersistenceFailure ||
		error instanceof ToolExecutionUnavailable
	) {
		return error;
	}

	return new ToolExecutionPersistenceFailure({ reason: "unexpected" });
}

function canonical_json(value: unknown): string {
	if (Array.isArray(value)) {
		return `[${value.map(canonical_json).join(",")}]`;
	}

	if (typeof value === "object" && value !== null) {
		const record = value as Readonly<Record<string, unknown>>;

		return `{${Object.keys(record)
			.toSorted()
			.map((key) => `${JSON.stringify(key)}:${canonical_json(record[key])}`)
			.join(",")}}`;
	}

	return JSON.stringify(value);
}

function Decode<A>(schema: Schema.Codec<A, A>, value: unknown, message: string) {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(() => invariant(message)),
	);
}

function DecodeStoredJson<A>(schema: Schema.Codec<A, A>, value: string, message: string) {
	return Effect.try({
		try: () => JSON.parse(value) as unknown,
		catch: () => invariant(message),
	}).pipe(Effect.flatMap((parsed) => Decode(schema, parsed, message)));
}

function DecodeDateTime(value: string, message: string) {
	return Decode(IsoDateTime, value, message).pipe(
		Effect.flatMap((decoded) =>
			Option.match(DateTime.make(decoded), {
				onNone: () => Effect.fail(invariant(message)),
				onSome: Effect.succeed,
			}),
		),
	);
}

/** Supplies the SQLite-backed phase-two tool execution repository. */
export const ToolExecutionRepositoryLive = Layer.effect(
	ToolExecutionRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const Hash = (value: string) =>
			crypto.digest("SHA-256", text_encoder.encode(value)).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(() =>
					invariant("Tool execution private data could not be verified"),
				),
			);

		const LeaseExpiry = (now: string) =>
			DecodeDateTime(now, "Tool execution clock is invalid").pipe(
				Effect.map((date_time) =>
					DateTime.formatIso(
						DateTime.add(date_time, { seconds: execution_lease_seconds }),
					),
				),
			);

		const IsExpired = (lease_expires_at: string, now: string) =>
			Effect.all([
				DecodeDateTime(lease_expires_at, "Tool execution lease is invalid"),
				DecodeDateTime(now, "Tool execution clock is invalid"),
			]).pipe(
				Effect.map(
					([expiry, current]) =>
						DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current),
				),
			);

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [[thread], [erasure], [tombstone]] = yield* Effect.all([
					transaction
						.select()
						.from(Threads)
						.where(eq(Threads.thread_id, thread_id))
						.limit(1),
					transaction
						.select()
						.from(ThreadErasureClaims)
						.where(eq(ThreadErasureClaims.thread_id, thread_id))
						.limit(1),
					transaction
						.select()
						.from(ThreadTombstones)
						.where(eq(ThreadTombstones.thread_id, thread_id))
						.limit(1),
				]);

				if (erasure || tombstone)
					return yield* new ToolExecutionUnavailable({ reason: "erased" });
				if (!thread) return yield* new ToolExecutionUnavailable({ reason: "missing" });
			});

		const DispatchAvailable = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [[thread], [erasure], [tombstone], [dispatch_state]] = yield* Effect.all([
					transaction
						.select({ thread_id: Threads.thread_id })
						.from(Threads)
						.where(eq(Threads.thread_id, thread_id))
						.limit(1),
					transaction
						.select({ thread_id: ThreadErasureClaims.thread_id })
						.from(ThreadErasureClaims)
						.where(eq(ThreadErasureClaims.thread_id, thread_id))
						.limit(1),
					transaction
						.select({ thread_id: ThreadTombstones.thread_id })
						.from(ThreadTombstones)
						.where(eq(ThreadTombstones.thread_id, thread_id))
						.limit(1),
					transaction
						.select({ quiesced_at: ToolThreadDispatchState.quiesced_at })
						.from(ToolThreadDispatchState)
						.where(eq(ToolThreadDispatchState.thread_id, thread_id))
						.limit(1),
				]);

				if (erasure || tombstone) {
					return false;
				}

				if (!thread) {
					return yield* new ToolExecutionUnavailable({ reason: "missing" });
				}

				return dispatch_state === undefined || dispatch_state.quiesced_at === null;
			});

		const SerializeDispatchAdmission = (
			transaction: typeof database.client,
			thread_id: string,
		) =>
			Effect.gen(function* () {
				yield* transaction
					.insert(ToolThreadDispatchState)
					.values({ admission_version: 0, quiesced_at: null, thread_id })
					.onConflictDoNothing();
				const [admitted] = yield* transaction
					.update(ToolThreadDispatchState)
					.set({
						admission_version: sql`${ToolThreadDispatchState.admission_version} + 1`,
					})
					.where(
						and(
							eq(ToolThreadDispatchState.thread_id, thread_id),
							isNull(ToolThreadDispatchState.quiesced_at),
						),
					)
					.returning({ thread_id: ToolThreadDispatchState.thread_id });

				return admitted !== undefined;
			});

		const DecodeInvocation = (row: InvocationRow) =>
			Effect.gen(function* () {
				const input_schema = yield* DecodeStoredJson(
					Schema.Unknown,
					row.input_schema_json,
					"Tool invocation descriptor is corrupt",
				);
				const invocation = {
					context: {
						agent_id: row.agent_id,
						run_id: row.run_id,
						thread_id: row.thread_id,
						...(row.workspace_id === null ? {} : { workspace_id: row.workspace_id }),
					},
					created_at: row.created_at,
					invocation_id: row.invocation_id,
					request_id: row.request_id,
					tool: {
						approval_policy: row.approval_policy,
						effect: row.effect,
						label: row.label,
						revision: row.revision,
						source: row.source,
						summary: row.summary,
						tool_id: row.tool_id,
					},
					updated_at: row.updated_at,
				};
				const approval =
					row.decision_id === null
						? undefined
						: {
								approval_id: row.approval_id,
								context: invocation.context,
								decided_at: row.decided_at,
								decision: row.decision,
								decision_id: row.decision_id,
								invocation_id: row.invocation_id,
								request_id: row.request_id,
								tool: { revision: row.revision, tool_id: row.tool_id },
							};

				const descriptor = yield* Decode(
					ToolDescriptor,
					{ ...invocation.tool, input_schema },
					"Tool invocation descriptor is corrupt",
				);
				const descriptor_fingerprint = yield* Hash(canonical_json(descriptor));
				if (descriptor_fingerprint !== row.descriptor_fingerprint) {
					return yield* invariant("Tool invocation descriptor is corrupt");
				}
				if (row.recovery_policy === "retry" && descriptor.effect !== "read") {
					return yield* invariant("Tool invocation recovery policy is unsafe");
				}

				const projection =
					row.state === "pending"
						? {
								...invocation,
								...(approval === undefined ? {} : { approval }),
								state: "pending",
							}
						: row.state === "running"
							? {
									...invocation,
									...(approval === undefined ? {} : { approval }),
									started_at: row.started_at,
									state: "running",
								}
							: row.state === "completed" ||
								  row.state === "failed" ||
								  row.state === "outcome_unknown"
								? {
										...invocation,
										...(approval === undefined ? {} : { approval }),
										settled_at: row.settled_at,
										started_at: row.started_at,
										state: row.state,
									}
								: undefined;

				if (!projection) return yield* invariant("Tool execution lifecycle is corrupt");

				return yield* Decode(
					ToolInvocationProjection,
					projection,
					"Tool execution projection is corrupt",
				);
			});

		const DecodeApproval = (row: InvocationRow) =>
			Effect.gen(function* () {
				if (row.approval_id === null || row.approval_policy !== "required") {
					return yield* invariant("Tool execution approval is corrupt");
				}

				const invocation = yield* DecodeInvocation(row);
				const state =
					row.state === "pending"
						? "approved"
						: row.state === "running"
							? "executing"
							: row.state === "completed" ||
								  row.state === "failed" ||
								  row.state === "outcome_unknown"
								? "settled"
								: undefined;
				if (!state) return yield* invariant("Tool execution approval is corrupt");

				return yield* Decode(
					ToolApprovalProjection,
					{
						approval_id: row.approval_id,
						context: invocation.context,
						created_at: row.created_at,
						decided_at: row.decided_at,
						decision_id: row.decision_id,
						...(state === "executing" ? { started_at: row.started_at } : {}),
						...(state === "settled"
							? { settled_at: row.settled_at, started_at: row.started_at }
							: {}),
						invocation_id: row.invocation_id,
						request_id: row.request_id,
						state,
						tool: invocation.tool,
						updated_at: row.updated_at,
					},
					"Tool execution approval is corrupt",
				);
			});

		const ReadPrivate = (transaction: typeof database.client, row: InvocationRow) =>
			Effect.gen(function* () {
				const [private_row] = yield* transaction
					.select()
					.from(ToolInvocationPrivate)
					.where(eq(ToolInvocationPrivate.invocation_id, row.invocation_id))
					.limit(1);
				if (!private_row) return yield* invariant("Tool execution private data is missing");

				const arguments_ = yield* DecodeStoredJson(
					ToolArguments,
					private_row.arguments_json,
					"Tool execution private arguments are corrupt",
				);
				const arguments_json = canonical_json(arguments_);
				const arguments_digest = yield* Hash(arguments_json);
				if (
					arguments_json !== private_row.arguments_json ||
					arguments_digest !== private_row.arguments_digest
				) {
					return yield* invariant("Tool execution private arguments are corrupt");
				}
				const request_fingerprint = yield* Hash(
					canonical_json({
						arguments: arguments_,
						context: {
							agent_id: row.agent_id,
							run_id: row.run_id,
							thread_id: row.thread_id,
							...(row.workspace_id === null
								? {}
								: { workspace_id: row.workspace_id }),
						},
						request_id: row.request_id,
						tool: { revision: row.revision, tool_id: row.tool_id },
					}),
				);

				if (request_fingerprint !== private_row.request_fingerprint) {
					return yield* invariant("Tool execution private arguments are corrupt");
				}

				const result =
					private_row.result_json === null
						? undefined
						: yield* DecodeStoredJson(
								ToolResult,
								private_row.result_json,
								"Tool execution private result is corrupt",
							);
				if (result !== undefined) {
					const result_json = canonical_json(result);
					const result_digest = yield* Hash(result_json);
					if (
						result_json !== private_row.result_json ||
						result_digest !== private_row.result_digest
					) {
						return yield* invariant("Tool execution private result is corrupt");
					}
				} else if (private_row.result_digest !== null) {
					return yield* invariant("Tool execution private result is corrupt");
				}

				return {
					arguments_,
					request_fingerprint: private_row.request_fingerprint,
					result,
				};
			});

		const VerifyExactRequest = (
			row: InvocationRow,
			request_fingerprint: string,
			request: typeof InvokeRequest.Type,
		) =>
			Effect.gen(function* () {
				if (
					request.request_id !== row.request_id ||
					request.context.thread_id !== row.thread_id ||
					request.context.run_id !== row.run_id ||
					request.context.agent_id !== row.agent_id ||
					(request.context.workspace_id ?? null) !== row.workspace_id ||
					request.tool.tool_id !== row.tool_id ||
					request.tool.revision !== row.revision
				) {
					return yield* invariant("Tool execution request does not match admission");
				}

				const supplied_fingerprint = yield* Hash(canonical_json(request));

				if (supplied_fingerprint !== request_fingerprint) {
					return yield* invariant("Tool execution request does not match admission");
				}
			});

		const ReadClaim = (transaction: typeof database.client, invocation_id: string) =>
			Effect.gen(function* () {
				const [claim] = yield* transaction
					.select()
					.from(ToolExecutionClaims)
					.where(eq(ToolExecutionClaims.invocation_id, invocation_id))
					.limit(1);
				if (!claim) return yield* conflict("lease_conflict");

				return claim;
			});

		const ReadRow = (transaction: typeof database.client, invocation_id: string) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(ToolInvocations)
					.where(eq(ToolInvocations.invocation_id, invocation_id))
					.limit(1);
				if (!row) return yield* new ToolExecutionUnavailable({ reason: "missing" });

				return row;
			});

		const AssertOwnedLiveClaim = (
			transaction: typeof database.client,
			identity: typeof ToolExecutionIdentity.Type,
		) =>
			Effect.gen(function* () {
				const claim = yield* ReadClaim(transaction, identity.invocation_id);
				const now = yield* metadata.Now;
				const expired = yield* IsExpired(claim.lease_expires_at, now);
				if (
					expired ||
					claim.claim_token !== identity.claim_token ||
					claim.owner_instance_id !== metadata.instance_id
				) {
					return yield* conflict("lease_conflict");
				}

				return { claim, now };
			});

		const Append = (
			transaction: typeof database.client,
			row: InvocationRow,
			occurred_at: string,
			payload: typeof EventPayload.Type,
			idempotency_key: string,
		) =>
			Effect.gen(function* () {
				const decoded_payload = yield* Decode(
					EventPayload,
					payload,
					"Tool execution event is invalid",
				);
				const stream_id = `thread:${row.thread_id}`;
				const [stream] = yield* transaction
					.select()
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");

				yield* RecordThreadActivity(
					transaction,
					row.thread_id,
					occurred_at,
					decoded_payload,
				);
				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: stream_sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction
						.insert(EventStreams)
						.values({ last_sequence: stream_sequence, stream_id });
				}
				const [event] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: row.agent_id,
						causation_id: row.invocation_id,
						correlation_id: row.request_id,
						event_id,
						event_type: decoded_payload.type,
						idempotency_key,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(decoded_payload),
						raw_origin_json: null,
						run_id: row.run_id,
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: row.thread_id,
					})
					.returning({ sequence: JournalEvents.sequence });
				if (!event) return yield* invariant("Tool execution event was not persisted");

				return event.sequence;
			});

		const UpdateProjection = (
			transaction: typeof database.client,
			row: InvocationRow,
			next_row: InvocationRow,
			event_suffix: string,
		) =>
			Effect.gen(function* () {
				const invocation = yield* DecodeInvocation(next_row);
				const descriptor = yield* Decode(
					ToolDescriptor,
					{
						approval_policy: next_row.approval_policy,
						effect: next_row.effect,
						input_schema: yield* DecodeStoredJson(
							Schema.Unknown,
							next_row.input_schema_json,
							"Tool invocation descriptor is corrupt",
						),
						label: next_row.label,
						revision: next_row.revision,
						source: next_row.source,
						summary: next_row.summary,
						tool_id: next_row.tool_id,
					},
					"Tool invocation descriptor is corrupt",
				);
				const approval =
					next_row.approval_policy === "required"
						? yield* DecodeApproval(next_row)
						: undefined;
				const capability_state =
					next_row.state === "running"
						? "running"
						: next_row.state === "completed"
							? "completed"
							: next_row.state === "failed"
								? "failed"
								: "outcome_unknown";
				let sequence = 0;

				if (approval !== undefined) {
					sequence = yield* Append(
						transaction,
						next_row,
						next_row.updated_at,
						{ approval, type: "tool.approval.updated" },
						`tool_approval:${next_row.approval_id}:${event_suffix}`,
					);
				}
				sequence = yield* Append(
					transaction,
					next_row,
					next_row.updated_at,
					{
						effect: descriptor.effect,
						invocation_id: next_row.invocation_id,
						label: next_row.label,
						source: descriptor.source,
						state: capability_state,
						summary: next_row.summary,
						type: "capability.invocation.updated",
					},
					`tool_invocation:${next_row.invocation_id}:${event_suffix}:capability`,
				);
				sequence = yield* Append(
					transaction,
					next_row,
					next_row.updated_at,
					{ invocation, type: "tool.invocation.updated" },
					`tool_invocation:${next_row.invocation_id}:${event_suffix}:full`,
				);

				const [updated] = yield* transaction
					.update(ToolInvocations)
					.set({ ...next_row, current_journal_sequence: sequence })
					.where(
						and(
							eq(ToolInvocations.invocation_id, row.invocation_id),
							eq(ToolInvocations.state, row.state),
							eq(ToolInvocations.updated_at, row.updated_at),
						),
					)
					.returning({ invocation_id: ToolInvocations.invocation_id });
				if (!updated) return yield* conflict("invalid_transition");

				return { invocation, sequence };
			});

		const BuildExecution = (
			transaction: typeof database.client,
			row: InvocationRow,
			claim: ClaimRow,
		) =>
			Effect.gen(function* () {
				const private_data = yield* ReadPrivate(transaction, row);
				const invocation = yield* DecodeInvocation(row);

				const execution: ToolExecution = {
					arguments: private_data.arguments_,
					claim_token: claim.claim_token,
					invocation,
					launch_started: claim.launch_started_at !== null,
					...(row.state === "completed" && private_data.result !== undefined
						? { result: private_data.result }
						: {}),
				};

				return execution;
			});

		const ClaimPending = (invocation_id: string) =>
			Decode(Identifier, invocation_id, "Tool execution identifier is invalid").pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const claimed = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);
									if (row.state !== "pending")
										return Option.none<{
											readonly execution: ToolExecution;
											readonly sequence: number;
										}>();
									yield* EnsureLiveThread(transaction, row.thread_id);
									const dispatch_available = yield* DispatchAvailable(
										transaction,
										row.thread_id,
									);

									if (!dispatch_available) {
										return Option.none();
									}

									const admitted = yield* SerializeDispatchAdmission(
										transaction,
										row.thread_id,
									);

									if (!admitted) {
										return Option.none();
									}

									const [active] = yield* transaction
										.select({ invocation_id: ToolInvocations.invocation_id })
										.from(ToolInvocations)
										.where(
											and(
												eq(ToolInvocations.thread_id, row.thread_id),
												eq(ToolInvocations.state, "running"),
											),
										)
										.limit(1);

									if (active) {
										return Option.none();
									}

									yield* ReadPrivate(transaction, row);
									const now = yield* metadata.Now;
									const claim_token = yield* metadata.MakeId("claim");
									const lease_expires_at = yield* LeaseExpiry(now);
									const next_row = {
										...row,
										started_at: now,
										state: "running",
										updated_at: now,
									};
									const [claim] = yield* transaction
										.insert(ToolExecutionClaims)
										.values({
											claim_token,
											claimed_at: now,
											invocation_id: row.invocation_id,
											launch_started_at: null,
											lease_expires_at,
											owner_instance_id: metadata.instance_id,
										})
										.returning();
									if (!claim) return yield* conflict("lease_conflict");
									const updated = yield* UpdateProjection(
										transaction,
										row,
										next_row,
										"running",
									);

									return Option.some({
										execution: yield* BuildExecution(
											transaction,
											next_row,
											claim,
										),
										sequence: updated.sequence,
									});
								}),
							),
						);
						if (Option.isNone(claimed)) return Option.none<ToolExecution>();
						yield* notifier.Publish(claimed.value.sequence).pipe(Effect.ignoreCause);

						return Option.some(claimed.value.execution);
					}),
				),
				Effect.mapError(normalize_error),
			);

		const ReadExecution = (identity: typeof ToolExecutionIdentity.Type) =>
			Decode(ToolExecutionIdentity, identity, "Tool execution identity is invalid").pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded.invocation_id);

							if (row.state !== "running") {
								return yield* conflict("invalid_transition");
							}

							yield* EnsureLiveThread(transaction, row.thread_id);
							const { claim } = yield* AssertOwnedLiveClaim(transaction, decoded);

							return yield* BuildExecution(transaction, row, claim);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadCompleted = (request: typeof InvokeRequest.Type) =>
			Decode(InvokeRequest, request, "Tool execution request is invalid").pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [row] = yield* transaction
								.select()
								.from(ToolInvocations)
								.where(eq(ToolInvocations.request_id, decoded.request_id))
								.limit(1);

							if (!row || row.state !== "completed") {
								return Option.none<CompletedToolExecutionRead>();
							}

							yield* EnsureLiveThread(transaction, row.thread_id);
							const private_data = yield* ReadPrivate(transaction, row);

							yield* VerifyExactRequest(
								row,
								private_data.request_fingerprint,
								decoded,
							);

							if (private_data.result === undefined) {
								return yield* invariant("Tool execution private result is missing");
							}

							return Option.some({
								invocation: yield* DecodeInvocation(row),
								result: private_data.result,
							});
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const RenewLease = (identity: typeof ToolExecutionIdentity.Type) =>
			Decode(ToolExecutionIdentity, identity, "Tool execution identity is invalid").pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const row = yield* ReadRow(transaction, decoded.invocation_id);
								yield* EnsureLiveThread(transaction, row.thread_id);
								const { claim, now } = yield* AssertOwnedLiveClaim(
									transaction,
									decoded,
								);
								const lease_expires_at = yield* LeaseExpiry(now);
								const [renewed] = yield* transaction
									.update(ToolExecutionClaims)
									.set({ lease_expires_at })
									.where(
										and(
											eq(
												ToolExecutionClaims.invocation_id,
												decoded.invocation_id,
											),
											eq(
												ToolExecutionClaims.claim_token,
												decoded.claim_token,
											),
											eq(
												ToolExecutionClaims.owner_instance_id,
												metadata.instance_id,
											),
											eq(
												ToolExecutionClaims.lease_expires_at,
												claim.lease_expires_at,
											),
										),
									)
									.returning({
										invocation_id: ToolExecutionClaims.invocation_id,
									});
								if (!renewed) return yield* conflict("lease_conflict");
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const MarkLaunchStarted = (identity: typeof ToolExecutionIdentity.Type) =>
			Decode(ToolExecutionIdentity, identity, "Tool execution identity is invalid").pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const row = yield* ReadRow(transaction, decoded.invocation_id);
								if (row.state !== "running")
									return yield* conflict("invalid_transition");
								yield* EnsureLiveThread(transaction, row.thread_id);
								const { claim, now } = yield* AssertOwnedLiveClaim(
									transaction,
									decoded,
								);

								if (claim.launch_started_at !== null) {
									return;
								}

								const [started] = yield* transaction
									.update(ToolExecutionClaims)
									.set({ launch_started_at: now })
									.where(
										and(
											eq(
												ToolExecutionClaims.invocation_id,
												decoded.invocation_id,
											),
											eq(
												ToolExecutionClaims.claim_token,
												decoded.claim_token,
											),
											eq(
												ToolExecutionClaims.owner_instance_id,
												metadata.instance_id,
											),
											eq(
												ToolExecutionClaims.lease_expires_at,
												claim.lease_expires_at,
											),
											isNull(ToolExecutionClaims.launch_started_at),
										),
									)
									.returning({
										invocation_id: ToolExecutionClaims.invocation_id,
									});
								if (!started) return yield* conflict("lease_conflict");
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const Settle = (
			identity: typeof ToolExecutionIdentity.Type,
			state: "completed" | "failed",
			result?: unknown,
		) =>
			Decode(ToolExecutionIdentity, identity, "Tool execution identity is invalid").pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const decoded_result =
							state === "completed"
								? yield* Decode(
										ToolResult,
										result,
										"Tool execution result is invalid",
									)
								: undefined;
						const result_json =
							decoded_result === undefined
								? undefined
								: canonical_json(decoded_result);
						const result_digest =
							result_json === undefined ? undefined : yield* Hash(result_json);
						const settled = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.invocation_id);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === state) {
										const claim = yield* ReadClaim(
											transaction,
											decoded.invocation_id,
										);

										if (
											claim.claim_token !== decoded.claim_token ||
											claim.owner_instance_id !== metadata.instance_id
										) {
											return yield* conflict("lease_conflict");
										}

										const private_data = yield* ReadPrivate(transaction, row);

										if (
											state === "completed" &&
											(private_data.result === undefined ||
												canonical_json(private_data.result) !== result_json)
										) {
											return yield* conflict("invalid_transition");
										}

										return {
											invocation: yield* DecodeInvocation(row),
											notify: false,
											sequence: row.current_journal_sequence,
										};
									}

									if (row.state !== "running") {
										return yield* conflict("invalid_transition");
									}

									const { claim, now } = yield* AssertOwnedLiveClaim(
										transaction,
										decoded,
									);
									if (claim.launch_started_at === null)
										return yield* conflict("invalid_transition");
									const next_row = {
										...row,
										settled_at: now,
										state,
										updated_at: now,
									};
									const [private_updated] = yield* transaction
										.update(ToolInvocationPrivate)
										.set(
											state === "completed"
												? {
														result_digest: result_digest!,
														result_json: result_json!,
													}
												: { result_digest: null, result_json: null },
										)
										.where(
											eq(
												ToolInvocationPrivate.invocation_id,
												row.invocation_id,
											),
										)
										.returning({
											invocation_id: ToolInvocationPrivate.invocation_id,
										});
									if (!private_updated)
										return yield* invariant(
											"Tool execution private data is missing",
										);
									const updated = yield* UpdateProjection(
										transaction,
										row,
										next_row,
										state,
									);
									return {
										invocation: updated.invocation,
										notify: true,
										sequence: updated.sequence,
									};
								}),
							),
						);
						if (settled.notify) {
							yield* notifier.Publish(settled.sequence).pipe(Effect.ignoreCause);
						}

						return settled.invocation;
					}),
				),
				Effect.mapError(normalize_error),
			);

		const SettleCompleted = (identity: typeof ToolExecutionIdentity.Type, result: unknown) =>
			Settle(identity, "completed", result);
		const SettleFailed = (identity: typeof ToolExecutionIdentity.Type) =>
			Settle(identity, "failed");

		const ListPending = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(ToolInvocations)
						.where(eq(ToolInvocations.state, "pending"))
						.orderBy(
							asc(ToolInvocations.created_at),
							asc(ToolInvocations.invocation_id),
						);
					const pending: Array<{
						readonly invocation_id: string;
						readonly thread_id: string;
					}> = [];

					for (const row of rows) {
						const dispatch_available = yield* DispatchAvailable(
							transaction,
							row.thread_id,
						);

						if (dispatch_available) {
							pending.push({
								invocation_id: row.invocation_id,
								thread_id: row.thread_id,
							});
						}
					}

					return pending;
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		const ListRunning = Effect.gen(function* () {
			const now = yield* metadata.Now;

			return yield* database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const rows = yield* transaction
							.select({ claim: ToolExecutionClaims, invocation: ToolInvocations })
							.from(ToolExecutionClaims)
							.innerJoin(
								ToolInvocations,
								eq(
									ToolExecutionClaims.invocation_id,
									ToolInvocations.invocation_id,
								),
							)
							.where(eq(ToolInvocations.state, "running"))
							.orderBy(
								asc(ToolInvocations.created_at),
								asc(ToolInvocations.invocation_id),
							);
						const dispatches: Array<ToolExecutionDispatch> = [];

						for (const { claim, invocation } of rows) {
							const dispatch_available = yield* DispatchAvailable(
								transaction,
								invocation.thread_id,
							);

							if (!dispatch_available) {
								continue;
							}

							yield* ReadPrivate(transaction, invocation);
							const expired = yield* IsExpired(claim.lease_expires_at, now);
							const recovery = !expired
								? claim.owner_instance_id === metadata.instance_id
									? "owned"
									: "waiting"
								: claim.launch_started_at === null ||
									  (invocation.recovery_policy === "retry" &&
											invocation.effect === "read")
									? "recoverable"
									: "quarantine";
							dispatches.push({
								...(recovery === "owned"
									? {
											identity: {
												claim_token: claim.claim_token,
												invocation_id: invocation.invocation_id,
											},
										}
									: {}),
								invocation_id: invocation.invocation_id,
								recovery,
								thread_id: invocation.thread_id,
							});
						}

						return dispatches;
					}),
				)
				.pipe(Effect.mapError(normalize_error));
		});

		const ClaimRecovery = (invocation_id: string) =>
			Decode(Identifier, invocation_id, "Tool execution identifier is invalid").pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const row = yield* ReadRow(transaction, decoded);
								if (row.state !== "running") return Option.none<ToolExecution>();
								yield* EnsureLiveThread(transaction, row.thread_id);
								const dispatch_available = yield* DispatchAvailable(
									transaction,
									row.thread_id,
								);

								if (!dispatch_available) {
									return Option.none<ToolExecution>();
								}

								const admitted = yield* SerializeDispatchAdmission(
									transaction,
									row.thread_id,
								);

								if (!admitted) {
									return Option.none<ToolExecution>();
								}

								const [other_active] = yield* transaction
									.select({ invocation_id: ToolInvocations.invocation_id })
									.from(ToolInvocations)
									.where(
										and(
											eq(ToolInvocations.thread_id, row.thread_id),
											eq(ToolInvocations.state, "running"),
											ne(ToolInvocations.invocation_id, row.invocation_id),
										),
									)
									.limit(1);

								if (other_active) {
									return Option.none<ToolExecution>();
								}

								yield* DecodeInvocation(row);
								yield* ReadPrivate(transaction, row);
								const claim = yield* ReadClaim(transaction, decoded);
								const now = yield* metadata.Now;
								const expired = yield* IsExpired(claim.lease_expires_at, now);
								const recoverable =
									expired &&
									(claim.launch_started_at === null ||
										(row.recovery_policy === "retry" && row.effect === "read"));
								if (!recoverable) return Option.none<ToolExecution>();
								const claim_token = yield* metadata.MakeId("claim");
								const lease_expires_at = yield* LeaseExpiry(now);
								const [recovered] = yield* transaction
									.update(ToolExecutionClaims)
									.set({
										claim_token,
										claimed_at: now,
										launch_started_at: null,
										lease_expires_at,
										owner_instance_id: metadata.instance_id,
									})
									.where(
										and(
											eq(ToolExecutionClaims.invocation_id, decoded),
											eq(ToolExecutionClaims.claim_token, claim.claim_token),
											eq(
												ToolExecutionClaims.owner_instance_id,
												claim.owner_instance_id,
											),
											eq(
												ToolExecutionClaims.lease_expires_at,
												claim.lease_expires_at,
											),
										),
									)
									.returning();
								if (!recovered) return Option.none<ToolExecution>();

								return Option.some(
									yield* BuildExecution(transaction, row, recovered),
								);
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const QuarantineInterrupted = (invocation_id: string) =>
			Decode(Identifier, invocation_id, "Tool execution identifier is invalid").pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const settled = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);
									if (row.state === "outcome_unknown")
										return Option.none<number>();
									if (
										row.state !== "running" ||
										row.recovery_policy === "retry"
									) {
										return yield* conflict("invalid_transition");
									}
									yield* EnsureLiveThread(transaction, row.thread_id);
									const claim = yield* ReadClaim(transaction, decoded);
									const now = yield* metadata.Now;
									const expired = yield* IsExpired(claim.lease_expires_at, now);
									if (!expired || claim.launch_started_at === null) {
										return yield* conflict("invalid_transition");
									}
									const next_row = {
										...row,
										settled_at: now,
										state: "outcome_unknown",
										updated_at: now,
									};
									const updated = yield* UpdateProjection(
										transaction,
										row,
										next_row,
										"outcome_unknown",
									);
									return Option.some(updated.sequence);
								}),
							),
						);
						if (Option.isSome(settled)) {
							yield* notifier.Publish(settled.value).pipe(Effect.ignoreCause);
						}
					}),
				),
				Effect.mapError(normalize_error),
			);

		const BeginThreadQuiescence = (thread_id: string) =>
			Decode(Identifier, thread_id, "Tool execution thread identifier is invalid").pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [[thread], [tombstone]] = yield* Effect.all([
									transaction
										.select({ thread_id: Threads.thread_id })
										.from(Threads)
										.where(eq(Threads.thread_id, decoded))
										.limit(1),
									transaction
										.select({ thread_id: ThreadTombstones.thread_id })
										.from(ThreadTombstones)
										.where(eq(ThreadTombstones.thread_id, decoded))
										.limit(1),
								]);

								if (tombstone) {
									return;
								}

								if (!thread) {
									return yield* new ToolExecutionUnavailable({
										reason: "missing",
									});
								}

								const now = yield* metadata.Now;

								yield* transaction
									.insert(ToolThreadDispatchState)
									.values({
										admission_version: 0,
										quiesced_at: now,
										thread_id: decoded,
									})
									.onConflictDoNothing();
								yield* transaction
									.update(ToolThreadDispatchState)
									.set({ quiesced_at: now })
									.where(
										and(
											eq(ToolThreadDispatchState.thread_id, decoded),
											isNull(ToolThreadDispatchState.quiesced_at),
										),
									);
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ThreadQuiescencePending = (thread_id: string) =>
			Decode(Identifier, thread_id, "Tool execution thread identifier is invalid").pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const claims = yield* database.client
							.select({ lease_expires_at: ToolExecutionClaims.lease_expires_at })
							.from(ToolExecutionClaims)
							.innerJoin(
								ToolInvocations,
								eq(
									ToolExecutionClaims.invocation_id,
									ToolInvocations.invocation_id,
								),
							)
							.where(
								and(
									eq(ToolInvocations.thread_id, decoded),
									eq(ToolInvocations.state, "running"),
								),
							);

						for (const claim of claims) {
							const expired = yield* IsExpired(claim.lease_expires_at, now);

							if (!expired) {
								return true;
							}
						}

						return false;
					}),
				),
				Effect.mapError(normalize_error),
			);

		const ActiveClaimsForThread = (thread_id: string) =>
			Decode(Identifier, thread_id, "Tool execution thread identifier is invalid").pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, decoded);
							const rows = yield* transaction
								.select({ invocation_id: ToolExecutionClaims.invocation_id })
								.from(ToolExecutionClaims)
								.innerJoin(
									ToolInvocations,
									eq(
										ToolExecutionClaims.invocation_id,
										ToolInvocations.invocation_id,
									),
								)
								.where(
									and(
										eq(ToolInvocations.thread_id, decoded),
										eq(ToolInvocations.state, "running"),
									),
								);

							return rows.length > 0;
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const AbandonClaim = (transaction: typeof database.client, claim: ClaimRow, now: string) =>
			Effect.gen(function* () {
				const current = yield* DecodeDateTime(now, "Tool execution clock is invalid");
				const minimum = yield* DecodeDateTime(
					claim.launch_started_at ?? claim.claimed_at,
					"Tool execution lease is invalid",
				);
				const expires_at = DateTime.formatIso(
					DateTime.toEpochMillis(current) > DateTime.toEpochMillis(minimum)
						? current
						: minimum,
				);

				yield* transaction
					.update(ToolExecutionClaims)
					.set({ lease_expires_at: expires_at })
					.where(
						and(
							eq(ToolExecutionClaims.invocation_id, claim.invocation_id),
							eq(ToolExecutionClaims.claim_token, claim.claim_token),
							eq(ToolExecutionClaims.owner_instance_id, metadata.instance_id),
						),
					);
			});

		const AbandonExecution = (identity: typeof ToolExecutionIdentity.Type) =>
			Decode(ToolExecutionIdentity, identity, "Tool execution identity is invalid").pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;

						yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const [claim] = yield* transaction
										.select()
										.from(ToolExecutionClaims)
										.where(
											and(
												eq(
													ToolExecutionClaims.invocation_id,
													decoded.invocation_id,
												),
												eq(
													ToolExecutionClaims.claim_token,
													decoded.claim_token,
												),
												eq(
													ToolExecutionClaims.owner_instance_id,
													metadata.instance_id,
												),
											),
										)
										.limit(1);

									if (claim) {
										yield* AbandonClaim(transaction, claim, now);
									}
								}),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);

		const AbandonOwnedExecutions = Effect.gen(function* () {
			const now = yield* metadata.Now;

			yield* RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const claims = yield* transaction
							.select()
							.from(ToolExecutionClaims)
							.where(eq(ToolExecutionClaims.owner_instance_id, metadata.instance_id));
						for (const claim of claims) {
							yield* AbandonClaim(transaction, claim, now);
						}
					}),
				),
			);
		}).pipe(Effect.mapError(normalize_error));

		return {
			AbandonExecution,
			AbandonOwnedExecutions,
			ActiveClaimsForThread,
			BeginThreadQuiescence,
			ClaimPending,
			ClaimRecovery,
			ListPending,
			ListRunning,
			MarkLaunchStarted,
			QuarantineInterrupted,
			ReadCompleted,
			ReadExecution,
			RenewLease,
			SettleCompleted,
			SettleFailed,
			ThreadQuiescencePending,
		};
	}),
);
