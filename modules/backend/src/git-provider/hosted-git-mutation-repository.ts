import { and, asc, eq, isNull, or } from "drizzle-orm";
import {
	Cause,
	Context,
	Crypto,
	Data,
	DateTime,
	Effect,
	Encoding,
	Exit,
	Layer,
	Option,
	Schema,
} from "effect";
import { isSqlError, type SqlError } from "effect/unstable/sql/SqlError";

import {
	EventEnvelope,
	HostedGitMutationApproval,
	HostedGitMutationApprovalQuery,
	HostedGitMutationApprovalQueryResult,
	HostedGitMutationApprovalUpdatedEvent,
	HostedGitMutationCommandRequest,
	HostedGitMutationSummary,
	HostedGitOrigin,
	HostedGitPullRequestLookup,
	HostedGitRepositoryIdentity,
	HostedProjectSelection,
	Identifier,
	IsoDateTime,
	ProjectRef,
	RawOrigin,
	summarize_hosted_git_mutation,
	type EventEnvelope as EventEnvelopeValue,
	type HostedGitMutationApproval as HostedGitMutationApprovalValue,
	type HostedGitMutationApprovalQueryResult as HostedGitMutationApprovalQueryResultValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { WorkspaceGitExecutionGate } from "../git/workspace-git-execution-gate";
import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalStoreFailure } from "../persistence/journal-store";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	HostedGitMutationApprovals,
	HostedGitMutationArtifacts,
	HostedGitMutationClaims,
	HostedGitSnapshots,
	JournalCommands,
	JournalEvents,
	ProjectHostedOrigins,
	Projects,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceGitSessions,
	WorkspaceGitCheckoutClaims,
	WorkspaceGitMutationClaims,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));

const CommandMetadata = Schema.Struct({
	message_id: Identifier,
	sent_at: IsoDateTime,
});

const MutationRequest = Schema.Struct({
	approval_id: Identifier,
	command: HostedGitMutationCommandRequest,
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	thread_id: Identifier,
});

const MutationDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	decision_command: CommandMetadata,
	thread_id: Identifier,
});

export const ClaimIdentity = Schema.Struct({
	approval_id: Identifier,
	claim_token: Identifier,
});

const StoredRequestPayload = Schema.Struct({
	approval_id: Identifier,
	request_fingerprint: RequestFingerprint,
	selection: HostedProjectSelection,
	summary: HostedGitMutationSummary,
	type: Schema.Literal("hosted.git.mutation.request"),
});

const StoredDecisionPayload = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	type: Schema.Literal("hosted.git.mutation.approval.respond"),
});

export type RequestHostedGitMutation = typeof MutationRequest.Type;
export type HostedGitMutationDecision = typeof MutationDecision.Type;

export interface HostedGitMutationAcceptance {
	readonly approval: HostedGitMutationApprovalValue;
	readonly event: EventEnvelopeValue;
	readonly status: "accepted" | "duplicate";
}

export interface HostedGitMutationExecution {
	readonly approval: HostedGitMutationApprovalValue;
	readonly claim_token: string;
	readonly command: typeof HostedGitMutationCommandRequest.Type;
}

export class HostedGitMutationConflict extends Data.TaggedError("HostedGitMutationConflict")<{
	readonly reason:
		| "artifact_conflict"
		| "claim_conflict"
		| "decision_conflict"
		| "invalid_transition"
		| "lease_conflict"
		| "request_conflict";
}> {}

export class HostedGitMutationUnavailable extends Data.TaggedError("HostedGitMutationUnavailable")<{
	readonly reason: "erased" | "missing" | "project_missing" | "thread_not_attached";
}> {}

export class HostedGitMutationInvariant extends Data.TaggedError("HostedGitMutationInvariant")<{
	readonly message: string;
}> {}

export type HostedGitMutationRepositoryError =
	| HostedGitMutationConflict
	| HostedGitMutationInvariant
	| HostedGitMutationUnavailable
	| JournalStoreFailure;

/** Owns source-safe approval journals and private hosted-mutation artifacts. */
export class HostedGitMutationRepository extends Context.Service<
	HostedGitMutationRepository,
	{
		readonly ActiveClaimsForThread: (
			thread_id: string,
		) => Effect.Effect<boolean, HostedGitMutationRepositoryError>;
		readonly Decide: (
			input: unknown,
		) => Effect.Effect<HostedGitMutationAcceptance, HostedGitMutationRepositoryError>;
		readonly ExecuteClaimed: <A, E, R>(
			identity: typeof ClaimIdentity.Type,
			execution: Effect.Effect<A, E, R>,
		) => Effect.Effect<A, E | HostedGitMutationRepositoryError, R>;
		readonly ListApproved: Effect.Effect<
			ReadonlyArray<{ readonly approval_id: string; readonly thread_id: string }>,
			HostedGitMutationRepositoryError
		>;
		readonly MarkExecuting: (
			approval_id: string,
		) => Effect.Effect<HostedGitMutationAcceptance, HostedGitMutationRepositoryError>;
		readonly Query: (
			input: unknown,
		) => Effect.Effect<
			HostedGitMutationApprovalQueryResultValue,
			HostedGitMutationRepositoryError
		>;
		readonly ReadBySourceCommand: (
			message_id: string,
		) => Effect.Effect<
			Option.Option<HostedGitMutationAcceptance>,
			HostedGitMutationRepositoryError
		>;
		readonly ReplayRequest: (
			input: unknown,
		) => Effect.Effect<
			Option.Option<HostedGitMutationAcceptance>,
			HostedGitMutationRepositoryError
		>;
		readonly ReadExecution: (
			approval_id: string,
		) => Effect.Effect<HostedGitMutationExecution, HostedGitMutationRepositoryError>;
		readonly RenewLease: (
			identity: typeof ClaimIdentity.Type,
		) => Effect.Effect<void, HostedGitMutationRepositoryError>;
		readonly Request: (
			input: unknown,
		) => Effect.Effect<HostedGitMutationAcceptance, HostedGitMutationRepositoryError>;
	}
>()("Artisan/HostedGitMutationRepository") {}

type ApprovalRow = typeof HostedGitMutationApprovals.$inferSelect;
type ArtifactRow = typeof HostedGitMutationArtifacts.$inferSelect;
type ClaimRow = typeof HostedGitMutationClaims.$inferSelect;

type StoredArtifact =
	| {
			readonly _tag: "private";
			readonly command: typeof HostedGitMutationCommandRequest.Type;
			readonly operation_binding: string;
	  }
	| {
			readonly _tag: "scrubbed";
			readonly operation_binding: string;
	  };

const execution_lease_seconds = 30;

function conflict(reason: HostedGitMutationConflict["reason"]) {
	return new HostedGitMutationConflict({ reason });
}

function invariant() {
	return new HostedGitMutationInvariant({
		message: "Stored hosted Git mutation state is invalid",
	});
}

function normalize_error(error: unknown): HostedGitMutationRepositoryError {
	if (
		error instanceof HostedGitMutationConflict ||
		error instanceof HostedGitMutationInvariant ||
		error instanceof HostedGitMutationUnavailable ||
		error instanceof JournalStoreFailure
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function normalize_execution_error<E>(
	error: E | HostedGitMutationRepositoryError | SqlError,
): E | HostedGitMutationRepositoryError {
	if (isSqlError(error)) {
		return normalize_error(error);
	}

	if (
		error instanceof HostedGitMutationConflict ||
		error instanceof HostedGitMutationInvariant ||
		error instanceof HostedGitMutationUnavailable ||
		error instanceof JournalStoreFailure
	) {
		return error;
	}

	return error;
}

function json_equals(left: unknown, right: unknown) {
	return JSON.stringify(left) === JSON.stringify(right);
}

function DecodeDateTime(value: unknown) {
	return Schema.decodeUnknownEffect(IsoDateTime)(value).pipe(
		Effect.mapError(invariant),
		Effect.flatMap((decoded) =>
			Option.match(DateTime.make(decoded), {
				onNone: () => Effect.fail(invariant()),
				onSome: Effect.succeed,
			}),
		),
	);
}

function LeaseExpiry(now: string) {
	return DecodeDateTime(now).pipe(
		Effect.map((date_time) =>
			DateTime.formatIso(DateTime.add(date_time, { seconds: execution_lease_seconds })),
		),
	);
}

function request_payload(input: typeof MutationRequest.Type) {
	return JSON.stringify({
		approval_id: input.approval_id,
		request_fingerprint: input.request_fingerprint,
		selection: input.command.selection,
		summary: summarize_hosted_git_mutation(input.command.mutation),
		type: "hosted.git.mutation.request",
	});
}

function decision_payload(input: typeof MutationDecision.Type) {
	return JSON.stringify({
		approval_id: input.approval_id,
		approved: input.approved,
		type: "hosted.git.mutation.approval.respond",
	});
}

function event_key(approval_id: string, state: HostedGitMutationApprovalValue["state"]) {
	return `hosted_git_mutation:${approval_id}:${state}`;
}

/** Supplies the SQLite-backed hosted Git mutation journal repository. */
export const HostedGitMutationRepositoryLive = Layer.effect(
	HostedGitMutationRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const execution_gate = yield* WorkspaceGitExecutionGate;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const DecodeJson = <A>(schema: Schema.Schema<A>, value: string) =>
			Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
				Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
				Effect.mapError(invariant),
			) as Effect.Effect<A, HostedGitMutationInvariant>;

		const OperationBinding = (input: {
			readonly approval_id: string;
			readonly command: typeof HostedGitMutationCommandRequest.Type;
			readonly request_fingerprint: string;
			readonly summary: typeof HostedGitMutationSummary.Type;
		}) =>
			crypto
				.digest(
					"SHA-256",
					new TextEncoder().encode(
						JSON.stringify({
							approval_id: input.approval_id,
							command: input.command,
							request_fingerprint: input.request_fingerprint,
							summary: input.summary,
							type: "hosted.git.mutation.binding",
							version: 1,
						}),
					),
				)
				.pipe(Effect.map(Encoding.encodeHex));

		const DecodeApproval = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const requested =
					row.state === "requested" &&
					row.decision_message_id === null &&
					row.approved === null &&
					row.decided_at === null &&
					row.execution_started_at === null &&
					row.result_json === null &&
					row.rejection_reason === null &&
					row.unknown_reason === null &&
					row.updated_at === row.created_at;
				const approved =
					(row.state === "approved" || row.state === "executing") &&
					row.approved === true;
				const denied = row.state === "denied" && row.approved === false;

				if (
					!requested &&
					((!approved && !denied) ||
						row.decision_message_id === null ||
						row.decided_at === null ||
						(row.state !== "executing" && row.execution_started_at !== null) ||
						row.result_json !== null ||
						row.rejection_reason !== null ||
						row.unknown_reason !== null ||
						(row.state !== "executing" && row.updated_at !== row.decided_at))
				) {
					return yield* invariant();
				}

				yield* Schema.decodeUnknownEffect(RequestFingerprint)(row.request_fingerprint).pipe(
					Effect.mapError(invariant),
				);
				const summary = yield* DecodeJson(
					HostedGitMutationSummary,
					row.operation_summary_json,
				);
				const pull_request_origin = yield* DecodeJson(
					HostedGitOrigin,
					row.pull_request_origin_json,
				);
				const repository = yield* DecodeJson(
					HostedGitRepositoryIdentity,
					row.repository_json,
				);
				const selection = yield* DecodeJson(HostedProjectSelection, row.selection_json);
				const operation = yield* Schema.decodeUnknownEffect(HostedGitMutationApproval, {
					onExcessProperty: "error",
				})({
					approval_id: row.approval_id,
					created_at: row.created_at,
					expected_head_commit: row.expected_head_commit,
					operation: summary,
					pull_request_number: row.pull_request_number,
					pull_request_origin,
					repository,
					selection,
					snapshot_version: row.snapshot_version,
					source_command_id: row.source_command_id,
					state: row.state,
					thread_id: row.thread_id,
					updated_at: row.updated_at,
					workspace_id: row.workspace_id,
					...(row.decision_message_id === null
						? {}
						: { decision_message_id: row.decision_message_id }),
					...(row.approved === null
						? {}
						: { decision: row.approved ? "approved" : "denied" }),
					...(row.decided_at === null ? {} : { decided_at: row.decided_at }),
				}).pipe(Effect.mapError(invariant));

				return operation;
			});

		const ReadRow = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(HostedGitMutationApprovals)
				.where(eq(HostedGitMutationApprovals.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(new HostedGitMutationUnavailable({ reason: "missing" })),
					),
				);

		const ReadArtifact = (
			transaction: typeof database.client,
			row: ApprovalRow,
			approval: HostedGitMutationApprovalValue,
		) =>
			transaction
				.select()
				.from(HostedGitMutationArtifacts)
				.where(eq(HostedGitMutationArtifacts.approval_id, row.approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([artifact]) =>
						artifact ? Effect.succeed(artifact) : Effect.fail(invariant()),
					),
					Effect.flatMap((artifact: ArtifactRow) =>
						Effect.gen(function* () {
							const operation_binding = yield* Schema.decodeUnknownEffect(
								RequestFingerprint,
							)(artifact.operation_binding).pipe(Effect.mapError(invariant));
							const private_fields_scrubbed =
								artifact.operation_json === null &&
								artifact.selection_json === null;

							if (artifact.updated_at !== row.updated_at) {
								return yield* invariant();
							}

							if (approval.state === "denied") {
								if (
									!private_fields_scrubbed ||
									artifact.provider_result_json !== null
								) {
									return yield* invariant();
								}

								return {
									_tag: "scrubbed",
									operation_binding,
								} satisfies StoredArtifact;
							}

							if (
								(approval.state !== "requested" &&
									approval.state !== "approved" &&
									approval.state !== "executing") ||
								private_fields_scrubbed ||
								artifact.operation_json === null ||
								artifact.selection_json === null ||
								artifact.provider_result_json !== null
							) {
								return yield* invariant();
							}

							const mutation = yield* DecodeJson(
								HostedGitMutationCommandRequest,
								artifact.operation_json,
							);
							const selection = yield* DecodeJson(
								HostedProjectSelection,
								artifact.selection_json,
							);
							const summary = summarize_hosted_git_mutation(mutation.mutation);
							const binding = yield* OperationBinding({
								approval_id: row.approval_id,
								command: mutation,
								request_fingerprint: row.request_fingerprint,
								summary,
							});

							if (
								!json_equals(selection, mutation.selection) ||
								!json_equals(selection, approval.selection) ||
								!json_equals(summary, approval.operation) ||
								operation_binding !== binding
							) {
								return yield* invariant();
							}

							return {
								_tag: "private",
								command: mutation,
								operation_binding,
							} satisfies StoredArtifact;
						}),
					),
				);

		const ArtifactMatchesRequest = (
			artifact: StoredArtifact,
			input: typeof MutationRequest.Type,
		) =>
			Effect.gen(function* () {
				if (artifact._tag === "private") {
					return json_equals(artifact.command, input.command);
				}

				const summary = summarize_hosted_git_mutation(input.command.mutation);
				const binding = yield* OperationBinding({
					approval_id: input.approval_id,
					command: input.command,
					request_fingerprint: input.request_fingerprint,
					summary,
				});

				return artifact.operation_binding === binding;
			});

		const DecodeEvent = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* DecodeJson(
					HostedGitMutationApprovalUpdatedEvent,
					row.payload_json,
				);
				const raw_origin =
					row.raw_origin_json === null
						? undefined
						: yield* DecodeJson(RawOrigin, row.raw_origin_json);

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
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
					schema_version: row.schema_version,
					sent_at: row.occurred_at,
					sequence: row.stream_sequence,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				}).pipe(Effect.mapError(invariant));
			});

		const ReadAcceptance = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const approval = yield* DecodeApproval(row);
				yield* ReadArtifact(transaction, row, approval);
				const [event_row] = yield* transaction
					.select()
					.from(JournalEvents)
					.where(
						eq(
							JournalEvents.idempotency_key,
							event_key(row.approval_id, approval.state),
						),
					)
					.limit(1);

				if (!event_row) return yield* invariant();

				const event = yield* DecodeEvent(event_row);
				const expected_causation =
					approval.state === "requested"
						? row.source_command_id
						: row.decision_message_id;

				if (
					event.payload.type !== "hosted.git.mutation.approval.updated" ||
					expected_causation === null ||
					!json_equals(event.payload.approval, approval) ||
					event_row.event_type !== event.payload.type ||
					event.schema_version !== 1 ||
					event.agent_id !== undefined ||
					event.causation_id !== expected_causation ||
					event.correlation_id !== row.approval_id ||
					event.origin !== "backend" ||
					event.raw_origin !== undefined ||
					event.run_id !== undefined ||
					event.sent_at !== approval.updated_at ||
					event.stream_id !== `thread:${approval.thread_id}` ||
					event.thread_id !== approval.thread_id
				) {
					return yield* invariant();
				}

				return { approval, event };
			});

		const AppendEvent = (
			transaction: typeof database.client,
			approval: HostedGitMutationApprovalValue,
			causation_id: string,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${approval.thread_id}`;
				const [stream] = yield* transaction
					.select()
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;

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

				const payload: typeof HostedGitMutationApprovalUpdatedEvent.Type = {
					approval,
					type: "hosted.git.mutation.approval.updated",
				};
				const [row] = yield* transaction
					.insert(JournalEvents)
					.values({
						causation_id,
						correlation_id: approval.approval_id,
						event_id: yield* metadata.MakeId("event"),
						event_type: payload.type,
						idempotency_key: event_key(approval.approval_id, approval.state),
						occurred_at: approval.updated_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: approval.thread_id,
					})
					.returning();

				const event = row ? yield* DecodeEvent(row) : yield* invariant();

				return event;
			});

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select()
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [erasing] = yield* transaction
					.select()
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select()
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (!thread || erasing || tombstone) {
					return yield* new HostedGitMutationUnavailable({ reason: "erased" });
				}

				return thread;
			});

		const EnsureProjectThreadSnapshot = (
			transaction: typeof database.client,
			input: typeof MutationRequest.Type,
		) =>
			Effect.gen(function* () {
				const mutation = input.command.mutation;
				const [project] = yield* transaction
					.select()
					.from(Projects)
					.where(eq(Projects.workspace_id, mutation.workspace_id))
					.limit(1);
				if (!project)
					return yield* new HostedGitMutationUnavailable({ reason: "project_missing" });

				const [origin] = yield* transaction
					.select()
					.from(ProjectHostedOrigins)
					.where(eq(ProjectHostedOrigins.project_id, project.project_id))
					.limit(1);
				if (
					!origin ||
					origin.provider_id !== mutation.repository.provider_id ||
					origin.canonical_host !== mutation.repository.host ||
					origin.owner !== mutation.repository.owner ||
					origin.name !== mutation.repository.name ||
					origin.selected_account_login !== input.command.selection.account_login
				) {
					return yield* conflict("request_conflict");
				}

				const [session] = yield* transaction
					.select()
					.from(WorkspaceGitSessions)
					.where(eq(WorkspaceGitSessions.workspace_id, mutation.workspace_id))
					.limit(1);
				if (
					!session ||
					session.state !== "ready" ||
					session.branch !== mutation.selected_branch ||
					session.head !== mutation.expected_head_commit
				) {
					return yield* conflict("request_conflict");
				}
				if (
					session.repository_root !== project.canonical_root ||
					session.selected_worktree_path !== project.canonical_root
				) {
					return yield* invariant();
				}

				const thread = yield* EnsureLiveThread(transaction, input.thread_id);

				const linked = yield* DecodeJson(
					Schema.Array(ProjectRef),
					thread.linked_projects_json,
				);
				const primary =
					thread.primary_project_id === null
						? thread.primary_project_json === null
							? undefined
							: yield* invariant()
						: thread.primary_project_json === null
							? yield* invariant()
							: yield* DecodeJson(ProjectRef, thread.primary_project_json);

				if (primary !== undefined && primary.project_id !== thread.primary_project_id) {
					return yield* invariant();
				}

				const attached = [...(primary === undefined ? [] : [primary]), ...linked].find(
					(reference) => reference.project_id === project.project_id,
				);
				if (!attached)
					return yield* new HostedGitMutationUnavailable({
						reason: "thread_not_attached",
					});
				if (
					attached.display_name !== project.display_name ||
					attached.root_path !== project.canonical_root
				) {
					return yield* invariant();
				}

				const [snapshot] = yield* transaction
					.select()
					.from(HostedGitSnapshots)
					.where(eq(HostedGitSnapshots.project_id, project.project_id))
					.limit(1);
				if (!snapshot || snapshot.version !== mutation.snapshot_version)
					return yield* conflict("request_conflict");
				const lookup = yield* DecodeJson(HostedGitPullRequestLookup, snapshot.lookup_json);
				if (
					lookup.association._tag !== "matched" ||
					lookup.association.freshness !== "current" ||
					lookup.branch !== mutation.selected_branch ||
					lookup.expected_head_commit !== mutation.expected_head_commit ||
					!json_equals(lookup.repository, mutation.repository)
				)
					return yield* conflict("request_conflict");

				const pull_request = lookup.association.pull_request;
				const base_matches =
					pull_request.number === mutation.pull_request_number &&
					json_equals(pull_request.origin, mutation.pull_request_origin) &&
					pull_request.head_branch === mutation.selected_branch &&
					pull_request.head_commit === mutation.expected_head_commit &&
					pull_request.state === "open";
				const thread_matches =
					(mutation.operation !== "reply_review_thread" &&
						mutation.operation !== "resolve_review_thread") ||
					pull_request.review_threads.some((review_thread) =>
						json_equals(review_thread.origin, mutation.thread_origin),
					);
				const workflow_matches =
					(mutation.operation !== "rerun_workflow" &&
						mutation.operation !== "cancel_workflow") ||
					pull_request.checks.some((check) =>
						json_equals(check.workflow_origin, mutation.workflow_origin),
					);
				if (!base_matches || !thread_matches || !workflow_matches)
					return yield* conflict("request_conflict");
			});

		const InsertCommand = (
			transaction: typeof database.client,
			metadata_value: typeof CommandMetadata.Type,
			thread_id: string,
			payload_type: string,
			payload_json: string,
		) =>
			Effect.gen(function* () {
				yield* transaction.insert(JournalCommands).values({
					accepted_at: yield* metadata.Now,
					message_id: metadata_value.message_id,
					origin: "frontend",
					payload_json,
					payload_type,
					schema_version: 1,
					sent_at: metadata_value.sent_at,
					status: "accepted",
					thread_id,
				});
			});

		const ValidateRequestCommand = (
			transaction: typeof database.client,
			row: ApprovalRow,
			approval: HostedGitMutationApprovalValue,
		) =>
			Effect.gen(function* () {
				const [command] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, row.source_command_id))
					.limit(1);
				if (
					!command ||
					command.schema_version !== 1 ||
					command.origin !== "frontend" ||
					command.payload_type !== "hosted.git.mutation.request" ||
					command.status !== "accepted" ||
					command.thread_id !== row.thread_id ||
					command.run_id !== null ||
					command.agent_id !== null ||
					command.causation_id !== null ||
					command.raw_origin_json !== null ||
					command.assigned_run_id !== null
				)
					return yield* invariant();
				const payload = yield* DecodeJson(StoredRequestPayload, command.payload_json);
				if (
					payload.approval_id !== row.approval_id ||
					payload.request_fingerprint !== row.request_fingerprint ||
					!json_equals(payload.selection, approval.selection) ||
					!json_equals(payload.summary, approval.operation)
				)
					return yield* invariant();
				return command;
			});

		const ValidateDecisionCommand = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				if (
					row.decision_message_id === null ||
					row.approved === null ||
					row.decided_at === null
				)
					return;
				const [command] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, row.decision_message_id))
					.limit(1);
				if (
					!command ||
					command.schema_version !== 1 ||
					command.origin !== "frontend" ||
					command.payload_type !== "hosted.git.mutation.approval.respond" ||
					command.status !== "accepted" ||
					command.thread_id !== row.thread_id ||
					command.run_id !== null ||
					command.agent_id !== null ||
					command.causation_id !== null ||
					command.raw_origin_json !== null ||
					command.assigned_run_id !== null ||
					command.sent_at !== row.decided_at
				)
					return yield* invariant();
				const payload = yield* DecodeJson(StoredDecisionPayload, command.payload_json);
				if (payload.approval_id !== row.approval_id || payload.approved !== row.approved)
					return yield* invariant();
			});

		const ValidateStoredBase = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const approval = yield* DecodeApproval(row);
				yield* ValidateRequestCommand(transaction, row, approval);
				yield* ValidateDecisionCommand(transaction, row);
				return yield* ReadAcceptance(transaction, row);
			});

		const ReadStoredClaim = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const [claim] = yield* transaction
					.select()
					.from(HostedGitMutationClaims)
					.where(eq(HostedGitMutationClaims.approval_id, row.approval_id))
					.limit(1);

				if (!claim) return yield* invariant();

				yield* Schema.decodeUnknownEffect(Identifier)(claim.claim_token).pipe(
					Effect.mapError(invariant),
				);
				yield* Schema.decodeUnknownEffect(Identifier)(claim.owner_instance_id).pipe(
					Effect.mapError(invariant),
				);
				const claimed_at = yield* DecodeDateTime(claim.claimed_at);
				const lease_expires_at = yield* DecodeDateTime(claim.lease_expires_at);
				const execution_started_at =
					claim.execution_started_at === null
						? undefined
						: yield* DecodeDateTime(claim.execution_started_at);
				const execution_completed_at =
					claim.execution_completed_at === null
						? undefined
						: yield* DecodeDateTime(claim.execution_completed_at);
				const claimed_at_millis = DateTime.toEpochMillis(claimed_at);
				const lease_expires_at_millis = DateTime.toEpochMillis(lease_expires_at);
				const execution_started_at_millis =
					execution_started_at === undefined
						? undefined
						: DateTime.toEpochMillis(execution_started_at);
				const execution_completed_at_millis =
					execution_completed_at === undefined
						? undefined
						: DateTime.toEpochMillis(execution_completed_at);

				if (
					row.state !== "executing" ||
					claim.workspace_id !== row.workspace_id ||
					claim.thread_id !== row.thread_id ||
					claim.claimed_at !== row.updated_at ||
					claim.execution_started_at !== row.execution_started_at ||
					lease_expires_at_millis <= claimed_at_millis ||
					(execution_started_at_millis !== undefined &&
						(execution_started_at_millis < claimed_at_millis ||
							execution_started_at_millis >= lease_expires_at_millis)) ||
					(execution_completed_at_millis !== undefined &&
						(execution_started_at_millis === undefined ||
							execution_completed_at_millis < execution_started_at_millis))
				) {
					return yield* invariant();
				}

				return claim;
			});

		const ReadClaim = (
			transaction: typeof database.client,
			row: ApprovalRow,
			claim_token?: string,
			owner_instance_id?: string,
		) =>
			ReadStoredClaim(transaction, row).pipe(
				Effect.flatMap((claim) =>
					(claim_token !== undefined && claim.claim_token !== claim_token) ||
					(owner_instance_id !== undefined &&
						claim.owner_instance_id !== owner_instance_id)
						? Effect.fail(conflict("lease_conflict"))
						: Effect.succeed(claim),
				),
			);

		const ValidateStored = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const acceptance = yield* ValidateStoredBase(transaction, row);
				const [claim] = yield* transaction
					.select({ approval_id: HostedGitMutationClaims.approval_id })
					.from(HostedGitMutationClaims)
					.where(eq(HostedGitMutationClaims.approval_id, row.approval_id))
					.limit(1);

				if (row.state === "executing") {
					yield* ReadStoredClaim(transaction, row);
				} else if (claim) {
					return yield* invariant();
				}

				return acceptance;
			});

		const LeaseExpired = (expires_at: string, now: string) =>
			Effect.all([DecodeDateTime(expires_at), DecodeDateTime(now)]).pipe(
				Effect.map(
					([expiry, current]) =>
						DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current),
				),
			);

		const TimestampBefore = (left: string, right: string) =>
			Effect.all([DecodeDateTime(left), DecodeDateTime(right)]).pipe(
				Effect.map(
					([left_time, right_time]) =>
						DateTime.toEpochMillis(left_time) < DateTime.toEpochMillis(right_time),
				),
			);

		const BuildExecution = (row: ApprovalRow, claim: ClaimRow, artifact: StoredArtifact) => {
			if (artifact._tag !== "private") {
				return Effect.fail(invariant());
			}

			return DecodeApproval(row).pipe(
				Effect.map(
					(approval) =>
						({
							approval,
							claim_token: claim.claim_token,
							command: artifact.command,
						}) satisfies HostedGitMutationExecution,
				),
			);
		};

		const MarkExecuting = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new HostedGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === "executing") {
										const acceptance = yield* ValidateStored(transaction, row);
										yield* ReadClaim(transaction, row);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "approved") {
										return yield* conflict("invalid_transition");
									}

									yield* ValidateStored(transaction, row);
									const [checkout_claim, mutation_claim, hosted_claim] =
										yield* Effect.all([
											transaction
												.select({
													workspace_id:
														WorkspaceGitCheckoutClaims.workspace_id,
												})
												.from(WorkspaceGitCheckoutClaims)
												.where(
													eq(
														WorkspaceGitCheckoutClaims.workspace_id,
														row.workspace_id,
													),
												)
												.limit(1),
											transaction
												.select({
													workspace_id:
														WorkspaceGitMutationClaims.workspace_id,
												})
												.from(WorkspaceGitMutationClaims)
												.where(
													eq(
														WorkspaceGitMutationClaims.workspace_id,
														row.workspace_id,
													),
												)
												.limit(1),
											transaction
												.select({
													approval_id:
														HostedGitMutationClaims.approval_id,
												})
												.from(HostedGitMutationClaims)
												.where(
													or(
														eq(
															HostedGitMutationClaims.workspace_id,
															row.workspace_id,
														),
														eq(
															HostedGitMutationClaims.approval_id,
															row.approval_id,
														),
													),
												)
												.limit(1),
										]);

									if (checkout_claim[0] || mutation_claim[0] || hosted_claim[0]) {
										return yield* conflict("claim_conflict");
									}

									const claimed_at = yield* metadata.Now;
									const lease_expires_at = yield* LeaseExpiry(claimed_at);
									const claim_token = yield* metadata.MakeId("claim");
									const [claim] = yield* transaction
										.insert(HostedGitMutationClaims)
										.values({
											approval_id: row.approval_id,
											claimed_at,
											claim_token,
											lease_expires_at,
											owner_instance_id: metadata.instance_id,
											thread_id: row.thread_id,
											workspace_id: row.workspace_id,
										})
										.onConflictDoNothing()
										.returning();

									if (!claim) {
										return yield* conflict("claim_conflict");
									}

									const [updated] = yield* transaction
										.update(HostedGitMutationApprovals)
										.set({ state: "executing", updated_at: claimed_at })
										.where(
											and(
												eq(
													HostedGitMutationApprovals.approval_id,
													row.approval_id,
												),
												eq(HostedGitMutationApprovals.state, "approved"),
											),
										)
										.returning();

									if (!updated || updated.decision_message_id === null) {
										return yield* invariant();
									}

									yield* transaction
										.update(HostedGitMutationArtifacts)
										.set({ updated_at: claimed_at })
										.where(
											eq(
												HostedGitMutationArtifacts.approval_id,
												row.approval_id,
											),
										);

									const updated_approval = yield* DecodeApproval(updated);
									yield* AppendEvent(
										transaction,
										updated_approval,
										updated.decision_message_id,
									);
									const acceptance = yield* ValidateStored(transaction, updated);

									return {
										...acceptance,
										status: "accepted" as const,
									};
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

		const ReadExecution = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new HostedGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded);

							yield* EnsureLiveThread(transaction, row.thread_id);

							if (row.state !== "executing") {
								return yield* conflict("invalid_transition");
							}

							const acceptance = yield* ValidateStored(transaction, row);
							const artifact = yield* ReadArtifact(
								transaction,
								row,
								acceptance.approval,
							);
							const claim = yield* ReadClaim(
								transaction,
								row,
								undefined,
								metadata.instance_id,
							);

							return yield* BuildExecution(row, claim, artifact);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const RenewLease = (identity: typeof ClaimIdentity.Type) =>
			Schema.decodeUnknownEffect(ClaimIdentity, { onExcessProperty: "error" })(identity).pipe(
				Effect.mapError(() => conflict("lease_conflict")),
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const row = yield* ReadRow(transaction, decoded.approval_id);

								yield* EnsureLiveThread(transaction, row.thread_id);

								if (row.state !== "executing") {
									return yield* conflict("invalid_transition");
								}

								yield* ValidateStored(transaction, row);
								const claim = yield* ReadClaim(
									transaction,
									row,
									decoded.claim_token,
									metadata.instance_id,
								);
								const now = yield* metadata.Now;
								const expired = yield* LeaseExpired(claim.lease_expires_at, now);
								const before_claim = yield* TimestampBefore(now, claim.claimed_at);
								const before_start =
									claim.execution_started_at === null
										? false
										: yield* TimestampBefore(now, claim.execution_started_at);

								if (expired || before_claim || before_start) {
									return yield* conflict("lease_conflict");
								}
								const lease_expires_at = yield* LeaseExpiry(now);

								const [renewed] = yield* transaction
									.update(HostedGitMutationClaims)
									.set({ lease_expires_at })
									.where(
										and(
											eq(
												HostedGitMutationClaims.approval_id,
												decoded.approval_id,
											),
											eq(
												HostedGitMutationClaims.claim_token,
												decoded.claim_token,
											),
											eq(
												HostedGitMutationClaims.owner_instance_id,
												metadata.instance_id,
											),
											eq(
												HostedGitMutationClaims.lease_expires_at,
												claim.lease_expires_at,
											),
										),
									)
									.returning({
										approval_id: HostedGitMutationClaims.approval_id,
									});

								if (!renewed) {
									return yield* conflict("lease_conflict");
								}
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const MarkExecutionStarted = (identity: typeof ClaimIdentity.Type) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const row = yield* ReadRow(transaction, identity.approval_id);

						yield* EnsureLiveThread(transaction, row.thread_id);

						if (row.state !== "executing") {
							return yield* conflict("invalid_transition");
						}

						yield* ValidateStored(transaction, row);
						const claim = yield* ReadClaim(
							transaction,
							row,
							identity.claim_token,
							metadata.instance_id,
						);

						const execution_started_at = yield* metadata.Now;
						const lease_expires_at = yield* LeaseExpiry(execution_started_at);
						const expired = yield* LeaseExpired(
							claim.lease_expires_at,
							execution_started_at,
						);
						const before_claim = yield* TimestampBefore(
							execution_started_at,
							claim.claimed_at,
						);

						if (
							row.execution_started_at !== null ||
							claim.execution_started_at !== null ||
							claim.execution_completed_at !== null ||
							expired ||
							before_claim
						) {
							return yield* conflict("lease_conflict");
						}

						const [started_claim] = yield* transaction
							.update(HostedGitMutationClaims)
							.set({ execution_started_at, lease_expires_at })
							.where(
								and(
									eq(HostedGitMutationClaims.approval_id, identity.approval_id),
									eq(HostedGitMutationClaims.claim_token, identity.claim_token),
									eq(
										HostedGitMutationClaims.owner_instance_id,
										metadata.instance_id,
									),
									eq(
										HostedGitMutationClaims.lease_expires_at,
										claim.lease_expires_at,
									),
									isNull(HostedGitMutationClaims.execution_started_at),
									isNull(HostedGitMutationClaims.execution_completed_at),
								),
							)
							.returning({ approval_id: HostedGitMutationClaims.approval_id });

						if (!started_claim) {
							return yield* conflict("lease_conflict");
						}

						const [started_approval] = yield* transaction
							.update(HostedGitMutationApprovals)
							.set({ execution_started_at })
							.where(
								and(
									eq(
										HostedGitMutationApprovals.approval_id,
										identity.approval_id,
									),
									eq(HostedGitMutationApprovals.state, "executing"),
									isNull(HostedGitMutationApprovals.execution_started_at),
								),
							)
							.returning({ approval_id: HostedGitMutationApprovals.approval_id });

						if (!started_approval) {
							return yield* invariant();
						}
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const MarkExecutionCompleted = (identity: typeof ClaimIdentity.Type) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const row = yield* ReadRow(transaction, identity.approval_id);
						yield* ValidateStored(transaction, row);
						const claim = yield* ReadClaim(
							transaction,
							row,
							identity.claim_token,
							metadata.instance_id,
						);

						const execution_completed_at = yield* metadata.Now;
						const before_start =
							claim.execution_started_at === null
								? true
								: yield* TimestampBefore(
										execution_completed_at,
										claim.execution_started_at,
									);

						if (
							claim.execution_started_at === null ||
							claim.execution_completed_at !== null ||
							before_start
						) {
							return yield* conflict("lease_conflict");
						}

						const [completed] = yield* transaction
							.update(HostedGitMutationClaims)
							.set({ execution_completed_at })
							.where(
								and(
									eq(HostedGitMutationClaims.approval_id, identity.approval_id),
									eq(HostedGitMutationClaims.claim_token, identity.claim_token),
									eq(
										HostedGitMutationClaims.owner_instance_id,
										metadata.instance_id,
									),
									eq(
										HostedGitMutationClaims.execution_started_at,
										claim.execution_started_at,
									),
									isNull(HostedGitMutationClaims.execution_completed_at),
								),
							)
							.returning({ approval_id: HostedGitMutationClaims.approval_id });

						if (!completed) {
							return yield* conflict("lease_conflict");
						}
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const ExecuteClaimed = <A, E, R>(
			identity: typeof ClaimIdentity.Type,
			execution: Effect.Effect<A, E, R>,
		) =>
			Schema.decodeUnknownEffect(ClaimIdentity, { onExcessProperty: "error" })(identity).pipe(
				Effect.mapError(() => conflict("lease_conflict")),
				Effect.flatMap((decoded) =>
					ReadExecution(decoded.approval_id).pipe(
						Effect.flatMap((claimed) =>
							execution_gate.Run(
								`hosted_git_mutation:${claimed.approval.workspace_id}`,
								decoded.claim_token,
								Effect.gen(function* () {
									yield* RenewLease(decoded);
									yield* MarkExecutionStarted(decoded);

									return yield* execution.pipe(
										Effect.onExit((exit) =>
											Exit.isSuccess(exit) ||
											exit.cause.reasons.every(Cause.isFailReason)
												? MarkExecutionCompleted(decoded)
												: Effect.void,
										),
									);
								}),
							),
						),
					),
				),
				Effect.mapError(normalize_execution_error<E>),
			);

		const ListApproved = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(HostedGitMutationApprovals)
						.where(eq(HostedGitMutationApprovals.state, "approved"))
						.orderBy(
							asc(HostedGitMutationApprovals.created_at),
							asc(HostedGitMutationApprovals.approval_id),
						);

					yield* Effect.forEach(rows, (row) =>
						EnsureLiveThread(transaction, row.thread_id).pipe(
							Effect.andThen(ValidateStored(transaction, row)),
						),
					);

					return rows.map(({ approval_id, thread_id }) => ({ approval_id, thread_id }));
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		const ActiveClaimsForThread = (thread_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(thread_id).pipe(
				Effect.mapError(() => new HostedGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [claims, approvals] = yield* Effect.all([
								transaction
									.select()
									.from(HostedGitMutationClaims)
									.where(eq(HostedGitMutationClaims.thread_id, decoded)),
								transaction
									.select()
									.from(HostedGitMutationApprovals)
									.where(
										and(
											eq(HostedGitMutationApprovals.thread_id, decoded),
											eq(HostedGitMutationApprovals.state, "executing"),
										),
									),
							]);
							const rows = new Map(approvals.map((row) => [row.approval_id, row]));

							for (const claim of claims) {
								const row = yield* ReadRow(transaction, claim.approval_id);
								rows.set(row.approval_id, row);
							}

							for (const row of rows.values()) {
								yield* EnsureLiveThread(transaction, row.thread_id);
								yield* ValidateStored(transaction, row);
							}

							return rows.size > 0;
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const Request = (input: unknown) =>
			Schema.decodeUnknownEffect(MutationRequest, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("request_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const existing = yield* transaction
										.select()
										.from(HostedGitMutationApprovals)
										.where(
											or(
												eq(
													HostedGitMutationApprovals.source_command_id,
													decoded.source_command.message_id,
												),
												eq(
													HostedGitMutationApprovals.approval_id,
													decoded.approval_id,
												),
											),
										)
										.limit(2);
									if (existing.length > 1)
										return yield* conflict("request_conflict");
									if (existing[0]) {
										const acceptance = yield* ValidateStored(
											transaction,
											existing[0],
										);
										const artifact = yield* ReadArtifact(
											transaction,
											existing[0],
											acceptance.approval,
										);
										const command = yield* ValidateRequestCommand(
											transaction,
											existing[0],
											acceptance.approval,
										);
										const artifact_matches = yield* ArtifactMatchesRequest(
											artifact,
											decoded,
										);
										const matches =
											existing[0].approval_id === decoded.approval_id &&
											existing[0].source_command_id ===
												decoded.source_command.message_id &&
											existing[0].thread_id === decoded.thread_id &&
											existing[0].request_fingerprint ===
												decoded.request_fingerprint &&
											command.sent_at === decoded.source_command.sent_at &&
											artifact_matches &&
											json_equals(
												acceptance.approval.operation,
												summarize_hosted_git_mutation(
													decoded.command.mutation,
												),
											);
										if (!matches) return yield* conflict("request_conflict");
										return { ...acceptance, status: "duplicate" as const };
									}
									yield* EnsureProjectThreadSnapshot(transaction, decoded);
									const [command_collision] = yield* transaction
										.select()
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.source_command.message_id,
											),
										)
										.limit(1);
									if (command_collision)
										return yield* conflict("request_conflict");
									const now = yield* metadata.Now;
									const summary = summarize_hosted_git_mutation(
										decoded.command.mutation,
									);
									yield* InsertCommand(
										transaction,
										decoded.source_command,
										decoded.thread_id,
										"hosted.git.mutation.request",
										request_payload(decoded),
									);
									yield* transaction.insert(HostedGitMutationApprovals).values({
										approval_id: decoded.approval_id,
										created_at: now,
										expected_head_commit: summary.expected_head_commit,
										operation_summary_json: JSON.stringify(summary),
										pull_request_number: summary.pull_request_number,
										pull_request_origin_json: JSON.stringify(
											summary.pull_request_origin,
										),
										repository_json: JSON.stringify(summary.repository),
										request_fingerprint: decoded.request_fingerprint,
										selection_json: JSON.stringify(decoded.command.selection),
										snapshot_version: summary.snapshot_version,
										source_command_id: decoded.source_command.message_id,
										state: "requested",
										thread_id: decoded.thread_id,
										updated_at: now,
										workspace_id: summary.workspace_id,
									});
									yield* transaction.insert(HostedGitMutationArtifacts).values({
										approval_id: decoded.approval_id,
										operation_binding: yield* OperationBinding({
											approval_id: decoded.approval_id,
											command: decoded.command,
											request_fingerprint: decoded.request_fingerprint,
											summary,
										}),
										operation_json: JSON.stringify(decoded.command),
										selection_json: JSON.stringify(decoded.command.selection),
										updated_at: now,
									});
									const row = yield* ReadRow(transaction, decoded.approval_id);
									const approval = yield* DecodeApproval(row);
									const event = yield* AppendEvent(
										transaction,
										approval,
										decoded.source_command.message_id,
									);
									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));
						if (result.status === "accepted")
							yield* notifier.Publish(result.event.journal_sequence);
						return result;
					}),
				),
			);

		const ReplayRequest = (input: unknown) =>
			Schema.decodeUnknownEffect(MutationRequest, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("request_conflict")),
				Effect.flatMap((decoded) =>
					database.client
						.transaction((transaction) =>
							Effect.gen(function* () {
								const [row] = yield* transaction
									.select()
									.from(HostedGitMutationApprovals)
									.where(
										eq(
											HostedGitMutationApprovals.source_command_id,
											decoded.source_command.message_id,
										),
									)
									.limit(1);
								if (!row) return Option.none<HostedGitMutationAcceptance>();
								const acceptance = yield* ValidateStored(transaction, row);
								const artifact = yield* ReadArtifact(
									transaction,
									row,
									acceptance.approval,
								);
								const command = yield* ValidateRequestCommand(
									transaction,
									row,
									acceptance.approval,
								);
								const artifact_matches = yield* ArtifactMatchesRequest(
									artifact,
									decoded,
								);
								if (
									row.approval_id !== decoded.approval_id ||
									row.thread_id !== decoded.thread_id ||
									row.request_fingerprint !== decoded.request_fingerprint ||
									command.sent_at !== decoded.source_command.sent_at ||
									!artifact_matches
								)
									return yield* conflict("request_conflict");
								return Option.some({ ...acceptance, status: "duplicate" as const });
							}),
						)
						.pipe(Effect.mapError(normalize_error)),
				),
			);

		const ReadBySourceCommand = (message_id: string) =>
			database.client
				.transaction((transaction) =>
					transaction
						.select()
						.from(HostedGitMutationApprovals)
						.where(eq(HostedGitMutationApprovals.source_command_id, message_id))
						.limit(1)
						.pipe(
							Effect.flatMap(([row]) =>
								row
									? ValidateStored(transaction, row).pipe(
											Effect.map((acceptance) =>
												Option.some({
													...acceptance,
													status: "duplicate" as const,
												}),
											),
										)
									: Effect.succeed(Option.none<HostedGitMutationAcceptance>()),
							),
						),
				)
				.pipe(Effect.mapError(normalize_error));

		const Decide = (input: unknown) =>
			Schema.decodeUnknownEffect(MutationDecision, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("decision_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const rows = yield* transaction
										.select()
										.from(HostedGitMutationApprovals)
										.where(
											or(
												eq(
													HostedGitMutationApprovals.approval_id,
													decoded.approval_id,
												),
												eq(
													HostedGitMutationApprovals.decision_message_id,
													decoded.decision_command.message_id,
												),
											),
										)
										.limit(2);
									if (rows.length > 1)
										return yield* conflict("decision_conflict");
									const row = rows[0];
									if (!row)
										return yield* new HostedGitMutationUnavailable({
											reason: "missing",
										});
									if (row.thread_id !== decoded.thread_id)
										return yield* new HostedGitMutationUnavailable({
											reason: "missing",
										});
									const acceptance = yield* ValidateStored(transaction, row);
									if (row.decision_message_id !== null) {
										const matches =
											row.approval_id === decoded.approval_id &&
											row.approved === decoded.approved &&
											row.decision_message_id ===
												decoded.decision_command.message_id &&
											row.decided_at === decoded.decision_command.sent_at;
										if (!matches) return yield* conflict("decision_conflict");
										return { ...acceptance, status: "duplicate" as const };
									}
									yield* EnsureLiveThread(transaction, row.thread_id);
									if (
										row.approval_id !== decoded.approval_id ||
										row.state !== "requested"
									)
										return yield* conflict("decision_conflict");
									const [command_collision] = yield* transaction
										.select()
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.decision_command.message_id,
											),
										)
										.limit(1);
									if (command_collision)
										return yield* conflict("decision_conflict");
									const state = decoded.approved ? "approved" : "denied";
									yield* InsertCommand(
										transaction,
										decoded.decision_command,
										decoded.thread_id,
										"hosted.git.mutation.approval.respond",
										decision_payload(decoded),
									);
									yield* transaction
										.update(HostedGitMutationApprovals)
										.set({
											approved: decoded.approved,
											decided_at: decoded.decision_command.sent_at,
											decision_message_id:
												decoded.decision_command.message_id,
											state,
											updated_at: decoded.decision_command.sent_at,
										})
										.where(
											eq(
												HostedGitMutationApprovals.approval_id,
												row.approval_id,
											),
										);
									yield* transaction
										.update(HostedGitMutationArtifacts)
										.set({
											...(decoded.approved
												? {}
												: {
														operation_json: null,
														provider_result_json: null,
														selection_json: null,
													}),
											updated_at: decoded.decision_command.sent_at,
										})
										.where(
											eq(
												HostedGitMutationArtifacts.approval_id,
												row.approval_id,
											),
										);
									const updated = yield* ReadRow(transaction, row.approval_id);
									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										decoded.decision_command.message_id,
									);
									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));
						if (result.status === "accepted")
							yield* notifier.Publish(result.event.journal_sequence);
						return result;
					}),
				),
			);

		const Query = (input: unknown) =>
			Schema.decodeUnknownEffect(HostedGitMutationApprovalQuery, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => new HostedGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((query) =>
					database.client
						.transaction((transaction) =>
							Effect.gen(function* () {
								const [row] = yield* transaction
									.select()
									.from(HostedGitMutationApprovals)
									.where(
										eq(
											HostedGitMutationApprovals.approval_id,
											query.approval_id,
										),
									)
									.limit(1);
								if (!row || row.thread_id !== query.thread_id)
									return yield* new HostedGitMutationUnavailable({
										reason: "missing",
									});
								const acceptance = yield* ValidateStored(transaction, row);
								return yield* Schema.decodeUnknownEffect(
									HostedGitMutationApprovalQueryResult,
									{ onExcessProperty: "error" },
								)({ approval: acceptance.approval }).pipe(
									Effect.mapError(invariant),
								);
							}),
						)
						.pipe(Effect.mapError(normalize_error)),
				),
			);

		return {
			ActiveClaimsForThread,
			Decide,
			ExecuteClaimed,
			ListApproved,
			MarkExecuting,
			Query,
			ReadBySourceCommand,
			ReadExecution,
			RenewLease,
			ReplayRequest,
			Request,
		};
	}),
);
