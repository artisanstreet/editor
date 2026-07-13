import { and, asc, eq, inArray, ne, or } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	ContentIdentity,
	EventEnvelope,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceChangeDiffQueryResult,
	WorkspaceReplaceApproval,
	WorkspaceReplaceApprovalQuery,
	WorkspaceReplaceApprovalQueryResult,
	WorkspaceReplaceApprovalReason,
	workspace_diff_maximum_rendered_lines,
	type EventEnvelope as EventEnvelopeValue,
	type RawOrigin as RawOriginValue,
	type WorkspaceReplaceApproval as WorkspaceReplaceApprovalValue,
	type WorkspaceReplaceApprovalQueryResult as WorkspaceReplaceApprovalQueryResultValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChangeSnapshots,
	WorkspaceMutationPayloads,
	WorkspaceMutationAuthorities,
	WorkspaceReplaceApprovals,
} from "../persistence/schema";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
} from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	PreparedWorkspaceChangeDiff,
	type PreparedWorkspaceChangeDiff as PreparedWorkspaceChangeDiffValue,
} from "./workspace-change-diff-service";
import { workspace_diff_patch_matches_path } from "./workspace-change-diff-format";
import type { WorkspaceChangeOperation } from "./workspace-change-repository";

const WorkspaceReplaceApprovalDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});
const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));

/** Binds one pending approval request to an already-claimed replacement and exact diff. */
export interface RequestWorkspaceReplaceApproval {
	readonly operation: Extract<WorkspaceChangeOperation, { readonly action: "replace" }>;
	readonly policy: "on_request" | "always";
	readonly prepared_diff: PreparedWorkspaceChangeDiffValue;
	readonly reason: string;
}

/** Carries one user decision for a pending controlled replacement. */
export type WorkspaceReplaceApprovalDecision = typeof WorkspaceReplaceApprovalDecision.Type;

/** Returns the durable event that accepted or replayed an approval transition. */
export interface WorkspaceReplaceApprovalAcceptance {
	readonly approval: WorkspaceReplaceApprovalValue;
	readonly event: EventEnvelopeValue;
	readonly status: "accepted" | "duplicate";
}

/** Carries the private data needed to resume one approved replacement. */
export interface WorkspaceReplaceApprovalExecution {
	readonly approval: WorkspaceReplaceApprovalValue;
	readonly message_id: string;
	readonly prepared_diff: PreparedWorkspaceChangeDiffValue;
	readonly raw_origin?: RawOriginValue;
	readonly request_fingerprint: string;
	readonly sent_at: string;
}

/** Carries the immutable operation binding needed to settle a denied approval. */
export interface WorkspaceReplaceApprovalDenial {
	readonly approval: Extract<WorkspaceReplaceApprovalValue, { readonly state: "denied" }>;
	readonly message_id: string;
}

/** Reports an immutable request or decision collision. */
export class WorkspaceReplaceApprovalConflict extends Data.TaggedError(
	"WorkspaceReplaceApprovalConflict",
)<{ readonly reason: "decision_conflict" | "request_conflict" | "terminal_state" }> {}

/** Reports that an approval is missing or belongs to erased thread content. */
export class WorkspaceReplaceApprovalUnavailable extends Data.TaggedError(
	"WorkspaceReplaceApprovalUnavailable",
)<{ readonly reason: "erased" | "missing" }> {}

/** Reports corrupt approval, operation, diff, or journal state without exposing source bytes. */
export class WorkspaceReplaceApprovalInvariant extends Data.TaggedError(
	"WorkspaceReplaceApprovalInvariant",
)<{ readonly message: string }> {}

/** Represents failures surfaced by the durable workspace approval repository. */
export type WorkspaceReplaceApprovalRepositoryError =
	| CommandIdConflict
	| WorkspaceReplaceApprovalConflict
	| WorkspaceReplaceApprovalInvariant
	| WorkspaceReplaceApprovalUnavailable
	| JournalStoreFailure;

/** Owns durable controlled-replacement approval state and its private pending diff. */
export class WorkspaceReplaceApprovalRepository extends Context.Service<
	WorkspaceReplaceApprovalRepository,
	{
		readonly Request: (
			input: RequestWorkspaceReplaceApproval,
		) => Effect.Effect<
			WorkspaceReplaceApprovalAcceptance,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly Decide: (
			input: WorkspaceReplaceApprovalDecision,
		) => Effect.Effect<
			WorkspaceReplaceApprovalAcceptance,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly MarkExecuting: (
			approval_id: string,
		) => Effect.Effect<
			WorkspaceReplaceApprovalAcceptance,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly MarkApplied: (
			approval_id: string,
		) => Effect.Effect<
			WorkspaceReplaceApprovalAcceptance,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly MarkRejected: (
			approval_id: string,
		) => Effect.Effect<
			WorkspaceReplaceApprovalAcceptance,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly ReadExecution: (
			approval_id: string,
		) => Effect.Effect<
			WorkspaceReplaceApprovalExecution,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly ReadByMessage: (
			message_id: string,
		) => Effect.Effect<
			Option.Option<WorkspaceReplaceApprovalAcceptance>,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly ReadDenied: (
			approval_id: string,
		) => Effect.Effect<WorkspaceReplaceApprovalDenial, WorkspaceReplaceApprovalRepositoryError>;
		readonly ListExecutable: Effect.Effect<
			ReadonlyArray<string>,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly ListDeniedUnsettled: Effect.Effect<
			ReadonlyArray<string>,
			WorkspaceReplaceApprovalRepositoryError
		>;
		readonly Query: (
			query: typeof WorkspaceReplaceApprovalQuery.Type,
		) => Effect.Effect<
			WorkspaceReplaceApprovalQueryResultValue,
			WorkspaceReplaceApprovalRepositoryError
		>;
	}
>()("Artisan/WorkspaceReplaceApprovalRepository") {}

type ApprovalRow = typeof WorkspaceReplaceApprovals.$inferSelect;
type OperationRow = typeof WorkspaceChangeOperations.$inferSelect;

function identities_match(left: typeof ContentIdentity.Type, right: typeof ContentIdentity.Type) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

function raw_origins_match(left: RawOriginValue | undefined, right: RawOriginValue | undefined) {
	return JSON.stringify(left) === JSON.stringify(right);
}

function replace_operation_state_is_valid(operation: OperationRow) {
	const has_journal_sequence =
		operation.journal_sequence !== null &&
		Number.isInteger(operation.journal_sequence) &&
		operation.journal_sequence >= 1;

	if (operation.lifecycle === "committed") {
		return has_journal_sequence;
	}

	if (
		operation.lifecycle === "claimed" ||
		operation.lifecycle === "applied" ||
		operation.lifecycle === "rejected"
	) {
		return operation.journal_sequence === null && !operation.evidence_recorded;
	}

	return false;
}

function operation_state_matches_approval(
	operation: OperationRow,
	state: WorkspaceReplaceApprovalValue["state"],
) {
	if (state === "applied") {
		return operation.lifecycle === "committed" && operation.evidence_recorded;
	}

	if (state === "rejected") {
		return operation.lifecycle === "rejected";
	}

	return true;
}

function normalize_error(error: unknown): WorkspaceReplaceApprovalRepositoryError {
	if (
		error instanceof CommandIdConflict ||
		error instanceof WorkspaceReplaceApprovalConflict ||
		error instanceof WorkspaceReplaceApprovalInvariant ||
		error instanceof WorkspaceReplaceApprovalUnavailable
	) {
		return error;
	}

	if (error instanceof JournalInvariantError) {
		return new WorkspaceReplaceApprovalInvariant({ message: error.message });
	}

	return new JournalStoreFailure({ cause: error });
}

function invariant(message: string) {
	return new WorkspaceReplaceApprovalInvariant({ message });
}

const DecodeJson = Schema.decodeUnknownEffect(Schema.UnknownFromJsonString);

function DecodeStoredJson<A>(schema: Schema.Codec<A, A>, value: string, message: string) {
	return DecodeJson(value).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError(() => invariant(message)),
	);
}

function DecodeStoredRawOrigin(value: string | null, message: string) {
	return value === null
		? Effect.succeed<RawOriginValue | undefined>(undefined)
		: DecodeStoredJson(RawOrigin, value, message).pipe(
				Effect.map((raw_origin): RawOriginValue | undefined => raw_origin),
			);
}

function approval_event_key(approval_id: string, state: WorkspaceReplaceApprovalValue["state"]) {
	return `workspace_replace_approval:${approval_id}:${state}`;
}

function decision_payload_json(input: WorkspaceReplaceApprovalDecision) {
	return JSON.stringify({
		approval_id: input.approval_id,
		approved: input.approved,
		type: "workspace.replace_approval.response",
	});
}

/** Supplies the SQLite-backed controlled-replacement approval repository. */
export const WorkspaceReplaceApprovalRepositoryLive = Layer.effect(
	WorkspaceReplaceApprovalRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select({ thread_id: Threads.thread_id })
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [claim] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (!thread || claim || tombstone) {
					return yield* new WorkspaceReplaceApprovalUnavailable({ reason: "erased" });
				}
			});

		const DecodeApproval = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const before_identity = yield* DecodeStoredJson(
					ContentIdentity,
					row.before_identity_json,
					`Workspace approval ${row.approval_id} has an invalid before identity`,
				);
				const after_identity = yield* DecodeStoredJson(
					ContentIdentity,
					row.after_identity_json,
					`Workspace approval ${row.approval_id} has an invalid after identity`,
				);
				const common = {
					after_identity,
					agent_id: row.agent_id,
					approval_id: row.approval_id,
					before_identity,
					change_id: row.change_id,
					created_at: row.created_at,
					path: row.path,
					policy: row.policy,
					reason: row.reason,
					run_id: row.run_id,
					thread_id: row.thread_id,
					updated_at: row.updated_at,
					workspace_id: row.workspace_id,
				};
				const value =
					row.state === "requested"
						? { ...common, state: row.state }
						: row.state === "denied"
							? {
									...common,
									decided_at: row.decided_at,
									decision: "denied" as const,
									decision_message_id: row.decision_message_id,
									state: row.state,
								}
							: {
									...common,
									decided_at: row.decided_at,
									decision: "approved" as const,
									decision_message_id: row.decision_message_id,
									state: row.state,
								};

				return yield* Schema.decodeUnknownEffect(WorkspaceReplaceApproval, {
					onExcessProperty: "error",
				})(value).pipe(
					Effect.mapError(() =>
						invariant(`Workspace approval ${row.approval_id} is corrupt`),
					),
				);
			});

		const DecodePreparedDiff = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const before_identity = yield* DecodeStoredJson(
					ContentIdentity,
					row.before_identity_json,
					`Workspace approval ${row.approval_id} has an invalid before identity`,
				);
				const after_identity = yield* DecodeStoredJson(
					ContentIdentity,
					row.after_identity_json,
					`Workspace approval ${row.approval_id} has an invalid after identity`,
				);
				const patch = Uint8Array.from(row.patch);
				const digest = yield* crypto
					.digest("SHA-256", patch)
					.pipe(
						Effect.mapError(() =>
							invariant(
								`Workspace approval ${row.approval_id} diff cannot be verified`,
							),
						),
					);
				const patch_text = yield* Effect.try({
					catch: () =>
						invariant(`Workspace approval ${row.approval_id} diff is not UTF-8`),
					try: () => new TextDecoder("utf-8", { fatal: true }).decode(patch),
				});
				const rendered_line_count =
					patch_text.length === 0
						? 0
						: patch_text.split("\n").length - (patch_text.endsWith("\n") ? 1 : 0);
				const prepared = yield* Schema.decodeUnknownEffect(PreparedWorkspaceChangeDiff, {
					onExcessProperty: "error",
				})({
					added_line_count: row.added_line_count,
					after_identity,
					before_identity,
					change_id: row.change_id,
					context_lines: row.context_lines,
					format: row.format,
					format_version: row.format_version,
					message_id: row.message_id,
					patch,
					patch_identity: {
						algorithm: "sha256",
						byte_count: row.patch_byte_count,
						content_hash: row.patch_hash,
					},
					path: row.path,
					removed_line_count: row.removed_line_count,
					thread_id: row.thread_id,
					workspace_id: row.workspace_id,
				}).pipe(
					Effect.mapError(() =>
						invariant(`Workspace approval ${row.approval_id} diff is corrupt`),
					),
				);

				if (
					Encoding.encodeHex(digest) !== row.patch_hash ||
					rendered_line_count > workspace_diff_maximum_rendered_lines ||
					!workspace_diff_patch_matches_path(patch_text, row.path)
				) {
					return yield* invariant(
						`Workspace approval ${row.approval_id} diff is corrupt`,
					);
				}

				return { patch_text, prepared };
			});

		const ReadRecord = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(WorkspaceReplaceApprovals)
				.where(eq(WorkspaceReplaceApprovals.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(
									new WorkspaceReplaceApprovalUnavailable({ reason: "missing" }),
								),
					),
				);

		const ReadEvent = (transaction: typeof database.client, idempotency_key: string) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.idempotency_key, idempotency_key))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						Effect.gen(function* () {
							if (!row) {
								return yield* Effect.fail(
									invariant(`Approval event ${idempotency_key} is missing`),
								);
							}

							if (
								row.event_type !== "workspace.replace.approval.updated" ||
								row.origin !== "backend" ||
								row.stream_id !== `thread:${row.thread_id}`
							) {
								return yield* Effect.fail(
									invariant(
										`Approval event ${idempotency_key} has invalid ownership`,
									),
								);
							}

							const raw_origin =
								row.raw_origin_json === null
									? undefined
									: yield* Effect.try({
											catch: () =>
												invariant(
													`Approval event ${idempotency_key} is corrupt`,
												),
											try: () => JSON.parse(row.raw_origin_json!) as unknown,
										});
							const payload = yield* Effect.try({
								catch: () =>
									invariant(`Approval event ${idempotency_key} is corrupt`),
								try: () => JSON.parse(row.payload_json) as unknown,
							});

							return yield* Schema.decodeUnknownEffect(EventEnvelope, {
								onExcessProperty: "error",
							})({
								agent_id: row.agent_id ?? undefined,
								causation_id: row.causation_id,
								correlation_id: row.correlation_id,
								journal_sequence: row.sequence,
								kind: "event",
								message_id: row.event_id,
								origin: row.origin,
								protocol_version: 1,
								raw_origin,
								run_id: row.run_id ?? undefined,
								schema_version: row.schema_version,
								sent_at: row.occurred_at,
								sequence: row.stream_sequence,
								stream_id: row.stream_id,
								thread_id: row.thread_id,
								payload,
							}).pipe(
								Effect.mapError(() =>
									invariant(`Approval event ${idempotency_key} is corrupt`),
								),
							);
						}),
					),
				);

		const ReadStateAcceptance = (
			transaction: typeof database.client,
			row: ApprovalRow,
			state: WorkspaceReplaceApprovalValue["state"],
		) =>
			Effect.gen(function* () {
				const current = yield* DecodeApproval(row);
				const current_raw_origin = yield* DecodeStoredRawOrigin(
					row.raw_origin_json,
					`Workspace approval ${row.approval_id} has invalid attribution`,
				);
				const event = yield* ReadEvent(
					transaction,
					approval_event_key(row.approval_id, state),
				);

				if (event.payload.type !== "workspace.replace.approval.updated") {
					return yield* invariant(
						`Approval event ${row.approval_id}:${state} has an invalid payload`,
					);
				}

				const approval = event.payload.approval;
				const is_request = state === "requested";
				const is_decision = state === "approved" || state === "denied";
				const expected_updated_at = is_request
					? row.created_at
					: is_decision
						? row.decided_at
						: row.updated_at;
				const expected_causation_id =
					is_request || is_decision ? row.message_id : row.decision_message_id;
				const expected_correlation_id = is_request
					? row.approval_id
					: is_decision
						? row.decision_message_id
						: row.approval_id;

				if (
					expected_updated_at === null ||
					expected_causation_id === null ||
					expected_correlation_id === null ||
					(row.state === state && current.updated_at !== expected_updated_at) ||
					approval.approval_id !== row.approval_id ||
					approval.state !== state ||
					approval.change_id !== current.change_id ||
					approval.thread_id !== current.thread_id ||
					approval.run_id !== current.run_id ||
					approval.agent_id !== current.agent_id ||
					approval.workspace_id !== current.workspace_id ||
					approval.path !== current.path ||
					approval.policy !== current.policy ||
					approval.reason !== current.reason ||
					approval.created_at !== current.created_at ||
					approval.updated_at !== expected_updated_at ||
					event.agent_id !== current.agent_id ||
					event.causation_id !== expected_causation_id ||
					event.correlation_id !== expected_correlation_id ||
					event.origin !== "backend" ||
					event.run_id !== current.run_id ||
					event.sent_at !== expected_updated_at ||
					event.stream_id !== `thread:${current.thread_id}` ||
					event.thread_id !== current.thread_id ||
					!raw_origins_match(event.raw_origin, current_raw_origin) ||
					!identities_match(approval.before_identity, current.before_identity) ||
					!identities_match(approval.after_identity, current.after_identity)
				) {
					return yield* invariant(
						`Approval event ${row.approval_id}:${state} does not match its record`,
					);
				}

				if (
					!is_request &&
					(approval.state === "requested" ||
						approval.decision !== (row.approved ? "approved" : "denied") ||
						approval.decision_message_id !== row.decision_message_id ||
						approval.decided_at !== row.decided_at)
				) {
					return yield* invariant(
						`Approval decision event ${row.approval_id}:${state} does not match its record`,
					);
				}

				return { approval, event, status: "duplicate" as const };
			});

		const ReadRequestAcceptance = (transaction: typeof database.client, row: ApprovalRow) =>
			ReadStateAcceptance(transaction, row, "requested");

		const ReadDecisionAcceptance = (transaction: typeof database.client, row: ApprovalRow) =>
			row.approved === null || row.decision_message_id === null || row.decided_at === null
				? Effect.fail(
						invariant(`Workspace approval ${row.approval_id} has an invalid decision`),
					)
				: ReadStateAcceptance(transaction, row, row.approved ? "approved" : "denied");

		const ReadCanonicalBinding = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const approval = yield* DecodeApproval(row);
				const { patch_text, prepared } = yield* DecodePreparedDiff(row);
				const raw_origin = yield* DecodeStoredRawOrigin(
					row.raw_origin_json,
					`Workspace approval ${row.approval_id} has invalid attribution`,
				);
				const request_fingerprint = yield* Schema.decodeUnknownEffect(RequestFingerprint)(
					row.request_fingerprint,
				).pipe(
					Effect.mapError(() =>
						invariant(
							`Workspace approval ${row.approval_id} has an invalid request fingerprint`,
						),
					),
				);
				const sent_at = yield* Schema.decodeUnknownEffect(IsoDateTime)(
					row.operation_sent_at,
				).pipe(
					Effect.mapError(() =>
						invariant(
							`Workspace approval ${row.approval_id} has an invalid operation time`,
						),
					),
				);
				const [operation] = yield* transaction
					.select()
					.from(WorkspaceChangeOperations)
					.where(eq(WorkspaceChangeOperations.message_id, row.message_id))
					.limit(1);
				const [authority] = yield* transaction
					.select()
					.from(WorkspaceMutationAuthorities)
					.where(eq(WorkspaceMutationAuthorities.message_id, row.message_id))
					.limit(1);

				if (!operation || !authority) {
					return yield* invariant(
						`Workspace approval ${row.approval_id} is missing its canonical operation`,
					);
				}

				const operation_before = yield* DecodeStoredJson(
					ContentIdentity,
					operation.expected_identity_json ?? "",
					`Workspace approval ${row.approval_id} has an invalid operation before identity`,
				);
				const operation_after = yield* DecodeStoredJson(
					ContentIdentity,
					operation.result_identity_json ?? "",
					`Workspace approval ${row.approval_id} has an invalid operation after identity`,
				);
				const operation_raw_origin = yield* DecodeStoredRawOrigin(
					operation.raw_origin_json,
					`Workspace approval ${row.approval_id} has invalid operation attribution`,
				);
				const request_acceptance = yield* ReadRequestAcceptance(transaction, row);
				const decision_acceptance =
					approval.state === "requested"
						? undefined
						: yield* ReadDecisionAcceptance(transaction, row);
				const current_acceptance =
					approval.state === "requested"
						? request_acceptance
						: approval.state === "approved" || approval.state === "denied"
							? decision_acceptance!
							: yield* ReadStateAcceptance(transaction, row, approval.state);
				const expected_policy = authority.approval ?? "on_request";

				if (
					operation.action !== "replace" ||
					!replace_operation_state_is_valid(operation) ||
					!operation_state_matches_approval(operation, approval.state) ||
					operation.message_id !== row.message_id ||
					operation.change_id !== approval.change_id ||
					operation.thread_id !== approval.thread_id ||
					operation.run_id !== approval.run_id ||
					operation.agent_id !== approval.agent_id ||
					operation.workspace_id !== approval.workspace_id ||
					operation.path !== approval.path ||
					operation.request_fingerprint !== request_fingerprint ||
					operation.sent_at !== sent_at ||
					operation.diff_format_version !== prepared.format_version ||
					authority.change_id !== approval.change_id ||
					authority.thread_id !== approval.thread_id ||
					authority.run_id !== approval.run_id ||
					authority.agent_id !== approval.agent_id ||
					authority.workspace_id !== approval.workspace_id ||
					expected_policy !== approval.policy ||
					prepared.message_id !== row.message_id ||
					prepared.change_id !== approval.change_id ||
					prepared.thread_id !== approval.thread_id ||
					prepared.workspace_id !== approval.workspace_id ||
					prepared.path !== approval.path ||
					request_acceptance.approval.state !== "requested" ||
					(approval.state !== "requested" &&
						decision_acceptance?.approval.state !==
							(approval.state === "denied" ? "denied" : "approved")) ||
					current_acceptance.approval.state !== approval.state ||
					!identities_match(operation_before, approval.before_identity) ||
					!identities_match(operation_after, approval.after_identity) ||
					!identities_match(prepared.before_identity, approval.before_identity) ||
					!identities_match(prepared.after_identity, approval.after_identity) ||
					!raw_origins_match(raw_origin, operation_raw_origin)
				) {
					return yield* invariant(
						`Workspace approval ${row.approval_id} does not match its canonical operation`,
					);
				}

				return {
					approval,
					current_acceptance,
					message_id: row.message_id,
					operation,
					patch_text,
					prepared_diff: prepared,
					...(operation_raw_origin === undefined
						? {}
						: { raw_origin: operation_raw_origin }),
					request_acceptance,
					request_fingerprint,
					sent_at,
				};
			});

		const AppendEvent = (
			transaction: typeof database.client,
			input: {
				readonly approval: WorkspaceReplaceApprovalValue;
				readonly causation_id: string;
				readonly correlation_id: string;
				readonly idempotency_key: string;
				readonly raw_origin?: RawOriginValue;
			},
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.approval.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const occurred_at = input.approval.updated_at;
				const payload = {
					approval: input.approval,
					type: "workspace.replace.approval.updated",
				} as const;

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction
						.insert(EventStreams)
						.values({ last_sequence: sequence, stream_id });
				}

				const [row] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.approval.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: payload.type,
						idempotency_key: input.idempotency_key,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						raw_origin_json:
							input.raw_origin === undefined
								? null
								: JSON.stringify(input.raw_origin),
						run_id: input.approval.run_id,
						schema_version: 1,
						stream_id,
						stream_sequence: sequence,
						thread_id: input.approval.thread_id,
					})
					.returning();

				if (!row) {
					return yield* invariant("Workspace approval event was not persisted");
				}

				return yield* ReadEvent(transaction, input.idempotency_key);
			});

		const ValidateRequestBinding = (
			transaction: typeof database.client,
			input: RequestWorkspaceReplaceApproval,
		) =>
			Effect.gen(function* () {
				const operation = input.operation;
				const reason = yield* Schema.decodeUnknownEffect(WorkspaceReplaceApprovalReason)(
					input.reason,
				).pipe(
					Effect.mapError(
						() => new WorkspaceReplaceApprovalConflict({ reason: "request_conflict" }),
					),
				);
				const prepared_diff = yield* Schema.decodeUnknownEffect(
					PreparedWorkspaceChangeDiff,
					{
						onExcessProperty: "error",
					},
				)(input.prepared_diff).pipe(
					Effect.mapError(() => invariant("Workspace approval diff input is invalid")),
				);
				const [stored_operation] = yield* transaction
					.select()
					.from(WorkspaceChangeOperations)
					.where(eq(WorkspaceChangeOperations.message_id, operation.message_id))
					.limit(1);
				const [authority] = yield* transaction
					.select()
					.from(WorkspaceMutationAuthorities)
					.where(eq(WorkspaceMutationAuthorities.message_id, operation.message_id))
					.limit(1);

				if (!stored_operation || !authority) {
					return yield* invariant("Workspace approval is missing its mutation authority");
				}

				const stored_before = yield* DecodeStoredJson(
					ContentIdentity,
					stored_operation.expected_identity_json ?? "",
					"Workspace approval operation has an invalid before identity",
				);
				const stored_after = yield* DecodeStoredJson(
					ContentIdentity,
					stored_operation.result_identity_json ?? "",
					"Workspace approval operation has an invalid after identity",
				);
				const stored_raw_origin = yield* DecodeStoredRawOrigin(
					stored_operation.raw_origin_json,
					"Workspace approval operation has invalid attribution",
				);
				const expected_policy = authority.approval ?? "on_request";
				const patch_digest = yield* crypto
					.digest("SHA-256", prepared_diff.patch)
					.pipe(
						Effect.mapError(() =>
							invariant("Workspace approval diff input cannot be verified"),
						),
					);
				const patch_text = yield* Effect.try({
					catch: () => invariant("Workspace approval diff input is not UTF-8"),
					try: () =>
						new TextDecoder("utf-8", { fatal: true }).decode(prepared_diff.patch),
				});
				const rendered_line_count =
					patch_text.length === 0
						? 0
						: patch_text.split("\n").length - (patch_text.endsWith("\n") ? 1 : 0);

				if (
					stored_operation.action !== "replace" ||
					stored_operation.message_id !== operation.message_id ||
					stored_operation.change_id !== operation.change_id ||
					stored_operation.thread_id !== operation.thread_id ||
					stored_operation.run_id !== operation.run_id ||
					stored_operation.agent_id !== operation.agent_id ||
					stored_operation.workspace_id !== operation.workspace_id ||
					stored_operation.path !== operation.path ||
					stored_operation.request_fingerprint !== operation.request_fingerprint ||
					stored_operation.sent_at !== operation.sent_at ||
					!identities_match(stored_before, operation.expected_identity) ||
					!identities_match(stored_after, operation.result_identity) ||
					!raw_origins_match(stored_raw_origin, operation.raw_origin) ||
					authority.change_id !== operation.change_id ||
					authority.thread_id !== operation.thread_id ||
					authority.run_id !== operation.run_id ||
					authority.agent_id !== operation.agent_id ||
					authority.workspace_id !== operation.workspace_id ||
					expected_policy !== input.policy ||
					prepared_diff.message_id !== operation.message_id ||
					prepared_diff.change_id !== operation.change_id ||
					prepared_diff.thread_id !== operation.thread_id ||
					prepared_diff.workspace_id !== operation.workspace_id ||
					prepared_diff.path !== operation.path ||
					!identities_match(prepared_diff.before_identity, operation.expected_identity) ||
					!identities_match(prepared_diff.after_identity, operation.result_identity) ||
					Encoding.encodeHex(patch_digest) !==
						prepared_diff.patch_identity.content_hash ||
					rendered_line_count > workspace_diff_maximum_rendered_lines ||
					!workspace_diff_patch_matches_path(patch_text, operation.path)
				) {
					return yield* new WorkspaceReplaceApprovalConflict({
						reason: "request_conflict",
					});
				}

				return {
					operation,
					prepared_diff,
					reason,
					stored_lifecycle: stored_operation.lifecycle,
				};
			});

		const ValidateExistingRequest = (
			row: ApprovalRow,
			input: RequestWorkspaceReplaceApproval,
		) =>
			Effect.gen(function* () {
				const approval = yield* DecodeApproval(row);
				const { prepared } = yield* DecodePreparedDiff(row);
				const operation = input.operation;

				if (
					row.message_id !== operation.message_id ||
					row.request_fingerprint !== operation.request_fingerprint ||
					row.operation_sent_at !== operation.sent_at ||
					approval.change_id !== operation.change_id ||
					approval.thread_id !== operation.thread_id ||
					approval.run_id !== operation.run_id ||
					approval.agent_id !== operation.agent_id ||
					approval.workspace_id !== operation.workspace_id ||
					approval.path !== operation.path ||
					approval.policy !== input.policy ||
					approval.reason !== input.reason ||
					!identities_match(approval.before_identity, operation.expected_identity) ||
					!identities_match(approval.after_identity, operation.result_identity) ||
					prepared.patch_identity.content_hash !==
						input.prepared_diff.patch_identity.content_hash ||
					prepared.patch_identity.byte_count !==
						input.prepared_diff.patch_identity.byte_count
				) {
					return yield* new WorkspaceReplaceApprovalConflict({
						reason: "request_conflict",
					});
				}

				return approval;
			});

		const Request = (input: RequestWorkspaceReplaceApproval) =>
			Effect.gen(function* () {
				const result = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const validated = yield* ValidateRequestBinding(transaction, input);

							yield* EnsureLiveThread(transaction, validated.operation.thread_id);

							const existing_rows = yield* transaction
								.select()
								.from(WorkspaceReplaceApprovals)
								.where(
									or(
										eq(
											WorkspaceReplaceApprovals.message_id,
											validated.operation.message_id,
										),
										eq(
											WorkspaceReplaceApprovals.change_id,
											validated.operation.change_id,
										),
									),
								)
								.limit(2);

							if (existing_rows.length > 1) {
								return yield* invariant(
									"Workspace approval request identity is corrupt",
								);
							}

							const existing = existing_rows[0];

							if (existing) {
								yield* ValidateExistingRequest(existing, input);

								return (yield* ReadCanonicalBinding(transaction, existing))
									.request_acceptance;
							}

							if (validated.stored_lifecycle !== "claimed") {
								return yield* new WorkspaceReplaceApprovalConflict({
									reason: "terminal_state",
								});
							}

							const approval_id = yield* metadata.MakeId("approval");
							const now = yield* metadata.Now;
							const operation = validated.operation;
							const prepared_diff = validated.prepared_diff;
							const [inserted] = yield* transaction
								.insert(WorkspaceReplaceApprovals)
								.values({
									added_line_count: prepared_diff.added_line_count,
									after_identity_json: JSON.stringify(operation.result_identity),
									agent_id: operation.agent_id,
									approval_id,
									approved: null,
									before_identity_json: JSON.stringify(
										operation.expected_identity,
									),
									change_id: operation.change_id,
									context_lines: prepared_diff.context_lines,
									created_at: now,
									decided_at: null,
									decision_message_id: null,
									format: prepared_diff.format,
									format_version: prepared_diff.format_version,
									message_id: operation.message_id,
									operation_sent_at: operation.sent_at,
									patch: Buffer.from(prepared_diff.patch),
									patch_byte_count: prepared_diff.patch_identity.byte_count,
									patch_hash: prepared_diff.patch_identity.content_hash,
									path: operation.path,
									policy: input.policy,
									raw_origin_json:
										operation.raw_origin === undefined
											? null
											: JSON.stringify(operation.raw_origin),
									reason: validated.reason,
									request_fingerprint: operation.request_fingerprint,
									removed_line_count: prepared_diff.removed_line_count,
									run_id: operation.run_id,
									state: "requested",
									thread_id: operation.thread_id,
									updated_at: now,
									workspace_id: operation.workspace_id,
								})
								.onConflictDoNothing()
								.returning();

							if (!inserted) {
								const [concurrent] = yield* transaction
									.select()
									.from(WorkspaceReplaceApprovals)
									.where(
										or(
											eq(
												WorkspaceReplaceApprovals.message_id,
												operation.message_id,
											),
											eq(
												WorkspaceReplaceApprovals.change_id,
												operation.change_id,
											),
										),
									)
									.limit(1);

								if (!concurrent) {
									return yield* invariant(
										"Workspace approval request lost its insert race",
									);
								}

								yield* ValidateExistingRequest(concurrent, input);

								return (yield* ReadCanonicalBinding(transaction, concurrent))
									.request_acceptance;
							}

							const approval = yield* DecodeApproval(inserted);
							const event = yield* AppendEvent(transaction, {
								approval,
								causation_id: operation.message_id,
								correlation_id: approval.approval_id,
								idempotency_key: approval_event_key(
									approval.approval_id,
									approval.state,
								),
								...(operation.raw_origin === undefined
									? {}
									: { raw_origin: operation.raw_origin }),
							});

							return { approval, event, status: "accepted" as const };
						}),
					),
				);

				if (result.status === "accepted") {
					yield* notifier.Publish(result.event.journal_sequence);
				}

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const ValidateDecisionCommand = (
			transaction: typeof database.client,
			input: WorkspaceReplaceApprovalDecision,
		) =>
			Effect.gen(function* () {
				const [command] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, input.message_id))
					.limit(1);

				if (!command) {
					return yield* invariant(
						`Workspace approval decision ${input.message_id} is missing`,
					);
				}

				if (
					command.agent_id !== null ||
					command.assigned_run_id !== null ||
					command.causation_id !== null ||
					command.origin !== "frontend" ||
					command.payload_json !== decision_payload_json(input) ||
					command.payload_type !== "workspace.replace_approval.response" ||
					command.raw_origin_json !== null ||
					command.run_id !== null ||
					command.schema_version !== 1 ||
					command.sent_at !== input.sent_at ||
					command.status !== "accepted" ||
					command.thread_id !== input.thread_id
				) {
					return yield* new CommandIdConflict({ message_id: input.message_id });
				}
			});

		const Decide = (input: WorkspaceReplaceApprovalDecision) =>
			Schema.decodeUnknownEffect(WorkspaceReplaceApprovalDecision, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(
					() => new WorkspaceReplaceApprovalConflict({ reason: "decision_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRecord(transaction, decoded.approval_id);

									yield* EnsureLiveThread(transaction, decoded.thread_id);

									if (row.thread_id !== decoded.thread_id) {
										return yield* new WorkspaceReplaceApprovalUnavailable({
											reason: "missing",
										});
									}

									yield* ReadCanonicalBinding(transaction, row);

									if (row.decision_message_id !== null) {
										if (
											row.decision_message_id !== decoded.message_id ||
											row.approved !== decoded.approved
										) {
											return yield* new WorkspaceReplaceApprovalConflict({
												reason: "decision_conflict",
											});
										}

										yield* ValidateDecisionCommand(transaction, decoded);

										return yield* ReadDecisionAcceptance(transaction, row);
									}

									if (row.state !== "requested") {
										return yield* new WorkspaceReplaceApprovalConflict({
											reason: "terminal_state",
										});
									}

									const [existing_command] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(eq(JournalCommands.message_id, decoded.message_id))
										.limit(1);
									const [existing_operation] = yield* transaction
										.select({
											message_id: WorkspaceChangeOperations.message_id,
										})
										.from(WorkspaceChangeOperations)
										.where(
											eq(
												WorkspaceChangeOperations.message_id,
												decoded.message_id,
											),
										)
										.limit(1);

									if (existing_command || existing_operation) {
										return yield* new CommandIdConflict({
											message_id: decoded.message_id,
										});
									}

									const decided_at = yield* metadata.Now;
									const state = decoded.approved ? "approved" : "denied";
									const [updated] = yield* transaction
										.update(WorkspaceReplaceApprovals)
										.set({
											approved: decoded.approved,
											decided_at,
											decision_message_id: decoded.message_id,
											state,
											updated_at: decided_at,
										})
										.where(
											and(
												eq(
													WorkspaceReplaceApprovals.approval_id,
													decoded.approval_id,
												),
												eq(WorkspaceReplaceApprovals.state, "requested"),
											),
										)
										.returning();

									if (!updated) {
										return yield* new WorkspaceReplaceApprovalConflict({
											reason: "decision_conflict",
										});
									}

									yield* transaction.insert(JournalCommands).values({
										accepted_at: decided_at,
										agent_id: null,
										causation_id: null,
										message_id: decoded.message_id,
										origin: "frontend",
										payload_json: decision_payload_json(decoded),
										payload_type: "workspace.replace_approval.response",
										raw_origin_json: null,
										run_id: null,
										schema_version: 1,
										sent_at: decoded.sent_at,
										status: "accepted",
										thread_id: decoded.thread_id,
									});

									const approval = yield* DecodeApproval(updated);
									const raw_origin = yield* DecodeStoredRawOrigin(
										updated.raw_origin_json,
										`Workspace approval ${updated.approval_id} has invalid attribution`,
									);
									const event = yield* AppendEvent(transaction, {
										approval,
										causation_id: updated.message_id,
										correlation_id: decoded.message_id,
										idempotency_key: approval_event_key(
											approval.approval_id,
											approval.state,
										),
										...(raw_origin === undefined ? {} : { raw_origin }),
									});

									return { approval, event, status: "accepted" as const };
								}),
							),
						);

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
				Effect.mapError(normalize_error),
			);

		const Transition = (approval_id: string, target: "executing" | "applied" | "rejected") =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => invariant("Workspace approval id is invalid")),
				Effect.flatMap((decoded_approval_id) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRecord(transaction, decoded_approval_id);

									yield* EnsureLiveThread(transaction, row.thread_id);
									const binding = yield* ReadCanonicalBinding(transaction, row);

									if (row.state === target) {
										return binding.current_acceptance;
									}

									const allowed =
										target === "executing"
											? row.state === "approved"
											: row.state === "approved" || row.state === "executing";

									if (!allowed) {
										return yield* new WorkspaceReplaceApprovalConflict({
											reason: "terminal_state",
										});
									}

									const operation = binding.operation;
									const expected_lifecycle =
										target === "executing"
											? "claimed"
											: target === "applied"
												? "committed"
												: "rejected";

									if (
										operation.lifecycle !== expected_lifecycle ||
										!operation_state_matches_approval(operation, target)
									) {
										return yield* invariant(
											`Workspace approval ${row.approval_id} does not match its operation lifecycle`,
										);
									}

									const updated_at = yield* metadata.Now;
									const [updated] = yield* transaction
										.update(WorkspaceReplaceApprovals)
										.set({ state: target, updated_at })
										.where(
											and(
												eq(
													WorkspaceReplaceApprovals.approval_id,
													row.approval_id,
												),
												eq(WorkspaceReplaceApprovals.state, row.state),
											),
										)
										.returning();

									if (!updated) {
										return yield* new WorkspaceReplaceApprovalConflict({
											reason: "terminal_state",
										});
									}

									const approval = yield* DecodeApproval(updated);
									const raw_origin = yield* DecodeStoredRawOrigin(
										updated.raw_origin_json,
										`Workspace approval ${updated.approval_id} has invalid attribution`,
									);
									const event = yield* AppendEvent(transaction, {
										approval,
										causation_id:
											updated.decision_message_id ?? updated.message_id,
										correlation_id: updated.approval_id,
										idempotency_key: approval_event_key(
											approval.approval_id,
											approval.state,
										),
										...(raw_origin === undefined ? {} : { raw_origin }),
									});

									return { approval, event, status: "accepted" as const };
								}),
							),
						);

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
				Effect.mapError(normalize_error),
			);

		const ReadExecution = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => invariant("Workspace approval id is invalid")),
				Effect.flatMap((decoded_approval_id) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRecord(transaction, decoded_approval_id);

							yield* EnsureLiveThread(transaction, row.thread_id);

							if (row.state !== "approved" && row.state !== "executing") {
								return yield* new WorkspaceReplaceApprovalConflict({
									reason: "terminal_state",
								});
							}

							return yield* ReadCanonicalBinding(transaction, row);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadDenied = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => invariant("Workspace approval id is invalid")),
				Effect.flatMap((decoded_approval_id) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRecord(transaction, decoded_approval_id);

							yield* EnsureLiveThread(transaction, row.thread_id);

							const binding = yield* ReadCanonicalBinding(transaction, row);
							const approval = binding.approval;

							if (approval.state !== "denied") {
								return yield* new WorkspaceReplaceApprovalConflict({
									reason: "terminal_state",
								});
							}

							return { approval, message_id: binding.message_id };
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadByMessage = (message_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(message_id).pipe(
				Effect.mapError(() => invariant("Workspace operation message id is invalid")),
				Effect.flatMap((decoded_message_id) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [row] = yield* transaction
								.select()
								.from(WorkspaceReplaceApprovals)
								.where(eq(WorkspaceReplaceApprovals.message_id, decoded_message_id))
								.limit(1);

							if (!row) {
								return Option.none<WorkspaceReplaceApprovalAcceptance>();
							}

							yield* EnsureLiveThread(transaction, row.thread_id);

							return Option.some(
								(yield* ReadCanonicalBinding(transaction, row)).request_acceptance,
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListExecutable = database.client
			.select({ approval_id: WorkspaceReplaceApprovals.approval_id })
			.from(WorkspaceReplaceApprovals)
			.where(inArray(WorkspaceReplaceApprovals.state, ["approved", "executing"]))
			.orderBy(asc(WorkspaceReplaceApprovals.created_at))
			.pipe(
				Effect.map((rows) => rows.map((row) => row.approval_id)),
				Effect.mapError(normalize_error),
			);
		const ListDeniedUnsettled = database.client
			.select({ approval_id: WorkspaceReplaceApprovals.approval_id })
			.from(WorkspaceReplaceApprovals)
			.innerJoin(
				WorkspaceChangeOperations,
				eq(WorkspaceChangeOperations.message_id, WorkspaceReplaceApprovals.message_id),
			)
			.leftJoin(
				WorkspaceMutationPayloads,
				eq(WorkspaceMutationPayloads.message_id, WorkspaceReplaceApprovals.message_id),
			)
			.leftJoin(
				WorkspaceChangeSnapshots,
				eq(WorkspaceChangeSnapshots.change_id, WorkspaceReplaceApprovals.change_id),
			)
			.where(
				and(
					eq(WorkspaceReplaceApprovals.state, "denied"),
					or(
						ne(WorkspaceChangeOperations.lifecycle, "rejected"),
						eq(WorkspaceMutationPayloads.state, "available"),
						eq(WorkspaceChangeSnapshots.state, "available"),
					),
				),
			)
			.orderBy(asc(WorkspaceReplaceApprovals.created_at))
			.pipe(
				Effect.map((rows) => rows.map((row) => row.approval_id)),
				Effect.mapError(normalize_error),
			);

		const Query = (query: typeof WorkspaceReplaceApprovalQuery.Type) =>
			Schema.decodeUnknownEffect(WorkspaceReplaceApprovalQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(
					() => new WorkspaceReplaceApprovalUnavailable({ reason: "missing" }),
				),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, decoded.thread_id);

							const row = yield* ReadRecord(transaction, decoded.approval_id);

							if (row.thread_id !== decoded.thread_id) {
								return yield* new WorkspaceReplaceApprovalUnavailable({
									reason: "missing",
								});
							}

							const binding = yield* ReadCanonicalBinding(transaction, row);
							const approval = binding.approval;
							const patch_text = binding.patch_text;
							const prepared = binding.prepared_diff;
							const diff = yield* Schema.decodeUnknownEffect(
								WorkspaceChangeDiffQueryResult,
								{ onExcessProperty: "error" },
							)({
								added_line_count: prepared.added_line_count,
								after_identity: prepared.after_identity,
								before_identity: prepared.before_identity,
								change_id: prepared.change_id,
								context_lines: prepared.context_lines,
								format: prepared.format,
								format_version: prepared.format_version,
								patch: patch_text,
								patch_identity: prepared.patch_identity,
								path: prepared.path,
								removed_line_count: prepared.removed_line_count,
								thread_id: prepared.thread_id,
								truncated: false,
								workspace_id: prepared.workspace_id,
							}).pipe(
								Effect.mapError(() =>
									invariant(
										`Workspace approval ${row.approval_id} query is corrupt`,
									),
								),
							);

							return yield* Schema.decodeUnknownEffect(
								WorkspaceReplaceApprovalQueryResult,
								{ onExcessProperty: "error" },
							)({ approval, diff }).pipe(
								Effect.mapError(() =>
									invariant(
										`Workspace approval ${row.approval_id} query is corrupt`,
									),
								),
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		return {
			Decide,
			ListDeniedUnsettled,
			ListExecutable,
			MarkApplied: (approval_id) => Transition(approval_id, "applied"),
			MarkExecuting: (approval_id) => Transition(approval_id, "executing"),
			MarkRejected: (approval_id) => Transition(approval_id, "rejected"),
			Query,
			ReadByMessage,
			ReadDenied,
			ReadExecution,
			Request,
		};
	}),
);
