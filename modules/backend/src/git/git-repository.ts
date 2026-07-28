import { and, asc, desc, eq, inArray, isNull, lte, or } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";

import {
	EventEnvelope,
	EventPayload,
	GitBranchState,
	GitDiffSummary,
	GitFileChange,
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
	GitMutationFailure,
	GitMutationKind,
	GitMutationLifecycle,
	GitMutationPaths,
	GitMutationProjection,
	GitMutationResolveEnvelope,
	GitMutationUpdatedEvent,
	GitRepositoryProjection,
	GitSnapshotId,
	GitWorkspaceProjection,
	GitWorkspaceUpdatedEvent,
	GitWorktree,
	Identifier,
	IsoDateTime,
	JournalSequence,
	PositiveInt,
	RawOrigin,
	git_workspace_maximum_changed_paths,
	git_workspace_maximum_pending_mutations,
	git_workspace_maximum_worktrees,
	type GitMutationProjection as GitMutationProjectionValue,
	type GitWorkspaceProjection as GitWorkspaceProjectionValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import {
	EventStreams,
	GitMutationOperations,
	GitWorkspaceProjections,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
} from "../persistence/schema";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { IsThreadLive } from "../persistence/thread-liveness";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";

const RequestFingerprint = GitSnapshotId;
const NullableIdentifier = Schema.Union([Identifier, Schema.Null]);
const NullableIsoDateTime = Schema.Union([IsoDateTime, Schema.Null]);
const NullableJournalSequence = Schema.Union([JournalSequence, Schema.Null]);
const NullablePositiveInt = Schema.Union([PositiveInt, Schema.Null]);
const NullableSnapshotId = Schema.Union([GitSnapshotId, Schema.Null]);
const NullableString = Schema.Union([Schema.String, Schema.Null]);
const git_dispatch_lease_milliseconds = 60_000;

const GitRepositoryObservation = Schema.Struct({
	aggregate: GitDiffSummary,
	branch: GitBranchState,
	clean: Schema.Boolean,
	files: Schema.Array(GitFileChange).check(
		Schema.isMaxLength(git_workspace_maximum_changed_paths),
	),
	head: Schema.optional(GitRepositoryProjection.fields.head),
	observed_at: IsoDateTime,
	repository_state: Schema.Literal("repository"),
	snapshot_id: GitSnapshotId,
	staged: GitDiffSummary,
	unstaged: GitDiffSummary,
	workspace_id: Identifier,
	worktrees: Schema.NonEmptyArray(GitWorktree).check(
		Schema.isMaxLength(git_workspace_maximum_worktrees),
	),
});

const GitNotRepositoryObservation = Schema.Struct({
	observed_at: IsoDateTime,
	repository_state: Schema.Literal("not_repository"),
	snapshot_id: GitSnapshotId,
	workspace_id: Identifier,
});

/** Carries one complete observed workspace without repository-owned sequence fields. */
export const GitWorkspaceObservation = Schema.Union([
	GitNotRepositoryObservation,
	GitRepositoryObservation,
]);

export type GitWorkspaceObservation = typeof GitWorkspaceObservation.Type;

const GitEventTrace = {
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	correlation_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
};

const GitWorkspaceRecordInput = Schema.Struct({
	...GitEventTrace,
	cause: GitWorkspaceUpdatedEvent.fields.cause,
	workspace: GitWorkspaceObservation,
});

export type GitWorkspaceRecordInput = typeof GitWorkspaceRecordInput.Type;

const GitMutationSucceededInput = Schema.Struct({
	mutation_id: Identifier,
	workspace: GitWorkspaceObservation,
});

export type GitMutationSucceededInput = typeof GitMutationSucceededInput.Type;

const GitMutationTerminalInput = Schema.Struct({
	failure: GitMutationFailure,
	mutation_id: Identifier,
	state: Schema.Literals(["failed", "ambiguous"]),
});

export type GitMutationTerminalInput = typeof GitMutationTerminalInput.Type;

const GitMutationRequestEnvelope = Schema.Union([
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
]);

export type GitMutationRequestEnvelope = typeof GitMutationRequestEnvelope.Type;

const StoredGitWorkspaceRow = Schema.Struct({
	journal_sequence: JournalSequence,
	observed_at: IsoDateTime,
	projection_json: Schema.String,
	snapshot_id: GitSnapshotId,
	updated_at: IsoDateTime,
	version: PositiveInt,
	workspace_id: Identifier,
});

const StoredGitMutationRow = Schema.Struct({
	agent_id: NullableIdentifier,
	approval_id: Identifier,
	completed_at: NullableIsoDateTime,
	decision_at: NullableIsoDateTime,
	decision_message_id: NullableIdentifier,
	dispatched_at: NullableIsoDateTime,
	dispatch_lease_expires_at: NullableIsoDateTime,
	dispatch_owner_id: NullableIdentifier,
	expected_snapshot_id: GitSnapshotId,
	expected_workspace_version: PositiveInt,
	failure_code: NullableString,
	journal_sequence: NullableJournalSequence,
	kind: GitMutationKind,
	lifecycle: GitMutationLifecycle,
	mutation_id: Identifier,
	paths_json: Schema.String,
	raw_origin_json: NullableString,
	request_fingerprint: RequestFingerprint,
	requested_at: IsoDateTime,
	result_snapshot_id: NullableSnapshotId,
	result_workspace_version: NullablePositiveInt,
	run_id: NullableIdentifier,
	source_message_id: Identifier,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	workspace_id: Identifier,
});

const StoredJournalEventRow = Schema.Struct({
	agent_id: NullableIdentifier,
	causation_id: Identifier,
	correlation_id: Identifier,
	event_id: Identifier,
	event_type: Identifier,
	journal_sequence: JournalSequence,
	occurred_at: IsoDateTime,
	origin: Schema.Literal("backend"),
	payload_json: Schema.String,
	protocol_version: Schema.optional(Schema.Literal(1)),
	raw_origin_json: NullableString,
	run_id: NullableIdentifier,
	schema_version: Schema.Literal(1),
	stream_id: Identifier,
	stream_sequence: JournalSequence,
	thread_id: Identifier,
});

interface MutationIdentity {
	readonly agent_id?: string;
	readonly approval_id: string;
	readonly expected_snapshot_id: string;
	readonly expected_workspace_version: number;
	readonly kind: typeof GitMutationKind.Type;
	readonly mutation_id: string;
	readonly paths: ReadonlyArray<string>;
	readonly raw_origin?: typeof RawOrigin.Type;
	readonly request_message_id: string;
	readonly requested_at: string;
	readonly run_id?: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

interface DecodedMutationRow {
	readonly identity: MutationIdentity;
	readonly projection: GitMutationProjectionValue;
	readonly request_fingerprint: string;
	readonly result_snapshot_id?: string;
	readonly row: typeof StoredGitMutationRow.Type;
}

export interface GitMutationAcceptance {
	readonly event: typeof EventEnvelope.Type;
	readonly mutation: GitMutationProjectionValue;
	readonly status: "accepted" | "duplicate";
}

export interface GitWorkspaceCommit {
	readonly event: typeof EventEnvelope.Type;
	readonly status: "accepted" | "duplicate";
	readonly workspace: GitWorkspaceProjectionValue;
}

export interface GitMutationSuccessCommit {
	readonly mutation: GitMutationProjectionValue;
	readonly mutation_event: typeof EventEnvelope.Type;
	readonly status: "accepted" | "duplicate";
	readonly workspace: GitWorkspaceProjectionValue;
	readonly workspace_event: typeof EventEnvelope.Type;
}

export interface GitRepositoryRecovery {
	readonly ambiguous: ReadonlyArray<GitMutationProjectionValue>;
	readonly approved: ReadonlyArray<GitMutationProjectionValue>;
}

export type GitRepositoryConflictReason =
	| "decision_conflict"
	| "dispatch_conflict"
	| "mutation_conflict"
	| "terminal_conflict"
	| "thread_unavailable"
	| "workspace_busy"
	| "workspace_changed";

/** Reports malformed repository input before persistence is touched. */
export class GitRepositoryInvalid extends Data.TaggedError("GitRepositoryInvalid")<{
	readonly operation: string;
}> {}

/** Reports exact identity reuse or optimistic concurrency failure without side effects. */
export class GitRepositoryConflict extends Data.TaggedError("GitRepositoryConflict")<{
	readonly reason: GitRepositoryConflictReason;
}> {}

/** Reports a missing durable Git workspace or mutation without leaking another identity. */
export class GitRepositoryNotFound extends Data.TaggedError("GitRepositoryNotFound")<{
	readonly resource: "mutation" | "workspace";
}> {}

/** Reports malformed or internally inconsistent durable Git state. */
export class GitRepositoryInvariantError extends Data.TaggedError("GitRepositoryInvariantError")<{
	readonly message: string;
}> {}

/** Conceals unexpected SQLite and cryptographic infrastructure failures. */
export class GitRepositoryPersistenceFailure extends Data.TaggedError(
	"GitRepositoryPersistenceFailure",
)<{ readonly cause: unknown }> {}

export type GitRepositoryError =
	| GitRepositoryConflict
	| GitRepositoryInvalid
	| GitRepositoryInvariantError
	| GitRepositoryNotFound
	| GitRepositoryPersistenceFailure;

/** Owns durable Git workspace observations and approval-bound mutation lifecycles. */
export class GitRepository extends Context.Service<
	GitRepository,
	{
		readonly ClaimApproved: (
			mutation_id: string,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
		readonly CommitSucceeded: (
			input: GitMutationSucceededInput,
		) => Effect.Effect<GitMutationSuccessCommit, GitRepositoryError>;
		readonly CommitTerminal: (
			input: GitMutationTerminalInput,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
		readonly ListPending: (
			workspace_id?: string,
		) => Effect.Effect<ReadonlyArray<GitMutationProjectionValue>, GitRepositoryError>;
		readonly ReadMutation: (
			mutation_id: string,
		) => Effect.Effect<GitMutationProjectionValue, GitRepositoryError>;
		readonly ReadWorkspace: (
			workspace_id: string,
		) => Effect.Effect<GitWorkspaceProjectionValue, GitRepositoryError>;
		readonly RecordWorkspace: (
			input: GitWorkspaceRecordInput,
		) => Effect.Effect<GitWorkspaceCommit, GitRepositoryError>;
		readonly RecoverDispatching: () => Effect.Effect<GitRepositoryRecovery, GitRepositoryError>;
		readonly RequestMutation: (
			envelope: GitMutationRequestEnvelope,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
		readonly ResolveMutation: (
			envelope: typeof GitMutationResolveEnvelope.Type,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
	}
>()("Artisan/GitRepository") {}

function normalize_error(error: unknown): GitRepositoryError {
	if (
		error instanceof GitRepositoryConflict ||
		error instanceof GitRepositoryInvalid ||
		error instanceof GitRepositoryInvariantError ||
		error instanceof GitRepositoryNotFound ||
		error instanceof GitRepositoryPersistenceFailure
	) {
		return error;
	}

	return new GitRepositoryPersistenceFailure({ cause: error });
}

function invariant(message: string) {
	return new GitRepositoryInvariantError({ message });
}

function canonical_paths(paths: ReadonlyArray<string>) {
	return [...paths].toSorted((left, right) => (left < right ? -1 : left > right ? 1 : 0));
}

function optional_equal(left: string | undefined, right: string | undefined) {
	return left === right;
}

function raw_origins_equal(
	left: typeof RawOrigin.Type | undefined,
	right: typeof RawOrigin.Type | undefined,
) {
	return (
		(left === undefined && right === undefined) ||
		(left !== undefined &&
			right !== undefined &&
			left.provider === right.provider &&
			left.reference === right.reference)
	);
}

function traces_equal(
	left: {
		readonly agent_id?: string;
		readonly raw_origin?: typeof RawOrigin.Type;
		readonly run_id?: string;
		readonly thread_id: string;
	},
	right: {
		readonly agent_id?: string;
		readonly raw_origin?: typeof RawOrigin.Type;
		readonly run_id?: string;
		readonly thread_id: string;
	},
) {
	return (
		optional_equal(left.agent_id, right.agent_id) &&
		raw_origins_equal(left.raw_origin, right.raw_origin) &&
		optional_equal(left.run_id, right.run_id) &&
		left.thread_id === right.thread_id
	);
}

const Decode = <A>(schema: Schema.Codec<A, A>, input: unknown, operation: string) =>
	Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => new GitRepositoryInvalid({ operation })),
	);

const ParseJson = (json: string, context: string) =>
	Effect.try({
		try: () => JSON.parse(json) as unknown,
		catch: () => invariant(`${context} contains invalid JSON`),
	});

/** Supplies the SQLite-backed Git repository. */
export const GitRepositoryLive = Layer.effect(
	GitRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ComputeFingerprint = (identity: MutationIdentity) =>
			Effect.gen(function* () {
				const canonical = {
					agent_id: identity.agent_id ?? null,
					approval_id: identity.approval_id,
					expected_snapshot_id: identity.expected_snapshot_id,
					expected_workspace_version: identity.expected_workspace_version,
					kind: identity.kind,
					mutation_id: identity.mutation_id,
					paths: canonical_paths(identity.paths),
					raw_origin: identity.raw_origin ?? null,
					request_message_id: identity.request_message_id,
					requested_at: identity.requested_at,
					run_id: identity.run_id ?? null,
					thread_id: identity.thread_id,
					workspace_id: identity.workspace_id,
				};
				const digest = yield* crypto.digest(
					"SHA-256",
					new TextEncoder().encode(JSON.stringify(canonical)),
				);

				return Encoding.encodeHex(digest);
			}).pipe(Effect.mapError((cause) => new GitRepositoryPersistenceFailure({ cause })));

		const DecodeOptionalJson = <A>(
			schema: Schema.Codec<A, A>,
			json: string | null,
			context: string,
		) =>
			json === null
				? Effect.succeed(undefined)
				: ParseJson(json, context).pipe(
						Effect.flatMap(
							Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" }),
						),
						Effect.mapError(() => invariant(`${context} does not match its schema`)),
					);

		const DecodeWorkspaceRow = (input: unknown) =>
			Schema.decodeUnknownEffect(StoredGitWorkspaceRow, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => invariant("Stored Git workspace row is invalid")),
				Effect.flatMap((row) =>
					ParseJson(row.projection_json, "Stored Git workspace projection").pipe(
						Effect.flatMap(
							Schema.decodeUnknownEffect(GitWorkspaceProjection, {
								onExcessProperty: "error",
							}),
						),
						Effect.mapError(() =>
							invariant("Stored Git workspace projection does not match its schema"),
						),
						Effect.flatMap((projection) =>
							projection.workspace_id !== row.workspace_id ||
							projection.snapshot_id !== row.snapshot_id ||
							projection.version !== row.version ||
							projection.journal_sequence !== row.journal_sequence ||
							projection.observed_at !== row.observed_at ||
							row.journal_sequence <= 0
								? Effect.fail(invariant("Stored Git workspace aliases disagree"))
								: Effect.succeed(projection),
						),
					),
				),
			);

		const MakeWorkspaceProjection = (
			observation: GitWorkspaceObservation,
			version: number,
			journal_sequence: number,
		) =>
			Schema.decodeUnknownEffect(GitWorkspaceProjection, {
				onExcessProperty: "error",
			})({ ...observation, journal_sequence, version }).pipe(
				Effect.mapError(() => invariant("Git workspace observation is inconsistent")),
			);

		const MakeMutationProjection = (
			row: typeof StoredGitMutationRow.Type,
			paths: typeof GitMutationPaths.Type,
			raw_origin: typeof RawOrigin.Type | undefined,
			failure: typeof GitMutationFailure.Type | undefined,
			journal_sequence: number,
		) =>
			Schema.decodeUnknownEffect(GitMutationProjection, {
				onExcessProperty: "error",
			})({
				...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
				approval_id: row.approval_id,
				...(row.completed_at === null ? {} : { completed_at: row.completed_at }),
				...(row.decision_at === null ? {} : { decision_at: row.decision_at }),
				...(row.decision_message_id === null
					? {}
					: { decision_message_id: row.decision_message_id }),
				...(row.dispatched_at === null ? {} : { dispatched_at: row.dispatched_at }),
				expected_snapshot_id: row.expected_snapshot_id,
				expected_workspace_version: row.expected_workspace_version,
				...(failure === undefined ? {} : { failure }),
				journal_sequence,
				kind: row.kind,
				lifecycle: row.lifecycle,
				mutation_id: row.mutation_id,
				paths,
				...(raw_origin === undefined ? {} : { raw_origin }),
				requested_at: row.requested_at,
				...(row.result_snapshot_id === null
					? {}
					: { result_snapshot_id: row.result_snapshot_id }),
				...(row.result_workspace_version === null
					? {}
					: { result_workspace_version: row.result_workspace_version }),
				...(row.run_id === null ? {} : { run_id: row.run_id }),
				source_message_id: row.source_message_id,
				thread_id: row.thread_id,
				updated_at: row.updated_at,
				workspace_id: row.workspace_id,
			}).pipe(
				Effect.mapError((cause) =>
					invariant(
						`Stored Git mutation ${row.mutation_id} is invalid: ${String(cause)}`,
					),
				),
			);

		const DecodeMutationRow = (
			input: unknown,
			allow_provisional_journal_sequence = false,
		): Effect.Effect<
			DecodedMutationRow,
			GitRepositoryInvariantError | GitRepositoryPersistenceFailure
		> =>
			Effect.gen(function* () {
				const row = yield* Schema.decodeUnknownEffect(StoredGitMutationRow, {
					onExcessProperty: "error",
				})(input).pipe(
					Effect.mapError(() => invariant("Stored Git mutation row is invalid")),
				);
				const paths = yield* ParseJson(
					row.paths_json,
					`Stored Git mutation ${row.mutation_id} paths`,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(GitMutationPaths, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(`Stored Git mutation ${row.mutation_id} paths are invalid`),
					),
				);

				if (JSON.stringify(paths) !== JSON.stringify(canonical_paths(paths))) {
					return yield* Effect.fail(
						invariant(`Stored Git mutation ${row.mutation_id} paths are not canonical`),
					);
				}

				const raw_origin = yield* DecodeOptionalJson(
					RawOrigin,
					row.raw_origin_json,
					`Stored Git mutation ${row.mutation_id} raw origin`,
				);
				const failure = yield* DecodeOptionalJson(
					GitMutationFailure,
					row.failure_code,
					`Stored Git mutation ${row.mutation_id} failure`,
				);

				if (
					row.journal_sequence === null ||
					(!allow_provisional_journal_sequence && row.journal_sequence <= 0)
				) {
					return yield* Effect.fail(
						invariant(`Stored Git mutation ${row.mutation_id} has no journal event`),
					);
				}

				const terminal = ["denied", "succeeded", "failed", "ambiguous"].includes(
					row.lifecycle,
				);
				const resolved = row.lifecycle !== "awaiting_approval";
				const dispatched = ["dispatching", "succeeded", "failed", "ambiguous"].includes(
					row.lifecycle,
				);
				const failed = row.lifecycle === "failed" || row.lifecycle === "ambiguous";
				const succeeded = row.lifecycle === "succeeded";
				const has_partial_dispatch_lease =
					(row.dispatch_owner_id === null) !== (row.dispatch_lease_expires_at === null);

				if (
					resolved !== (row.decision_message_id !== null && row.decision_at !== null) ||
					dispatched !== (row.dispatched_at !== null) ||
					has_partial_dispatch_lease ||
					terminal !== (row.completed_at !== null) ||
					failed !== (failure !== undefined) ||
					succeeded !==
						(row.result_snapshot_id !== null &&
							row.result_workspace_version !== null) ||
					(!succeeded &&
						(row.result_snapshot_id !== null || row.result_workspace_version !== null))
				) {
					return yield* Effect.fail(
						invariant(
							`Stored Git mutation ${row.mutation_id} lifecycle aliases disagree`,
						),
					);
				}

				const identity: MutationIdentity = {
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
					approval_id: row.approval_id,
					expected_snapshot_id: row.expected_snapshot_id,
					expected_workspace_version: row.expected_workspace_version,
					kind: row.kind,
					mutation_id: row.mutation_id,
					paths,
					...(raw_origin === undefined ? {} : { raw_origin }),
					request_message_id: row.source_message_id,
					requested_at: row.requested_at,
					...(row.run_id === null ? {} : { run_id: row.run_id }),
					thread_id: row.thread_id,
					workspace_id: row.workspace_id,
				};
				const request_fingerprint = yield* ComputeFingerprint(identity);

				if (request_fingerprint !== row.request_fingerprint) {
					return yield* Effect.fail(
						invariant(`Stored Git mutation ${row.mutation_id} fingerprint is invalid`),
					);
				}

				const projection = yield* MakeMutationProjection(
					row,
					paths,
					raw_origin,
					failure,
					row.journal_sequence,
				);

				return {
					identity,
					projection,
					request_fingerprint,
					...(row.result_snapshot_id === null
						? {}
						: { result_snapshot_id: row.result_snapshot_id }),
					row,
				};
			});

		const DecodeEventRow = (input: unknown) =>
			Effect.gen(function* () {
				const row = yield* Schema.decodeUnknownEffect(StoredJournalEventRow, {
					onExcessProperty: "error",
				})(input).pipe(Effect.mapError(() => invariant("Stored Git event row is invalid")));
				const payload = yield* ParseJson(
					row.payload_json,
					`Stored event ${row.event_id}`,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(EventPayload, { onExcessProperty: "error" }),
					),
					Effect.mapError(() =>
						invariant(`Stored event ${row.event_id} payload is invalid`),
					),
				);
				const raw_origin = yield* DecodeOptionalJson(
					RawOrigin,
					row.raw_origin_json,
					`Stored event ${row.event_id} raw origin`,
				);

				if (payload.type !== row.event_type) {
					return yield* Effect.fail(
						invariant(`Stored event ${row.event_id} type disagrees`),
					);
				}

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
					causation_id: row.causation_id,
					correlation_id: row.correlation_id,
					journal_sequence: row.journal_sequence,
					kind: "event",
					message_id: row.event_id,
					origin: "backend",
					payload,
					protocol_version: 1,
					...(raw_origin === undefined ? {} : { raw_origin }),
					...(row.run_id === null ? {} : { run_id: row.run_id }),
					schema_version: 1,
					sequence: row.stream_sequence,
					sent_at: row.occurred_at,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				}).pipe(
					Effect.mapError(() =>
						invariant(`Stored event ${row.event_id} envelope is invalid`),
					),
				);
			});

		const event_columns = {
			agent_id: JournalEvents.agent_id,
			causation_id: JournalEvents.causation_id,
			correlation_id: JournalEvents.correlation_id,
			event_id: JournalEvents.event_id,
			event_type: JournalEvents.event_type,
			journal_sequence: JournalEvents.sequence,
			occurred_at: JournalEvents.occurred_at,
			origin: JournalEvents.origin,
			payload_json: JournalEvents.payload_json,
			raw_origin_json: JournalEvents.raw_origin_json,
			run_id: JournalEvents.run_id,
			schema_version: JournalEvents.schema_version,
			stream_id: JournalEvents.stream_id,
			stream_sequence: JournalEvents.stream_sequence,
			thread_id: JournalEvents.thread_id,
		};

		const ReadEventBySequence = (transaction: typeof database.client, sequence: number) =>
			transaction
				.select(event_columns)
				.from(JournalEvents)
				.where(eq(JournalEvents.sequence, sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row === undefined
							? Effect.fail(invariant(`Git event ${sequence} is missing`))
							: DecodeEventRow(row),
					),
				);

		const ReadFirstEvent = (
			transaction: typeof database.client,
			correlation_id: string,
			event_type: "git.mutation.updated" | "git.workspace.updated",
		) =>
			transaction
				.select(event_columns)
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, correlation_id),
						eq(JournalEvents.event_type, event_type),
					),
				)
				.orderBy(asc(JournalEvents.sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row === undefined
							? Effect.fail(
									invariant(`Correlated Git event ${correlation_id} is missing`),
								)
							: DecodeEventRow(row),
					),
				);

		const ReadLastEvent = (
			transaction: typeof database.client,
			correlation_id: string,
			event_type: "git.mutation.updated" | "git.workspace.updated",
		) =>
			transaction
				.select(event_columns)
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, correlation_id),
						eq(JournalEvents.event_type, event_type),
					),
				)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row === undefined
							? Effect.fail(
									invariant(`Correlated Git event ${correlation_id} is missing`),
								)
							: DecodeEventRow(row),
					),
				);

		const AppendEvent = (
			transaction: typeof database.client,
			input: {
				readonly agent_id?: string;
				readonly causation_id: string;
				readonly correlation_id: string;
				readonly occurred_at: string;
				readonly payload_at: (
					journal_sequence: number,
				) => Effect.Effect<typeof EventPayload.Type, GitRepositoryInvariantError>;
				readonly raw_origin?: typeof RawOrigin.Type;
				readonly run_id?: string;
				readonly thread_id: string;
			},
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const provisional_payload = yield* input.payload_at(0);

				if (stream === undefined) {
					yield* transaction.insert(EventStreams).values({
						last_sequence: stream_sequence,
						stream_id,
					});
				} else {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: stream_sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				}

				const [inserted] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.agent_id ?? null,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: provisional_payload.type,
						occurred_at: input.occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(provisional_payload),
						raw_origin_json:
							input.raw_origin === undefined
								? null
								: JSON.stringify(input.raw_origin),
						run_id: input.run_id ?? null,
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: input.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				if (inserted === undefined) {
					return yield* Effect.fail(invariant("Git event reservation was not persisted"));
				}

				const payload = yield* input.payload_at(inserted.journal_sequence);
				yield* transaction
					.update(JournalEvents)
					.set({ payload_json: JSON.stringify(payload) })
					.where(eq(JournalEvents.event_id, event_id));
				yield* RecordThreadActivity(
					transaction,
					input.thread_id,
					input.occurred_at,
					payload,
				);

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					journal_sequence: inserted.journal_sequence,
					kind: "event",
					message_id: event_id,
					origin: "backend",
					payload,
					protocol_version: 1,
					...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
					...(input.run_id === undefined ? {} : { run_id: input.run_id }),
					schema_version: 1,
					sequence: stream_sequence,
					sent_at: input.occurred_at,
					stream_id,
					thread_id: input.thread_id,
				}).pipe(Effect.mapError(() => invariant(`Git event ${event_id} is invalid`)));
			});

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			IsThreadLive(transaction, thread_id).pipe(
				Effect.filterOrFail(
					(live) => live,
					() => new GitRepositoryConflict({ reason: "thread_unavailable" }),
				),
				Effect.asVoid,
			);

		const ReadWorkspaceTransaction = (
			transaction: typeof database.client,
			workspace_id: string,
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(GitWorkspaceProjections)
					.where(eq(GitWorkspaceProjections.workspace_id, workspace_id))
					.limit(1);

				if (row === undefined) {
					return yield* Effect.fail(new GitRepositoryNotFound({ resource: "workspace" }));
				}

				return yield* DecodeWorkspaceRow(row);
			});

		const ReadMutationTransaction = (
			transaction: typeof database.client,
			mutation_id: string,
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(GitMutationOperations)
					.where(eq(GitMutationOperations.mutation_id, mutation_id))
					.limit(1);

				if (row === undefined) {
					return yield* Effect.fail(new GitRepositoryNotFound({ resource: "mutation" }));
				}

				return yield* DecodeMutationRow(row);
			});

		const WriteWorkspaceProjection = (
			transaction: typeof database.client,
			projection: GitWorkspaceProjectionValue,
			updated_at: string,
			expected?: GitWorkspaceProjectionValue,
		) =>
			Effect.gen(function* () {
				const values = {
					journal_sequence: projection.journal_sequence,
					observed_at: projection.observed_at,
					projection_json: JSON.stringify(projection),
					snapshot_id: projection.snapshot_id,
					updated_at,
					version: projection.version,
					workspace_id: projection.workspace_id,
				};

				if (expected === undefined) {
					const [inserted] = yield* transaction
						.insert(GitWorkspaceProjections)
						.values(values)
						.returning();

					if (inserted === undefined) {
						return yield* Effect.fail(
							invariant("Git workspace projection was not inserted"),
						);
					}
				} else {
					const [updated] = yield* transaction
						.update(GitWorkspaceProjections)
						.set(values)
						.where(
							and(
								eq(GitWorkspaceProjections.workspace_id, expected.workspace_id),
								eq(GitWorkspaceProjections.snapshot_id, expected.snapshot_id),
								eq(GitWorkspaceProjections.version, expected.version),
							),
						)
						.returning();

					if (updated === undefined) {
						return yield* Effect.fail(
							new GitRepositoryConflict({ reason: "workspace_changed" }),
						);
					}
				}
			});

		const RecordWorkspaceTransaction = (
			transaction: typeof database.client,
			input: GitWorkspaceRecordInput,
		) =>
			Effect.gen(function* () {
				yield* EnsureLiveThread(transaction, input.thread_id);
				const [dispatching] = yield* transaction
					.select({ mutation_id: GitMutationOperations.mutation_id })
					.from(GitMutationOperations)
					.where(
						and(
							eq(GitMutationOperations.workspace_id, input.workspace.workspace_id),
							eq(GitMutationOperations.lifecycle, "dispatching"),
						),
					)
					.limit(1);

				if (dispatching !== undefined) {
					return yield* Effect.fail(
						new GitRepositoryConflict({ reason: "workspace_busy" }),
					);
				}

				const [stored] = yield* transaction
					.select()
					.from(GitWorkspaceProjections)
					.where(eq(GitWorkspaceProjections.workspace_id, input.workspace.workspace_id))
					.limit(1);
				const current =
					stored === undefined ? undefined : yield* DecodeWorkspaceRow(stored);

				if (current?.snapshot_id === input.workspace.snapshot_id) {
					const candidate = yield* MakeWorkspaceProjection(
						{ ...input.workspace, observed_at: current.observed_at },
						current.version,
						current.journal_sequence,
					);

					if (JSON.stringify(candidate) !== JSON.stringify(current)) {
						return yield* Effect.fail(
							invariant(
								"A Git snapshot identifier was reused for different workspace state",
							),
						);
					}

					return {
						event: yield* ReadEventBySequence(transaction, current.journal_sequence),
						status: "duplicate" as const,
						workspace: current,
					};
				}

				const version = (current?.version ?? 0) + 1;
				const updated_at = yield* metadata.Now;
				const event = yield* AppendEvent(transaction, {
					...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					occurred_at: updated_at,
					payload_at: (journal_sequence) =>
						MakeWorkspaceProjection(input.workspace, version, journal_sequence).pipe(
							Effect.flatMap((workspace) =>
								Schema.decodeUnknownEffect(GitWorkspaceUpdatedEvent, {
									onExcessProperty: "error",
								})({
									cause: input.cause,
									type: "git.workspace.updated",
									workspace,
								}),
							),
							Effect.mapError(() => invariant("Git workspace event is invalid")),
						),
					...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
					...(input.run_id === undefined ? {} : { run_id: input.run_id }),
					thread_id: input.thread_id,
				});
				const workspace = yield* MakeWorkspaceProjection(
					input.workspace,
					version,
					event.journal_sequence,
				);

				yield* WriteWorkspaceProjection(transaction, workspace, updated_at, current);

				return { event, status: "accepted" as const, workspace };
			});

		const RequestIdentity = (envelope: GitMutationRequestEnvelope): MutationIdentity => ({
			...(envelope.agent_id === undefined ? {} : { agent_id: envelope.agent_id }),
			approval_id: envelope.payload.approval_id,
			expected_snapshot_id: envelope.payload.expected_snapshot_id,
			expected_workspace_version: envelope.payload.expected_workspace_version,
			kind: envelope.kind === "git.index.stage.request" ? "stage" : "unstage",
			mutation_id: envelope.payload.mutation_id,
			paths: canonical_paths(envelope.payload.paths),
			...(envelope.raw_origin === undefined ? {} : { raw_origin: envelope.raw_origin }),
			request_message_id: envelope.message_id,
			requested_at: envelope.sent_at,
			...(envelope.run_id === undefined ? {} : { run_id: envelope.run_id }),
			thread_id: envelope.thread_id,
			workspace_id: envelope.payload.workspace_id,
		});

		const RequestMutation = (input: GitMutationRequestEnvelope) =>
			Decode(GitMutationRequestEnvelope, input, "request_mutation").pipe(
				Effect.flatMap((envelope) =>
					Effect.gen(function* () {
						const identity = RequestIdentity(envelope);
						const request_fingerprint = yield* ComputeFingerprint(identity);

						return yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									yield* EnsureLiveThread(transaction, envelope.thread_id);
									const collisions = yield* transaction
										.select()
										.from(GitMutationOperations)
										.where(
											or(
												eq(
													GitMutationOperations.mutation_id,
													identity.mutation_id,
												),
												eq(
													GitMutationOperations.approval_id,
													identity.approval_id,
												),
												eq(
													GitMutationOperations.source_message_id,
													identity.request_message_id,
												),
												eq(
													GitMutationOperations.decision_message_id,
													identity.request_message_id,
												),
											),
										);

									if (collisions.length > 0) {
										if (collisions.length !== 1) {
											return yield* Effect.fail(
												invariant(
													"Git mutation identities resolve to multiple rows",
												),
											);
										}

										const existing = yield* DecodeMutationRow(collisions[0]);

										if (
											existing.request_fingerprint !== request_fingerprint ||
											existing.identity.mutation_id !==
												identity.mutation_id ||
											existing.identity.approval_id !==
												identity.approval_id ||
											existing.identity.request_message_id !==
												identity.request_message_id
										) {
											return yield* Effect.fail(
												new GitRepositoryConflict({
													reason: "mutation_conflict",
												}),
											);
										}

										return {
											event: yield* ReadFirstEvent(
												transaction,
												identity.request_message_id,
												"git.mutation.updated",
											),
											mutation: existing.projection,
											status: "duplicate" as const,
										};
									}

									const workspace = yield* ReadWorkspaceTransaction(
										transaction,
										identity.workspace_id,
									);

									if (
										workspace.snapshot_id !== identity.expected_snapshot_id ||
										workspace.version !== identity.expected_workspace_version
									) {
										return yield* Effect.fail(
											new GitRepositoryConflict({
												reason: "workspace_changed",
											}),
										);
									}

									const updated_at = yield* metadata.Now;
									const row = yield* Schema.decodeUnknownEffect(
										StoredGitMutationRow,
										{
											onExcessProperty: "error",
										},
									)({
										agent_id: identity.agent_id ?? null,
										approval_id: identity.approval_id,
										completed_at: null,
										decision_at: null,
										decision_message_id: null,
										dispatched_at: null,
										dispatch_lease_expires_at: null,
										dispatch_owner_id: null,
										expected_snapshot_id: identity.expected_snapshot_id,
										expected_workspace_version:
											identity.expected_workspace_version,
										failure_code: null,
										journal_sequence: null,
										kind: identity.kind,
										lifecycle: "awaiting_approval",
										mutation_id: identity.mutation_id,
										paths_json: JSON.stringify(identity.paths),
										raw_origin_json:
											identity.raw_origin === undefined
												? null
												: JSON.stringify(identity.raw_origin),
										request_fingerprint,
										requested_at: identity.requested_at,
										result_snapshot_id: null,
										result_workspace_version: null,
										run_id: identity.run_id ?? null,
										source_message_id: identity.request_message_id,
										thread_id: identity.thread_id,
										updated_at,
										workspace_id: identity.workspace_id,
									}).pipe(
										Effect.mapError(() =>
											invariant("New Git mutation row is invalid"),
										),
									);

									yield* transaction.insert(GitMutationOperations).values(row);
									const event = yield* AppendEvent(transaction, {
										...(identity.agent_id === undefined
											? {}
											: { agent_id: identity.agent_id }),
										causation_id: identity.request_message_id,
										correlation_id: identity.request_message_id,
										occurred_at: updated_at,
										payload_at: (journal_sequence) =>
											MakeMutationProjection(
												row,
												identity.paths as typeof GitMutationPaths.Type,
												identity.raw_origin,
												undefined,
												journal_sequence,
											).pipe(
												Effect.flatMap((mutation) =>
													Schema.decodeUnknownEffect(
														GitMutationUpdatedEvent,
														{
															onExcessProperty: "error",
														},
													)({ mutation, type: "git.mutation.updated" }),
												),
												Effect.mapError(() =>
													invariant("Git mutation event is invalid"),
												),
											),
										...(identity.raw_origin === undefined
											? {}
											: { raw_origin: identity.raw_origin }),
										...(identity.run_id === undefined
											? {}
											: { run_id: identity.run_id }),
										thread_id: identity.thread_id,
									});
									yield* transaction
										.update(GitMutationOperations)
										.set({ journal_sequence: event.journal_sequence })
										.where(
											eq(
												GitMutationOperations.mutation_id,
												identity.mutation_id,
											),
										);
									const mutation = yield* ReadMutationTransaction(
										transaction,
										identity.mutation_id,
									);

									return {
										event,
										mutation: mutation.projection,
										status: "accepted" as const,
									};
								}),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.event.journal_sequence)
						: Effect.void,
				),
			);

		const ResolveMutation = (input: typeof GitMutationResolveEnvelope.Type) =>
			Decode(GitMutationResolveEnvelope, input, "resolve_mutation").pipe(
				Effect.flatMap((envelope) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								yield* EnsureLiveThread(transaction, envelope.thread_id);
								const [message_collision] = yield* transaction
									.select()
									.from(GitMutationOperations)
									.where(
										or(
											eq(
												GitMutationOperations.source_message_id,
												envelope.message_id,
											),
											eq(
												GitMutationOperations.decision_message_id,
												envelope.message_id,
											),
										),
									)
									.limit(1);
								const mutation = yield* ReadMutationTransaction(
									transaction,
									envelope.payload.mutation_id,
								);
								const decision_trace = {
									...(envelope.agent_id === undefined
										? {}
										: { agent_id: envelope.agent_id }),
									...(envelope.raw_origin === undefined
										? {}
										: { raw_origin: envelope.raw_origin }),
									...(envelope.run_id === undefined
										? {}
										: { run_id: envelope.run_id }),
									thread_id: envelope.thread_id,
								};

								if (
									envelope.payload.approval_id !==
										mutation.identity.approval_id ||
									!traces_equal(decision_trace, mutation.identity)
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "decision_conflict" }),
									);
								}

								if (mutation.row.decision_message_id !== null) {
									const prior_approved = mutation.row.lifecycle !== "denied";

									if (
										mutation.row.decision_message_id !== envelope.message_id ||
										mutation.row.decision_at !== envelope.sent_at ||
										prior_approved !== envelope.payload.approved
									) {
										return yield* Effect.fail(
											new GitRepositoryConflict({
												reason: "decision_conflict",
											}),
										);
									}

									return {
										event: yield* ReadEventBySequence(
											transaction,
											mutation.projection.journal_sequence,
										),
										mutation: mutation.projection,
										status: "duplicate" as const,
									};
								}

								if (
									mutation.row.lifecycle !== "awaiting_approval" ||
									(message_collision !== undefined &&
										message_collision.mutation_id !== mutation.row.mutation_id)
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "decision_conflict" }),
									);
								}

								const updated_at = yield* metadata.Now;
								const lifecycle = envelope.payload.approved ? "approved" : "denied";
								const [updated] = yield* transaction
									.update(GitMutationOperations)
									.set({
										completed_at: envelope.payload.approved ? null : updated_at,
										decision_at: envelope.sent_at,
										decision_message_id: envelope.message_id,
										lifecycle,
										updated_at,
									})
									.where(
										and(
											eq(
												GitMutationOperations.mutation_id,
												mutation.row.mutation_id,
											),
											eq(
												GitMutationOperations.lifecycle,
												"awaiting_approval",
											),
										),
									)
									.returning();

								if (updated === undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "decision_conflict" }),
									);
								}

								const event = yield* AppendEvent(transaction, {
									...(envelope.agent_id === undefined
										? {}
										: { agent_id: envelope.agent_id }),
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										DecodeMutationRow(
											{ ...updated, journal_sequence },
											true,
										).pipe(
											Effect.flatMap(({ projection }) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													mutation: projection,
													type: "git.mutation.updated",
												}),
											),
											Effect.mapError((cause) =>
												invariant(
													`Git decision event is invalid: ${String(cause)}`,
												),
											),
										),
									...(envelope.raw_origin === undefined
										? {}
										: { raw_origin: envelope.raw_origin }),
									...(envelope.run_id === undefined
										? {}
										: { run_id: envelope.run_id }),
									thread_id: envelope.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: event.journal_sequence })
									.where(
										eq(
											GitMutationOperations.mutation_id,
											mutation.row.mutation_id,
										),
									);
								const committed = yield* ReadMutationTransaction(
									transaction,
									mutation.row.mutation_id,
								);

								return {
									event,
									mutation: committed.projection,
									status: "accepted" as const,
								};
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.event.journal_sequence)
						: Effect.void,
				),
			);

		const ClaimApproved = (input: string) =>
			Decode(Identifier, input, "claim_approved").pipe(
				Effect.flatMap((mutation_id) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const mutation = yield* ReadMutationTransaction(
									transaction,
									mutation_id,
								);

								if (mutation.row.lifecycle !== "approved") {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "dispatch_conflict" }),
									);
								}

								yield* EnsureLiveThread(transaction, mutation.row.thread_id);
								const [busy] = yield* transaction
									.select({ mutation_id: GitMutationOperations.mutation_id })
									.from(GitMutationOperations)
									.where(
										and(
											eq(
												GitMutationOperations.workspace_id,
												mutation.row.workspace_id,
											),
											eq(GitMutationOperations.lifecycle, "dispatching"),
										),
									)
									.limit(1);

								if (busy !== undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "workspace_busy" }),
									);
								}

								const updated_at = yield* metadata.Now;
								const dispatch_lease_expires_at = new Date(
									Date.parse(updated_at) + git_dispatch_lease_milliseconds,
								).toISOString();
								const [updated] = yield* transaction
									.update(GitMutationOperations)
									.set({
										dispatched_at: updated_at,
										dispatch_lease_expires_at,
										dispatch_owner_id: metadata.instance_id,
										lifecycle: "dispatching",
										updated_at,
									})
									.where(
										and(
											eq(GitMutationOperations.mutation_id, mutation_id),
											eq(GitMutationOperations.lifecycle, "approved"),
										),
									)
									.returning();

								if (updated === undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "dispatch_conflict" }),
									);
								}

								const correlation_id = `git_mutation:${mutation_id}:dispatch`;
								const event = yield* AppendEvent(transaction, {
									...(mutation.identity.agent_id === undefined
										? {}
										: { agent_id: mutation.identity.agent_id }),
									causation_id: mutation.row.decision_message_id!,
									correlation_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										DecodeMutationRow(
											{ ...updated, journal_sequence },
											true,
										).pipe(
											Effect.flatMap(({ projection }) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													mutation: projection,
													type: "git.mutation.updated",
												}),
											),
											Effect.mapError(() =>
												invariant("Git dispatch event is invalid"),
											),
										),
									...(mutation.identity.raw_origin === undefined
										? {}
										: { raw_origin: mutation.identity.raw_origin }),
									...(mutation.identity.run_id === undefined
										? {}
										: { run_id: mutation.identity.run_id }),
									thread_id: mutation.row.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: event.journal_sequence })
									.where(eq(GitMutationOperations.mutation_id, mutation_id));
								const committed = yield* ReadMutationTransaction(
									transaction,
									mutation_id,
								);

								return {
									event,
									mutation: committed.projection,
									status: "accepted" as const,
								};
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) => notifier.Publish(result.event.journal_sequence)),
			);

		const CommitTerminal = (input: GitMutationTerminalInput) =>
			Decode(GitMutationTerminalInput, input, "commit_terminal").pipe(
				Effect.flatMap((terminal) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const mutation = yield* ReadMutationTransaction(
									transaction,
									terminal.mutation_id,
								);

								if (mutation.row.lifecycle === terminal.state) {
									if (
										JSON.stringify(mutation.projection.failure) !==
										JSON.stringify(terminal.failure)
									) {
										return yield* Effect.fail(
											new GitRepositoryConflict({
												reason: "terminal_conflict",
											}),
										);
									}

									return {
										event: yield* ReadEventBySequence(
											transaction,
											mutation.projection.journal_sequence,
										),
										mutation: mutation.projection,
										status: "duplicate" as const,
									};
								}

								if (mutation.row.lifecycle !== "dispatching") {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "terminal_conflict" }),
									);
								}

								const updated_at = yield* metadata.Now;
								const [updated] = yield* transaction
									.update(GitMutationOperations)
									.set({
										completed_at: updated_at,
										failure_code: JSON.stringify(terminal.failure),
										lifecycle: terminal.state,
										updated_at,
									})
									.where(
										and(
											eq(
												GitMutationOperations.mutation_id,
												terminal.mutation_id,
											),
											eq(GitMutationOperations.lifecycle, "dispatching"),
										),
									)
									.returning();

								if (updated === undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "terminal_conflict" }),
									);
								}

								const event = yield* AppendEvent(transaction, {
									...(mutation.identity.agent_id === undefined
										? {}
										: { agent_id: mutation.identity.agent_id }),
									causation_id: terminal.mutation_id,
									correlation_id: mutation.identity.request_message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										DecodeMutationRow(
											{ ...updated, journal_sequence },
											true,
										).pipe(
											Effect.flatMap(({ projection }) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													mutation: projection,
													type: "git.mutation.updated",
												}),
											),
											Effect.mapError(() =>
												invariant("Terminal Git event is invalid"),
											),
										),
									...(mutation.identity.raw_origin === undefined
										? {}
										: { raw_origin: mutation.identity.raw_origin }),
									...(mutation.identity.run_id === undefined
										? {}
										: { run_id: mutation.identity.run_id }),
									thread_id: mutation.identity.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: event.journal_sequence })
									.where(
										eq(GitMutationOperations.mutation_id, terminal.mutation_id),
									);
								const committed = yield* ReadMutationTransaction(
									transaction,
									terminal.mutation_id,
								);

								return {
									event,
									mutation: committed.projection,
									status: "accepted" as const,
								};
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.event.journal_sequence)
						: Effect.void,
				),
			);

		const CommitSucceeded = (input: GitMutationSucceededInput) =>
			Decode(GitMutationSucceededInput, input, "commit_succeeded").pipe(
				Effect.flatMap((success) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const mutation = yield* ReadMutationTransaction(
									transaction,
									success.mutation_id,
								);

								if (mutation.row.lifecycle === "succeeded") {
									const workspace = yield* ReadWorkspaceTransaction(
										transaction,
										mutation.row.workspace_id,
									);
									const candidate = yield* MakeWorkspaceProjection(
										success.workspace,
										workspace.version,
										workspace.journal_sequence,
									);

									if (
										mutation.result_snapshot_id !==
											success.workspace.snapshot_id ||
										mutation.row.result_workspace_version !==
											workspace.version ||
										JSON.stringify(candidate) !== JSON.stringify(workspace)
									) {
										return yield* Effect.fail(
											new GitRepositoryConflict({
												reason: "terminal_conflict",
											}),
										);
									}

									return {
										mutation: mutation.projection,
										mutation_event: yield* ReadEventBySequence(
											transaction,
											mutation.projection.journal_sequence,
										),
										status: "duplicate" as const,
										workspace,
										workspace_event: yield* ReadLastEvent(
											transaction,
											mutation.identity.request_message_id,
											"git.workspace.updated",
										),
									};
								}

								if (
									mutation.row.lifecycle !== "dispatching" ||
									success.workspace.workspace_id !== mutation.row.workspace_id
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "terminal_conflict" }),
									);
								}

								const current = yield* ReadWorkspaceTransaction(
									transaction,
									mutation.row.workspace_id,
								);

								if (
									current.snapshot_id !== mutation.row.expected_snapshot_id ||
									current.version !== mutation.row.expected_workspace_version
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "workspace_changed" }),
									);
								}

								const updated_at = yield* metadata.Now;
								const next_version = current.version + 1;
								const workspace_event = yield* AppendEvent(transaction, {
									...(mutation.identity.agent_id === undefined
										? {}
										: { agent_id: mutation.identity.agent_id }),
									causation_id: mutation.identity.mutation_id,
									correlation_id: mutation.identity.request_message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										MakeWorkspaceProjection(
											success.workspace,
											next_version,
											journal_sequence,
										).pipe(
											Effect.flatMap((workspace) =>
												Schema.decodeUnknownEffect(
													GitWorkspaceUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													cause: "mutation",
													type: "git.workspace.updated",
													workspace,
												}),
											),
											Effect.mapError(() =>
												invariant(
													"Succeeded Git workspace event is invalid",
												),
											),
										),
									...(mutation.identity.raw_origin === undefined
										? {}
										: { raw_origin: mutation.identity.raw_origin }),
									...(mutation.identity.run_id === undefined
										? {}
										: { run_id: mutation.identity.run_id }),
									thread_id: mutation.identity.thread_id,
								});
								const workspace = yield* MakeWorkspaceProjection(
									success.workspace,
									next_version,
									workspace_event.journal_sequence,
								);
								yield* WriteWorkspaceProjection(
									transaction,
									workspace,
									updated_at,
									current,
								);
								const [updated] = yield* transaction
									.update(GitMutationOperations)
									.set({
										completed_at: updated_at,
										lifecycle: "succeeded",
										result_snapshot_id: workspace.snapshot_id,
										result_workspace_version: workspace.version,
										updated_at,
									})
									.where(
										and(
											eq(
												GitMutationOperations.mutation_id,
												success.mutation_id,
											),
											eq(GitMutationOperations.lifecycle, "dispatching"),
										),
									)
									.returning();

								if (updated === undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "terminal_conflict" }),
									);
								}

								const mutation_event = yield* AppendEvent(transaction, {
									...(mutation.identity.agent_id === undefined
										? {}
										: { agent_id: mutation.identity.agent_id }),
									causation_id: mutation.identity.mutation_id,
									correlation_id: mutation.identity.request_message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										DecodeMutationRow(
											{ ...updated, journal_sequence },
											true,
										).pipe(
											Effect.flatMap(({ projection }) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													mutation: projection,
													type: "git.mutation.updated",
												}),
											),
											Effect.mapError(() =>
												invariant(
													"Succeeded Git mutation event is invalid",
												),
											),
										),
									...(mutation.identity.raw_origin === undefined
										? {}
										: { raw_origin: mutation.identity.raw_origin }),
									...(mutation.identity.run_id === undefined
										? {}
										: { run_id: mutation.identity.run_id }),
									thread_id: mutation.identity.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: mutation_event.journal_sequence })
									.where(
										eq(GitMutationOperations.mutation_id, success.mutation_id),
									);
								const committed = yield* ReadMutationTransaction(
									transaction,
									success.mutation_id,
								);

								return {
									mutation: committed.projection,
									mutation_event,
									status: "accepted" as const,
									workspace,
									workspace_event,
								};
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.mutation_event.journal_sequence)
						: Effect.void,
				),
			);

		const RecordWorkspace = (input: GitWorkspaceRecordInput) =>
			Decode(GitWorkspaceRecordInput, input, "record_workspace").pipe(
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							RecordWorkspaceTransaction(transaction, decoded),
						),
					),
				),
				Effect.mapError(normalize_error),
				Effect.tap((result) =>
					result.status === "accepted"
						? notifier.Publish(result.event.journal_sequence)
						: Effect.void,
				),
			);

		const ReadWorkspace = (input: string) =>
			Decode(Identifier, input, "read_workspace").pipe(
				Effect.flatMap((workspace_id) =>
					ReadWorkspaceTransaction(database.client, workspace_id),
				),
				Effect.mapError(normalize_error),
			);

		const ReadMutation = (input: string) =>
			Decode(Identifier, input, "read_mutation").pipe(
				Effect.flatMap((mutation_id) =>
					ReadMutationTransaction(database.client, mutation_id).pipe(
						Effect.map(({ projection }) => projection),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListPending = (input?: string) =>
			Effect.gen(function* () {
				const workspace_id =
					input === undefined
						? undefined
						: yield* Decode(Identifier, input, "list_pending");
				{
					const states = [
						"awaiting_approval",
						"approved",
						"dispatching",
						"ambiguous",
					] as const;
					const query =
						workspace_id === undefined
							? database.client
									.select()
									.from(GitMutationOperations)
									.where(inArray(GitMutationOperations.lifecycle, states))
							: database.client
									.select()
									.from(GitMutationOperations)
									.where(
										and(
											eq(GitMutationOperations.workspace_id, workspace_id),
											inArray(GitMutationOperations.lifecycle, states),
										),
									);

					return yield* query
						.orderBy(
							asc(GitMutationOperations.requested_at),
							asc(GitMutationOperations.mutation_id),
						)
						.pipe(
							Effect.flatMap((rows) =>
								Effect.forEach(rows, (row) =>
									DecodeMutationRow(row).pipe(
										Effect.map(({ projection }) => projection),
									),
								),
							),
							Effect.flatMap(
								Schema.decodeUnknownEffect(
									Schema.Array(GitMutationProjection).check(
										Schema.isMaxLength(git_workspace_maximum_pending_mutations),
									),
									{ onExcessProperty: "error" },
								),
							),
							Effect.mapError(() =>
								invariant("Pending Git mutation list is invalid"),
							),
						);
				}
			}).pipe(Effect.mapError(normalize_error));

		const RecoverDispatching = () =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const recovery_at = yield* metadata.Now;
						const rows = yield* transaction
							.select()
							.from(GitMutationOperations)
							.where(
								and(
									eq(GitMutationOperations.lifecycle, "dispatching"),
									or(
										isNull(GitMutationOperations.dispatch_lease_expires_at),
										lte(
											GitMutationOperations.dispatch_lease_expires_at,
											recovery_at,
										),
									),
								),
							)
							.orderBy(
								asc(GitMutationOperations.requested_at),
								asc(GitMutationOperations.mutation_id),
							);
						const ambiguous = yield* Effect.forEach(rows, (row) =>
							Effect.gen(function* () {
								const mutation = yield* DecodeMutationRow(row);
								yield* EnsureLiveThread(transaction, mutation.row.thread_id);
								const failure = { code: "git_dispatch_recovery" } as const;
								const updated_at = recovery_at;
								const [updated] = yield* transaction
									.update(GitMutationOperations)
									.set({
										completed_at: updated_at,
										failure_code: JSON.stringify(failure),
										lifecycle: "ambiguous",
										updated_at,
									})
									.where(
										and(
											eq(
												GitMutationOperations.mutation_id,
												mutation.row.mutation_id,
											),
											eq(GitMutationOperations.lifecycle, "dispatching"),
										),
									)
									.returning();

								if (updated === undefined) {
									return yield* Effect.fail(
										new GitRepositoryConflict({ reason: "terminal_conflict" }),
									);
								}

								const event = yield* AppendEvent(transaction, {
									...(mutation.identity.agent_id === undefined
										? {}
										: { agent_id: mutation.identity.agent_id }),
									causation_id: mutation.identity.mutation_id,
									correlation_id: mutation.identity.request_message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										DecodeMutationRow(
											{ ...updated, journal_sequence },
											true,
										).pipe(
											Effect.flatMap(({ projection }) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({
													mutation: projection,
													type: "git.mutation.updated",
												}),
											),
											Effect.mapError(() =>
												invariant("Recovered Git event is invalid"),
											),
										),
									...(mutation.identity.raw_origin === undefined
										? {}
										: { raw_origin: mutation.identity.raw_origin }),
									...(mutation.identity.run_id === undefined
										? {}
										: { run_id: mutation.identity.run_id }),
									thread_id: mutation.identity.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: event.journal_sequence })
									.where(
										eq(
											GitMutationOperations.mutation_id,
											mutation.row.mutation_id,
										),
									);

								return (yield* ReadMutationTransaction(
									transaction,
									mutation.row.mutation_id,
								)).projection;
							}),
						);
						const approved_rows = yield* transaction
							.select()
							.from(GitMutationOperations)
							.where(eq(GitMutationOperations.lifecycle, "approved"))
							.orderBy(
								asc(GitMutationOperations.requested_at),
								asc(GitMutationOperations.mutation_id),
							);
						const approved = yield* Effect.forEach(approved_rows, (row) =>
							DecodeMutationRow(row).pipe(Effect.map(({ projection }) => projection)),
						);

						return { ambiguous, approved };
					}),
				),
			).pipe(
				Effect.mapError(normalize_error),
				Effect.tap((result) => {
					const journal_sequence = result.ambiguous.at(-1)?.journal_sequence;

					return journal_sequence === undefined
						? Effect.void
						: notifier.Publish(journal_sequence);
				}),
			);

		return {
			ClaimApproved,
			CommitSucceeded,
			CommitTerminal,
			ListPending,
			ReadMutation,
			ReadWorkspace,
			RecordWorkspace,
			RecoverDispatching,
			RequestMutation,
			ResolveMutation,
		};
	}),
);
