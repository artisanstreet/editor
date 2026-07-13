import { and, asc, desc, eq, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	ContentIdentity,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceChange,
	WorkspaceChangeUpdatedEvent,
	WorkspacePath,
	type ContentIdentity as ContentIdentityValue,
	type RawOrigin as RawOriginValue,
	type WorkspaceChange as WorkspaceChangeValue,
	type WorkspaceChangeUpdatedEvent as WorkspaceChangeUpdatedEventValue,
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
	WorkspaceChanges,
	WorkspaceMutationPayloads,
} from "../persistence/schema";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
} from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const WorkspaceChangeLifecycle = Schema.Literals(["claimed", "applied", "committed", "rejected"]);
const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));
const JournalSequence = Schema.Int.check(Schema.isGreaterThanOrEqualTo(1));

const WorkspaceChangeOperationBase = {
	change_id: Identifier,
	evidence_recorded: Schema.Boolean,
	journal_sequence: Schema.optional(JournalSequence),
	lifecycle: WorkspaceChangeLifecycle,
	message_id: Identifier,
	request_fingerprint: RequestFingerprint,
	sent_at: IsoDateTime,
	thread_id: Identifier,
};

const WorkspaceChangeOperation = Schema.Union([
	Schema.Struct({
		...WorkspaceChangeOperationBase,
		action: Schema.Literal("replace"),
		agent_id: Identifier,
		expected_identity: ContentIdentity,
		path: WorkspacePath,
		raw_origin: Schema.optional(RawOrigin),
		result_identity: ContentIdentity,
		run_id: Identifier,
		workspace_id: Identifier,
	}),
	Schema.Struct({
		...WorkspaceChangeOperationBase,
		action: Schema.Literal("review"),
	}),
	Schema.Struct({
		...WorkspaceChangeOperationBase,
		action: Schema.Literal("rollback"),
		expected_identity: ContentIdentity,
	}),
]);

const WorkspaceChangeCommandIdentity = Schema.Struct({
	action: Schema.Literals(["replace", "review", "rollback"]),
	change_id: Identifier,
	request_fingerprint: RequestFingerprint,
	type: Schema.Literal("workspace.change.command"),
});

const WorkspaceChangeJournalEvent = Schema.Struct({
	causation_id: Identifier,
	correlation_id: Identifier,
	event_id: Identifier,
	journal_sequence: JournalSequence,
	occurred_at: IsoDateTime,
	payload: WorkspaceChangeUpdatedEvent,
	sequence: JournalSequence,
});

/** Identifies a replacement operation before filesystem mutation. */
export interface ClaimReplace {
	readonly _tag: "replace";
	readonly agent_id: string;
	readonly change_id: string;
	readonly expected_before: ContentIdentityValue;
	readonly intended_after: ContentIdentityValue;
	readonly message_id: string;
	readonly path: string;
	readonly raw_origin?: RawOriginValue;
	readonly request_fingerprint: string;
	readonly run_id: string;
	readonly sent_at: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

/** Identifies a user review operation before projection transition. */
export interface ClaimReview {
	readonly _tag: "review";
	readonly change_id: string;
	readonly message_id: string;
	readonly request_fingerprint: string;
	readonly sent_at: string;
	readonly thread_id: string;
}

/** Identifies a guarded rollback operation before filesystem mutation. */
export interface ClaimRollback {
	readonly _tag: "rollback";
	readonly change_id: string;
	readonly expected_after: ContentIdentityValue;
	readonly message_id: string;
	readonly request_fingerprint: string;
	readonly sent_at: string;
	readonly thread_id: string;
}

/** Represents the immutable identity of a workspace operation. */
export type WorkspaceChangeOperation = typeof WorkspaceChangeOperation.Type;

/** Returns the result of claiming a workspace operation. */
export type WorkspaceChangeClaim =
	| { readonly _tag: "claimed"; readonly operation: WorkspaceChangeOperation }
	| { readonly _tag: "incomplete_retry"; readonly operation: WorkspaceChangeOperation }
	| { readonly _tag: "rejected"; readonly operation: WorkspaceChangeOperation }
	| {
			readonly _tag: "duplicate";
			readonly event: WorkspaceChangeEvent;
			readonly operation: WorkspaceChangeOperation;
	  };

/** Carries one stored workspace-change journal event. */
export type WorkspaceChangeEvent = typeof WorkspaceChangeJournalEvent.Type;

/** Returns an accepted transition or its exact duplicate. */
export interface WorkspaceChangeCommit {
	readonly event: WorkspaceChangeEvent;
	readonly status: "accepted" | "duplicate";
}

/** Resolves a native changed observation against one exact durable operation. */
export type WorkspaceChangeReconciliation =
	| {
			readonly _tag: "applied";
			readonly operation: WorkspaceChangeOperation;
	  }
	| {
			readonly _tag: "committed";
			readonly event: WorkspaceChangeEvent;
			readonly operation: WorkspaceChangeOperation;
	  }
	| {
			readonly _tag: "rejected";
			readonly operation: WorkspaceChangeOperation;
	  }
	| {
			readonly _tag: "staged";
			readonly operation: WorkspaceChangeOperation;
	  };

/** Identifies where a changed file observation occurred in mutation execution. */
export interface ReconcileWorkspaceChange {
	readonly message_id: string;
	readonly observation: "native_changed" | "preflight_changed";
}

/** Reports an immutable collision between distinct replacement operations. */
export class WorkspaceChangeIdConflict extends Data.TaggedError("WorkspaceChangeIdConflict")<{
	readonly change_id: string;
}> {}

/** Reports an invalid operation lifecycle, action, or change transition. */
export class WorkspaceChangeTransitionError extends Data.TaggedError(
	"WorkspaceChangeTransitionError",
)<{ readonly message: string }> {}

/** Represents failures surfaced by the workspace change repository. */
export type WorkspaceChangeRepositoryError =
	| CommandIdConflict
	| JournalInvariantError
	| JournalStoreFailure
	| WorkspaceChangeIdConflict
	| WorkspaceChangeTransitionError;

/** Owns durable, source-free workspace change operations and projections. */
export class WorkspaceChangeRepository extends Context.Service<
	WorkspaceChangeRepository,
	{
		readonly ClaimReplace: (
			input: ClaimReplace,
		) => Effect.Effect<WorkspaceChangeClaim, WorkspaceChangeRepositoryError>;
		readonly ClaimReview: (
			input: ClaimReview,
		) => Effect.Effect<WorkspaceChangeClaim, WorkspaceChangeRepositoryError>;
		readonly ClaimRollback: (
			input: ClaimRollback,
		) => Effect.Effect<WorkspaceChangeClaim, WorkspaceChangeRepositoryError>;
		readonly MarkApplied: (
			input:
				| {
						readonly _tag: "replace";
						readonly message_id: string;
						readonly result_identity: ContentIdentityValue;
				  }
				| { readonly _tag: "rollback"; readonly message_id: string },
		) => Effect.Effect<WorkspaceChangeOperation, WorkspaceChangeRepositoryError>;
		readonly RejectChanged: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeOperation, WorkspaceChangeRepositoryError>;
		readonly ReconcileChanged: (
			input: ReconcileWorkspaceChange,
		) => Effect.Effect<WorkspaceChangeReconciliation, WorkspaceChangeRepositoryError>;
		readonly CommitRecorded: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceChangeRepositoryError>;
		readonly CommitReviewed: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceChangeRepositoryError>;
		readonly CommitRolledBack: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceChangeRepositoryError>;
		readonly MarkEvidenceRecorded: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeOperation, WorkspaceChangeRepositoryError>;
		readonly ReadChange: (
			change_id: string,
		) => Effect.Effect<Option.Option<WorkspaceChangeValue>, WorkspaceChangeRepositoryError>;
		readonly ReadOperation: (
			message_id: string,
		) => Effect.Effect<Option.Option<WorkspaceChangeOperation>, WorkspaceChangeRepositoryError>;
		readonly List: (
			thread_id: string,
			workspace_id?: string,
		) => Effect.Effect<
			{
				readonly changes: ReadonlyArray<WorkspaceChangeValue>;
				readonly journal_sequence: number;
			},
			WorkspaceChangeRepositoryError
		>;
	}
>()("Artisan/WorkspaceChangeRepository") {}

const DecodeJson = Schema.decodeUnknownEffect(Schema.UnknownFromJsonString);

function DecodeStoredRawOrigin(raw_origin_json: string | null, message: string) {
	if (raw_origin_json === null) {
		return Effect.succeed<RawOriginValue | undefined>(undefined);
	}

	return DecodeJson(raw_origin_json).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(RawOrigin)),
		Effect.map((raw_origin): RawOriginValue | undefined => raw_origin),
		Effect.mapError(() => new JournalInvariantError({ message })),
	);
}

function normalize_error(error: unknown): WorkspaceChangeRepositoryError {
	if (
		error instanceof CommandIdConflict ||
		error instanceof JournalInvariantError ||
		error instanceof WorkspaceChangeIdConflict ||
		error instanceof WorkspaceChangeTransitionError
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function optional_fields<T extends Readonly<Record<string, unknown>>>(input: T) {
	return Object.fromEntries(Object.entries(input).filter(([, value]) => value != null));
}

function command_payload_json(operation: WorkspaceChangeOperation) {
	return JSON.stringify({
		action: operation.action,
		change_id: operation.change_id,
		request_fingerprint: operation.request_fingerprint,
		type: "workspace.change.command",
	});
}

function identities_match(left: ContentIdentityValue, right: ContentIdentityValue) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

function raw_origins_match(left: RawOriginValue | undefined, right: RawOriginValue | undefined) {
	return (
		(left === undefined && right === undefined) ||
		(left !== undefined &&
			right !== undefined &&
			left.provider === right.provider &&
			left.reference === right.reference)
	);
}

function operation_state_is_valid(operation: WorkspaceChangeOperation) {
	const is_committed = operation.lifecycle === "committed";
	const has_journal_sequence = operation.journal_sequence !== undefined;

	if (operation.lifecycle === "rejected") {
		return (
			operation.action !== "review" && !operation.evidence_recorded && !has_journal_sequence
		);
	}

	return (
		is_committed === has_journal_sequence &&
		!(operation.action === "review" && operation.lifecycle === "applied") &&
		(!operation.evidence_recorded ||
			(operation.action !== "review" && operation.lifecycle === "committed"))
	);
}

function change_state_is_valid(change: WorkspaceChangeValue) {
	if (change.review_state === "needs_review") {
		return (
			change.reviewed_at === undefined &&
			change.rollback_state === "available" &&
			change.rolled_back_at === undefined &&
			change.version === 1
		);
	}

	if (change.review_state === "reviewed") {
		return (
			change.reviewed_at !== undefined &&
			change.rollback_state === "available" &&
			change.rolled_back_at === undefined &&
			change.version === 2
		);
	}

	return (
		change.rollback_state === "consumed" &&
		change.rolled_back_at !== undefined &&
		change.version === (change.reviewed_at === undefined ? 2 : 3)
	);
}

function event_action_for(operation: WorkspaceChangeOperation) {
	return operation.action === "replace"
		? "recorded"
		: operation.action === "review"
			? "reviewed"
			: "rolled_back";
}

function change_identity_matches(left: WorkspaceChangeValue, right: WorkspaceChangeValue) {
	return (
		left.agent_id === right.agent_id &&
		identities_match(left.after_identity, right.after_identity) &&
		identities_match(left.before_identity, right.before_identity) &&
		left.change_id === right.change_id &&
		left.created_at === right.created_at &&
		left.path === right.path &&
		raw_origins_match(left.raw_origin, right.raw_origin) &&
		left.run_id === right.run_id &&
		left.source_command_id === right.source_command_id &&
		left.thread_id === right.thread_id &&
		left.workspace_id === right.workspace_id
	);
}

function event_matches_operation(event: WorkspaceChangeEvent, operation: WorkspaceChangeOperation) {
	const change = event.payload.change;

	if (
		event.causation_id !== operation.message_id ||
		event.correlation_id !== operation.message_id ||
		event.journal_sequence !== operation.journal_sequence ||
		event.payload.action !== event_action_for(operation) ||
		change.change_id !== operation.change_id ||
		change.thread_id !== operation.thread_id
	) {
		return false;
	}

	if (
		(operation.action === "replace" &&
			(event.occurred_at !== change.created_at || event.occurred_at !== change.updated_at)) ||
		(operation.action === "review" &&
			(event.occurred_at !== change.reviewed_at ||
				event.occurred_at !== change.updated_at)) ||
		(operation.action === "rollback" &&
			(event.occurred_at !== change.rolled_back_at ||
				event.occurred_at !== change.updated_at))
	) {
		return false;
	}

	if (operation.action !== "replace") {
		return true;
	}

	return (
		change.agent_id === operation.agent_id &&
		identities_match(change.after_identity, operation.result_identity) &&
		identities_match(change.before_identity, operation.expected_identity) &&
		change.path === operation.path &&
		raw_origins_match(change.raw_origin, operation.raw_origin) &&
		change.run_id === operation.run_id &&
		change.source_command_id === operation.message_id &&
		change.workspace_id === operation.workspace_id
	);
}

function operation_from_claim(
	input: ClaimReplace | ClaimReview | ClaimRollback,
): WorkspaceChangeOperation {
	const base = {
		change_id: input.change_id,
		evidence_recorded: false,
		lifecycle: "claimed" as const,
		message_id: input.message_id,
		request_fingerprint: input.request_fingerprint,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
	};

	if (input._tag === "replace") {
		return {
			...base,
			action: "replace",
			agent_id: input.agent_id,
			expected_identity: input.expected_before,
			path: input.path,
			...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
			result_identity: input.intended_after,
			run_id: input.run_id,
			workspace_id: input.workspace_id,
		};
	}

	if (input._tag === "rollback") {
		return { ...base, action: "rollback", expected_identity: input.expected_after };
	}

	return { ...base, action: "review" };
}

function immutable_operations_match(
	stored: WorkspaceChangeOperation,
	claimed: WorkspaceChangeOperation,
) {
	if (
		stored.action !== claimed.action ||
		stored.change_id !== claimed.change_id ||
		stored.message_id !== claimed.message_id ||
		stored.request_fingerprint !== claimed.request_fingerprint ||
		stored.sent_at !== claimed.sent_at ||
		stored.thread_id !== claimed.thread_id
	) {
		return false;
	}

	if (stored.action === "review" && claimed.action === "review") {
		return true;
	}

	if (stored.action === "rollback" && claimed.action === "rollback") {
		return identities_match(stored.expected_identity, claimed.expected_identity);
	}

	if (stored.action !== "replace" || claimed.action !== "replace") {
		return false;
	}

	return (
		stored.agent_id === claimed.agent_id &&
		identities_match(stored.expected_identity, claimed.expected_identity) &&
		identities_match(stored.result_identity, claimed.result_identity) &&
		stored.path === claimed.path &&
		raw_origins_match(stored.raw_origin, claimed.raw_origin) &&
		stored.run_id === claimed.run_id &&
		stored.workspace_id === claimed.workspace_id
	);
}

function DecodeOperation(row: typeof WorkspaceChangeOperations.$inferSelect) {
	return Effect.all({
		expected_identity:
			row.expected_identity_json === null
				? Effect.succeed(undefined)
				: DecodeJson(row.expected_identity_json).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
					),
		result_identity:
			row.result_identity_json === null
				? Effect.succeed(undefined)
				: DecodeJson(row.result_identity_json).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
					),
		raw_origin:
			row.raw_origin_json === null
				? Effect.succeed(undefined)
				: DecodeJson(row.raw_origin_json).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(RawOrigin)),
					),
	}).pipe(
		Effect.flatMap((identities) =>
			Schema.decodeUnknownEffect(WorkspaceChangeOperation, { onExcessProperty: "error" })(
				optional_fields({
					action: row.action,
					agent_id: row.agent_id,
					change_id: row.change_id,
					evidence_recorded: row.evidence_recorded,
					expected_identity: identities.expected_identity,
					journal_sequence: row.journal_sequence,
					lifecycle: row.lifecycle,
					message_id: row.message_id,
					path: row.path,
					raw_origin: identities.raw_origin,
					request_fingerprint: row.request_fingerprint,
					result_identity: identities.result_identity,
					run_id: row.run_id,
					sent_at: row.sent_at,
					thread_id: row.thread_id,
					workspace_id: row.workspace_id,
				}),
			),
		),
		Effect.flatMap((operation) =>
			operation_state_is_valid(operation)
				? Effect.succeed(operation)
				: Effect.fail(new Error("Invalid workspace operation lifecycle")),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored workspace operation ${row.message_id} is invalid`,
				}),
		),
	);
}

function DecodeChange(row: typeof WorkspaceChanges.$inferSelect) {
	return Effect.all({
		after_identity: DecodeJson(row.after_identity_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
		),
		before_identity: DecodeJson(row.before_identity_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
		),
		raw_origin:
			row.raw_origin_json === null
				? Effect.succeed(undefined)
				: DecodeJson(row.raw_origin_json).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(RawOrigin)),
					),
	}).pipe(
		Effect.flatMap((json) =>
			Schema.decodeUnknownEffect(WorkspaceChange, { onExcessProperty: "error" })(
				optional_fields({
					after_identity: json.after_identity,
					agent_id: row.agent_id,
					before_identity: json.before_identity,
					change_id: row.change_id,
					created_at: row.created_at,
					path: row.path,
					raw_origin: json.raw_origin,
					review_state: row.review_state,
					reviewed_at: row.reviewed_at,
					rollback_state: row.rollback_state,
					rolled_back_at: row.rolled_back_at,
					run_id: row.run_id,
					source_command_id: row.source_command_id,
					thread_id: row.thread_id,
					updated_at: row.updated_at,
					version: row.version,
					workspace_id: row.workspace_id,
				}),
			),
		),
		Effect.flatMap((change) =>
			change_state_is_valid(change)
				? Effect.succeed(change)
				: Effect.fail(new Error("Invalid workspace change lifecycle")),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored workspace change ${row.change_id} is invalid`,
				}),
		),
	);
}

function DecodeEvent(row: typeof JournalEvents.$inferSelect) {
	return DecodeJson(row.payload_json).pipe(
		Effect.flatMap((payload) =>
			Schema.decodeUnknownEffect(WorkspaceChangeJournalEvent, { onExcessProperty: "error" })({
				causation_id: row.causation_id,
				correlation_id: row.correlation_id,
				event_id: row.event_id,
				journal_sequence: row.sequence,
				occurred_at: row.occurred_at,
				payload,
				sequence: row.stream_sequence,
			}),
		),
		Effect.flatMap((event) =>
			change_state_is_valid(event.payload.change)
				? Effect.succeed(event)
				: Effect.fail(new Error("Invalid workspace event projection")),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored workspace event ${row.event_id} is invalid`,
				}),
		),
	);
}

/** Supplies the SQLite-backed workspace change repository. */
export const WorkspaceChangeRepositoryLive = Layer.effect(
	WorkspaceChangeRepository,
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
					return yield* new WorkspaceChangeTransitionError({
						message: `Thread ${thread_id} is unavailable for workspace changes`,
					});
				}
			});

		const ReadTransitionChange = (
			transaction: typeof database.client,
			operation: WorkspaceChangeOperation,
		) =>
			Effect.gen(function* () {
				const [stored] = yield* transaction
					.select()
					.from(WorkspaceChanges)
					.where(
						and(
							eq(WorkspaceChanges.change_id, operation.change_id),
							eq(WorkspaceChanges.thread_id, operation.thread_id),
						),
					)
					.limit(1);

				if (!stored) {
					return yield* new WorkspaceChangeTransitionError({
						message: `Workspace change ${operation.change_id} is missing for this thread`,
					});
				}

				return { change: yield* DecodeChange(stored), row: stored };
			});

		const ValidateTransition = (
			transaction: typeof database.client,
			operation: WorkspaceChangeOperation,
		) =>
			Effect.gen(function* () {
				if (operation.action === "replace") {
					return undefined;
				}

				const stored = yield* ReadTransitionChange(transaction, operation);

				if (
					operation.action === "review" &&
					(stored.change.review_state !== "needs_review" ||
						stored.change.rollback_state !== "available")
				) {
					return yield* new WorkspaceChangeTransitionError({
						message: `Workspace change ${operation.change_id} cannot be reviewed`,
					});
				}

				if (
					operation.action === "rollback" &&
					((stored.change.review_state !== "needs_review" &&
						stored.change.review_state !== "reviewed") ||
						stored.change.rollback_state !== "available" ||
						!identities_match(
							stored.change.after_identity,
							operation.expected_identity,
						))
				) {
					return yield* new WorkspaceChangeTransitionError({
						message: `Workspace change ${operation.change_id} cannot be rolled back`,
					});
				}

				return stored;
			});

		const ValidateRejectedState = (
			transaction: typeof database.client,
			operation: WorkspaceChangeOperation,
		) =>
			Effect.gen(function* () {
				const [command] = yield* transaction
					.select({ message_id: JournalCommands.message_id })
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, operation.message_id))
					.limit(1);
				const [event] = yield* transaction
					.select({ event_id: JournalEvents.event_id })
					.from(JournalEvents)
					.where(eq(JournalEvents.correlation_id, operation.message_id))
					.limit(1);

				if (command || event) {
					return yield* new JournalInvariantError({
						message: `Rejected workspace operation ${operation.message_id} has journal state`,
					});
				}

				if (operation.action === "replace") {
					const [projection] = yield* transaction
						.select({ change_id: WorkspaceChanges.change_id })
						.from(WorkspaceChanges)
						.where(
							or(
								eq(WorkspaceChanges.change_id, operation.change_id),
								eq(WorkspaceChanges.source_command_id, operation.message_id),
							),
						)
						.limit(1);

					if (projection) {
						return yield* new JournalInvariantError({
							message: `Rejected workspace operation ${operation.message_id} has projection state`,
						});
					}

					return;
				}

				if (operation.action === "rollback") {
					yield* ValidateTransition(transaction, operation);

					return;
				}

				return yield* new JournalInvariantError({
					message: `Rejected workspace operation ${operation.message_id} has invalid action`,
				});
			});

		const HasAvailablePayload = (
			transaction: typeof database.client,
			operation: WorkspaceChangeOperation,
		) =>
			Effect.gen(function* () {
				const [payload] = yield* transaction
					.select({
						expected_byte_count: WorkspaceMutationPayloads.expected_byte_count,
						expected_hash: WorkspaceMutationPayloads.expected_hash,
						replacement_byte_count: WorkspaceMutationPayloads.replacement_byte_count,
						replacement_hash: WorkspaceMutationPayloads.replacement_hash,
						state: WorkspaceMutationPayloads.state,
						thread_id: WorkspaceMutationPayloads.thread_id,
					})
					.from(WorkspaceMutationPayloads)
					.where(eq(WorkspaceMutationPayloads.message_id, operation.message_id))
					.limit(1);

				if (!payload) {
					return false;
				}

				if (
					payload.state !== "available" ||
					payload.thread_id !== operation.thread_id ||
					typeof payload.expected_byte_count !== "number" ||
					typeof payload.expected_hash !== "string" ||
					typeof payload.replacement_byte_count !== "number" ||
					typeof payload.replacement_hash !== "string"
				) {
					return yield* new JournalInvariantError({
						message: `Workspace operation ${operation.message_id} has invalid staged payload state`,
					});
				}

				if (operation.action === "review") {
					return yield* new JournalInvariantError({
						message: `Workspace review ${operation.message_id} owns mutation payload state`,
					});
				}

				const expected_identity = {
					algorithm: "sha256" as const,
					byte_count: payload.expected_byte_count,
					content_hash: payload.expected_hash,
				};
				const replacement_identity = {
					algorithm: "sha256" as const,
					byte_count: payload.replacement_byte_count,
					content_hash: payload.replacement_hash,
				};
				let intended_replacement: ContentIdentityValue;

				if (operation.action === "replace") {
					intended_replacement = operation.result_identity;
				} else {
					const transition = yield* ValidateTransition(transaction, operation);

					if (!transition) {
						return yield* new JournalInvariantError({
							message: `Workspace rollback ${operation.message_id} has no source transition`,
						});
					}

					intended_replacement = transition.change.before_identity;
				}

				if (
					!identities_match(expected_identity, operation.expected_identity) ||
					!identities_match(replacement_identity, intended_replacement)
				) {
					return yield* new JournalInvariantError({
						message: `Workspace operation ${operation.message_id} has mismatched staged payload state`,
					});
				}

				return true;
			});

		const ReadOperation = (message_id: string) =>
			database.client
				.select()
				.from(WorkspaceChangeOperations)
				.where(eq(WorkspaceChangeOperations.message_id, message_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeOperation(row).pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none<WorkspaceChangeOperation>()),
					),
					Effect.mapError(normalize_error),
				);

		const ReadChange = (change_id: string) =>
			database.client
				.select()
				.from(WorkspaceChanges)
				.where(eq(WorkspaceChanges.change_id, change_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeChange(row).pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none<WorkspaceChangeValue>()),
					),
					Effect.mapError(normalize_error),
				);

		const ReadDuplicate = (
			transaction: typeof database.client,
			operation: WorkspaceChangeOperation,
		) =>
			Effect.gen(function* () {
				const [command] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, operation.message_id))
					.limit(1);

				if (!command) {
					return yield* new JournalInvariantError({
						message: `Workspace command ${operation.message_id} is invalid`,
					});
				}

				const command_raw_origin = yield* DecodeStoredRawOrigin(
					command.raw_origin_json,
					`Workspace command ${operation.message_id} has invalid attribution`,
				);
				const expected_agent_id =
					operation.action === "replace" ? operation.agent_id : null;
				const expected_raw_origin =
					operation.action === "replace" ? operation.raw_origin : undefined;
				const expected_run_id = operation.action === "replace" ? operation.run_id : null;

				if (
					command.agent_id !== expected_agent_id ||
					command.assigned_run_id !== null ||
					command.causation_id !== null ||
					command.origin !== "frontend" ||
					command.payload_type !== "workspace.change.command" ||
					!raw_origins_match(command_raw_origin, expected_raw_origin) ||
					command.run_id !== expected_run_id ||
					command.schema_version !== 1 ||
					command.sent_at !== operation.sent_at ||
					command.status !== "accepted" ||
					command.thread_id !== operation.thread_id
				) {
					return yield* new JournalInvariantError({
						message: `Workspace command ${operation.message_id} has invalid journal ownership`,
					});
				}

				const identity = yield* DecodeJson(command.payload_json).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(WorkspaceChangeCommandIdentity, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(
						() =>
							new JournalInvariantError({
								message: `Workspace command ${operation.message_id} has invalid identity`,
							}),
					),
				);

				if (
					identity.action !== operation.action ||
					identity.change_id !== operation.change_id ||
					identity.request_fingerprint !== operation.request_fingerprint
				) {
					return yield* new JournalInvariantError({
						message: `Workspace command ${operation.message_id} identity does not match operation`,
					});
				}

				const rows = yield* transaction
					.select()
					.from(JournalEvents)
					.where(eq(JournalEvents.correlation_id, operation.message_id))
					.orderBy(asc(JournalEvents.sequence));

				if (rows.length !== 1) {
					return yield* new JournalInvariantError({
						message: `Workspace operation ${operation.message_id} must have exactly one event`,
					});
				}

				const row = rows[0]!;
				const event_raw_origin = yield* DecodeStoredRawOrigin(
					row.raw_origin_json,
					`Workspace event ${row.event_id} has invalid attribution`,
				);
				const expected_event_agent_id =
					operation.action === "replace" ? operation.agent_id : null;
				const expected_event_raw_origin =
					operation.action === "replace" ? operation.raw_origin : undefined;
				const expected_event_run_id =
					operation.action === "replace" ? operation.run_id : null;

				if (
					row.agent_id !== expected_event_agent_id ||
					row.causation_id !== operation.message_id ||
					row.event_type !== "workspace.change.updated" ||
					row.origin !== "backend" ||
					!raw_origins_match(event_raw_origin, expected_event_raw_origin) ||
					row.run_id !== expected_event_run_id ||
					row.schema_version !== 1 ||
					row.stream_id !== `thread:${operation.thread_id}` ||
					row.thread_id !== operation.thread_id
				) {
					return yield* new JournalInvariantError({
						message: `Workspace event ${row.event_id} has invalid journal ownership`,
					});
				}

				const event = yield* DecodeEvent(row);

				if (!event_matches_operation(event, operation)) {
					return yield* new JournalInvariantError({
						message: `Workspace event ${row.event_id} does not match its operation`,
					});
				}

				const projection = yield* ReadTransitionChange(transaction, operation);

				if (!change_identity_matches(event.payload.change, projection.change)) {
					return yield* new JournalInvariantError({
						message: `Workspace event ${row.event_id} does not match its projection`,
					});
				}

				return event;
			});

		const Claim = (input: ClaimReplace | ClaimReview | ClaimRollback) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const claimed = operation_from_claim(input);
						const decoded_claim = yield* Schema.decodeUnknownEffect(
							WorkspaceChangeOperation,
							{ onExcessProperty: "error" },
						)(claimed).pipe(
							Effect.mapError(
								() =>
									new JournalInvariantError({
										message: `Workspace claim ${input.message_id} is invalid`,
									}),
							),
						);

						yield* EnsureLiveThread(transaction, decoded_claim.thread_id);

						const [existing] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.message_id, input.message_id))
							.limit(1);

						if (existing) {
							const operation = yield* DecodeOperation(existing);

							if (!immutable_operations_match(operation, decoded_claim)) {
								return yield* new CommandIdConflict({
									message_id: input.message_id,
								});
							}

							if (operation.lifecycle === "committed") {
								return {
									_tag: "duplicate" as const,
									event: yield* ReadDuplicate(transaction, operation),
									operation,
								};
							}

							if (operation.lifecycle === "rejected") {
								yield* ValidateRejectedState(transaction, operation);

								return { _tag: "rejected" as const, operation };
							}

							const [command] = yield* transaction
								.select({ message_id: JournalCommands.message_id })
								.from(JournalCommands)
								.where(eq(JournalCommands.message_id, operation.message_id))
								.limit(1);
							const [event] = yield* transaction
								.select({ event_id: JournalEvents.event_id })
								.from(JournalEvents)
								.where(eq(JournalEvents.correlation_id, operation.message_id))
								.limit(1);

							if (command || event) {
								return yield* new JournalInvariantError({
									message: `Incomplete workspace operation ${operation.message_id} has committed journal state`,
								});
							}

							yield* ValidateTransition(transaction, operation);

							return { _tag: "incomplete_retry" as const, operation };
						}

						const [journal_command] = yield* transaction
							.select({ message_id: JournalCommands.message_id })
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, decoded_claim.message_id))
							.limit(1);

						if (journal_command) {
							return yield* new CommandIdConflict({ message_id: input.message_id });
						}

						const [orphaned_event] = yield* transaction
							.select({ event_id: JournalEvents.event_id })
							.from(JournalEvents)
							.where(eq(JournalEvents.correlation_id, decoded_claim.message_id))
							.limit(1);

						if (orphaned_event) {
							return yield* new JournalInvariantError({
								message: `Workspace message ${input.message_id} already owns an orphaned event`,
							});
						}

						const [claimed_change] = yield* transaction
							.select({ message_id: WorkspaceChangeOperations.message_id })
							.from(WorkspaceChangeOperations)
							.where(
								and(
									eq(WorkspaceChangeOperations.change_id, input.change_id),
									eq(WorkspaceChangeOperations.action, decoded_claim.action),
								),
							)
							.limit(1);

						if (claimed_change) {
							return yield* new WorkspaceChangeIdConflict({
								change_id: input.change_id,
							});
						}

						yield* ValidateTransition(transaction, decoded_claim);

						const now = yield* metadata.Now;
						const row = {
							action: decoded_claim.action,
							agent_id:
								decoded_claim.action === "replace" ? decoded_claim.agent_id : null,
							change_id: decoded_claim.change_id,
							created_at: now,
							evidence_recorded: false,
							expected_identity_json:
								decoded_claim.action === "review"
									? null
									: JSON.stringify(decoded_claim.expected_identity),
							journal_sequence: null,
							lifecycle: "claimed",
							message_id: decoded_claim.message_id,
							path: decoded_claim.action === "replace" ? decoded_claim.path : null,
							raw_origin_json:
								decoded_claim.action === "replace" &&
								decoded_claim.raw_origin !== undefined
									? JSON.stringify(decoded_claim.raw_origin)
									: null,
							request_fingerprint: decoded_claim.request_fingerprint,
							result_identity_json:
								decoded_claim.action === "replace"
									? JSON.stringify(decoded_claim.result_identity)
									: null,
							run_id:
								decoded_claim.action === "replace" ? decoded_claim.run_id : null,
							sent_at: decoded_claim.sent_at,
							thread_id: decoded_claim.thread_id,
							updated_at: now,
							workspace_id:
								decoded_claim.action === "replace"
									? decoded_claim.workspace_id
									: null,
						};

						yield* transaction.insert(WorkspaceChangeOperations).values(row);

						return { _tag: "claimed" as const, operation: yield* DecodeOperation(row) };
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		const MarkApplied = (
			input:
				| {
						readonly _tag: "replace";
						readonly message_id: string;
						readonly result_identity: ContentIdentityValue;
				  }
				| { readonly _tag: "rollback"; readonly message_id: string },
		) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [row] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.message_id, input.message_id))
							.limit(1);

						if (!row)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${input.message_id} is missing`,
							});

						const operation = yield* DecodeOperation(row);

						yield* EnsureLiveThread(transaction, operation.thread_id);

						if (operation.action !== input._tag)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${input.message_id} cannot be applied`,
							});

						if (
							input._tag === "replace" &&
							(operation.action !== "replace" ||
								!identities_match(operation.result_identity, input.result_identity))
						) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${input.message_id} did not produce its intended result`,
							});
						}

						if (operation.action === "rollback") {
							yield* ValidateTransition(transaction, operation);
						}

						if (
							operation.lifecycle === "committed" ||
							operation.lifecycle === "applied"
						)
							return operation;

						const updated_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(WorkspaceChangeOperations)
							.set({ lifecycle: "applied", updated_at })
							.where(
								and(
									eq(WorkspaceChangeOperations.message_id, input.message_id),
									eq(WorkspaceChangeOperations.lifecycle, "claimed"),
								),
							)
							.returning({ message_id: WorkspaceChangeOperations.message_id });

						if (!updated) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${input.message_id} changed before it was applied`,
							});
						}

						return yield* DecodeOperation({ ...row, lifecycle: "applied", updated_at });
					}),
				)
				.pipe(RetrySqliteWrite, Effect.mapError(normalize_error));

		const RejectChanged = (message_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [row] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.message_id, message_id))
							.limit(1);

						if (!row)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} is missing`,
							});

						const operation = yield* DecodeOperation(row);

						yield* EnsureLiveThread(transaction, operation.thread_id);

						if (operation.action === "review")
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} cannot be rejected`,
							});

						if (operation.lifecycle === "rejected") {
							yield* ValidateRejectedState(transaction, operation);

							return operation;
						}

						if (operation.lifecycle !== "claimed")
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} cannot be rejected`,
							});

						yield* ValidateRejectedState(transaction, operation);

						const updated_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(WorkspaceChangeOperations)
							.set({ lifecycle: "rejected", updated_at })
							.where(
								and(
									eq(WorkspaceChangeOperations.message_id, message_id),
									eq(WorkspaceChangeOperations.lifecycle, "claimed"),
								),
							)
							.returning({ message_id: WorkspaceChangeOperations.message_id });

						if (!updated) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} changed before it was rejected`,
							});
						}

						return yield* DecodeOperation({
							...row,
							lifecycle: "rejected",
							updated_at,
						});
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		const ReconcileChanged = (input: ReconcileWorkspaceChange) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const message_id = input.message_id;
						const [row] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.message_id, message_id))
							.limit(1);

						if (!row) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} is missing`,
							});
						}

						const operation = yield* DecodeOperation(row);

						yield* EnsureLiveThread(transaction, operation.thread_id);

						if (operation.action === "review") {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} cannot reconcile a file change`,
							});
						}

						if (operation.lifecycle === "committed") {
							return {
								_tag: "committed" as const,
								event: yield* ReadDuplicate(transaction, operation),
								operation,
							};
						}

						if (operation.lifecycle === "applied") {
							return { _tag: "applied" as const, operation };
						}

						if (operation.lifecycle === "rejected") {
							yield* ValidateRejectedState(transaction, operation);

							return { _tag: "rejected" as const, operation };
						}

						if (
							input.observation === "preflight_changed" &&
							(yield* HasAvailablePayload(transaction, operation))
						) {
							return { _tag: "staged" as const, operation };
						}

						yield* ValidateRejectedState(transaction, operation);

						const updated_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(WorkspaceChangeOperations)
							.set({ lifecycle: "rejected", updated_at })
							.where(
								and(
									eq(WorkspaceChangeOperations.message_id, message_id),
									eq(WorkspaceChangeOperations.lifecycle, "claimed"),
								),
							)
							.returning({ message_id: WorkspaceChangeOperations.message_id });

						if (!updated) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} changed before reconciliation`,
							});
						}

						return {
							_tag: "rejected" as const,
							operation: yield* DecodeOperation({
								...row,
								lifecycle: "rejected",
								updated_at,
							}),
						};
					}),
				)
				.pipe(RetrySqliteWrite, Effect.mapError(normalize_error));

		const AppendEvent = (
			transaction: typeof database.client,
			operation: WorkspaceChangeOperation,
			action: WorkspaceChangeUpdatedEventValue["action"],
			change: WorkspaceChangeValue,
			occurred_at: string,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${operation.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const payload = { action, change, type: "workspace.change.updated" } as const;

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
						agent_id:
							action === "recorded" && operation.action === "replace"
								? operation.agent_id
								: null,
						causation_id: operation.message_id,
						correlation_id: operation.message_id,
						event_id,
						event_type: payload.type,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						raw_origin_json:
							action === "recorded" &&
							operation.action === "replace" &&
							operation.raw_origin !== undefined
								? JSON.stringify(operation.raw_origin)
								: null,
						run_id:
							action === "recorded" && operation.action === "replace"
								? operation.run_id
								: null,
						schema_version: 1,
						stream_id,
						stream_sequence: sequence,
						thread_id: operation.thread_id,
					})
					.returning();

				if (!row)
					return yield* new JournalInvariantError({
						message: "Workspace event was not persisted",
					});

				return yield* DecodeEvent(row);
			});

		const Commit = (message_id: string, action: "recorded" | "reviewed" | "rolled_back") =>
			Effect.gen(function* () {
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [row] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.message_id, message_id))
							.limit(1);

						if (!row)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} is missing`,
							});

						const operation = yield* DecodeOperation(row);
						const expected_action =
							action === "recorded"
								? "replace"
								: action === "reviewed"
									? "review"
									: "rollback";

						yield* EnsureLiveThread(transaction, operation.thread_id);

						if (operation.action !== expected_action)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} has the wrong action`,
							});

						if (operation.lifecycle === "committed")
							return {
								event: yield* ReadDuplicate(transaction, operation),
								status: "duplicate" as const,
							};

						if (
							(action === "recorded" || action === "rolled_back") &&
							operation.lifecycle !== "applied"
						)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} must be applied before commit`,
							});

						const now = yield* metadata.Now;
						let change: WorkspaceChangeValue;

						if (action === "recorded") {
							if (operation.action !== "replace")
								return yield* new JournalInvariantError({
									message: `Workspace replace ${message_id} has invalid action`,
								});

							const [existing_change] = yield* transaction
								.select()
								.from(WorkspaceChanges)
								.where(eq(WorkspaceChanges.change_id, operation.change_id))
								.limit(1);

							if (existing_change)
								return yield* new WorkspaceChangeIdConflict({
									change_id: operation.change_id,
								});

							const inserted = {
								after_identity_json: JSON.stringify(operation.result_identity),
								agent_id: operation.agent_id,
								before_identity_json: JSON.stringify(operation.expected_identity),
								change_id: operation.change_id,
								created_at: now,
								path: operation.path,
								raw_origin_json:
									operation.raw_origin === undefined
										? null
										: JSON.stringify(operation.raw_origin),
								review_state: "needs_review",
								reviewed_at: null,
								rollback_state: "available",
								rolled_back_at: null,
								run_id: operation.run_id,
								source_command_id: operation.message_id,
								thread_id: operation.thread_id,
								updated_at: now,
								version: 1,
								workspace_id: operation.workspace_id,
							};
							yield* transaction.insert(WorkspaceChanges).values(inserted);
							change = yield* DecodeChange(inserted);
						} else {
							const transition = yield* ValidateTransition(transaction, operation);

							if (transition === undefined) {
								return yield* new JournalInvariantError({
									message: `Workspace transition ${message_id} has invalid action`,
								});
							}

							const stored = transition.row;
							const updated =
								action === "reviewed"
									? {
											...stored,
											review_state: "reviewed",
											reviewed_at: now,
											updated_at: now,
											version: stored.version + 1,
										}
									: {
											...stored,
											review_state: "rolled_back",
											rollback_state: "consumed",
											rolled_back_at: now,
											updated_at: now,
											version: stored.version + 1,
										};
							const [written] = yield* transaction
								.update(WorkspaceChanges)
								.set(updated)
								.where(
									and(
										eq(WorkspaceChanges.change_id, operation.change_id),
										eq(WorkspaceChanges.thread_id, operation.thread_id),
										eq(WorkspaceChanges.version, stored.version),
										eq(WorkspaceChanges.review_state, stored.review_state),
										eq(WorkspaceChanges.rollback_state, stored.rollback_state),
									),
								)
								.returning({ change_id: WorkspaceChanges.change_id });

							if (!written) {
								return yield* new WorkspaceChangeTransitionError({
									message: `Workspace change ${operation.change_id} changed before commit`,
								});
							}

							change = yield* DecodeChange(updated);
						}

						const event = yield* AppendEvent(
							transaction,
							operation,
							action,
							change,
							now,
						);
						yield* transaction.insert(JournalCommands).values({
							accepted_at: now,
							agent_id: operation.action === "replace" ? operation.agent_id : null,
							causation_id: null,
							message_id: operation.message_id,
							origin: "frontend",
							payload_json: command_payload_json(operation),
							payload_type: "workspace.change.command",
							raw_origin_json:
								operation.action === "replace" && operation.raw_origin !== undefined
									? JSON.stringify(operation.raw_origin)
									: null,
							run_id: operation.action === "replace" ? operation.run_id : null,
							schema_version: 1,
							sent_at: operation.sent_at,
							status: "accepted",
							thread_id: operation.thread_id,
						});
						const [committed] = yield* transaction
							.update(WorkspaceChangeOperations)
							.set({
								journal_sequence: event.journal_sequence,
								lifecycle: "committed",
								updated_at: now,
							})
							.where(
								and(
									eq(WorkspaceChangeOperations.message_id, message_id),
									eq(WorkspaceChangeOperations.lifecycle, operation.lifecycle),
								),
							)
							.returning({ message_id: WorkspaceChangeOperations.message_id });

						if (!committed) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} changed before commit`,
							});
						}

						return { event, status: "accepted" as const };
					}),
				);

				if (result.status === "accepted")
					yield* notifier.Publish(result.event.journal_sequence);

				return result;
			}).pipe(Effect.mapError(normalize_error));

		const MarkEvidenceRecorded = (message_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [row] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(eq(WorkspaceChangeOperations.message_id, message_id))
							.limit(1);
						if (!row)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} is missing`,
							});
						const operation = yield* DecodeOperation(row);

						yield* EnsureLiveThread(transaction, operation.thread_id);

						if (
							(operation.action !== "replace" && operation.action !== "rollback") ||
							operation.lifecycle !== "committed"
						)
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} cannot record evidence`,
							});

						yield* ReadDuplicate(transaction, operation);

						if (operation.evidence_recorded) return operation;
						const updated_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(WorkspaceChangeOperations)
							.set({ evidence_recorded: true, updated_at })
							.where(
								and(
									eq(WorkspaceChangeOperations.message_id, message_id),
									eq(WorkspaceChangeOperations.lifecycle, "committed"),
									eq(WorkspaceChangeOperations.evidence_recorded, false),
								),
							)
							.returning({ message_id: WorkspaceChangeOperations.message_id });

						if (!updated) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} changed before evidence was recorded`,
							});
						}

						return yield* DecodeOperation({
							...row,
							evidence_recorded: true,
							updated_at,
						});
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		const List = (thread_id: string, workspace_id?: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						yield* EnsureLiveThread(transaction, thread_id);

						const rows = yield* (
							workspace_id === undefined
								? transaction
										.select()
										.from(WorkspaceChanges)
										.where(eq(WorkspaceChanges.thread_id, thread_id))
								: transaction
										.select()
										.from(WorkspaceChanges)
										.where(
											and(
												eq(WorkspaceChanges.thread_id, thread_id),
												eq(WorkspaceChanges.workspace_id, workspace_id),
											),
										)
						).orderBy(
							desc(WorkspaceChanges.updated_at),
							asc(WorkspaceChanges.change_id),
						);
						const [latest] = yield* transaction
							.select({ journal_sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						return {
							changes: yield* Effect.forEach(rows, DecodeChange),
							journal_sequence: latest?.journal_sequence ?? 0,
						};
					}),
				)
				.pipe(Effect.mapError(normalize_error));

		return {
			ClaimReplace: Claim,
			ClaimReview: Claim,
			ClaimRollback: Claim,
			CommitRecorded: (message_id) => Commit(message_id, "recorded"),
			CommitReviewed: (message_id) => Commit(message_id, "reviewed"),
			CommitRolledBack: (message_id) => Commit(message_id, "rolled_back"),
			List,
			MarkApplied,
			MarkEvidenceRecorded,
			ReconcileChanged,
			RejectChanged,
			ReadChange,
			ReadOperation,
		};
	}),
);
