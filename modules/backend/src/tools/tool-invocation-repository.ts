import { and, asc, eq, gt, sql } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	ArtisanApprovalProjection,
	ArtisanApprovalRequest,
	ArtisanApprovalResolution,
	ArtisanApprovalUpdatedEvent,
	ArtisanToolInvocation,
	ArtisanToolExecutionInput,
	ArtisanToolInvocationEvent,
	ArtisanToolInvocationOutcome,
	ArtisanToolUsage,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceEvidenceBinding,
	type ArtisanApprovalProjection as ArtisanApprovalProjectionValue,
	type ArtisanApprovalRequest as ArtisanApprovalRequestValue,
	type ArtisanApprovalResolution as ArtisanApprovalResolutionValue,
	type ArtisanToolInvocation as ArtisanToolInvocationValue,
	type ArtisanToolInvocationOutcome as ArtisanToolInvocationOutcomeValue,
	type ArtisanToolUsage as ArtisanToolUsageValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	ArtisanToolApprovals,
	ArtisanToolInvocations,
	EventStreams,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";

const request_fingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));
const lease_milliseconds = 60_000;

const BeginInput = Schema.Struct({
	approval: Schema.optional(ArtisanApprovalRequest),
	execution_input: ArtisanToolExecutionInput,
	invocation: ArtisanToolInvocation,
	request_fingerprint,
});

const StoredInvocation = Schema.Struct({
	agent_id: Schema.NullOr(Identifier),
	approval_id: Schema.NullOr(Identifier),
	claim_lease_expires_at: Schema.NullOr(IsoDateTime),
	claim_owner_id: Schema.NullOr(Identifier),
	completed_at: Schema.NullOr(IsoDateTime),
	execution_input_json: Schema.String,
	invocation_id: Identifier,
	input_summary: ArtisanToolInvocation.fields.input_summary,
	journal_sequence: Schema.NullOr(Schema.Int.check(Schema.isGreaterThanOrEqualTo(1))),
	lifecycle: ArtisanToolInvocation.fields.lifecycle,
	outcome_json: Schema.NullOr(Schema.String),
	permission_json: Schema.String,
	raw_origin_json: Schema.NullOr(Schema.String),
	request_fingerprint,
	requested_at: IsoDateTime,
	run_id: Schema.NullOr(Identifier),
	thread_id: Identifier,
	tool_id: ArtisanToolInvocation.fields.tool_id,
	updated_at: IsoDateTime,
	workspace_evidence_json: Schema.NullOr(Schema.String),
});

const StoredApproval = Schema.Struct({
	approval_id: Identifier,
	created_at: IsoDateTime,
	invocation_id: Identifier,
	request_json: Schema.String,
	resolution_id: Schema.NullOr(Identifier),
	resolution_json: Schema.NullOr(Schema.String),
	state: Schema.Literals(["pending", "resolved"]),
	updated_at: IsoDateTime,
});

/** Reports a conflict, invalid persisted invariant, or infrastructure failure. */
export class ToolInvocationRepositoryFailure extends Data.TaggedError(
	"ToolInvocationRepositoryFailure",
)<{
	readonly cause: unknown;
}> {}

/** Rejects a reused invocation or resolution identifier carrying different intent. */
export class ToolInvocationConflict extends Data.TaggedError("ToolInvocationConflict")<{
	readonly identifier: string;
}> {}

/** Rejects a lifecycle operation that is no longer valid. */
export class ToolInvocationLifecycleConflict extends Data.TaggedError(
	"ToolInvocationLifecycleConflict",
)<{ readonly invocation_id: string; readonly lifecycle: string }> {}

export type ToolInvocationRepositoryError =
	| ToolInvocationConflict
	| ToolInvocationLifecycleConflict
	| ToolInvocationRepositoryFailure;

export interface ToolInvocationClaim {
	readonly invocation: ArtisanToolInvocationValue;
	readonly status: "claimed" | "awaiting_approval" | "busy" | "terminal";
}

export interface ToolInvocationListInput {
	readonly after_journal_sequence?: number;
	readonly lifecycle?: ArtisanToolInvocationValue["lifecycle"];
	readonly limit?: number;
	readonly run_id?: string;
	readonly thread_id: string;
	readonly tool_id?: ArtisanToolInvocationValue["tool_id"];
}

/** Owns durable, cross-runtime tool invocation and approval lifecycles. */
export class ToolInvocationRepository extends Context.Service<
	ToolInvocationRepository,
	{
		readonly Begin: (
			input: typeof BeginInput.Type,
		) => Effect.Effect<ArtisanToolInvocationValue, ToolInvocationRepositoryError>;
		readonly ReadExecutionInput: (
			invocation_id: string,
		) => Effect.Effect<typeof ArtisanToolExecutionInput.Type, ToolInvocationRepositoryError>;
		readonly ReadInvocation: (
			invocation_id: string,
		) => Effect.Effect<ArtisanToolInvocationValue, ToolInvocationRepositoryError>;
		readonly Claim: (
			invocation_id: string,
		) => Effect.Effect<ToolInvocationClaim, ToolInvocationRepositoryError>;
		readonly Finalize: (
			invocation_id: string,
			outcome: ArtisanToolInvocationOutcomeValue,
			workspace_evidence?: typeof WorkspaceEvidenceBinding.Type,
		) => Effect.Effect<ArtisanToolInvocationValue, ToolInvocationRepositoryError>;
		readonly ListApprovals: (
			thread_id: string,
			state?: "pending" | "resolved",
		) => Effect.Effect<
			ReadonlyArray<ArtisanApprovalProjectionValue>,
			ToolInvocationRepositoryError
		>;
		readonly ListInvocations: (
			input: ToolInvocationListInput,
		) => Effect.Effect<
			ReadonlyArray<ArtisanToolInvocationValue>,
			ToolInvocationRepositoryError
		>;
		readonly ResolveApproval: (
			resolution: ArtisanApprovalResolutionValue,
		) => Effect.Effect<ArtisanApprovalProjectionValue, ToolInvocationRepositoryError>;
		readonly Usage: (
			thread_id: string,
		) => Effect.Effect<ReadonlyArray<ArtisanToolUsageValue>, ToolInvocationRepositoryError>;
	}
>()("Artisan/ToolInvocationRepository") {}

const ParseJson = (json: string, schema: Schema.Codec<any, any>, context: string) =>
	Effect.try({
		try: () => JSON.parse(json) as unknown,
		catch: (cause) => new ToolInvocationRepositoryFailure({ cause }),
	}).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError((cause) =>
			cause instanceof ToolInvocationRepositoryFailure
				? cause
				: new ToolInvocationRepositoryFailure({
						cause: new Error(`${context} is invalid`),
					}),
		),
	);

export const ToolInvocationRepositoryLive = Layer.effect(
	ToolInvocationRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const DecodeInvocation = (row: typeof ArtisanToolInvocations.$inferSelect) =>
			Effect.gen(function* () {
				const stored = yield* Schema.decodeUnknownEffect(StoredInvocation, {
					onExcessProperty: "error",
				})(row).pipe(
					Effect.mapError((cause) => new ToolInvocationRepositoryFailure({ cause })),
				);
				const permission = yield* ParseJson(
					stored.permission_json,
					ArtisanToolInvocation.fields.permission,
					`Invocation ${stored.invocation_id} permission`,
				);
				const outcome = stored.outcome_json
					? yield* ParseJson(
							stored.outcome_json,
							ArtisanToolInvocationOutcome,
							"Invocation outcome",
						)
					: undefined;
				const raw_origin = stored.raw_origin_json
					? yield* ParseJson(stored.raw_origin_json, RawOrigin, "Invocation raw origin")
					: undefined;
				const workspace_evidence = stored.workspace_evidence_json
					? yield* ParseJson(
							stored.workspace_evidence_json,
							WorkspaceEvidenceBinding,
							"Invocation evidence binding",
						)
					: undefined;

				return yield* Schema.decodeUnknownEffect(ArtisanToolInvocation, {
					onExcessProperty: "error",
				})({
					...(stored.agent_id ? { agent_id: stored.agent_id } : {}),
					...(stored.approval_id ? { approval_id: stored.approval_id } : {}),
					...(stored.completed_at ? { completed_at: stored.completed_at } : {}),
					invocation_id: stored.invocation_id,
					input_summary: stored.input_summary,
					lifecycle: stored.lifecycle,
					...(outcome ? { outcome } : {}),
					permission,
					...(raw_origin ? { raw_origin } : {}),
					requested_at: stored.requested_at,
					...(stored.run_id ? { run_id: stored.run_id } : {}),
					thread_id: stored.thread_id,
					tool_id: stored.tool_id,
					updated_at: stored.updated_at,
					...(workspace_evidence ? { workspace_evidence } : {}),
				}).pipe(Effect.mapError((cause) => new ToolInvocationRepositoryFailure({ cause })));
			});

		const DecodeApproval = (row: typeof ArtisanToolApprovals.$inferSelect) =>
			Effect.gen(function* () {
				const stored = yield* Schema.decodeUnknownEffect(StoredApproval, {
					onExcessProperty: "error",
				})(row).pipe(
					Effect.mapError((cause) => new ToolInvocationRepositoryFailure({ cause })),
				);
				const request = yield* ParseJson(
					stored.request_json,
					ArtisanApprovalRequest,
					"Approval request",
				);
				const resolution = stored.resolution_json
					? yield* ParseJson(
							stored.resolution_json,
							ArtisanApprovalResolution,
							"Approval resolution",
						)
					: undefined;
				return yield* Schema.decodeUnknownEffect(ArtisanApprovalProjection)({
					request,
					...(resolution ? { resolution } : {}),
					state: stored.state,
				}).pipe(Effect.mapError((cause) => new ToolInvocationRepositoryFailure({ cause })));
			});

		const AppendEvent = (
			transaction: typeof database.client,
			invocation: ArtisanToolInvocationValue,
			payload:
				| typeof ArtisanToolInvocationEvent.Type
				| typeof ArtisanApprovalUpdatedEvent.Type,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${invocation.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const occurred_at = yield* metadata.Now;
				yield* RecordThreadActivity(
					transaction,
					invocation.thread_id,
					occurred_at,
					payload,
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
						agent_id: invocation.agent_id ?? null,
						causation_id: invocation.invocation_id,
						correlation_id: invocation.invocation_id,
						event_id,
						event_type: payload.type,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						raw_origin_json: invocation.raw_origin
							? JSON.stringify(invocation.raw_origin)
							: null,
						run_id: invocation.run_id ?? null,
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: invocation.thread_id,
					})
					.returning({ sequence: JournalEvents.sequence });
				return event!.sequence;
			});

		const ValidateBegin = (input: typeof BeginInput.Type) =>
			Schema.decodeUnknownEffect(BeginInput, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					decoded.execution_input.tool_id === decoded.invocation.tool_id &&
					((decoded.invocation.lifecycle === "requested" &&
						decoded.invocation.outcome === undefined &&
						decoded.approval === undefined &&
						decoded.invocation.approval_id === undefined) ||
						(decoded.invocation.lifecycle === "awaiting_approval" &&
							decoded.invocation.outcome === undefined &&
							decoded.approval !== undefined &&
							decoded.approval.approval_id === decoded.invocation.approval_id &&
							decoded.approval.invocation_id === decoded.invocation.invocation_id) ||
						(["denied", "failed", "cancelled", "unsupported"].includes(
							decoded.invocation.lifecycle,
						) &&
							decoded.invocation.outcome !== undefined &&
							decoded.invocation.completed_at !== undefined &&
							decoded.approval === undefined &&
							decoded.invocation.approval_id === undefined))
						? Effect.succeed(decoded)
						: Effect.fail(
								new ToolInvocationRepositoryFailure({
									cause: new Error("Invalid initial tool invocation lifecycle"),
								}),
							),
				),
				Effect.mapError((cause) =>
					cause instanceof ToolInvocationRepositoryFailure
						? cause
						: new ToolInvocationRepositoryFailure({ cause }),
				),
			);

		const Begin = (input: typeof BeginInput.Type) =>
			Effect.gen(function* () {
				const decoded = yield* ValidateBegin(input);
				const sequences = yield* database.client
					.transaction((transaction) =>
						Effect.gen(function* () {
							const [existing] = yield* transaction
								.select()
								.from(ArtisanToolInvocations)
								.where(
									eq(
										ArtisanToolInvocations.invocation_id,
										decoded.invocation.invocation_id,
									),
								)
								.limit(1);
							if (existing) {
								if (existing.request_fingerprint !== decoded.request_fingerprint)
									return yield* new ToolInvocationConflict({
										identifier: decoded.invocation.invocation_id,
									});
								return {
									invocation: yield* DecodeInvocation(existing),
									sequences: [] as ReadonlyArray<number>,
								};
							}
							const [thread] = yield* transaction
								.select({ thread_id: Threads.thread_id })
								.from(Threads)
								.where(eq(Threads.thread_id, decoded.invocation.thread_id))
								.limit(1);
							const [erasing] = yield* transaction
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(
									eq(ThreadErasureClaims.thread_id, decoded.invocation.thread_id),
								)
								.limit(1);
							if (!thread || erasing)
								return yield* new ToolInvocationLifecycleConflict({
									invocation_id: decoded.invocation.invocation_id,
									lifecycle: "thread_unavailable",
								});
							yield* transaction.insert(ArtisanToolInvocations).values({
								agent_id: decoded.invocation.agent_id ?? null,
								approval_id: decoded.invocation.approval_id ?? null,
								claim_lease_expires_at: null,
								claim_owner_id: null,
								completed_at: decoded.invocation.completed_at ?? null,
								invocation_id: decoded.invocation.invocation_id,
								execution_input_json: JSON.stringify(decoded.execution_input),
								input_summary: decoded.invocation.input_summary,
								journal_sequence: null,
								lifecycle: decoded.invocation.lifecycle,
								outcome_json: decoded.invocation.outcome
									? JSON.stringify(decoded.invocation.outcome)
									: null,
								permission_json: JSON.stringify(decoded.invocation.permission),
								raw_origin_json: decoded.invocation.raw_origin
									? JSON.stringify(decoded.invocation.raw_origin)
									: null,
								request_fingerprint: decoded.request_fingerprint,
								requested_at: decoded.invocation.requested_at,
								run_id: decoded.invocation.run_id ?? null,
								thread_id: decoded.invocation.thread_id,
								tool_id: decoded.invocation.tool_id,
								updated_at: decoded.invocation.updated_at,
								workspace_evidence_json: null,
							});
							if (decoded.approval)
								yield* transaction.insert(ArtisanToolApprovals).values({
									approval_id: decoded.approval.approval_id,
									invocation_id: decoded.approval.invocation_id,
									request_json: JSON.stringify(decoded.approval),
									resolution_id: null,
									resolution_json: null,
									state: "pending",
									created_at: decoded.approval.requested_at,
									updated_at: decoded.approval.requested_at,
								});
							const invocation_sequence = yield* AppendEvent(
								transaction,
								decoded.invocation,
								{
									invocation: decoded.invocation,
									type: "artisan.tool.invocation.updated",
								},
							);
							const approval_sequence = decoded.approval
								? yield* AppendEvent(transaction, decoded.invocation, {
										approval: { request: decoded.approval, state: "pending" },
										type: "artisan.approval.updated",
									})
								: undefined;
							yield* transaction
								.update(ArtisanToolInvocations)
								.set({ journal_sequence: approval_sequence ?? invocation_sequence })
								.where(
									eq(
										ArtisanToolInvocations.invocation_id,
										decoded.invocation.invocation_id,
									),
								);
							return {
								invocation: decoded.invocation,
								sequences: [
									invocation_sequence,
									...(approval_sequence ? [approval_sequence] : []),
								],
							};
						}),
					)
					.pipe(RetrySqliteWrite);
				yield* Effect.forEach(
					sequences.sequences,
					(sequence) => notifier.Publish(sequence),
					{ discard: true },
				);
				return sequences.invocation;
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ToolInvocationConflict ||
					cause instanceof ToolInvocationLifecycleConflict ||
					cause instanceof ToolInvocationRepositoryFailure
						? cause
						: new ToolInvocationRepositoryFailure({ cause }),
				),
			);

		const Claim = (invocation_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const lease_until = new Date(
							new Date(now).getTime() + lease_milliseconds,
						).toISOString();
						const [row] = yield* transaction
							.select()
							.from(ArtisanToolInvocations)
							.where(eq(ArtisanToolInvocations.invocation_id, invocation_id))
							.limit(1);
						if (!row)
							return yield* new ToolInvocationLifecycleConflict({
								invocation_id,
								lifecycle: "missing",
							});
						const invocation = yield* DecodeInvocation(row);
						if (invocation.lifecycle === "awaiting_approval")
							return {
								invocation,
								status: "awaiting_approval" as const,
								sequences: [] as number[],
							};
						if (
							["succeeded", "denied", "failed", "cancelled", "unsupported"].includes(
								invocation.lifecycle,
							)
						)
							return {
								invocation,
								status: "terminal" as const,
								sequences: [] as number[],
							};
						if (
							row.lifecycle === "running" &&
							row.claim_lease_expires_at !== null &&
							row.claim_lease_expires_at > now &&
							row.claim_owner_id !== metadata.instance_id
						)
							return {
								invocation,
								status: "busy" as const,
								sequences: [] as number[],
							};
						const running = {
							...invocation,
							lifecycle: "running" as const,
							updated_at: now,
						};
						yield* transaction
							.update(ArtisanToolInvocations)
							.set({
								claim_owner_id: metadata.instance_id,
								claim_lease_expires_at: lease_until,
								lifecycle: "running",
								updated_at: now,
							})
							.where(eq(ArtisanToolInvocations.invocation_id, invocation_id));
						const sequence = yield* AppendEvent(transaction, running, {
							invocation: running,
							type: "artisan.tool.invocation.updated",
						});
						yield* transaction
							.update(ArtisanToolInvocations)
							.set({ journal_sequence: sequence })
							.where(eq(ArtisanToolInvocations.invocation_id, invocation_id));
						return {
							invocation: running,
							status: "claimed" as const,
							sequences: [sequence],
						};
					}),
				)
				.pipe(
					RetrySqliteWrite,
					Effect.tap((result) =>
						Effect.forEach(result.sequences, (sequence) => notifier.Publish(sequence), {
							discard: true,
						}),
					),
					Effect.map((result) => ({
						invocation: result.invocation,
						status: result.status,
					})),
					Effect.mapError((cause) =>
						cause instanceof ToolInvocationLifecycleConflict ||
						cause instanceof ToolInvocationRepositoryFailure
							? cause
							: new ToolInvocationRepositoryFailure({ cause }),
					),
				);

		const Finalize = (
			invocation_id: string,
			outcome: ArtisanToolInvocationOutcomeValue,
			workspace_evidence?: typeof WorkspaceEvidenceBinding.Type,
		) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const decoded_outcome = yield* Schema.decodeUnknownEffect(
							ArtisanToolInvocationOutcome,
						)(outcome).pipe(
							Effect.mapError(
								(cause) => new ToolInvocationRepositoryFailure({ cause }),
							),
						);
						const decoded_evidence =
							workspace_evidence === undefined
								? undefined
								: yield* Schema.decodeUnknownEffect(WorkspaceEvidenceBinding)(
										workspace_evidence,
									).pipe(
										Effect.mapError(
											(cause) =>
												new ToolInvocationRepositoryFailure({ cause }),
										),
									);
						const [row] = yield* transaction
							.select()
							.from(ArtisanToolInvocations)
							.where(eq(ArtisanToolInvocations.invocation_id, invocation_id))
							.limit(1);
						if (!row)
							return yield* new ToolInvocationLifecycleConflict({
								invocation_id,
								lifecycle: "missing",
							});
						const invocation = yield* DecodeInvocation(row);
						if (
							["succeeded", "denied", "failed", "cancelled", "unsupported"].includes(
								invocation.lifecycle,
							)
						) {
							if (
								JSON.stringify(invocation.outcome) !==
								JSON.stringify(decoded_outcome)
							)
								return yield* new ToolInvocationConflict({
									identifier: invocation_id,
								});
							return { invocation, sequence: undefined };
						}
						if (invocation.lifecycle !== "running")
							return yield* new ToolInvocationLifecycleConflict({
								invocation_id,
								lifecycle: invocation.lifecycle,
							});
						const completed_at = yield* metadata.Now;
						const terminal = {
							...invocation,
							completed_at,
							lifecycle: decoded_outcome.status,
							outcome: decoded_outcome,
							updated_at: completed_at,
							...(decoded_evidence ? { workspace_evidence: decoded_evidence } : {}),
						};
						yield* transaction
							.update(ArtisanToolInvocations)
							.set({
								claim_owner_id: null,
								claim_lease_expires_at: null,
								completed_at,
								lifecycle: decoded_outcome.status,
								outcome_json: JSON.stringify(decoded_outcome),
								updated_at: completed_at,
								workspace_evidence_json: decoded_evidence
									? JSON.stringify(decoded_evidence)
									: null,
							})
							.where(eq(ArtisanToolInvocations.invocation_id, invocation_id));
						const sequence = yield* AppendEvent(transaction, terminal, {
							invocation: terminal,
							type: "artisan.tool.invocation.updated",
						});
						yield* transaction
							.update(ArtisanToolInvocations)
							.set({ journal_sequence: sequence })
							.where(eq(ArtisanToolInvocations.invocation_id, invocation_id));
						return { invocation: terminal, sequence };
					}),
				)
				.pipe(
					RetrySqliteWrite,
					Effect.tap((result) =>
						result.sequence === undefined
							? Effect.void
							: notifier.Publish(result.sequence),
					),
					Effect.map((result) => result.invocation),
					Effect.mapError((cause) =>
						cause instanceof ToolInvocationConflict ||
						cause instanceof ToolInvocationLifecycleConflict ||
						cause instanceof ToolInvocationRepositoryFailure
							? cause
							: new ToolInvocationRepositoryFailure({ cause }),
					),
				);

		const ResolveApproval = (resolution: ArtisanApprovalResolutionValue) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const decoded = yield* Schema.decodeUnknownEffect(
							ArtisanApprovalResolution,
						)(resolution).pipe(
							Effect.mapError(
								(cause) => new ToolInvocationRepositoryFailure({ cause }),
							),
						);
						const [approval_row] = yield* transaction
							.select()
							.from(ArtisanToolApprovals)
							.where(eq(ArtisanToolApprovals.approval_id, decoded.approval_id))
							.limit(1);
						if (!approval_row)
							return yield* new ToolInvocationLifecycleConflict({
								invocation_id: decoded.invocation_id,
								lifecycle: "approval_missing",
							});
						const approval = yield* DecodeApproval(approval_row);
						if (approval.request.invocation_id !== decoded.invocation_id)
							return yield* new ToolInvocationConflict({
								identifier: decoded.approval_id,
							});
						if (
							approval.resolution &&
							JSON.stringify(approval.resolution) !== JSON.stringify(decoded)
						)
							return yield* new ToolInvocationConflict({
								identifier: decoded.resolution_id,
							});
						if (approval.resolution) return { approval, sequences: [] as number[] };
						const [row] = yield* transaction
							.select()
							.from(ArtisanToolInvocations)
							.where(eq(ArtisanToolInvocations.invocation_id, decoded.invocation_id))
							.limit(1);
						if (!row)
							return yield* new ToolInvocationLifecycleConflict({
								invocation_id: decoded.invocation_id,
								lifecycle: "missing",
							});
						const invocation = yield* DecodeInvocation(row);
						if (invocation.lifecycle !== "awaiting_approval")
							return yield* new ToolInvocationLifecycleConflict({
								invocation_id: decoded.invocation_id,
								lifecycle: invocation.lifecycle,
							});
						const updated_at = yield* metadata.Now;
						const next = decoded.approved
							? { ...invocation, lifecycle: "requested" as const, updated_at }
							: {
									...invocation,
									completed_at: updated_at,
									lifecycle: "denied" as const,
									outcome: { code: "approval_denied", status: "denied" as const },
									updated_at,
								};
						const resolved = {
							request: approval.request,
							resolution: decoded,
							state: "resolved" as const,
						};
						yield* transaction
							.update(ArtisanToolApprovals)
							.set({
								resolution_id: decoded.resolution_id,
								resolution_json: JSON.stringify(decoded),
								state: "resolved",
								updated_at,
							})
							.where(eq(ArtisanToolApprovals.approval_id, decoded.approval_id));
						yield* transaction
							.update(ArtisanToolInvocations)
							.set({
								completed_at: next.completed_at ?? null,
								lifecycle: next.lifecycle,
								outcome_json: next.outcome ? JSON.stringify(next.outcome) : null,
								updated_at,
							})
							.where(eq(ArtisanToolInvocations.invocation_id, decoded.invocation_id));
						const approval_sequence = yield* AppendEvent(transaction, next, {
							approval: resolved,
							type: "artisan.approval.updated",
						});
						const invocation_sequence = yield* AppendEvent(transaction, next, {
							invocation: next,
							type: "artisan.tool.invocation.updated",
						});
						yield* transaction
							.update(ArtisanToolInvocations)
							.set({ journal_sequence: invocation_sequence })
							.where(eq(ArtisanToolInvocations.invocation_id, decoded.invocation_id));
						return {
							approval: resolved,
							sequences: [approval_sequence, invocation_sequence],
						};
					}),
				)
				.pipe(
					RetrySqliteWrite,
					Effect.tap((result) =>
						Effect.forEach(result.sequences, (sequence) => notifier.Publish(sequence), {
							discard: true,
						}),
					),
					Effect.map((result) => result.approval),
					Effect.mapError((cause) =>
						cause instanceof ToolInvocationConflict ||
						cause instanceof ToolInvocationLifecycleConflict ||
						cause instanceof ToolInvocationRepositoryFailure
							? cause
							: new ToolInvocationRepositoryFailure({ cause }),
					),
				);

		const ListInvocations = (input: ToolInvocationListInput) =>
			database.client
				.select()
				.from(ArtisanToolInvocations)
				.where(
					and(
						eq(ArtisanToolInvocations.thread_id, input.thread_id),
						input.run_id ? eq(ArtisanToolInvocations.run_id, input.run_id) : undefined,
						input.tool_id
							? eq(ArtisanToolInvocations.tool_id, input.tool_id)
							: undefined,
						input.lifecycle
							? eq(ArtisanToolInvocations.lifecycle, input.lifecycle)
							: undefined,
						input.after_journal_sequence === undefined
							? undefined
							: gt(
									ArtisanToolInvocations.journal_sequence,
									input.after_journal_sequence,
								),
					),
				)
				.orderBy(
					asc(ArtisanToolInvocations.requested_at),
					asc(ArtisanToolInvocations.invocation_id),
				)
				.limit(input.limit ?? 100)
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeInvocation)),
					Effect.mapError((cause) =>
						cause instanceof ToolInvocationRepositoryFailure
							? cause
							: new ToolInvocationRepositoryFailure({ cause }),
					),
				);
		const ListApprovals = (thread_id: string, state?: "pending" | "resolved") =>
			database.client
				.select({ approval: ArtisanToolApprovals })
				.from(ArtisanToolApprovals)
				.innerJoin(
					ArtisanToolInvocations,
					eq(ArtisanToolApprovals.invocation_id, ArtisanToolInvocations.invocation_id),
				)
				.where(
					and(
						eq(ArtisanToolInvocations.thread_id, thread_id),
						state ? eq(ArtisanToolApprovals.state, state) : undefined,
					),
				)
				.orderBy(
					asc(ArtisanToolApprovals.created_at),
					asc(ArtisanToolApprovals.approval_id),
				)
				.pipe(
					Effect.flatMap((rows) =>
						Effect.forEach(rows, (row) => DecodeApproval(row.approval)),
					),
					Effect.mapError((cause) =>
						cause instanceof ToolInvocationRepositoryFailure
							? cause
							: new ToolInvocationRepositoryFailure({ cause }),
					),
				);
		const ReadExecutionInput = (
			invocation_id: string,
		): Effect.Effect<typeof ArtisanToolExecutionInput.Type, ToolInvocationRepositoryError> =>
			Effect.gen(function* () {
				const [row] = yield* database.client
					.select({ execution_input_json: ArtisanToolInvocations.execution_input_json })
					.from(ArtisanToolInvocations)
					.where(eq(ArtisanToolInvocations.invocation_id, invocation_id))
					.limit(1)
					.pipe(
						Effect.mapError((cause) => new ToolInvocationRepositoryFailure({ cause })),
					);
				if (!row) {
					return yield* new ToolInvocationLifecycleConflict({
						invocation_id,
						lifecycle: "missing",
					});
				}

				return yield* ParseJson(
					row.execution_input_json,
					ArtisanToolExecutionInput,
					"Invocation execution input",
				).pipe(Effect.map((value) => value as typeof ArtisanToolExecutionInput.Type));
			});
		const ReadInvocation = (
			invocation_id: string,
		): Effect.Effect<ArtisanToolInvocationValue, ToolInvocationRepositoryError> =>
			Effect.gen(function* () {
				const [row] = yield* database.client
					.select()
					.from(ArtisanToolInvocations)
					.where(eq(ArtisanToolInvocations.invocation_id, invocation_id))
					.limit(1)
					.pipe(
						Effect.mapError((cause) => new ToolInvocationRepositoryFailure({ cause })),
					);
				if (!row) {
					return yield* new ToolInvocationLifecycleConflict({
						invocation_id,
						lifecycle: "missing",
					});
				}

				return yield* DecodeInvocation(row);
			});
		const Usage = (thread_id: string) =>
			database.client
				.select({
					active_invocation_count: sql<number>`sum(case when ${ArtisanToolInvocations.lifecycle} in ('requested', 'awaiting_approval', 'running') then 1 else 0 end)`,
					last_invoked_at: sql<
						string | null
					>`max(${ArtisanToolInvocations.requested_at})`,
					tool_id: ArtisanToolInvocations.tool_id,
					total_invocation_count: sql<number>`count(*)`,
				})
				.from(ArtisanToolInvocations)
				.where(eq(ArtisanToolInvocations.thread_id, thread_id))
				.groupBy(ArtisanToolInvocations.tool_id)
				.orderBy(asc(ArtisanToolInvocations.tool_id))
				.pipe(
					Effect.flatMap((rows) =>
						Effect.forEach(rows, (row) =>
							Schema.decodeUnknownEffect(ArtisanToolUsage)({
								active_invocation_count: row.active_invocation_count,
								...(row.last_invoked_at
									? { last_invoked_at: row.last_invoked_at }
									: {}),
								tool_id: row.tool_id,
								total_invocation_count: row.total_invocation_count,
							}).pipe(
								Effect.mapError(
									(cause) => new ToolInvocationRepositoryFailure({ cause }),
								),
							),
						),
					),
					Effect.mapError((cause) =>
						cause instanceof ToolInvocationRepositoryFailure
							? cause
							: new ToolInvocationRepositoryFailure({ cause }),
					),
				);

		return {
			Begin,
			Claim,
			Finalize,
			ListApprovals,
			ListInvocations,
			ReadExecutionInput,
			ReadInvocation,
			ResolveApproval,
			Usage,
		};
	}),
);
