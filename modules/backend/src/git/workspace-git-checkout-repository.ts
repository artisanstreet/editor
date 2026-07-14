import { and, asc, eq, notInArray, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	EventEnvelope,
	GitBranchName,
	GitObjectId,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceGitCheckoutApproval,
	WorkspaceGitCheckoutApprovalQuery,
	WorkspaceGitCheckoutApprovalQueryResult,
	WorkspaceGitSessionBlocker,
	type EventEnvelope as EventEnvelopeValue,
	type WorkspaceGitCheckoutApproval as WorkspaceGitCheckoutApprovalValue,
	type WorkspaceGitCheckoutApprovalQueryResult as WorkspaceGitCheckoutApprovalQueryResultValue,
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
	WorkspaceGitChangedFiles,
	WorkspaceGitCheckoutApprovals,
	WorkspaceGitCheckoutClaims,
	WorkspaceGitMutationClaims,
	WorkspaceGitSessions,
	WorkspaceMutationAuthorities,
} from "../persistence/schema";
import { JournalStoreFailure } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));

const CommandMetadata = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	message_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	sent_at: IsoDateTime,
});

const RequestCheckout = Schema.Struct({
	approval_id: Identifier,
	expected_session_version: Schema.Int.check(Schema.isGreaterThan(0)),
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	target_branch: GitBranchName,
	target_head: GitObjectId,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const StoredCheckoutRequestPayload = Schema.Struct({
	expected_session_version: Schema.Int.check(Schema.isGreaterThan(0)),
	request_fingerprint: RequestFingerprint,
	target_branch: GitBranchName,
	type: Schema.Literal("workspace.git.checkout.request"),
	workspace_id: Identifier,
});

const StoredCheckoutDecisionPayload = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	type: Schema.Literal("workspace.git.checkout.approval.respond"),
});

/** Supplies one exact checkout command and its resolved target object. */
export type RequestWorkspaceGitCheckout = typeof RequestCheckout.Type;

const CheckoutDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	decision_command: CommandMetadata,
	thread_id: Identifier,
});

/** Supplies one exact user decision for a pending checkout approval. */
export type WorkspaceGitCheckoutDecision = typeof CheckoutDecision.Type;

/** Returns the durable event accepted or replayed for one approval transition. */
export interface WorkspaceGitCheckoutAcceptance {
	readonly approval: WorkspaceGitCheckoutApprovalValue;
	readonly event: EventEnvelopeValue;
	readonly status: "accepted" | "duplicate";
}

/** Carries private inputs needed to execute or recover one approved checkout. */
export interface WorkspaceGitCheckoutExecution {
	readonly approval: WorkspaceGitCheckoutApprovalValue;
	readonly repository_root: string;
	readonly selected_worktree_path: string;
	readonly target_head: string;
}

/** Reports immutable command reuse, a blocked session, claim collision, or bad transition. */
export class WorkspaceGitCheckoutConflict extends Data.TaggedError("WorkspaceGitCheckoutConflict")<{
	readonly reason:
		| "claim_conflict"
		| "command_conflict"
		| "decision_conflict"
		| "invalid_transition"
		| "request_conflict"
		| "session_dirty"
		| "session_stale"
		| "workspace_mutation_active";
}> {}

/** Reports an approval that is missing or belongs to erased thread content. */
export class WorkspaceGitCheckoutUnavailable extends Data.TaggedError(
	"WorkspaceGitCheckoutUnavailable",
)<{ readonly reason: "erased" | "missing" }> {}

/** Conceals corrupt checkout, session, claim, command, or event state. */
export class WorkspaceGitCheckoutInvariant extends Data.TaggedError(
	"WorkspaceGitCheckoutInvariant",
)<{ readonly message: string }> {}

/** Represents failures surfaced by the durable checkout repository. */
export type WorkspaceGitCheckoutRepositoryError =
	| JournalStoreFailure
	| WorkspaceGitCheckoutConflict
	| WorkspaceGitCheckoutInvariant
	| WorkspaceGitCheckoutUnavailable;

/** Owns checkout approval commands, lifecycle state, and the workspace-wide claim. */
export class WorkspaceGitCheckoutRepository extends Context.Service<
	WorkspaceGitCheckoutRepository,
	{
		readonly Decide: (
			input: WorkspaceGitCheckoutDecision,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutRepositoryError>;
		readonly ListApproved: Effect.Effect<
			ReadonlyArray<string>,
			WorkspaceGitCheckoutRepositoryError
		>;
		readonly ListExecuting: Effect.Effect<
			ReadonlyArray<string>,
			WorkspaceGitCheckoutRepositoryError
		>;
		readonly MarkApplied: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutRepositoryError>;
		readonly MarkExecuting: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutRepositoryError>;
		readonly MarkRejected: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutRepositoryError>;
		readonly MarkUnknown: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutRepositoryError>;
		readonly Query: (
			query: typeof WorkspaceGitCheckoutApprovalQuery.Type,
		) => Effect.Effect<
			WorkspaceGitCheckoutApprovalQueryResultValue,
			WorkspaceGitCheckoutRepositoryError
		>;
		readonly ReadExecution: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitCheckoutExecution, WorkspaceGitCheckoutRepositoryError>;
		readonly ReadBySourceCommand: (
			message_id: string,
		) => Effect.Effect<
			Option.Option<WorkspaceGitCheckoutAcceptance>,
			WorkspaceGitCheckoutRepositoryError
		>;
		readonly Request: (
			input: RequestWorkspaceGitCheckout,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutRepositoryError>;
	}
>()("Artisan/WorkspaceGitCheckoutRepository") {}

type ApprovalRow = typeof WorkspaceGitCheckoutApprovals.$inferSelect;
type CommandRow = typeof JournalCommands.$inferSelect;

function invariant(message: string) {
	return new WorkspaceGitCheckoutInvariant({ message });
}

function normalize_error(error: unknown): WorkspaceGitCheckoutRepositoryError {
	if (
		error instanceof WorkspaceGitCheckoutConflict ||
		error instanceof WorkspaceGitCheckoutInvariant ||
		error instanceof WorkspaceGitCheckoutUnavailable
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function approval_event_key(
	approval_id: string,
	state: WorkspaceGitCheckoutApprovalValue["state"],
) {
	return `workspace_git_checkout:${approval_id}:${state}`;
}

function request_payload(input: RequestWorkspaceGitCheckout) {
	return JSON.stringify({
		expected_session_version: input.expected_session_version,
		request_fingerprint: input.request_fingerprint,
		target_branch: input.target_branch,
		type: "workspace.git.checkout.request",
		workspace_id: input.workspace_id,
	});
}

function decision_payload(input: WorkspaceGitCheckoutDecision) {
	return JSON.stringify({
		approval_id: input.approval_id,
		approved: input.approved,
		type: "workspace.git.checkout.approval.respond",
	});
}

function command_matches(
	row: CommandRow,
	metadata: typeof CommandMetadata.Type,
	thread_id: string,
	payload_type: string,
	payload_json: string,
) {
	return (
		row.message_id === metadata.message_id &&
		row.schema_version === 1 &&
		row.thread_id === thread_id &&
		row.run_id === (metadata.run_id ?? null) &&
		row.agent_id === (metadata.agent_id ?? null) &&
		row.causation_id === (metadata.causation_id ?? null) &&
		row.origin === "frontend" &&
		row.raw_origin_json ===
			(metadata.raw_origin === undefined ? null : JSON.stringify(metadata.raw_origin)) &&
		row.sent_at === metadata.sent_at &&
		row.payload_type === payload_type &&
		row.payload_json === payload_json &&
		row.status === "accepted"
	);
}

/** Supplies the SQLite-backed checkout approval and claim repository. */
export const WorkspaceGitCheckoutRepositoryLive = Layer.effect(
	WorkspaceGitCheckoutRepository,
	Effect.gen(function* () {
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
					return yield* new WorkspaceGitCheckoutUnavailable({ reason: "erased" });
				}
			});

		const ReadRow = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(WorkspaceGitCheckoutApprovals)
				.where(eq(WorkspaceGitCheckoutApprovals.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(
									new WorkspaceGitCheckoutUnavailable({ reason: "missing" }),
								),
					),
				);

		const DecodeApproval = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const has_any_decision =
					row.decision_message_id !== null ||
					row.approved !== null ||
					row.decided_at !== null;
				const has_decision =
					row.decision_message_id !== null &&
					row.approved !== null &&
					row.decided_at !== null;
				const has_execution = row.execution_started_at !== null;
				const is_terminal_execution =
					row.state === "applied" || row.state === "rejected" || row.state === "unknown";
				const valid_state =
					(row.state === "requested" && !has_any_decision && !has_execution) ||
					(row.state === "denied" &&
						has_decision &&
						row.approved === false &&
						!has_execution) ||
					(row.state === "approved" &&
						has_decision &&
						row.approved === true &&
						!has_execution) ||
					((row.state === "executing" || is_terminal_execution) &&
						has_decision &&
						row.approved === true &&
						has_execution);

				if (!valid_state) {
					return yield* invariant(
						`Checkout approval ${row.approval_id} has an invalid state`,
					);
				}

				const expected_updated_at =
					row.state === "requested"
						? row.created_at
						: row.state === "approved" || row.state === "denied"
							? row.decided_at
							: row.state === "executing"
								? row.execution_started_at
								: row.updated_at;

				if (expected_updated_at === null || row.updated_at !== expected_updated_at) {
					return yield* invariant(
						`Checkout approval ${row.approval_id} has an invalid update time`,
					);
				}

				yield* Schema.decodeUnknownEffect(RequestFingerprint)(row.request_fingerprint).pipe(
					Effect.mapError(() =>
						invariant(
							`Checkout approval ${row.approval_id} has an invalid fingerprint`,
						),
					),
				);
				yield* Schema.decodeUnknownEffect(GitObjectId)(row.target_head).pipe(
					Effect.mapError(() =>
						invariant(
							`Checkout approval ${row.approval_id} has an invalid target head`,
						),
					),
				);

				if (row.execution_started_at !== null) {
					yield* Schema.decodeUnknownEffect(IsoDateTime)(row.execution_started_at).pipe(
						Effect.mapError(() =>
							invariant(
								`Checkout approval ${row.approval_id} has an invalid execution time`,
							),
						),
					);
				}

				const common = {
					approval_id: row.approval_id,
					created_at: row.created_at,
					expected_session_version: row.expected_session_version,
					source_branch: row.source_branch,
					source_command_id: row.source_command_id,
					source_head: row.source_head,
					target_branch: row.target_branch,
					thread_id: row.thread_id,
					updated_at: row.updated_at,
					workspace_id: row.workspace_id,
				};
				const approval =
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

				return yield* Schema.decodeUnknownEffect(WorkspaceGitCheckoutApproval, {
					onExcessProperty: "error",
				})(approval).pipe(
					Effect.mapError(() =>
						invariant(`Checkout approval ${row.approval_id} is corrupt`),
					),
				);
			});

		const DecodeEventRow = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.mapError(() => invariant("Stored checkout event payload is corrupt")),
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
					schema_version: row.schema_version,
					sent_at: row.occurred_at,
					sequence: row.stream_sequence,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				}).pipe(Effect.mapError(() => invariant("Stored checkout event is corrupt")));
			});

		const ReadEvent = (
			transaction: typeof database.client,
			approval_id: string,
			state: WorkspaceGitCheckoutApprovalValue["state"],
		) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.idempotency_key, approval_event_key(approval_id, state)))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeEventRow(row)
							: Effect.fail(
									invariant(
										`Checkout approval ${approval_id}:${state} event is missing`,
									),
								),
					),
				);

		const ReadAcceptance = (
			transaction: typeof database.client,
			row: ApprovalRow,
			state: WorkspaceGitCheckoutApprovalValue["state"],
		) =>
			Effect.gen(function* () {
				const current = yield* DecodeApproval(row);
				const event = yield* ReadEvent(transaction, row.approval_id, state);

				if (event.payload.type !== "workspace.git.checkout.approval.updated") {
					return yield* invariant(
						`Checkout approval ${row.approval_id}:${state} is corrupt`,
					);
				}

				const approval = event.payload.approval;
				const is_request = state === "requested";
				const is_decision = state === "approved" || state === "denied";
				const expected_updated_at = is_request
					? row.created_at
					: is_decision
						? row.decided_at
						: state === "executing"
							? row.execution_started_at
							: row.updated_at;
				const expected_causation_id =
					is_request || is_decision ? row.source_command_id : row.decision_message_id;
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
					approval.approval_id !== current.approval_id ||
					approval.state !== state ||
					approval.created_at !== current.created_at ||
					approval.expected_session_version !== current.expected_session_version ||
					approval.source_branch !== current.source_branch ||
					approval.source_command_id !== current.source_command_id ||
					approval.source_head !== current.source_head ||
					approval.target_branch !== current.target_branch ||
					approval.thread_id !== current.thread_id ||
					approval.updated_at !== expected_updated_at ||
					approval.workspace_id !== current.workspace_id ||
					event.causation_id !== expected_causation_id ||
					event.correlation_id !== expected_correlation_id ||
					event.origin !== "backend" ||
					event.sent_at !== expected_updated_at ||
					event.stream_id !== `thread:${current.thread_id}` ||
					event.thread_id !== current.thread_id
				) {
					return yield* invariant(
						`Checkout approval ${row.approval_id}:${state} is corrupt`,
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
						`Checkout approval ${row.approval_id}:${state} decision is corrupt`,
					);
				}

				return { approval, event };
			});

		const AppendEvent = (
			transaction: typeof database.client,
			approval: WorkspaceGitCheckoutApprovalValue,
			causation_id: string,
			correlation_id: string,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${approval.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const payload = {
					approval,
					type: "workspace.git.checkout.approval.updated",
				} as const;

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

				const [row] = yield* transaction
					.insert(JournalEvents)
					.values({
						causation_id,
						correlation_id,
						event_id,
						event_type: payload.type,
						idempotency_key: approval_event_key(approval.approval_id, approval.state),
						occurred_at: approval.updated_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: approval.thread_id,
					})
					.returning();

				if (!row) {
					return yield* invariant("Checkout approval event was not persisted");
				}

				return yield* DecodeEventRow(row);
			});

		const InsertCommand = (
			transaction: typeof database.client,
			command: typeof CommandMetadata.Type,
			thread_id: string,
			payload_type: string,
			payload_json: string,
		) =>
			Effect.gen(function* () {
				const accepted_at = yield* metadata.Now;

				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					agent_id: command.agent_id ?? null,
					causation_id: command.causation_id ?? null,
					message_id: command.message_id,
					origin: "frontend",
					payload_json,
					payload_type,
					raw_origin_json:
						command.raw_origin === undefined
							? null
							: JSON.stringify(command.raw_origin),
					run_id: command.run_id ?? null,
					schema_version: 1,
					sent_at: command.sent_at,
					status: "accepted",
					thread_id,
				});
			});

		const DecodeStoredCommandMetadata = (
			row: CommandRow,
			payload_type: string,
			label: string,
		) =>
			Effect.gen(function* () {
				if (
					row.schema_version !== 1 ||
					row.origin !== "frontend" ||
					row.payload_type !== payload_type ||
					row.status !== "accepted" ||
					row.assigned_run_id !== null
				) {
					return yield* invariant(`${label} command ${row.message_id} is corrupt`);
				}

				yield* Schema.decodeUnknownEffect(IsoDateTime)(row.accepted_at).pipe(
					Effect.mapError(() =>
						invariant(`${label} command ${row.message_id} has invalid acceptance time`),
					),
				);

				const raw_origin =
					row.raw_origin_json === null
						? undefined
						: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.raw_origin_json,
							).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(RawOrigin, {
										onExcessProperty: "error",
									}),
								),
								Effect.mapError(() =>
									invariant(
										`${label} command ${row.message_id} has invalid origin`,
									),
								),
							);
				const metadata_value = {
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
					...(row.causation_id === null ? {} : { causation_id: row.causation_id }),
					message_id: row.message_id,
					...(raw_origin === undefined ? {} : { raw_origin }),
					...(row.run_id === null ? {} : { run_id: row.run_id }),
					sent_at: row.sent_at,
				};
				const command = yield* Schema.decodeUnknownEffect(CommandMetadata, {
					onExcessProperty: "error",
				})(metadata_value).pipe(
					Effect.mapError(() =>
						invariant(`${label} command ${row.message_id} has invalid metadata`),
					),
				);

				return command;
			});

		const DecodeStoredRequestCommand = (row: CommandRow) =>
			Effect.gen(function* () {
				const command = yield* DecodeStoredCommandMetadata(
					row,
					"workspace.git.checkout.request",
					"Checkout request",
				);
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(StoredCheckoutRequestPayload, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(`Checkout request command ${row.message_id} has invalid payload`),
					),
				);

				return { command, payload };
			});

		const DecodeStoredDecisionCommand = (row: CommandRow) =>
			Effect.gen(function* () {
				const command = yield* DecodeStoredCommandMetadata(
					row,
					"workspace.git.checkout.approval.respond",
					"Checkout decision",
				);
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(StoredCheckoutDecisionPayload, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(
							`Checkout decision command ${row.message_id} has invalid payload`,
						),
					),
				);

				return { command, payload };
			});

		const ReadStoredRequestBinding = (
			transaction: typeof database.client,
			message_id: string,
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(WorkspaceGitCheckoutApprovals)
					.where(eq(WorkspaceGitCheckoutApprovals.source_command_id, message_id))
					.limit(1);
				const [command_row] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, message_id))
					.limit(1);

				if (!row) {
					if (command_row?.payload_type === "workspace.git.checkout.request") {
						return yield* invariant(
							`Checkout request command ${message_id} has no approval`,
						);
					}

					return Option.none<{
						readonly acceptance: WorkspaceGitCheckoutAcceptance;
						readonly command_row: CommandRow;
						readonly row: ApprovalRow;
					}>();
				}

				yield* EnsureLiveThread(transaction, row.thread_id);

				if (!command_row) {
					return yield* invariant(
						`Checkout approval ${row.approval_id} has no source command`,
					);
				}

				const stored = yield* DecodeStoredRequestCommand(command_row);
				const approval = yield* DecodeApproval(row);

				if (
					command_row.thread_id !== row.thread_id ||
					stored.command.sent_at !== row.created_at ||
					stored.payload.request_fingerprint !== row.request_fingerprint ||
					stored.payload.workspace_id !== row.workspace_id ||
					stored.payload.expected_session_version !== row.expected_session_version ||
					stored.payload.target_branch !== row.target_branch ||
					approval.source_command_id !== command_row.message_id
				) {
					return yield* invariant(
						`Checkout approval ${row.approval_id} has an invalid request binding`,
					);
				}

				const acceptance = yield* ReadAcceptance(transaction, row, "requested");

				return Option.some({
					acceptance: { ...acceptance, status: "duplicate" as const },
					command_row,
					row,
				});
			});

		const ReadReadyCleanSession = (
			transaction: typeof database.client,
			workspace_id: string,
			expected_version: number,
			expected_branch?: string,
			expected_head?: string,
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(WorkspaceGitSessions)
					.where(eq(WorkspaceGitSessions.workspace_id, workspace_id))
					.limit(1);

				if (!row) {
					return yield* new WorkspaceGitCheckoutConflict({ reason: "session_stale" });
				}

				const blockers = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.blockers_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(Schema.Array(WorkspaceGitSessionBlocker), {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant("Checkout source session blockers are corrupt"),
					),
				);
				const [changed] = yield* transaction
					.select({ path: WorkspaceGitChangedFiles.path })
					.from(WorkspaceGitChangedFiles)
					.where(eq(WorkspaceGitChangedFiles.workspace_id, workspace_id))
					.limit(1);

				if (
					row.version !== expected_version ||
					(expected_branch !== undefined && row.branch !== expected_branch) ||
					(expected_head !== undefined && row.head !== expected_head)
				) {
					return yield* new WorkspaceGitCheckoutConflict({ reason: "session_stale" });
				}

				if (
					row.state !== "ready" ||
					blockers.length > 0 ||
					row.branch === null ||
					row.head === null ||
					row.repository_root === null ||
					row.selected_worktree_path === null
				) {
					return yield* new WorkspaceGitCheckoutConflict({ reason: "session_stale" });
				}

				if (
					row.has_diff ||
					row.additions !== 0 ||
					row.deletions !== 0 ||
					row.files !== 0 ||
					changed
				) {
					return yield* new WorkspaceGitCheckoutConflict({ reason: "session_dirty" });
				}

				return {
					branch: yield* Schema.decodeUnknownEffect(GitBranchName)(row.branch).pipe(
						Effect.mapError(() => invariant("Checkout source branch is corrupt")),
					),
					head: yield* Schema.decodeUnknownEffect(GitObjectId)(row.head).pipe(
						Effect.mapError(() => invariant("Checkout source head is corrupt")),
					),
					repository_root: row.repository_root,
					selected_worktree_path: row.selected_worktree_path,
				};
			});

		const Request = (input: RequestWorkspaceGitCheckout) =>
			Schema.decodeUnknownEffect(RequestCheckout, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(
					() => new WorkspaceGitCheckoutConflict({ reason: "request_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const stored = yield* ReadStoredRequestBinding(
										transaction,
										decoded.source_command.message_id,
									);

									if (Option.isSome(stored)) {
										const binding = stored.value;
										const row = binding.row;

										if (
											row.approval_id !== decoded.approval_id ||
											row.request_fingerprint !==
												decoded.request_fingerprint ||
											row.thread_id !== decoded.thread_id ||
											row.workspace_id !== decoded.workspace_id ||
											row.expected_session_version !==
												decoded.expected_session_version ||
											row.target_branch !== decoded.target_branch ||
											!command_matches(
												binding.command_row,
												decoded.source_command,
												decoded.thread_id,
												"workspace.git.checkout.request",
												request_payload(decoded),
											)
										) {
											return yield* new WorkspaceGitCheckoutConflict({
												reason: "request_conflict",
											});
										}

										return binding.acceptance;
									}

									yield* EnsureLiveThread(transaction, decoded.thread_id);

									const [approval_collision] = yield* transaction
										.select({
											approval_id: WorkspaceGitCheckoutApprovals.approval_id,
										})
										.from(WorkspaceGitCheckoutApprovals)
										.where(
											eq(
												WorkspaceGitCheckoutApprovals.approval_id,
												decoded.approval_id,
											),
										)
										.limit(1);

									if (approval_collision) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "request_conflict",
										});
									}

									const [command_collision] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.source_command.message_id,
											),
										)
										.limit(1);

									if (command_collision) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "command_conflict",
										});
									}

									const source = yield* ReadReadyCleanSession(
										transaction,
										decoded.workspace_id,
										decoded.expected_session_version,
									);

									yield* InsertCommand(
										transaction,
										decoded.source_command,
										decoded.thread_id,
										"workspace.git.checkout.request",
										request_payload(decoded),
									);
									yield* transaction
										.insert(WorkspaceGitCheckoutApprovals)
										.values({
											approval_id: decoded.approval_id,
											created_at: decoded.source_command.sent_at,
											expected_session_version:
												decoded.expected_session_version,
											request_fingerprint: decoded.request_fingerprint,
											source_branch: source.branch,
											source_command_id: decoded.source_command.message_id,
											source_head: source.head,
											state: "requested",
											target_branch: decoded.target_branch,
											target_head: decoded.target_head,
											thread_id: decoded.thread_id,
											updated_at: decoded.source_command.sent_at,
											workspace_id: decoded.workspace_id,
										});

									const row = yield* ReadRow(transaction, decoded.approval_id);
									const approval = yield* DecodeApproval(row);
									const event = yield* AppendEvent(
										transaction,
										approval,
										decoded.source_command.message_id,
										decoded.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const Decide = (input: WorkspaceGitCheckoutDecision) =>
			Schema.decodeUnknownEffect(CheckoutDecision, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(
					() => new WorkspaceGitCheckoutConflict({ reason: "decision_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.thread_id !== decoded.thread_id) {
										return yield* new WorkspaceGitCheckoutUnavailable({
											reason: "missing",
										});
									}

									if (row.decision_message_id !== null) {
										if (
											row.decision_message_id !==
												decoded.decision_command.message_id ||
											row.approved !== decoded.approved ||
											row.decided_at !== decoded.decision_command.sent_at
										) {
											return yield* new WorkspaceGitCheckoutConflict({
												reason: "decision_conflict",
											});
										}

										const [command] = yield* transaction
											.select()
											.from(JournalCommands)
											.where(
												eq(
													JournalCommands.message_id,
													row.decision_message_id,
												),
											)
											.limit(1);

										if (!command) {
											return yield* invariant(
												`Checkout approval ${row.approval_id} has no decision command`,
											);
										}

										const stored_command =
											yield* DecodeStoredDecisionCommand(command);
										const expected_state = decoded.approved
											? "approved"
											: "denied";

										if (
											stored_command.command.sent_at !== row.decided_at ||
											stored_command.payload.approval_id !==
												row.approval_id ||
											stored_command.payload.approved !== row.approved ||
											!command_matches(
												command,
												decoded.decision_command,
												decoded.thread_id,
												"workspace.git.checkout.approval.respond",
												decision_payload(decoded),
											)
										) {
											return yield* new WorkspaceGitCheckoutConflict({
												reason: "decision_conflict",
											});
										}

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											expected_state,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "requested") {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "invalid_transition",
										});
									}

									const [collision] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.decision_command.message_id,
											),
										)
										.limit(1);

									if (collision) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "command_conflict",
										});
									}

									yield* InsertCommand(
										transaction,
										decoded.decision_command,
										decoded.thread_id,
										"workspace.git.checkout.approval.respond",
										decision_payload(decoded),
									);
									const target_state = decoded.approved ? "approved" : "denied";
									const [updated] = yield* transaction
										.update(WorkspaceGitCheckoutApprovals)
										.set({
											approved: decoded.approved,
											decided_at: decoded.decision_command.sent_at,
											decision_message_id:
												decoded.decision_command.message_id,
											state: target_state,
											updated_at: decoded.decision_command.sent_at,
										})
										.where(
											and(
												eq(
													WorkspaceGitCheckoutApprovals.approval_id,
													decoded.approval_id,
												),
												eq(
													WorkspaceGitCheckoutApprovals.state,
													"requested",
												),
											),
										)
										.returning();

									if (!updated) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "invalid_transition",
										});
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										row.source_command_id,
										decoded.decision_command.message_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const MarkExecuting = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitCheckoutUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === "executing") {
										const [claim] = yield* transaction
											.select()
											.from(WorkspaceGitCheckoutClaims)
											.where(
												eq(
													WorkspaceGitCheckoutClaims.approval_id,
													row.approval_id,
												),
											)
											.limit(1);

										if (
											!claim ||
											claim.workspace_id !== row.workspace_id ||
											claim.thread_id !== row.thread_id ||
											claim.claimed_at !== row.execution_started_at
										) {
											return yield* invariant(
												"Executing checkout claim is corrupt",
											);
										}

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											"executing",
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "approved") {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "invalid_transition",
										});
									}

									yield* ReadReadyCleanSession(
										transaction,
										row.workspace_id,
										row.expected_session_version,
										row.source_branch,
										row.source_head,
									);
									const [mutation] = yield* transaction
										.select({
											message_id: WorkspaceChangeOperations.message_id,
										})
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
												eq(
													WorkspaceMutationAuthorities.workspace_id,
													row.workspace_id,
												),
												notInArray(WorkspaceChangeOperations.lifecycle, [
													"committed",
													"rejected",
												]),
											),
										)
										.limit(1);

									if (mutation) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "workspace_mutation_active",
										});
									}

									const [git_mutation_claim] = yield* transaction
										.select({
											approval_id: WorkspaceGitMutationClaims.approval_id,
										})
										.from(WorkspaceGitMutationClaims)
										.where(
											eq(
												WorkspaceGitMutationClaims.workspace_id,
												row.workspace_id,
											),
										)
										.limit(1);

									if (git_mutation_claim) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "claim_conflict",
										});
									}

									const [existing_claim] = yield* transaction
										.select()
										.from(WorkspaceGitCheckoutClaims)
										.where(
											or(
												eq(
													WorkspaceGitCheckoutClaims.workspace_id,
													row.workspace_id,
												),
												eq(
													WorkspaceGitCheckoutClaims.approval_id,
													row.approval_id,
												),
											),
										)
										.limit(1);

									if (existing_claim) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "claim_conflict",
										});
									}

									const started_at = yield* metadata.Now;
									const [claim] = yield* transaction
										.insert(WorkspaceGitCheckoutClaims)
										.values({
											approval_id: row.approval_id,
											claimed_at: started_at,
											thread_id: row.thread_id,
											workspace_id: row.workspace_id,
										})
										.onConflictDoNothing()
										.returning();

									if (!claim) {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "claim_conflict",
										});
									}

									const [updated] = yield* transaction
										.update(WorkspaceGitCheckoutApprovals)
										.set({
											execution_started_at: started_at,
											state: "executing",
											updated_at: started_at,
										})
										.where(
											and(
												eq(
													WorkspaceGitCheckoutApprovals.approval_id,
													row.approval_id,
												),
												eq(WorkspaceGitCheckoutApprovals.state, "approved"),
											),
										)
										.returning();

									if (!updated) {
										return yield* invariant(
											"Checkout execution transition did not persist",
										);
									}

									if (updated.decision_message_id === null) {
										return yield* invariant(
											"Executing checkout has no decision command",
										);
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										updated.decision_message_id,
										updated.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const MarkTerminal = (approval_id: string, target: "applied" | "rejected" | "unknown") =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitCheckoutUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === target) {
										const [claim] = yield* transaction
											.select({
												approval_id: WorkspaceGitCheckoutClaims.approval_id,
											})
											.from(WorkspaceGitCheckoutClaims)
											.where(
												eq(
													WorkspaceGitCheckoutClaims.approval_id,
													row.approval_id,
												),
											)
											.limit(1);

										if (claim) {
											return yield* invariant(
												"Terminal checkout retained its workspace claim",
											);
										}

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											target,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state === "approved" && target === "rejected") {
										const [claim] = yield* transaction
											.select({
												approval_id: WorkspaceGitCheckoutClaims.approval_id,
											})
											.from(WorkspaceGitCheckoutClaims)
											.where(
												eq(
													WorkspaceGitCheckoutClaims.approval_id,
													row.approval_id,
												),
											)
											.limit(1);

										if (claim) {
											return yield* invariant(
												"Approved checkout unexpectedly retained a workspace claim",
											);
										}

										const updated_at = yield* metadata.Now;
										const [updated] = yield* transaction
											.update(WorkspaceGitCheckoutApprovals)
											.set({
												execution_started_at: updated_at,
												state: target,
												updated_at,
											})
											.where(
												and(
													eq(
														WorkspaceGitCheckoutApprovals.approval_id,
														row.approval_id,
													),
													eq(
														WorkspaceGitCheckoutApprovals.state,
														"approved",
													),
												),
											)
											.returning();

										if (!updated || updated.decision_message_id === null) {
											return yield* invariant(
												"Approved checkout rejection did not persist",
											);
										}

										const approval = yield* DecodeApproval(updated);
										const event = yield* AppendEvent(
											transaction,
											approval,
											updated.decision_message_id,
											updated.approval_id,
										);

										return { approval, event, status: "accepted" as const };
									}

									if (row.state !== "executing") {
										return yield* new WorkspaceGitCheckoutConflict({
											reason: "invalid_transition",
										});
									}

									const [claim] = yield* transaction
										.select()
										.from(WorkspaceGitCheckoutClaims)
										.where(
											eq(
												WorkspaceGitCheckoutClaims.approval_id,
												row.approval_id,
											),
										)
										.limit(1);

									if (
										!claim ||
										claim.workspace_id !== row.workspace_id ||
										claim.thread_id !== row.thread_id ||
										claim.claimed_at !== row.execution_started_at
									) {
										return yield* invariant(
											"Executing checkout claim is corrupt",
										);
									}

									const updated_at = yield* metadata.Now;
									const [updated] = yield* transaction
										.update(WorkspaceGitCheckoutApprovals)
										.set({ state: target, updated_at })
										.where(
											and(
												eq(
													WorkspaceGitCheckoutApprovals.approval_id,
													row.approval_id,
												),
												eq(
													WorkspaceGitCheckoutApprovals.state,
													"executing",
												),
											),
										)
										.returning();

									if (!updated) {
										return yield* invariant(
											"Checkout terminal transition did not persist",
										);
									}

									if (updated.decision_message_id === null) {
										return yield* invariant(
											"Terminal checkout has no decision command",
										);
									}

									yield* transaction
										.delete(WorkspaceGitCheckoutClaims)
										.where(
											and(
												eq(
													WorkspaceGitCheckoutClaims.workspace_id,
													row.workspace_id,
												),
												eq(
													WorkspaceGitCheckoutClaims.approval_id,
													row.approval_id,
												),
											),
										);

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										updated.decision_message_id,
										updated.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const Query = (query: typeof WorkspaceGitCheckoutApprovalQuery.Type) =>
			Schema.decodeUnknownEffect(WorkspaceGitCheckoutApprovalQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(() => new WorkspaceGitCheckoutUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, decoded.thread_id);

							const row = yield* ReadRow(transaction, decoded.approval_id);

							if (row.thread_id !== decoded.thread_id) {
								return yield* new WorkspaceGitCheckoutUnavailable({
									reason: "missing",
								});
							}

							const approval = yield* DecodeApproval(row);

							return yield* Schema.decodeUnknownEffect(
								WorkspaceGitCheckoutApprovalQueryResult,
								{ onExcessProperty: "error" },
							)({ approval }).pipe(
								Effect.mapError(() =>
									invariant("Checkout approval query is corrupt"),
								),
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadBySourceCommand = (message_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(message_id).pipe(
				Effect.mapError(() => invariant("Checkout request command id is invalid")),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						ReadStoredRequestBinding(transaction, decoded).pipe(
							Effect.map(Option.map((binding) => binding.acceptance)),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadExecution = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitCheckoutUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded);

							yield* EnsureLiveThread(transaction, row.thread_id);

							if (row.state !== "approved" && row.state !== "executing") {
								return yield* new WorkspaceGitCheckoutConflict({
									reason: "invalid_transition",
								});
							}

							const [session] = yield* transaction
								.select({
									repository_root: WorkspaceGitSessions.repository_root,
									selected_worktree_path:
										WorkspaceGitSessions.selected_worktree_path,
								})
								.from(WorkspaceGitSessions)
								.where(eq(WorkspaceGitSessions.workspace_id, row.workspace_id))
								.limit(1);

							if (
								!session ||
								session.repository_root === null ||
								session.selected_worktree_path === null
							) {
								return yield* invariant("Checkout execution paths are unavailable");
							}

							return {
								approval: yield* DecodeApproval(row),
								repository_root: session.repository_root,
								selected_worktree_path: session.selected_worktree_path,
								target_head: yield* Schema.decodeUnknownEffect(GitObjectId)(
									row.target_head,
								).pipe(
									Effect.mapError(() =>
										invariant("Checkout target head is corrupt"),
									),
								),
							};
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListState = (state: "approved" | "executing") =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const rows = yield* transaction
							.select({
								approval_id: WorkspaceGitCheckoutApprovals.approval_id,
								thread_id: WorkspaceGitCheckoutApprovals.thread_id,
							})
							.from(WorkspaceGitCheckoutApprovals)
							.where(eq(WorkspaceGitCheckoutApprovals.state, state))
							.orderBy(
								asc(WorkspaceGitCheckoutApprovals.created_at),
								asc(WorkspaceGitCheckoutApprovals.approval_id),
							);

						yield* Effect.forEach(
							rows,
							(row) => EnsureLiveThread(transaction, row.thread_id),
							{ discard: true },
						);

						return rows.map((row) => row.approval_id);
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		return {
			Decide,
			ListApproved: ListState("approved"),
			ListExecuting: ListState("executing"),
			MarkApplied: (approval_id) => MarkTerminal(approval_id, "applied"),
			MarkExecuting,
			MarkRejected: (approval_id) => MarkTerminal(approval_id, "rejected"),
			MarkUnknown: (approval_id) => MarkTerminal(approval_id, "unknown"),
			Query,
			ReadBySourceCommand,
			ReadExecution,
			Request,
		};
	}),
);
