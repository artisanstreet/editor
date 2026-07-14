import { asc, desc, eq, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	EventEnvelope,
	GitBranchName,
	GitDiffStats,
	GitObjectId,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceGitChangedFile,
	WorkspaceGitSession,
	WorkspaceGitSessionBlocker,
	WorkspaceGitSessionQuery,
	WorkspaceGitSessionQueryResult,
	WorkspaceGitSessionState,
	WorkspaceGitWorktree,
	type EventEnvelope as EventEnvelopeValue,
	type WorkspaceGitSession as WorkspaceGitSessionValue,
	type WorkspaceGitSessionQueryResult as WorkspaceGitSessionQueryResultValue,
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
	WorkspaceGitChangedFiles,
	WorkspaceGitOperations,
	WorkspaceGitSessions,
	WorkspaceGitWorktrees,
} from "../persistence/schema";
import { JournalStoreFailure } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));
const PrivatePath = Schema.String.check(
	Schema.makeFilter<string>((path) =>
		path.length === 0 || path.includes(String.fromCharCode(0))
			? "Expected a non-empty private path without null bytes"
			: undefined,
	),
);

const SourceCommandMetadata = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	message_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	sent_at: IsoDateTime,
});

const ProjectedSessionFacts = Schema.Struct({
	blockers: Schema.Array(WorkspaceGitSessionBlocker),
	branch: Schema.optional(GitBranchName),
	changed_files: Schema.Array(WorkspaceGitChangedFile),
	diff_stats: GitDiffStats,
	has_diff: Schema.Boolean,
	head: Schema.optional(GitObjectId),
	state: WorkspaceGitSessionState,
});

const PrivateWorktree = Schema.Struct({
	adapter_path: PrivatePath,
	worktree: WorkspaceGitWorktree,
});

const ProjectObservation = Schema.Struct({
	kind: Schema.Literals(["refresh", "checkout", "recovery", "mutation"]),
	observed_at: IsoDateTime,
	operation_id: Identifier,
	repository_root: Schema.optional(PrivatePath),
	request_fingerprint: RequestFingerprint,
	selected_worktree_path: Schema.optional(PrivatePath),
	session: ProjectedSessionFacts,
	source_command: Schema.optional(SourceCommandMetadata),
	thread_id: Identifier,
	workspace_id: Identifier,
	worktrees: Schema.Array(PrivateWorktree),
});

const ReplayProjection = Schema.Struct({
	kind: Schema.Literals(["checkout", "recovery", "mutation"]),
	operation_id: Identifier,
	request_fingerprint: RequestFingerprint,
	thread_id: Identifier,
	workspace_id: Identifier,
});

/** Supplies one complete private observation and its source-free public projection. */
export type ProjectObservation = typeof ProjectObservation.Type;

const PendingEvidence = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	branch: Schema.optional(GitBranchName),
	changed_file_count: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	has_diff: Schema.Boolean,
	operation_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	root_path: PrivatePath,
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
	worktree_path: PrivatePath,
});

/** Carries the exact pending input for `git.workspace.observed` evidence. */
export type PendingWorkspaceGitEvidence = typeof PendingEvidence.Type;

/** Returns the durable session event accepted or replayed by one projection operation. */
export interface WorkspaceGitSessionAcceptance {
	readonly event: EventEnvelopeValue;
	readonly session: WorkspaceGitSessionValue;
	readonly status: "accepted" | "duplicate";
}

/** Returns whether an evidence checkpoint was newly settled or already settled. */
export interface WorkspaceGitEvidenceSettlement {
	readonly status: "accepted" | "duplicate";
}

/** Reports immutable operation or source-command reuse with different intent. */
export class WorkspaceGitSessionConflict extends Data.TaggedError("WorkspaceGitSessionConflict")<{
	readonly reason: "operation_conflict" | "source_command_conflict";
}> {}

/** Reports an operation that is missing or belongs to erased thread content. */
export class WorkspaceGitSessionUnavailable extends Data.TaggedError(
	"WorkspaceGitSessionUnavailable",
)<{ readonly reason: "erased" | "missing" }> {}

/** Conceals corrupt persisted Git-session state behind one typed invariant failure. */
export class WorkspaceGitSessionInvariant extends Data.TaggedError("WorkspaceGitSessionInvariant")<{
	readonly message: string;
}> {}

/** Represents failures surfaced by the durable Git-session repository. */
export type WorkspaceGitSessionRepositoryError =
	| JournalStoreFailure
	| WorkspaceGitSessionConflict
	| WorkspaceGitSessionInvariant
	| WorkspaceGitSessionUnavailable;

/** Owns durable Git observations, public projections, and pending evidence checkpoints. */
export class WorkspaceGitSessionRepository extends Context.Service<
	WorkspaceGitSessionRepository,
	{
		readonly ListPendingEvidence: Effect.Effect<
			ReadonlyArray<PendingWorkspaceGitEvidence>,
			WorkspaceGitSessionRepositoryError
		>;
		readonly MarkEvidenceRecorded: (
			operation_id: string,
		) => Effect.Effect<WorkspaceGitEvidenceSettlement, WorkspaceGitSessionRepositoryError>;
		readonly Project: (
			input: ProjectObservation,
		) => Effect.Effect<WorkspaceGitSessionAcceptance, WorkspaceGitSessionRepositoryError>;
		readonly Query: (
			query: typeof WorkspaceGitSessionQuery.Type,
		) => Effect.Effect<WorkspaceGitSessionQueryResultValue, WorkspaceGitSessionRepositoryError>;
		readonly Replay: (
			input: typeof ReplayProjection.Type,
		) => Effect.Effect<
			Option.Option<WorkspaceGitSessionAcceptance>,
			WorkspaceGitSessionRepositoryError
		>;
	}
>()("Artisan/WorkspaceGitSessionRepository") {}

type OperationRow = typeof WorkspaceGitOperations.$inferSelect;
type SessionRow = typeof WorkspaceGitSessions.$inferSelect;

function invariant(message: string) {
	return new WorkspaceGitSessionInvariant({ message });
}

function normalize_error(error: unknown): WorkspaceGitSessionRepositoryError {
	if (
		error instanceof WorkspaceGitSessionConflict ||
		error instanceof WorkspaceGitSessionInvariant ||
		error instanceof WorkspaceGitSessionUnavailable
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function session_event_key(operation_id: string) {
	return `workspace_git_session:${operation_id}`;
}

function refresh_command_payload(input: ProjectObservation) {
	return JSON.stringify({
		kind: input.kind,
		operation_id: input.operation_id,
		request_fingerprint: input.request_fingerprint,
		type: "workspace.git.session.refresh",
		workspace_id: input.workspace_id,
	});
}

function source_commands_match(
	row: typeof JournalCommands.$inferSelect,
	input: ProjectObservation,
) {
	const source = input.source_command;

	return (
		source !== undefined &&
		row.message_id === source.message_id &&
		row.schema_version === 1 &&
		row.thread_id === input.thread_id &&
		row.run_id === (source.run_id ?? null) &&
		row.agent_id === (source.agent_id ?? null) &&
		row.causation_id === (source.causation_id ?? null) &&
		row.origin === "frontend" &&
		row.raw_origin_json ===
			(source.raw_origin === undefined ? null : JSON.stringify(source.raw_origin)) &&
		row.sent_at === source.sent_at &&
		row.payload_type === "workspace.git.session.refresh" &&
		row.payload_json === refresh_command_payload(input) &&
		row.status === "accepted"
	);
}

/** Supplies the SQLite-backed Git-session repository. */
export const WorkspaceGitSessionRepositoryLive = Layer.effect(
	WorkspaceGitSessionRepository,
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
					return yield* new WorkspaceGitSessionUnavailable({ reason: "erased" });
				}
			});

		const DecodeEventRow = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.mapError(() => invariant("Stored Git-session event payload is corrupt")),
				);
				const raw_origin =
					row.raw_origin_json === null
						? undefined
						: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.raw_origin_json,
							).pipe(
								Effect.mapError(() =>
									invariant("Stored Git-session event attribution is corrupt"),
								),
							);

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
					payload,
					protocol_version: 1,
					raw_origin,
					run_id: row.run_id ?? undefined,
					schema_version: row.schema_version,
					sent_at: row.occurred_at,
					sequence: row.stream_sequence,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				}).pipe(Effect.mapError(() => invariant("Stored Git-session event is corrupt")));
			});

		const ReadEvent = (transaction: typeof database.client, operation_id: string) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.idempotency_key, session_event_key(operation_id)))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeEventRow(row)
							: Effect.fail(invariant("Git-session operation event is missing")),
					),
				);

		const DecodeSession = (transaction: typeof database.client, row: SessionRow) =>
			Effect.gen(function* () {
				const blockers = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.blockers_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(Schema.Array(WorkspaceGitSessionBlocker), {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() => invariant("Stored Git-session blockers are corrupt")),
				);
				const worktree_rows = yield* transaction
					.select()
					.from(WorkspaceGitWorktrees)
					.where(eq(WorkspaceGitWorktrees.workspace_id, row.workspace_id))
					.orderBy(asc(WorkspaceGitWorktrees.ordinal));
				const changed_rows = yield* transaction
					.select()
					.from(WorkspaceGitChangedFiles)
					.where(eq(WorkspaceGitChangedFiles.workspace_id, row.workspace_id))
					.orderBy(asc(WorkspaceGitChangedFiles.path));

				if (
					worktree_rows.some(
						(worktree, ordinal) =>
							worktree.ordinal !== ordinal || worktree.version !== row.version,
					) ||
					changed_rows.some((changed) => changed.version !== row.version)
				) {
					return yield* invariant("Stored Git-session child projections are corrupt");
				}

				return yield* Schema.decodeUnknownEffect(WorkspaceGitSession, {
					onExcessProperty: "error",
				})({
					blockers,
					branch: row.branch ?? undefined,
					changed_files: changed_rows.map((changed) => ({
						conflicted: changed.conflicted,
						original_path: changed.original_path ?? undefined,
						path: changed.path,
						staged: changed.staged,
						status: changed.status,
						untracked: changed.untracked,
						unstaged: changed.unstaged,
					})),
					diff_stats: {
						additions: row.additions,
						deletions: row.deletions,
						files: row.files,
					},
					has_diff: row.has_diff,
					head: row.head ?? undefined,
					journal_sequence: row.journal_sequence,
					observed_at: row.observed_at,
					state: row.state,
					version: row.version,
					worktrees: worktree_rows.map((worktree) => ({
						bare: worktree.bare,
						branch: worktree.branch ?? undefined,
						detached: worktree.detached,
						head: worktree.head ?? undefined,
						locked: worktree.locked,
						location: worktree.location,
						prunable: worktree.prunable,
					})),
					workspace_id: row.workspace_id,
				}).pipe(
					Effect.mapError(() => invariant("Stored Git-session projection is corrupt")),
				);
			});

		const ReadSessionByWorkspace = (
			transaction: typeof database.client,
			workspace_id: string,
		) =>
			transaction
				.select()
				.from(WorkspaceGitSessions)
				.where(eq(WorkspaceGitSessions.workspace_id, workspace_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeSession(transaction, row).pipe(
									Effect.map((session) => ({ row, session })),
								)
							: Effect.succeed(undefined),
					),
				);

		const ValidateObservation = (input: ProjectObservation) =>
			Effect.gen(function* () {
				const decoded = yield* Schema.decodeUnknownEffect(ProjectObservation, {
					onExcessProperty: "error",
				})(input).pipe(
					Effect.mapError(
						() => new WorkspaceGitSessionConflict({ reason: "operation_conflict" }),
					),
				);
				const selected = decoded.worktrees.filter(
					(worktree) => worktree.worktree.location === "selected",
				);
				const unavailable = decoded.session.state === "unavailable";

				if (
					(decoded.kind === "refresh") !== (decoded.source_command !== undefined) ||
					unavailable !==
						(decoded.repository_root === undefined &&
							decoded.selected_worktree_path === undefined) ||
					(!unavailable &&
						(decoded.repository_root === undefined ||
							decoded.selected_worktree_path === undefined ||
							selected.length !== 1 ||
							selected[0]!.adapter_path !== decoded.selected_worktree_path))
				) {
					return yield* new WorkspaceGitSessionConflict({ reason: "operation_conflict" });
				}

				return decoded;
			});

		const ValidateReplay = (
			transaction: typeof database.client,
			operation: OperationRow,
			input: ProjectObservation,
		) =>
			Effect.gen(function* () {
				const event = yield* ReadEvent(transaction, operation.operation_id);

				if (event.payload.type !== "workspace.git.session.updated") {
					return yield* invariant("Git-session operation event has the wrong payload");
				}

				const session = event.payload.session;
				const source = input.source_command;
				const [command] =
					source === undefined
						? [undefined]
						: yield* transaction
								.select()
								.from(JournalCommands)
								.where(eq(JournalCommands.message_id, source.message_id))
								.limit(1);
				const public_input = {
					...input.session,
					journal_sequence: session.journal_sequence,
					observed_at: input.observed_at,
					version: session.version,
					workspace_id: input.workspace_id,
					worktrees: input.worktrees.map((worktree) => worktree.worktree),
				};
				const canonical_input = yield* Schema.decodeUnknownEffect(WorkspaceGitSession, {
					onExcessProperty: "error",
				})(public_input).pipe(
					Effect.mapError(
						() => new WorkspaceGitSessionConflict({ reason: "operation_conflict" }),
					),
				);
				const evidence_recorded =
					input.session.state === "unavailable" ||
					input.session.blockers.includes("not_repository");
				const evidence_matches_input =
					operation.evidence_recorded === evidence_recorded &&
					operation.evidence_root_path ===
						(evidence_recorded ? null : (input.repository_root ?? null)) &&
					operation.evidence_worktree_path ===
						(evidence_recorded ? null : (input.selected_worktree_path ?? null)) &&
					operation.evidence_branch === (input.session.branch ?? null) &&
					operation.evidence_changed_file_count === input.session.changed_files.length &&
					operation.evidence_has_diff === input.session.has_diff;
				const recorded_evidence_is_cleared =
					operation.evidence_recorded &&
					operation.evidence_root_path === null &&
					operation.evidence_worktree_path === null &&
					operation.evidence_branch === null &&
					operation.evidence_changed_file_count === null &&
					operation.evidence_has_diff === null;

				if (
					operation.operation_id !== input.operation_id ||
					operation.source_command_id !== (source?.message_id ?? null) ||
					operation.request_fingerprint !== input.request_fingerprint ||
					operation.kind !== input.kind ||
					operation.thread_id !== input.thread_id ||
					operation.workspace_id !== input.workspace_id ||
					operation.session_version !== session.version ||
					operation.journal_sequence !== session.journal_sequence ||
					(!evidence_matches_input && !recorded_evidence_is_cleared) ||
					operation.sent_at !== (source?.sent_at ?? input.observed_at) ||
					JSON.stringify(session) !== JSON.stringify(canonical_input) ||
					(source === undefined
						? command !== undefined
						: !command || !source_commands_match(command, input)) ||
					event.agent_id !== source?.agent_id ||
					event.causation_id !== (source?.message_id ?? input.operation_id) ||
					event.correlation_id !== input.operation_id ||
					event.origin !== "backend" ||
					event.run_id !== source?.run_id ||
					event.sent_at !== input.observed_at ||
					event.stream_id !== `thread:${input.thread_id}` ||
					event.thread_id !== input.thread_id ||
					JSON.stringify(event.raw_origin) !== JSON.stringify(source?.raw_origin)
				) {
					return yield* new WorkspaceGitSessionConflict({ reason: "operation_conflict" });
				}

				return { event, session, status: "duplicate" as const };
			});

		const AppendEvent = (
			transaction: typeof database.client,
			input: ProjectObservation,
			session: WorkspaceGitSessionValue,
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
				const source = input.source_command;
				const payload = { session, type: "workspace.git.session.updated" } as const;

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
						agent_id: source?.agent_id ?? null,
						causation_id: source?.message_id ?? input.operation_id,
						correlation_id: input.operation_id,
						event_id,
						event_type: payload.type,
						idempotency_key: session_event_key(input.operation_id),
						occurred_at: input.observed_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						raw_origin_json:
							source?.raw_origin === undefined
								? null
								: JSON.stringify(source.raw_origin),
						run_id: source?.run_id ?? null,
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: input.thread_id,
					})
					.returning();

				if (!row) {
					return yield* invariant("Git-session event was not persisted");
				}

				return row;
			});

		const Project = (input: ProjectObservation) =>
			ValidateObservation(input).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									yield* EnsureLiveThread(transaction, decoded.thread_id);

									const source_command_id = decoded.source_command?.message_id;
									const existing = yield* transaction
										.select()
										.from(WorkspaceGitOperations)
										.where(
											source_command_id === undefined
												? eq(
														WorkspaceGitOperations.operation_id,
														decoded.operation_id,
													)
												: or(
														eq(
															WorkspaceGitOperations.operation_id,
															decoded.operation_id,
														),
														eq(
															WorkspaceGitOperations.source_command_id,
															source_command_id,
														),
													),
										)
										.limit(2);

									if (existing.length > 1) {
										return yield* new WorkspaceGitSessionConflict({
											reason: "source_command_conflict",
										});
									}

									if (existing[0]) {
										if (existing[0].operation_id !== decoded.operation_id) {
											return yield* new WorkspaceGitSessionConflict({
												reason: "source_command_conflict",
											});
										}

										return yield* ValidateReplay(
											transaction,
											existing[0],
											decoded,
										);
									}

									if (source_command_id !== undefined) {
										const [command] = yield* transaction
											.select()
											.from(JournalCommands)
											.where(
												eq(JournalCommands.message_id, source_command_id),
											)
											.limit(1);

										if (command) {
											return yield* new WorkspaceGitSessionConflict({
												reason: "source_command_conflict",
											});
										}
									}

									const previous = yield* ReadSessionByWorkspace(
										transaction,
										decoded.workspace_id,
									);
									const version = (previous?.session.version ?? 0) + 1;
									const provisional: WorkspaceGitSessionValue = {
										...decoded.session,
										journal_sequence: 0,
										observed_at: decoded.observed_at,
										version,
										workspace_id: decoded.workspace_id,
										worktrees: decoded.worktrees.map(
											(worktree) => worktree.worktree,
										),
									};
									const event_row = yield* AppendEvent(
										transaction,
										decoded,
										provisional,
									);
									const session: WorkspaceGitSessionValue = {
										...provisional,
										journal_sequence: event_row.sequence,
									};
									const payload = {
										session,
										type: "workspace.git.session.updated",
									} as const;

									yield* transaction
										.update(JournalEvents)
										.set({ payload_json: JSON.stringify(payload) })
										.where(eq(JournalEvents.sequence, event_row.sequence));

									if (decoded.source_command) {
										const accepted_at = yield* metadata.Now;

										yield* transaction.insert(JournalCommands).values({
											accepted_at,
											agent_id: decoded.source_command.agent_id ?? null,
											causation_id:
												decoded.source_command.causation_id ?? null,
											message_id: decoded.source_command.message_id,
											origin: "frontend",
											payload_json: refresh_command_payload(decoded),
											payload_type: "workspace.git.session.refresh",
											raw_origin_json:
												decoded.source_command.raw_origin === undefined
													? null
													: JSON.stringify(
															decoded.source_command.raw_origin,
														),
											run_id: decoded.source_command.run_id ?? null,
											schema_version: 1,
											sent_at: decoded.source_command.sent_at,
											status: "accepted",
											thread_id: decoded.thread_id,
										});
									}

									yield* transaction
										.insert(WorkspaceGitSessions)
										.values({
											additions: session.diff_stats.additions,
											blockers_json: JSON.stringify(session.blockers),
											branch: session.branch ?? null,
											deletions: session.diff_stats.deletions,
											files: session.diff_stats.files,
											has_diff: session.has_diff,
											head: session.head ?? null,
											journal_sequence: session.journal_sequence,
											observed_at: session.observed_at,
											repository_root: decoded.repository_root ?? null,
											selected_worktree_path:
												decoded.selected_worktree_path ?? null,
											state: session.state,
											updated_at: decoded.observed_at,
											version: session.version,
											workspace_id: session.workspace_id,
										})
										.onConflictDoUpdate({
											set: {
												additions: session.diff_stats.additions,
												blockers_json: JSON.stringify(session.blockers),
												branch: session.branch ?? null,
												deletions: session.diff_stats.deletions,
												files: session.diff_stats.files,
												has_diff: session.has_diff,
												head: session.head ?? null,
												journal_sequence: session.journal_sequence,
												observed_at: session.observed_at,
												repository_root: decoded.repository_root ?? null,
												selected_worktree_path:
													decoded.selected_worktree_path ?? null,
												state: session.state,
												updated_at: decoded.observed_at,
												version: session.version,
											},
											target: WorkspaceGitSessions.workspace_id,
										});
									yield* transaction
										.delete(WorkspaceGitWorktrees)
										.where(
											eq(
												WorkspaceGitWorktrees.workspace_id,
												decoded.workspace_id,
											),
										);
									yield* transaction
										.delete(WorkspaceGitChangedFiles)
										.where(
											eq(
												WorkspaceGitChangedFiles.workspace_id,
												decoded.workspace_id,
											),
										);

									if (decoded.worktrees.length > 0) {
										yield* transaction.insert(WorkspaceGitWorktrees).values(
											decoded.worktrees.map((worktree, ordinal) => ({
												adapter_path: worktree.adapter_path,
												bare: worktree.worktree.bare,
												branch: worktree.worktree.branch ?? null,
												detached: worktree.worktree.detached,
												head: worktree.worktree.head ?? null,
												location: worktree.worktree.location,
												locked: worktree.worktree.locked,
												ordinal,
												prunable: worktree.worktree.prunable,
												version,
												workspace_id: decoded.workspace_id,
											})),
										);
									}

									if (session.changed_files.length > 0) {
										yield* transaction.insert(WorkspaceGitChangedFiles).values(
											session.changed_files.map((file) => ({
												conflicted: file.conflicted,
												original_path: file.original_path ?? null,
												path: file.path,
												staged: file.staged,
												status: file.status,
												untracked: file.untracked,
												unstaged: file.unstaged,
												version,
												workspace_id: decoded.workspace_id,
											})),
										);
									}

									const evidence_recorded =
										session.state === "unavailable" ||
										session.blockers.includes("not_repository");
									yield* transaction.insert(WorkspaceGitOperations).values({
										created_at: decoded.observed_at,
										evidence_branch: session.branch ?? null,
										evidence_changed_file_count: session.changed_files.length,
										evidence_has_diff: session.has_diff,
										evidence_recorded,
										evidence_root_path: evidence_recorded
											? null
											: decoded.repository_root,
										evidence_worktree_path: evidence_recorded
											? null
											: decoded.selected_worktree_path,
										journal_sequence: session.journal_sequence,
										kind: decoded.kind,
										operation_id: decoded.operation_id,
										request_fingerprint: decoded.request_fingerprint,
										sent_at:
											decoded.source_command?.sent_at ?? decoded.observed_at,
										session_version: session.version,
										source_command_id:
											decoded.source_command?.message_id ?? null,
										thread_id: decoded.thread_id,
										updated_at: decoded.observed_at,
										workspace_id: decoded.workspace_id,
									});

									const event = yield* ReadEvent(
										transaction,
										decoded.operation_id,
									);

									return { event, session, status: "accepted" as const };
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

		const CurrentJournalSequence = (transaction: typeof database.client) =>
			transaction
				.select({ sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1)
				.pipe(Effect.map(([row]) => row?.sequence ?? 0));

		const Query = (query: typeof WorkspaceGitSessionQuery.Type) =>
			Schema.decodeUnknownEffect(WorkspaceGitSessionQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(() => new WorkspaceGitSessionUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const stored = yield* ReadSessionByWorkspace(
								transaction,
								decoded.workspace_id,
							);
							const journal_sequence = yield* CurrentJournalSequence(transaction);
							const result = stored
								? { journal_sequence, session: stored.session }
								: { journal_sequence };

							return yield* Schema.decodeUnknownEffect(
								WorkspaceGitSessionQueryResult,
								{
									onExcessProperty: "error",
								},
							)(result).pipe(
								Effect.mapError(() =>
									invariant("Git-session query result is corrupt"),
								),
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListPendingEvidence = database.client
			.transaction((transaction) =>
				transaction
					.select({
						agent_id: JournalCommands.agent_id,
						branch: WorkspaceGitOperations.evidence_branch,
						changed_file_count: WorkspaceGitOperations.evidence_changed_file_count,
						has_diff: WorkspaceGitOperations.evidence_has_diff,
						operation_id: WorkspaceGitOperations.operation_id,
						raw_origin_json: JournalCommands.raw_origin_json,
						root_path: WorkspaceGitOperations.evidence_root_path,
						run_id: JournalCommands.run_id,
						thread_id: WorkspaceGitOperations.thread_id,
						worktree_path: WorkspaceGitOperations.evidence_worktree_path,
					})
					.from(WorkspaceGitOperations)
					.leftJoin(
						JournalCommands,
						eq(JournalCommands.message_id, WorkspaceGitOperations.source_command_id),
					)
					.where(eq(WorkspaceGitOperations.evidence_recorded, false))
					.orderBy(
						asc(WorkspaceGitOperations.created_at),
						asc(WorkspaceGitOperations.operation_id),
					)
					.pipe(
						Effect.flatMap((rows) =>
							Effect.forEach(rows, (row) =>
								Effect.gen(function* () {
									yield* EnsureLiveThread(transaction, row.thread_id);

									const raw_origin =
										row.raw_origin_json === null
											? undefined
											: yield* Schema.decodeUnknownEffect(
													Schema.UnknownFromJsonString,
												)(row.raw_origin_json).pipe(
													Effect.flatMap(
														Schema.decodeUnknownEffect(RawOrigin, {
															onExcessProperty: "error",
														}),
													),
													Effect.mapError(() =>
														invariant(
															"Pending Git evidence is corrupt",
														),
													),
												);

									return yield* Schema.decodeUnknownEffect(PendingEvidence, {
										onExcessProperty: "error",
									})({
										agent_id: row.agent_id ?? undefined,
										branch: row.branch ?? undefined,
										changed_file_count: row.changed_file_count,
										has_diff: row.has_diff,
										operation_id: row.operation_id,
										raw_origin,
										root_path: row.root_path,
										run_id: row.run_id ?? undefined,
										thread_id: row.thread_id,
										worktree_path: row.worktree_path,
									}).pipe(
										Effect.mapError(() =>
											invariant("Pending Git evidence is corrupt"),
										),
									);
								}),
							),
						),
					),
			)
			.pipe(Effect.mapError(normalize_error));

		const ValidateEvidenceCheckpoint = (row: OperationRow) =>
			Effect.gen(function* () {
				const cleared =
					row.evidence_root_path === null &&
					row.evidence_worktree_path === null &&
					row.evidence_branch === null &&
					row.evidence_changed_file_count === null &&
					row.evidence_has_diff === null;

				if (row.evidence_recorded) {
					const retained =
						row.evidence_root_path === null &&
						row.evidence_worktree_path === null &&
						row.evidence_changed_file_count !== null &&
						row.evidence_has_diff !== null;

					if (!cleared && !retained) {
						return yield* invariant("Recorded Git evidence checkpoint is corrupt");
					}

					if (retained) {
						yield* Schema.decodeUnknownEffect(
							Schema.Struct({
								branch: Schema.optional(GitBranchName),
								changed_file_count: Schema.Int.check(
									Schema.isGreaterThanOrEqualTo(0),
								),
								has_diff: Schema.Boolean,
							}),
							{ onExcessProperty: "error" },
						)({
							branch: row.evidence_branch ?? undefined,
							changed_file_count: row.evidence_changed_file_count,
							has_diff: row.evidence_has_diff,
						}).pipe(
							Effect.mapError(() =>
								invariant("Recorded Git evidence checkpoint is corrupt"),
							),
						);
					}

					return;
				}

				yield* Schema.decodeUnknownEffect(PendingEvidence, {
					onExcessProperty: "error",
				})({
					branch: row.evidence_branch ?? undefined,
					changed_file_count: row.evidence_changed_file_count,
					has_diff: row.evidence_has_diff,
					operation_id: row.operation_id,
					root_path: row.evidence_root_path,
					thread_id: row.thread_id,
					worktree_path: row.evidence_worktree_path,
				}).pipe(
					Effect.mapError(() => invariant("Pending Git evidence checkpoint is corrupt")),
				);
			});
		const Replay = (input: typeof ReplayProjection.Type) =>
			Schema.decodeUnknownEffect(ReplayProjection, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(
					() => new WorkspaceGitSessionConflict({ reason: "operation_conflict" }),
				),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [operation] = yield* transaction
								.select()
								.from(WorkspaceGitOperations)
								.where(
									eq(WorkspaceGitOperations.operation_id, decoded.operation_id),
								)
								.limit(1);

							if (!operation) {
								const [orphaned_event] = yield* transaction
									.select({ sequence: JournalEvents.sequence })
									.from(JournalEvents)
									.where(
										eq(
											JournalEvents.idempotency_key,
											session_event_key(decoded.operation_id),
										),
									)
									.limit(1);

								if (orphaned_event) {
									return yield* invariant(
										"Git-session event has no projection operation",
									);
								}

								return Option.none<WorkspaceGitSessionAcceptance>();
							}

							yield* EnsureLiveThread(transaction, operation.thread_id);

							if (
								operation.kind !== decoded.kind ||
								operation.request_fingerprint !== decoded.request_fingerprint ||
								operation.thread_id !== decoded.thread_id ||
								operation.workspace_id !== decoded.workspace_id
							) {
								return yield* new WorkspaceGitSessionConflict({
									reason: "operation_conflict",
								});
							}

							if (operation.source_command_id !== null) {
								return yield* invariant(
									"Backend Git-session projection has a source command",
								);
							}

							yield* ValidateEvidenceCheckpoint(operation);

							const event = yield* ReadEvent(transaction, operation.operation_id);

							if (event.payload.type !== "workspace.git.session.updated") {
								return yield* invariant(
									"Git-session operation event has the wrong payload",
								);
							}

							const session = event.payload.session;
							const current = yield* ReadSessionByWorkspace(
								transaction,
								operation.workspace_id,
							);
							const evidence_is_cleared =
								operation.evidence_root_path === null &&
								operation.evidence_worktree_path === null &&
								operation.evidence_branch === null &&
								operation.evidence_changed_file_count === null &&
								operation.evidence_has_diff === null;
							const evidence_matches_session =
								operation.evidence_branch === (session.branch ?? null) &&
								operation.evidence_changed_file_count ===
									session.changed_files.length &&
								operation.evidence_has_diff === session.has_diff;

							if (
								current === undefined ||
								current.session.version < operation.session_version ||
								(current.session.version === operation.session_version &&
									JSON.stringify(current.session) !== JSON.stringify(session)) ||
								operation.session_version !== session.version ||
								operation.journal_sequence !== session.journal_sequence ||
								operation.sent_at !== session.observed_at ||
								operation.created_at !== session.observed_at ||
								operation.updated_at !== session.observed_at ||
								(!evidence_is_cleared && !evidence_matches_session) ||
								event.agent_id !== undefined ||
								event.causation_id !== operation.operation_id ||
								event.correlation_id !== operation.operation_id ||
								event.journal_sequence !== session.journal_sequence ||
								event.origin !== "backend" ||
								event.raw_origin !== undefined ||
								event.run_id !== undefined ||
								event.sent_at !== session.observed_at ||
								event.stream_id !== `thread:${operation.thread_id}` ||
								event.thread_id !== operation.thread_id ||
								session.workspace_id !== operation.workspace_id
							) {
								return yield* invariant(
									"Stored Git-session projection binding is corrupt",
								);
							}

							return Option.some({ event, session, status: "duplicate" as const });
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const MarkEvidenceRecorded = (operation_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(operation_id).pipe(
				Effect.mapError(() => new WorkspaceGitSessionUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const [row] = yield* transaction
									.select()
									.from(WorkspaceGitOperations)
									.where(eq(WorkspaceGitOperations.operation_id, decoded))
									.limit(1);

								if (!row) {
									return yield* new WorkspaceGitSessionUnavailable({
										reason: "missing",
									});
								}

								yield* EnsureLiveThread(transaction, row.thread_id);
								yield* ValidateEvidenceCheckpoint(row);

								if (row.evidence_recorded) {
									return { status: "duplicate" as const };
								}

								yield* transaction
									.update(WorkspaceGitOperations)
									.set({
										evidence_branch: null,
										evidence_changed_file_count: null,
										evidence_has_diff: null,
										evidence_recorded: true,
										evidence_root_path: null,
										evidence_worktree_path: null,
									})
									.where(eq(WorkspaceGitOperations.operation_id, decoded));

								return { status: "accepted" as const };
							}),
						),
					).pipe(Effect.mapError(normalize_error)),
				),
			);

		return {
			ListPendingEvidence,
			MarkEvidenceRecorded,
			Project,
			Query,
			Replay,
		};
	}),
);
