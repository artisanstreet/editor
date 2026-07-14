import { and, asc, eq, isNull, or } from "drizzle-orm";
import { Context, Data, DateTime, Effect, Layer, Option, Schema } from "effect";

import {
	EventEnvelope,
	HostedProjectApprovalRepository,
	HostedProjectCloneApproval,
	HostedProjectCloneApprovalQuery,
	HostedProjectCloneApprovalQueryResult,
	Identifier,
	IsoDateTime,
	ProjectRef,
	RawOrigin,
	type EventEnvelope as EventEnvelopeValue,
	type HostedProjectCloneApproval as HostedProjectCloneApprovalValue,
	type HostedProjectCloneApprovalQueryResult as HostedProjectCloneApprovalQueryResultValue,
} from "@artisan/protocol";

import {
	GitProviderCloneDestinationProof,
	GitProviderClonePreparation,
	GitProviderCloneRequest,
	GitProviderCloneResult,
	GitProviderNativePath,
	type GitProviderCloneDestinationProof as GitProviderCloneDestinationProofValue,
	type GitProviderClonePreparation as GitProviderClonePreparationValue,
	type GitProviderCloneRequest as GitProviderCloneRequestValue,
	type GitProviderCloneResult as GitProviderCloneResultValue,
} from "../git-provider/git-provider";
import { WorkspaceGitExecutionGate } from "../git/workspace-git-execution-gate";
import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalStoreFailure } from "../persistence/journal-store";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	HostedProjectCloneApprovals,
	HostedProjectCloneArtifacts,
	HostedProjectCloneClaims,
	JournalCommands,
	JournalEvents,
	ProjectHostedOrigins,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RegisteredProject, type RegisteredProject as RegisteredProjectValue } from "./project";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));

const CommandMetadata = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	message_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	sent_at: IsoDateTime,
});

const CloneRequest = Schema.Struct({
	approval_id: Identifier,
	destination: GitProviderCloneDestinationProof,
	preparation: GitProviderClonePreparation,
	request: GitProviderCloneRequest,
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	thread_id: Identifier,
});

const CloneRequestReplay = Schema.Struct({
	request: GitProviderCloneRequest,
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	thread_id: Identifier,
});

const ReusedCloneRequest = Schema.Struct({
	approval_id: Identifier,
	attachment: Schema.Literals(["attached", "already_attached"]),
	destination_path: GitProviderNativePath,
	registered_project: RegisteredProject,
	request: GitProviderCloneRequest,
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	thread_id: Identifier,
});

const CloneDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	decision_command: CommandMetadata,
	thread_id: Identifier,
});

const ClaimIdentity = Schema.Struct({
	approval_id: Identifier,
	claim_token: Identifier,
});

const CloneResultInput = Schema.Struct({
	...ClaimIdentity.fields,
	result: GitProviderCloneResult,
});

const RegisteredProjectInput = Schema.Struct({
	...ClaimIdentity.fields,
	project: RegisteredProject,
});

const Settlement = Schema.Union([
	Schema.Struct({
		...ClaimIdentity.fields,
		attachment: Schema.Literals(["attached", "already_attached"]),
		project: ProjectRef,
		type: Schema.Literal("applied"),
	}),
	Schema.Struct({
		...ClaimIdentity.fields,
		project: ProjectRef,
		type: Schema.Literal("attachment_conflict"),
	}),
	Schema.Struct({
		...ClaimIdentity.fields,
		reason: Schema.Literals([
			"destination_unavailable",
			"provider_unavailable",
			"repository_unavailable",
			"thread_unavailable",
		]),
		type: Schema.Literal("rejected"),
	}),
	Schema.Struct({
		...ClaimIdentity.fields,
		reason: Schema.Literals(["interrupted", "verification_failed"]),
		type: Schema.Literal("outcome_unknown"),
	}),
]);

const StoredRequestPayload = Schema.Struct({
	approval_id: Identifier,
	destination_path: GitProviderNativePath,
	request_fingerprint: RequestFingerprint,
	type: Schema.Literal("hosted.project.clone.request"),
});

const StoredDecisionPayload = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	type: Schema.Literal("hosted.project.clone.approval.respond"),
});

export type RequestHostedProjectClone = typeof CloneRequest.Type;
export type ReplayHostedProjectClone = typeof CloneRequestReplay.Type;
export type ReuseHostedProjectClone = typeof ReusedCloneRequest.Type;
export type HostedProjectCloneDecision = typeof CloneDecision.Type;
export type HostedProjectCloneSettlement = typeof Settlement.Type;

export interface HostedProjectCloneAcceptance {
	readonly approval: HostedProjectCloneApprovalValue;
	readonly event: EventEnvelopeValue;
	readonly status: "accepted" | "duplicate";
}

export interface HostedProjectCloneExecution {
	readonly approval: HostedProjectCloneApprovalValue;
	readonly claim_token: string;
	readonly clone_result?: GitProviderCloneResultValue;
	readonly destination: GitProviderCloneDestinationProofValue;
	readonly preparation: GitProviderClonePreparationValue;
	readonly registered_project?: RegisteredProjectValue;
	readonly request: GitProviderCloneRequestValue;
}

export interface HostedProjectCloneDispatch {
	readonly approval_id: string;
	readonly recovery: "owned" | "quarantine" | "recoverable" | "waiting";
	readonly thread_id: string;
}

export class HostedProjectCloneConflict extends Data.TaggedError("HostedProjectCloneConflict")<{
	readonly reason:
		| "artifact_conflict"
		| "claim_conflict"
		| "command_conflict"
		| "decision_conflict"
		| "invalid_transition"
		| "lease_conflict"
		| "request_conflict";
}> {}

export class HostedProjectCloneUnavailable extends Data.TaggedError(
	"HostedProjectCloneUnavailable",
)<{
	readonly reason: "erased" | "missing";
}> {}

export class HostedProjectCloneInvariant extends Data.TaggedError("HostedProjectCloneInvariant")<{
	readonly message: string;
}> {}

export type HostedProjectCloneRepositoryError =
	| HostedProjectCloneConflict
	| HostedProjectCloneInvariant
	| HostedProjectCloneUnavailable
	| JournalStoreFailure;

/** Owns private clone evidence, public approval projections, and execution leases. */
export class HostedProjectCloneRepository extends Context.Service<
	HostedProjectCloneRepository,
	{
		readonly AbandonOwnedExecutions: Effect.Effect<void, HostedProjectCloneRepositoryError>;
		readonly ActiveClaimsForThread: (
			thread_id: string,
		) => Effect.Effect<boolean, HostedProjectCloneRepositoryError>;
		readonly ClaimRecovery: (
			approval_id: string,
		) => Effect.Effect<
			Option.Option<HostedProjectCloneExecution>,
			HostedProjectCloneRepositoryError
		>;
		readonly Decide: (
			input: HostedProjectCloneDecision,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneRepositoryError>;
		readonly ExecuteClaimed: <A, R>(
			identity: typeof ClaimIdentity.Type,
			execution: Effect.Effect<A, never, R>,
		) => Effect.Effect<A, HostedProjectCloneRepositoryError, R>;
		readonly ListApproved: Effect.Effect<
			ReadonlyArray<{ readonly approval_id: string; readonly thread_id: string }>,
			HostedProjectCloneRepositoryError
		>;
		readonly ListExecuting: Effect.Effect<
			ReadonlyArray<HostedProjectCloneDispatch>,
			HostedProjectCloneRepositoryError
		>;
		readonly MarkExecuting: (
			approval_id: string,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneRepositoryError>;
		readonly Query: (
			input: unknown,
		) => Effect.Effect<
			HostedProjectCloneApprovalQueryResultValue,
			HostedProjectCloneRepositoryError
		>;
		readonly QuarantineInterrupted: (
			approval_id: string,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneRepositoryError>;
		readonly ReadBySourceCommand: (
			source_command_id: string,
		) => Effect.Effect<
			Option.Option<HostedProjectCloneAcceptance>,
			HostedProjectCloneRepositoryError
		>;
		readonly ReplayRequest: (
			input: unknown,
		) => Effect.Effect<
			Option.Option<HostedProjectCloneAcceptance>,
			HostedProjectCloneRepositoryError
		>;
		readonly ReadExecution: (
			approval_id: string,
		) => Effect.Effect<HostedProjectCloneExecution, HostedProjectCloneRepositoryError>;
		readonly RecordCloneResult: (
			input: unknown,
		) => Effect.Effect<void, HostedProjectCloneRepositoryError>;
		readonly RecordRegisteredProject: (
			input: unknown,
		) => Effect.Effect<void, HostedProjectCloneRepositoryError>;
		readonly RecordReused: (
			input: unknown,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneRepositoryError>;
		readonly RenewLease: (
			identity: typeof ClaimIdentity.Type,
		) => Effect.Effect<void, HostedProjectCloneRepositoryError>;
		readonly Request: (
			input: unknown,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneRepositoryError>;
		readonly Settle: (
			input: unknown,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneRepositoryError>;
	}
>()("Artisan/HostedProjectCloneRepository") {}

type ApprovalRow = typeof HostedProjectCloneApprovals.$inferSelect;
type ArtifactRow = typeof HostedProjectCloneArtifacts.$inferSelect;
type ClaimRow = typeof HostedProjectCloneClaims.$inferSelect;
type CommandRow = typeof JournalCommands.$inferSelect;

interface DecodedArtifact {
	readonly clone_result?: GitProviderCloneResultValue;
	readonly destination: GitProviderCloneDestinationProofValue;
	readonly preparation: GitProviderClonePreparationValue;
	readonly registered_project?: RegisteredProjectValue;
	readonly request: GitProviderCloneRequestValue;
}

const execution_lease_seconds = 30;

function conflict(reason: HostedProjectCloneConflict["reason"]) {
	return new HostedProjectCloneConflict({ reason });
}

function invariant(message: string) {
	return new HostedProjectCloneInvariant({ message });
}

function normalize_error(error: unknown): HostedProjectCloneRepositoryError {
	if (
		error instanceof HostedProjectCloneConflict ||
		error instanceof HostedProjectCloneInvariant ||
		error instanceof HostedProjectCloneUnavailable
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function DecodeDateTime(value: unknown, label: string) {
	return Schema.decodeUnknownEffect(IsoDateTime)(value).pipe(
		Effect.mapError(() => invariant(`${label} is not a valid timestamp`)),
		Effect.flatMap((decoded) =>
			Option.match(DateTime.make(decoded), {
				onNone: () => Effect.fail(invariant(`${label} is not a valid timestamp`)),
				onSome: Effect.succeed,
			}),
		),
	);
}

function LeaseExpiry(now: string) {
	return DecodeDateTime(now, "Hosted clone lease clock").pipe(
		Effect.map((date_time) =>
			DateTime.formatIso(DateTime.add(date_time, { seconds: execution_lease_seconds })),
		),
	);
}

function LeaseExpired(expires_at: string, now: string) {
	return Effect.all([
		DecodeDateTime(expires_at, "Hosted clone lease expiry"),
		DecodeDateTime(now, "Hosted clone lease clock"),
	]).pipe(
		Effect.map(
			([expiry, current]) =>
				DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current),
		),
	);
}

function json_equals(left: unknown, right: unknown) {
	return JSON.stringify(left) === JSON.stringify(right);
}

function public_repository(request: GitProviderCloneRequestValue) {
	return {
		host: request.repository.identity.host,
		name: request.repository.identity.name,
		owner: request.repository.identity.owner,
		provider_id: request.repository.identity.provider_id,
		selected_account_login: request.selection.account_login,
		web_url: request.repository.web_url,
	};
}

function request_payload(input: {
	readonly approval_id: string;
	readonly destination_path: string;
	readonly request_fingerprint: string;
}) {
	return JSON.stringify({
		approval_id: input.approval_id,
		destination_path: input.destination_path,
		request_fingerprint: input.request_fingerprint,
		type: "hosted.project.clone.request",
	});
}

function decision_payload(input: HostedProjectCloneDecision) {
	return JSON.stringify({
		approval_id: input.approval_id,
		approved: input.approved,
		type: "hosted.project.clone.approval.respond",
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
		row.status === "accepted" &&
		row.assigned_run_id === null
	);
}

function approval_event_key(approval_id: string, state: HostedProjectCloneApprovalValue["state"]) {
	return `hosted_project_clone:${approval_id}:${state}`;
}

function registered_project_matches(
	project: RegisteredProjectValue,
	artifact: Omit<DecodedArtifact, "registered_project">,
) {
	const repository = artifact.preparation.repository;
	const origin = project.hosted_origin;

	return (
		artifact.clone_result !== undefined &&
		project.project.root_path === artifact.clone_result.canonical_root &&
		origin.provider_id === repository.identity.provider_id &&
		origin.canonical_host === repository.identity.host &&
		origin.native_id === repository.origin.native_id &&
		origin.owner === repository.identity.owner &&
		origin.name === repository.identity.name &&
		origin.clone_url === repository.clone_url &&
		origin.fetch_url === repository.clone_url &&
		origin.push_url === repository.clone_url &&
		origin.web_url === repository.web_url &&
		origin.selected_account_login === artifact.preparation.selection.account_login &&
		origin.remote_name === "origin"
	);
}

function reused_project_matches(
	project: RegisteredProjectValue,
	request: GitProviderCloneRequestValue,
	destination_path: string,
) {
	const repository = request.repository;
	const origin = project.hosted_origin;

	return (
		project.project.root_path === destination_path &&
		origin.provider_id === repository.identity.provider_id &&
		origin.canonical_host === repository.identity.host &&
		origin.native_id === repository.origin.native_id &&
		origin.owner === repository.identity.owner &&
		origin.name === repository.identity.name &&
		origin.clone_url === repository.clone_url &&
		origin.fetch_url === repository.clone_url &&
		origin.push_url === repository.clone_url &&
		origin.web_url === repository.web_url &&
		origin.selected_account_login === request.selection.account_login &&
		origin.remote_name === "origin"
	);
}

/** Supplies durable hosted clone records without exposing the SQLite client. */
export const HostedProjectCloneRepositoryLive = Layer.effect(
	HostedProjectCloneRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const execution_gate = yield* WorkspaceGitExecutionGate;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select({ thread_id: Threads.thread_id })
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [erasing] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (!thread || erasing || tombstone) {
					return yield* new HostedProjectCloneUnavailable({ reason: "erased" });
				}
			});

		const ReadRow = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(HostedProjectCloneApprovals)
				.where(eq(HostedProjectCloneApprovals.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(new HostedProjectCloneUnavailable({ reason: "missing" })),
					),
				);

		const ReadArtifactRow = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(HostedProjectCloneArtifacts)
				.where(eq(HostedProjectCloneArtifacts.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(
									invariant(
										`Hosted clone ${approval_id} has no private artifact`,
									),
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
				const terminal = [
					"applied",
					"attachment_conflict",
					"outcome_unknown",
					"rejected",
				].includes(row.state);
				const valid_state =
					((row.state === "requested" || row.state === "reused") &&
						!has_any_decision &&
						!has_execution) ||
					(row.state === "denied" &&
						has_decision &&
						row.approved === false &&
						!has_execution) ||
					(row.state === "approved" &&
						has_decision &&
						row.approved === true &&
						!has_execution) ||
					((row.state === "executing" || terminal) &&
						has_decision &&
						row.approved === true &&
						has_execution);
				const no_outcome =
					row.project_json === null &&
					row.attachment === null &&
					row.rejection_reason === null &&
					row.unknown_reason === null;
				const valid_outcome =
					(["requested", "approved", "executing", "denied"].includes(row.state) &&
						no_outcome) ||
					((row.state === "reused" || row.state === "applied") &&
						row.project_json !== null &&
						(row.attachment === "attached" || row.attachment === "already_attached") &&
						row.rejection_reason === null &&
						row.unknown_reason === null) ||
					(row.state === "attachment_conflict" &&
						row.project_json !== null &&
						row.attachment === null &&
						row.rejection_reason === null &&
						row.unknown_reason === null) ||
					(row.state === "rejected" &&
						row.project_json === null &&
						row.attachment === null &&
						row.rejection_reason !== null &&
						row.unknown_reason === null) ||
					(row.state === "outcome_unknown" &&
						row.project_json === null &&
						row.attachment === null &&
						row.rejection_reason === null &&
						row.unknown_reason !== null);
				const expected_updated_at =
					row.state === "requested" || row.state === "reused"
						? row.created_at
						: row.state === "approved" || row.state === "denied"
							? row.decided_at
							: row.state === "executing"
								? row.execution_started_at
								: row.updated_at;

				if (
					!valid_state ||
					!valid_outcome ||
					expected_updated_at === null ||
					row.updated_at !== expected_updated_at
				) {
					return yield* invariant(`Hosted clone ${row.approval_id} has an invalid state`);
				}

				yield* Schema.decodeUnknownEffect(RequestFingerprint)(row.request_fingerprint).pipe(
					Effect.mapError(() =>
						invariant(`Hosted clone ${row.approval_id} has an invalid fingerprint`),
					),
				);
				const repository = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.repository_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(HostedProjectApprovalRepository, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(`Hosted clone ${row.approval_id} has an invalid repository`),
					),
				);
				const project =
					row.project_json === null
						? undefined
						: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.project_json,
							).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(ProjectRef, {
										onExcessProperty: "error",
									}),
								),
								Effect.mapError(() =>
									invariant(
										`Hosted clone ${row.approval_id} has an invalid project`,
									),
								),
							);
				const value = {
					approval_id: row.approval_id,
					...(row.attachment === null ? {} : { attachment: row.attachment }),
					...(row.approved === null
						? {}
						: { decision: row.approved ? "approved" : "denied" }),
					...(row.decided_at === null ? {} : { decided_at: row.decided_at }),
					...(row.decision_message_id === null
						? {}
						: { decision_message_id: row.decision_message_id }),
					created_at: row.created_at,
					destination_path: row.destination_path,
					...(project === undefined ? {} : { project }),
					...(row.rejection_reason === null ? {} : { reason: row.rejection_reason }),
					...(row.unknown_reason === null ? {} : { reason: row.unknown_reason }),
					repository,
					source_command_id: row.source_command_id,
					state: row.state,
					thread_id: row.thread_id,
					updated_at: row.updated_at,
				};

				return yield* Schema.decodeUnknownEffect(HostedProjectCloneApproval, {
					onExcessProperty: "error",
				})(value).pipe(
					Effect.mapError(() =>
						invariant(
							`Hosted clone ${row.approval_id} has an invalid public projection`,
						),
					),
				);
			});

		const DecodeArtifact = (row: ArtifactRow, approval: HostedProjectCloneApprovalValue) =>
			Effect.gen(function* () {
				const request = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.request_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(GitProviderCloneRequest, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() => invariant("Hosted clone request artifact is corrupt")),
				);
				const preparation = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.preparation_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(GitProviderClonePreparation, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant("Hosted clone preparation artifact is corrupt"),
					),
				);
				const destination = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.destination_proof_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(GitProviderCloneDestinationProof, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant("Hosted clone destination artifact is corrupt"),
					),
				);
				const clone_result =
					row.clone_result_json === null
						? undefined
						: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.clone_result_json,
							).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(GitProviderCloneResult, {
										onExcessProperty: "error",
									}),
								),
								Effect.mapError(() =>
									invariant("Hosted clone result artifact is corrupt"),
								),
							);
				const registered_project =
					row.registered_project_json === null
						? undefined
						: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.registered_project_json,
							).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(RegisteredProject, {
										onExcessProperty: "error",
									}),
								),
								Effect.mapError(() =>
									invariant(
										"Hosted clone registered project artifact is corrupt",
									),
								),
							);
				const artifact = {
					...(clone_result === undefined ? {} : { clone_result }),
					destination,
					preparation,
					...(registered_project === undefined ? {} : { registered_project }),
					request,
				} satisfies DecodedArtifact;
				const base_matches =
					json_equals(request.repository, preparation.repository) &&
					json_equals(request.selection, preparation.selection) &&
					destination.canonical_root === approval.destination_path &&
					json_equals(public_repository(request), approval.repository);
				const result_matches =
					clone_result === undefined ||
					(clone_result.canonical_root === destination.canonical_root &&
						json_equals(clone_result.repository, preparation.repository));
				const registration_matches =
					registered_project === undefined ||
					registered_project_matches(registered_project, artifact);

				if (!base_matches || !result_matches || !registration_matches) {
					return yield* invariant(
						`Hosted clone ${row.approval_id} artifact binding is corrupt`,
					);
				}

				yield* Schema.decodeUnknownEffect(IsoDateTime)(row.updated_at).pipe(
					Effect.mapError(() =>
						invariant("Hosted clone artifact update time is corrupt"),
					),
				);

				return artifact;
			});

		const ReadArtifact = (
			transaction: typeof database.client,
			row: ApprovalRow,
			approval: HostedProjectCloneApprovalValue,
		) =>
			ReadArtifactRow(transaction, row.approval_id).pipe(
				Effect.flatMap((artifact) => DecodeArtifact(artifact, approval)),
			);

		const ReadClaim = (
			transaction: typeof database.client,
			row: ApprovalRow,
			artifact: DecodedArtifact,
			claim_token?: string,
			owner_instance_id?: string,
		) =>
			Effect.gen(function* () {
				const [claim] = yield* transaction
					.select()
					.from(HostedProjectCloneClaims)
					.where(eq(HostedProjectCloneClaims.approval_id, row.approval_id))
					.limit(1);

				if (
					!claim ||
					(claim_token !== undefined && claim.claim_token !== claim_token) ||
					(owner_instance_id !== undefined &&
						claim.owner_instance_id !== owner_instance_id)
				) {
					return yield* conflict("lease_conflict");
				}

				yield* Schema.decodeUnknownEffect(Identifier)(claim.claim_token).pipe(
					Effect.mapError(() =>
						invariant(`Hosted clone ${row.approval_id} claim token is corrupt`),
					),
				);
				if (claim.owner_instance_id !== "unowned") {
					yield* Schema.decodeUnknownEffect(Identifier)(claim.owner_instance_id).pipe(
						Effect.mapError(() =>
							invariant(`Hosted clone ${row.approval_id} claim owner is corrupt`),
						),
					);
				}
				yield* DecodeDateTime(
					claim.claimed_at,
					`Hosted clone ${row.approval_id} claim time`,
				);
				yield* DecodeDateTime(
					claim.lease_expires_at,
					`Hosted clone ${row.approval_id} lease expiry`,
				);
				if (claim.execution_started_at !== null) {
					yield* DecodeDateTime(
						claim.execution_started_at,
						`Hosted clone ${row.approval_id} external execution start`,
					);
				}
				if (claim.execution_completed_at !== null) {
					yield* DecodeDateTime(
						claim.execution_completed_at,
						`Hosted clone ${row.approval_id} external execution completion`,
					);
				}

				const repository = artifact.preparation.repository;
				const reserved = row.state === "requested" || row.state === "approved";
				const valid =
					claim.thread_id === row.thread_id &&
					claim.canonical_root === artifact.destination.canonical_root &&
					claim.provider_id === repository.identity.provider_id &&
					claim.canonical_host === repository.identity.host &&
					claim.native_id === repository.origin.native_id &&
					claim.claimed_at === row.created_at &&
					(claim.execution_completed_at === null ||
						claim.execution_started_at !== null) &&
					(!reserved ||
						(claim.owner_instance_id === "unowned" &&
							claim.execution_started_at === null &&
							claim.execution_completed_at === null));

				if (!valid) {
					return yield* invariant(
						`Hosted clone ${row.approval_id} claim binding is corrupt`,
					);
				}

				return claim;
			});

		const DecodeEventRow = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.mapError(() =>
						invariant("Stored hosted clone event payload is corrupt"),
					),
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
				}).pipe(Effect.mapError(() => invariant("Stored hosted clone event is corrupt")));
			});

		const ReadEvent = (
			transaction: typeof database.client,
			approval_id: string,
			state: HostedProjectCloneApprovalValue["state"],
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
										`Hosted clone ${approval_id}:${state} event is missing`,
									),
								),
					),
				);

		const ReadAcceptance = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const approval = yield* DecodeApproval(row);
				const event = yield* ReadEvent(transaction, row.approval_id, approval.state);
				const request_state = approval.state === "requested" || approval.state === "reused";
				const decision_state = approval.state === "approved" || approval.state === "denied";
				const expected_causation_id = request_state
					? row.source_command_id
					: decision_state
						? row.source_command_id
						: row.decision_message_id;
				const expected_correlation_id = request_state
					? row.approval_id
					: decision_state
						? row.decision_message_id
						: row.approval_id;

				if (
					event.payload.type !== "hosted.project.clone.approval.updated" ||
					expected_causation_id === null ||
					expected_correlation_id === null ||
					!json_equals(event.payload.approval, approval) ||
					event.causation_id !== expected_causation_id ||
					event.correlation_id !== expected_correlation_id ||
					event.origin !== "backend" ||
					event.sent_at !== approval.updated_at ||
					event.stream_id !== `thread:${approval.thread_id}` ||
					event.thread_id !== approval.thread_id
				) {
					return yield* invariant(
						`Hosted clone ${row.approval_id}:${approval.state} is corrupt`,
					);
				}

				return { approval, event };
			});

		const AppendEvent = (
			transaction: typeof database.client,
			approval: HostedProjectCloneApprovalValue,
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
					type: "hosted.project.clone.approval.updated",
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
					return yield* invariant("Hosted clone event was not persisted");
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

		const ReadCommand = (transaction: typeof database.client, message_id: string) =>
			transaction
				.select()
				.from(JournalCommands)
				.where(eq(JournalCommands.message_id, message_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(
									invariant(`Hosted clone command ${message_id} is missing`),
								),
					),
				);

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
					Effect.mapError(() => invariant(`${label} command acceptance time is corrupt`)),
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
									invariant(`${label} command origin is corrupt`),
								),
							);
				const value = {
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
					...(row.causation_id === null ? {} : { causation_id: row.causation_id }),
					message_id: row.message_id,
					...(raw_origin === undefined ? {} : { raw_origin }),
					...(row.run_id === null ? {} : { run_id: row.run_id }),
					sent_at: row.sent_at,
				};

				return yield* Schema.decodeUnknownEffect(CommandMetadata, {
					onExcessProperty: "error",
				})(value).pipe(
					Effect.mapError(() => invariant(`${label} command metadata is corrupt`)),
				);
			});

		const ValidateRequestCommand = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				const command = yield* ReadCommand(transaction, row.source_command_id);

				yield* DecodeStoredCommandMetadata(
					command,
					"hosted.project.clone.request",
					"Hosted clone request",
				);
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					command.payload_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(StoredRequestPayload, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() => invariant("Hosted clone request payload is corrupt")),
				);

				if (
					command.thread_id !== row.thread_id ||
					payload.approval_id !== row.approval_id ||
					payload.destination_path !== row.destination_path ||
					payload.request_fingerprint !== row.request_fingerprint
				) {
					return yield* invariant(
						`Hosted clone ${row.approval_id} request binding is corrupt`,
					);
				}

				return command;
			});

		const ValidateDecisionCommand = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				if (row.decision_message_id === null || row.approved === null) {
					return Option.none<CommandRow>();
				}

				const command = yield* ReadCommand(transaction, row.decision_message_id);

				yield* DecodeStoredCommandMetadata(
					command,
					"hosted.project.clone.approval.respond",
					"Hosted clone decision",
				);
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					command.payload_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(StoredDecisionPayload, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() => invariant("Hosted clone decision payload is corrupt")),
				);

				if (
					command.thread_id !== row.thread_id ||
					payload.approval_id !== row.approval_id ||
					payload.approved !== row.approved
				) {
					return yield* invariant(
						`Hosted clone ${row.approval_id} decision binding is corrupt`,
					);
				}

				return Option.some(command);
			});

		const ValidateStoredBinding = (transaction: typeof database.client, row: ApprovalRow) =>
			Effect.gen(function* () {
				yield* EnsureLiveThread(transaction, row.thread_id);

				const approval = yield* DecodeApproval(row);

				yield* ValidateRequestCommand(transaction, row);
				yield* ValidateDecisionCommand(transaction, row);

				const [artifact_row] = yield* transaction
					.select()
					.from(HostedProjectCloneArtifacts)
					.where(eq(HostedProjectCloneArtifacts.approval_id, row.approval_id))
					.limit(1);
				const [claim_row] = yield* transaction
					.select()
					.from(HostedProjectCloneClaims)
					.where(eq(HostedProjectCloneClaims.approval_id, row.approval_id))
					.limit(1);

				if (approval.state === "reused") {
					if (artifact_row || claim_row) {
						return yield* invariant(
							`Reused hosted clone ${row.approval_id} retained private state`,
						);
					}
				} else {
					if (!artifact_row) {
						return yield* invariant(
							`Hosted clone ${row.approval_id} has no private artifact`,
						);
					}

					const artifact = yield* DecodeArtifact(artifact_row, approval);
					const claim_expected = [
						"requested",
						"approved",
						"executing",
						"outcome_unknown",
					].includes(approval.state);

					if (claim_expected) {
						if (!claim_row) {
							return yield* invariant(
								`Hosted clone ${row.approval_id} has no active claim`,
							);
						}

						yield* ReadClaim(transaction, row, artifact);
					} else if (claim_row) {
						return yield* invariant(
							`Hosted clone ${row.approval_id} retained a terminal claim`,
						);
					}
				}

				return yield* ReadAcceptance(transaction, row);
			});

		const FindBySourceCommand = (transaction: typeof database.client, message_id: string) =>
			transaction
				.select()
				.from(HostedProjectCloneApprovals)
				.where(eq(HostedProjectCloneApprovals.source_command_id, message_id))
				.limit(1)
				.pipe(Effect.map(([row]) => Option.fromNullishOr(row)));

		const Request = (input: unknown) =>
			Schema.decodeUnknownEffect(CloneRequest, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("request_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						if (
							!json_equals(
								decoded.request.repository,
								decoded.preparation.repository,
							) ||
							!json_equals(decoded.request.selection, decoded.preparation.selection)
						) {
							return yield* conflict("artifact_conflict");
						}

						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const existing = yield* FindBySourceCommand(
										transaction,
										decoded.source_command.message_id,
									);

									if (Option.isSome(existing)) {
										const row = existing.value;

										if (row.state === "reused") {
											return yield* conflict("request_conflict");
										}

										const approval = yield* DecodeApproval(row);
										const artifact = yield* ReadArtifact(
											transaction,
											row,
											approval,
										);
										const command = yield* ReadCommand(
											transaction,
											row.source_command_id,
										);
										const matches =
											row.approval_id === decoded.approval_id &&
											row.thread_id === decoded.thread_id &&
											row.request_fingerprint ===
												decoded.request_fingerprint &&
											row.destination_path ===
												decoded.destination.canonical_root &&
											json_equals(
												approval.repository,
												public_repository(decoded.request),
											) &&
											json_equals(artifact.request, decoded.request) &&
											json_equals(
												artifact.preparation,
												decoded.preparation,
											) &&
											json_equals(
												artifact.destination,
												decoded.destination,
											) &&
											command_matches(
												command,
												decoded.source_command,
												decoded.thread_id,
												"hosted.project.clone.request",
												request_payload({
													approval_id: decoded.approval_id,
													destination_path:
														decoded.destination.canonical_root,
													request_fingerprint:
														decoded.request_fingerprint,
												}),
											);

										if (!matches) {
											return yield* conflict("request_conflict");
										}

										const acceptance = yield* ValidateStoredBinding(
											transaction,
											row,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									yield* EnsureLiveThread(transaction, decoded.thread_id);

									const [command] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.source_command.message_id,
											),
										)
										.limit(1);
									const [approval_collision] = yield* transaction
										.select({
											approval_id: HostedProjectCloneApprovals.approval_id,
										})
										.from(HostedProjectCloneApprovals)
										.where(
											eq(
												HostedProjectCloneApprovals.approval_id,
												decoded.approval_id,
											),
										)
										.limit(1);
									const [claim_collision] = yield* transaction
										.select({
											approval_id: HostedProjectCloneClaims.approval_id,
										})
										.from(HostedProjectCloneClaims)
										.where(
											or(
												eq(
													HostedProjectCloneClaims.canonical_root,
													decoded.destination.canonical_root,
												),
												and(
													eq(
														HostedProjectCloneClaims.provider_id,
														decoded.request.repository.identity
															.provider_id,
													),
													eq(
														HostedProjectCloneClaims.canonical_host,
														decoded.request.repository.identity.host,
													),
													eq(
														HostedProjectCloneClaims.native_id,
														decoded.request.repository.origin.native_id,
													),
												),
											),
										)
										.limit(1);
									const [registered_identity_collision] = yield* transaction
										.select({ project_id: ProjectHostedOrigins.project_id })
										.from(ProjectHostedOrigins)
										.where(
											and(
												eq(
													ProjectHostedOrigins.provider_id,
													decoded.request.repository.identity.provider_id,
												),
												eq(
													ProjectHostedOrigins.canonical_host,
													decoded.request.repository.identity.host,
												),
												eq(
													ProjectHostedOrigins.native_id,
													decoded.request.repository.origin.native_id,
												),
											),
										)
										.limit(1);

									if (command) {
										return yield* conflict("command_conflict");
									}
									if (approval_collision) {
										return yield* conflict("request_conflict");
									}
									if (claim_collision || registered_identity_collision) {
										return yield* conflict("claim_conflict");
									}

									const now = yield* metadata.Now;
									const claim_token = yield* metadata.MakeId("claim");
									const repository = public_repository(decoded.request);

									yield* InsertCommand(
										transaction,
										decoded.source_command,
										decoded.thread_id,
										"hosted.project.clone.request",
										request_payload({
											approval_id: decoded.approval_id,
											destination_path: decoded.destination.canonical_root,
											request_fingerprint: decoded.request_fingerprint,
										}),
									);
									yield* transaction.insert(HostedProjectCloneApprovals).values({
										approval_id: decoded.approval_id,
										created_at: now,
										destination_path: decoded.destination.canonical_root,
										repository_json: JSON.stringify(repository),
										request_fingerprint: decoded.request_fingerprint,
										source_command_id: decoded.source_command.message_id,
										state: "requested",
										thread_id: decoded.thread_id,
										updated_at: now,
									});
									yield* transaction.insert(HostedProjectCloneArtifacts).values({
										approval_id: decoded.approval_id,
										destination_proof_json: JSON.stringify(decoded.destination),
										preparation_json: JSON.stringify(decoded.preparation),
										request_json: JSON.stringify(decoded.request),
										updated_at: now,
									});
									yield* transaction.insert(HostedProjectCloneClaims).values({
										approval_id: decoded.approval_id,
										canonical_host: decoded.request.repository.identity.host,
										canonical_root: decoded.destination.canonical_root,
										claim_token,
										claimed_at: now,
										lease_expires_at: now,
										native_id: decoded.request.repository.origin.native_id,
										owner_instance_id: "unowned",
										provider_id:
											decoded.request.repository.identity.provider_id,
										thread_id: decoded.thread_id,
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

		const RecordReused = (input: unknown) =>
			Schema.decodeUnknownEffect(ReusedCloneRequest, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(() => conflict("request_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						if (
							!reused_project_matches(
								decoded.registered_project,
								decoded.request,
								decoded.destination_path,
							)
						) {
							return yield* conflict("artifact_conflict");
						}

						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const existing = yield* FindBySourceCommand(
										transaction,
										decoded.source_command.message_id,
									);

									if (Option.isSome(existing)) {
										const row = existing.value;
										const command = yield* ReadCommand(
											transaction,
											row.source_command_id,
										);
										const approval = yield* DecodeApproval(row);
										const matches =
											approval.state === "reused" &&
											row.approval_id === decoded.approval_id &&
											row.thread_id === decoded.thread_id &&
											row.request_fingerprint ===
												decoded.request_fingerprint &&
											row.destination_path === decoded.destination_path &&
											approval.attachment === decoded.attachment &&
											json_equals(
												approval.project,
												decoded.registered_project.project,
											) &&
											json_equals(
												approval.repository,
												public_repository(decoded.request),
											) &&
											command_matches(
												command,
												decoded.source_command,
												decoded.thread_id,
												"hosted.project.clone.request",
												request_payload({
													approval_id: decoded.approval_id,
													destination_path: decoded.destination_path,
													request_fingerprint:
														decoded.request_fingerprint,
												}),
											);

										if (!matches) {
											return yield* conflict("request_conflict");
										}

										const acceptance = yield* ValidateStoredBinding(
											transaction,
											row,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									yield* EnsureLiveThread(transaction, decoded.thread_id);

									const [command] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.source_command.message_id,
											),
										)
										.limit(1);
									const [approval_collision] = yield* transaction
										.select({
											approval_id: HostedProjectCloneApprovals.approval_id,
										})
										.from(HostedProjectCloneApprovals)
										.where(
											eq(
												HostedProjectCloneApprovals.approval_id,
												decoded.approval_id,
											),
										)
										.limit(1);
									const [reused_claim_collision] = yield* transaction
										.select({
											approval_id: HostedProjectCloneClaims.approval_id,
										})
										.from(HostedProjectCloneClaims)
										.where(
											or(
												eq(
													HostedProjectCloneClaims.canonical_root,
													decoded.destination_path,
												),
												and(
													eq(
														HostedProjectCloneClaims.provider_id,
														decoded.request.repository.identity
															.provider_id,
													),
													eq(
														HostedProjectCloneClaims.canonical_host,
														decoded.request.repository.identity.host,
													),
													eq(
														HostedProjectCloneClaims.native_id,
														decoded.request.repository.origin.native_id,
													),
												),
											),
										)
										.limit(1);

									if (command) {
										return yield* conflict("command_conflict");
									}
									if (approval_collision) {
										return yield* conflict("request_conflict");
									}
									if (reused_claim_collision) {
										return yield* conflict("claim_conflict");
									}

									const now = yield* metadata.Now;
									const repository = public_repository(decoded.request);

									yield* InsertCommand(
										transaction,
										decoded.source_command,
										decoded.thread_id,
										"hosted.project.clone.request",
										request_payload({
											approval_id: decoded.approval_id,
											destination_path: decoded.destination_path,
											request_fingerprint: decoded.request_fingerprint,
										}),
									);
									yield* transaction.insert(HostedProjectCloneApprovals).values({
										approval_id: decoded.approval_id,
										attachment: decoded.attachment,
										created_at: now,
										destination_path: decoded.destination_path,
										project_json: JSON.stringify(
											decoded.registered_project.project,
										),
										repository_json: JSON.stringify(repository),
										request_fingerprint: decoded.request_fingerprint,
										source_command_id: decoded.source_command.message_id,
										state: "reused",
										thread_id: decoded.thread_id,
										updated_at: now,
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

		const Decide = (input: HostedProjectCloneDecision) =>
			Schema.decodeUnknownEffect(CloneDecision, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("decision_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);

									if (row.thread_id !== decoded.thread_id) {
										return yield* new HostedProjectCloneUnavailable({
											reason: "missing",
										});
									}

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.decision_message_id !== null) {
										const command = yield* ReadCommand(
											transaction,
											row.decision_message_id,
										);
										const matches =
											row.approved === decoded.approved &&
											command_matches(
												command,
												decoded.decision_command,
												decoded.thread_id,
												"hosted.project.clone.approval.respond",
												decision_payload(decoded),
											);

										if (!matches) {
											return yield* conflict("decision_conflict");
										}

										const acceptance = yield* ValidateStoredBinding(
											transaction,
											row,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "requested") {
										return yield* conflict("invalid_transition");
									}

									const [command] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.decision_command.message_id,
											),
										)
										.limit(1);

									if (command) {
										return yield* conflict("command_conflict");
									}

									const now = yield* metadata.Now;
									const state = decoded.approved ? "approved" : "denied";

									yield* InsertCommand(
										transaction,
										decoded.decision_command,
										decoded.thread_id,
										"hosted.project.clone.approval.respond",
										decision_payload(decoded),
									);
									const [updated] = yield* transaction
										.update(HostedProjectCloneApprovals)
										.set({
											approved: decoded.approved,
											decided_at: now,
											decision_message_id:
												decoded.decision_command.message_id,
											state,
											updated_at: now,
										})
										.where(
											and(
												eq(
													HostedProjectCloneApprovals.approval_id,
													decoded.approval_id,
												),
												eq(HostedProjectCloneApprovals.state, "requested"),
											),
										)
										.returning();

									if (!updated) {
										return yield* conflict("invalid_transition");
									}

									if (!decoded.approved) {
										yield* transaction
											.delete(HostedProjectCloneClaims)
											.where(
												eq(
													HostedProjectCloneClaims.approval_id,
													decoded.approval_id,
												),
											);
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										updated.source_command_id,
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

		const BuildExecution = (
			transaction: typeof database.client,
			row: ApprovalRow,
			claim: ClaimRow,
			artifact: DecodedArtifact,
		) =>
			Effect.gen(function* () {
				const approval = yield* DecodeApproval(row);

				return {
					approval,
					claim_token: claim.claim_token,
					...(artifact.clone_result === undefined
						? {}
						: { clone_result: artifact.clone_result }),
					destination: artifact.destination,
					preparation: artifact.preparation,
					...(artifact.registered_project === undefined
						? {}
						: { registered_project: artifact.registered_project }),
					request: artifact.request,
				} satisfies HostedProjectCloneExecution;
			});

		const MarkExecuting = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new HostedProjectCloneUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === "executing") {
										const approval = yield* DecodeApproval(row);
										const artifact = yield* ReadArtifact(
											transaction,
											row,
											approval,
										);

										yield* ReadClaim(transaction, row, artifact);

										const acceptance = yield* ReadAcceptance(transaction, row);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "approved") {
										return yield* conflict("invalid_transition");
									}

									const approval = yield* DecodeApproval(row);
									const artifact = yield* ReadArtifact(
										transaction,
										row,
										approval,
									);
									const claim = yield* ReadClaim(transaction, row, artifact);
									const started_at = yield* metadata.Now;
									const lease_expires_at = yield* LeaseExpiry(started_at);
									const claim_token = yield* metadata.MakeId("claim");
									const [claimed] = yield* transaction
										.update(HostedProjectCloneClaims)
										.set({
											claim_token,
											lease_expires_at,
											owner_instance_id: metadata.instance_id,
										})
										.where(
											and(
												eq(HostedProjectCloneClaims.approval_id, decoded),
												eq(
													HostedProjectCloneClaims.claim_token,
													claim.claim_token,
												),
												eq(
													HostedProjectCloneClaims.owner_instance_id,
													"unowned",
												),
												isNull(
													HostedProjectCloneClaims.execution_started_at,
												),
												isNull(
													HostedProjectCloneClaims.execution_completed_at,
												),
											),
										)
										.returning({
											approval_id: HostedProjectCloneClaims.approval_id,
										});

									if (!claimed) {
										return yield* conflict("claim_conflict");
									}

									const [updated] = yield* transaction
										.update(HostedProjectCloneApprovals)
										.set({
											execution_started_at: started_at,
											state: "executing",
											updated_at: started_at,
										})
										.where(
											and(
												eq(
													HostedProjectCloneApprovals.approval_id,
													decoded,
												),
												eq(HostedProjectCloneApprovals.state, "approved"),
											),
										)
										.returning();

									if (!updated || updated.decision_message_id === null) {
										return yield* invariant(
											"Hosted clone execution transition did not persist",
										);
									}

									const updated_approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										updated_approval,
										updated.decision_message_id,
										updated.approval_id,
									);

									return {
										approval: updated_approval,
										event,
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
				Effect.mapError(() => new HostedProjectCloneUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded);

							yield* EnsureLiveThread(transaction, row.thread_id);

							if (row.state !== "executing") {
								return yield* conflict("invalid_transition");
							}

							const approval = yield* DecodeApproval(row);
							const artifact = yield* ReadArtifact(transaction, row, approval);
							const claim = yield* ReadClaim(
								transaction,
								row,
								artifact,
								undefined,
								metadata.instance_id,
							);

							return yield* BuildExecution(transaction, row, claim, artifact);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const RenewLease = (identity: typeof ClaimIdentity.Type) =>
			Schema.decodeUnknownEffect(ClaimIdentity, { onExcessProperty: "error" })(identity).pipe(
				Effect.mapError(() => conflict("lease_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const lease_expires_at = yield* LeaseExpiry(now);

						yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state !== "executing") {
										return yield* conflict("invalid_transition");
									}

									const approval = yield* DecodeApproval(row);
									const artifact = yield* ReadArtifact(
										transaction,
										row,
										approval,
									);
									const claim = yield* ReadClaim(
										transaction,
										row,
										artifact,
										decoded.claim_token,
										metadata.instance_id,
									);
									const [renewed] = yield* transaction
										.update(HostedProjectCloneClaims)
										.set({ lease_expires_at })
										.where(
											and(
												eq(
													HostedProjectCloneClaims.approval_id,
													decoded.approval_id,
												),
												eq(
													HostedProjectCloneClaims.claim_token,
													decoded.claim_token,
												),
												eq(
													HostedProjectCloneClaims.owner_instance_id,
													metadata.instance_id,
												),
												eq(
													HostedProjectCloneClaims.lease_expires_at,
													claim.lease_expires_at,
												),
											),
										)
										.returning({
											approval_id: HostedProjectCloneClaims.approval_id,
										});

									if (!renewed) {
										return yield* conflict("lease_conflict");
									}
								}),
							),
						);
					}),
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

						const approval = yield* DecodeApproval(row);
						const artifact = yield* ReadArtifact(transaction, row, approval);
						const claim = yield* ReadClaim(
							transaction,
							row,
							artifact,
							identity.claim_token,
							metadata.instance_id,
						);

						if (
							claim.execution_started_at !== null ||
							claim.execution_completed_at !== null
						) {
							return yield* conflict("lease_conflict");
						}

						const execution_started_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(HostedProjectCloneClaims)
							.set({ execution_started_at })
							.where(
								and(
									eq(HostedProjectCloneClaims.approval_id, identity.approval_id),
									eq(HostedProjectCloneClaims.claim_token, identity.claim_token),
									eq(
										HostedProjectCloneClaims.owner_instance_id,
										metadata.instance_id,
									),
									isNull(HostedProjectCloneClaims.execution_started_at),
									isNull(HostedProjectCloneClaims.execution_completed_at),
								),
							)
							.returning({ approval_id: HostedProjectCloneClaims.approval_id });

						if (!updated) {
							return yield* conflict("lease_conflict");
						}
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const MarkExecutionCompleted = (identity: typeof ClaimIdentity.Type) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const row = yield* ReadRow(transaction, identity.approval_id);
						const approval = yield* DecodeApproval(row);
						const artifact = yield* ReadArtifact(transaction, row, approval);
						const claim = yield* ReadClaim(
							transaction,
							row,
							artifact,
							identity.claim_token,
							metadata.instance_id,
						);

						if (
							claim.execution_started_at === null ||
							claim.execution_completed_at !== null
						) {
							return yield* conflict("lease_conflict");
						}

						const execution_completed_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(HostedProjectCloneClaims)
							.set({ execution_completed_at })
							.where(
								and(
									eq(HostedProjectCloneClaims.approval_id, identity.approval_id),
									eq(HostedProjectCloneClaims.claim_token, identity.claim_token),
									eq(
										HostedProjectCloneClaims.owner_instance_id,
										metadata.instance_id,
									),
									eq(
										HostedProjectCloneClaims.execution_started_at,
										claim.execution_started_at,
									),
									isNull(HostedProjectCloneClaims.execution_completed_at),
								),
							)
							.returning({ approval_id: HostedProjectCloneClaims.approval_id });

						if (!updated) {
							return yield* conflict("lease_conflict");
						}
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const ExecuteClaimed = <A, R>(
			identity: typeof ClaimIdentity.Type,
			execution: Effect.Effect<A, never, R>,
		) =>
			Schema.decodeUnknownEffect(ClaimIdentity, { onExcessProperty: "error" })(identity).pipe(
				Effect.mapError(() => conflict("lease_conflict")),
				Effect.flatMap((decoded) =>
					execution_gate.Run(
						`hosted_project_clone:${decoded.approval_id}`,
						decoded.claim_token,
						Effect.gen(function* () {
							yield* RenewLease(decoded);
							yield* MarkExecutionStarted(decoded);

							const result = yield* execution.pipe(
								Effect.onExit(() => MarkExecutionCompleted(decoded)),
							);

							yield* RenewLease(decoded);

							return result;
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const RecordCloneResult = (input: unknown) =>
			Schema.decodeUnknownEffect(CloneResultInput, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("artifact_conflict")),
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const row = yield* ReadRow(transaction, decoded.approval_id);

								yield* EnsureLiveThread(transaction, row.thread_id);

								if (row.state !== "executing") {
									return yield* conflict("invalid_transition");
								}

								const approval = yield* DecodeApproval(row);
								const artifact = yield* ReadArtifact(transaction, row, approval);
								const claim = yield* ReadClaim(
									transaction,
									row,
									artifact,
									decoded.claim_token,
									metadata.instance_id,
								);

								if (
									claim.execution_started_at === null ||
									decoded.result.canonical_root !==
										artifact.destination.canonical_root ||
									!json_equals(
										decoded.result.repository,
										artifact.preparation.repository,
									)
								) {
									return yield* conflict("artifact_conflict");
								}

								if (artifact.clone_result !== undefined) {
									if (!json_equals(artifact.clone_result, decoded.result)) {
										return yield* conflict("artifact_conflict");
									}

									return;
								}

								const updated_at = yield* metadata.Now;
								const [updated] = yield* transaction
									.update(HostedProjectCloneArtifacts)
									.set({
										clone_result_json: JSON.stringify(decoded.result),
										updated_at,
									})
									.where(
										and(
											eq(
												HostedProjectCloneArtifacts.approval_id,
												decoded.approval_id,
											),
											isNull(HostedProjectCloneArtifacts.clone_result_json),
										),
									)
									.returning({
										approval_id: HostedProjectCloneArtifacts.approval_id,
									});

								if (!updated) {
									return yield* conflict("artifact_conflict");
								}
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const RecordRegisteredProject = (input: unknown) =>
			Schema.decodeUnknownEffect(RegisteredProjectInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(() => conflict("artifact_conflict")),
				Effect.flatMap((decoded) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const row = yield* ReadRow(transaction, decoded.approval_id);

								yield* EnsureLiveThread(transaction, row.thread_id);

								if (row.state !== "executing") {
									return yield* conflict("invalid_transition");
								}

								const approval = yield* DecodeApproval(row);
								const artifact = yield* ReadArtifact(transaction, row, approval);

								yield* ReadClaim(
									transaction,
									row,
									artifact,
									decoded.claim_token,
									metadata.instance_id,
								);

								if (
									artifact.clone_result === undefined ||
									!registered_project_matches(decoded.project, {
										clone_result: artifact.clone_result,
										destination: artifact.destination,
										preparation: artifact.preparation,
										request: artifact.request,
									})
								) {
									return yield* conflict("artifact_conflict");
								}

								if (artifact.registered_project !== undefined) {
									if (
										!json_equals(artifact.registered_project, decoded.project)
									) {
										return yield* conflict("artifact_conflict");
									}

									return;
								}

								const updated_at = yield* metadata.Now;
								const [updated] = yield* transaction
									.update(HostedProjectCloneArtifacts)
									.set({
										registered_project_json: JSON.stringify(decoded.project),
										updated_at,
									})
									.where(
										and(
											eq(
												HostedProjectCloneArtifacts.approval_id,
												decoded.approval_id,
											),
											isNull(
												HostedProjectCloneArtifacts.registered_project_json,
											),
										),
									)
									.returning({
										approval_id: HostedProjectCloneArtifacts.approval_id,
									});

								if (!updated) {
									return yield* conflict("artifact_conflict");
								}
							}),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const settlement_matches = (
			approval: HostedProjectCloneApprovalValue,
			settlement: HostedProjectCloneSettlement,
		) => {
			if (settlement.type === "applied") {
				return (
					approval.state === "applied" &&
					approval.attachment === settlement.attachment &&
					json_equals(approval.project, settlement.project)
				);
			}

			if (settlement.type === "attachment_conflict") {
				return (
					approval.state === "attachment_conflict" &&
					json_equals(approval.project, settlement.project)
				);
			}

			return approval.state === settlement.type && approval.reason === settlement.reason;
		};

		const Settle = (input: unknown) =>
			Schema.decodeUnknownEffect(Settlement, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => conflict("artifact_conflict")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);
									const current = yield* DecodeApproval(row);

									if (settlement_matches(current, decoded)) {
										const acceptance = yield* ValidateStoredBinding(
											transaction,
											row,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state !== "executing") {
										return yield* conflict("invalid_transition");
									}

									const artifact = yield* ReadArtifact(transaction, row, current);
									const claim = yield* ReadClaim(
										transaction,
										row,
										artifact,
										decoded.claim_token,
										metadata.instance_id,
									);
									const project_settlement =
										decoded.type === "applied" ||
										decoded.type === "attachment_conflict";
									const private_state_valid = project_settlement
										? artifact.registered_project !== undefined &&
											json_equals(
												artifact.registered_project.project,
												decoded.project,
											)
										: artifact.clone_result === undefined &&
											artifact.registered_project === undefined;
									const execution_settled =
										claim.execution_started_at === null ||
										claim.execution_completed_at !== null;
									const unknown_valid =
										decoded.type !== "outcome_unknown" ||
										(claim.execution_started_at !== null &&
											artifact.clone_result === undefined);

									if (
										!private_state_valid ||
										!execution_settled ||
										!unknown_valid
									) {
										return yield* conflict("artifact_conflict");
									}

									const now = yield* metadata.Now;
									const patch =
										decoded.type === "applied"
											? {
													attachment: decoded.attachment,
													project_json: JSON.stringify(decoded.project),
													state: decoded.type,
												}
											: decoded.type === "attachment_conflict"
												? {
														project_json: JSON.stringify(
															decoded.project,
														),
														state: decoded.type,
													}
												: decoded.type === "rejected"
													? {
															rejection_reason: decoded.reason,
															state: decoded.type,
														}
													: {
															state: decoded.type,
															unknown_reason: decoded.reason,
														};
									const [updated] = yield* transaction
										.update(HostedProjectCloneApprovals)
										.set({ ...patch, updated_at: now })
										.where(
											and(
												eq(
													HostedProjectCloneApprovals.approval_id,
													decoded.approval_id,
												),
												eq(HostedProjectCloneApprovals.state, "executing"),
											),
										)
										.returning();

									if (!updated || updated.decision_message_id === null) {
										return yield* conflict("invalid_transition");
									}

									if (decoded.type !== "outcome_unknown") {
										yield* transaction
											.delete(HostedProjectCloneClaims)
											.where(
												eq(
													HostedProjectCloneClaims.approval_id,
													decoded.approval_id,
												),
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

		const ClaimRecovery = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new HostedProjectCloneUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const lease_expires_at = yield* LeaseExpiry(now);

						return yield* execution_gate.Run(
							`hosted_project_clone:${decoded}`,
							metadata.instance_id,
							RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const [row] = yield* transaction
											.select()
											.from(HostedProjectCloneApprovals)
											.where(
												eq(
													HostedProjectCloneApprovals.approval_id,
													decoded,
												),
											)
											.limit(1);

										if (!row || row.state !== "executing") {
											return Option.none<HostedProjectCloneExecution>();
										}

										yield* EnsureLiveThread(transaction, row.thread_id);

										const approval = yield* DecodeApproval(row);
										const artifact = yield* ReadArtifact(
											transaction,
											row,
											approval,
										);
										const claim = yield* ReadClaim(transaction, row, artifact);
										const expired = yield* LeaseExpired(
											claim.lease_expires_at,
											now,
										);
										const safe =
											claim.execution_started_at === null ||
											artifact.clone_result !== undefined;

										if (!expired || !safe) {
											return Option.none<HostedProjectCloneExecution>();
										}

										const claim_token = yield* metadata.MakeId("claim");
										const [recovered] = yield* transaction
											.update(HostedProjectCloneClaims)
											.set({
												claim_token,
												lease_expires_at,
												owner_instance_id: metadata.instance_id,
											})
											.where(
												and(
													eq(
														HostedProjectCloneClaims.approval_id,
														decoded,
													),
													eq(
														HostedProjectCloneClaims.claim_token,
														claim.claim_token,
													),
													eq(
														HostedProjectCloneClaims.owner_instance_id,
														claim.owner_instance_id,
													),
													eq(
														HostedProjectCloneClaims.lease_expires_at,
														claim.lease_expires_at,
													),
												),
											)
											.returning();

										if (!recovered) {
											return Option.none<HostedProjectCloneExecution>();
										}

										return Option.some(
											yield* BuildExecution(
												transaction,
												row,
												recovered,
												artifact,
											),
										);
									}),
								),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);

		const QuarantineInterrupted = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new HostedProjectCloneUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* execution_gate.Run(
							`hosted_project_clone:${decoded}`,
							metadata.instance_id,
							RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const row = yield* ReadRow(transaction, decoded);
										const current = yield* DecodeApproval(row);

										if (
											current.state === "outcome_unknown" &&
											current.reason === "interrupted"
										) {
											const acceptance = yield* ValidateStoredBinding(
												transaction,
												row,
											);

											return { ...acceptance, status: "duplicate" as const };
										}

										yield* EnsureLiveThread(transaction, row.thread_id);

										if (
											row.state !== "executing" ||
											row.decision_message_id === null
										) {
											return yield* conflict("invalid_transition");
										}

										const artifact = yield* ReadArtifact(
											transaction,
											row,
											current,
										);
										const claim = yield* ReadClaim(transaction, row, artifact);
										const now = yield* metadata.Now;
										const expired = yield* LeaseExpired(
											claim.lease_expires_at,
											now,
										);

										if (
											!expired ||
											claim.execution_started_at === null ||
											artifact.clone_result !== undefined
										) {
											return yield* conflict("invalid_transition");
										}

										const [updated] = yield* transaction
											.update(HostedProjectCloneApprovals)
											.set({
												state: "outcome_unknown",
												unknown_reason: "interrupted",
												updated_at: now,
											})
											.where(
												and(
													eq(
														HostedProjectCloneApprovals.approval_id,
														decoded,
													),
													eq(
														HostedProjectCloneApprovals.state,
														"executing",
													),
												),
											)
											.returning();

										if (!updated) {
											return yield* conflict("invalid_transition");
										}

										const approval = yield* DecodeApproval(updated);
										const event = yield* AppendEvent(
											transaction,
											approval,
											row.decision_message_id,
											row.approval_id,
										);

										return { approval, event, status: "accepted" as const };
									}),
								),
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

		const Query = (input: unknown) =>
			Schema.decodeUnknownEffect(HostedProjectCloneApprovalQuery, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => new HostedProjectCloneUnavailable({ reason: "missing" })),
				Effect.flatMap((query) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [row] = yield* transaction
								.select()
								.from(HostedProjectCloneApprovals)
								.where(
									and(
										eq(
											HostedProjectCloneApprovals.approval_id,
											query.approval_id,
										),
										eq(HostedProjectCloneApprovals.thread_id, query.thread_id),
									),
								)
								.limit(1);

							if (!row) {
								return yield* new HostedProjectCloneUnavailable({
									reason: "missing",
								});
							}

							const acceptance = yield* ValidateStoredBinding(transaction, row);

							return yield* Schema.decodeUnknownEffect(
								HostedProjectCloneApprovalQueryResult,
								{ onExcessProperty: "error" },
							)({ approval: acceptance.approval }).pipe(
								Effect.mapError(() =>
									invariant("Hosted clone query result is corrupt"),
								),
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReplayRequest = (input: unknown) =>
			Schema.decodeUnknownEffect(CloneRequestReplay, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(() => conflict("request_conflict")),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						FindBySourceCommand(transaction, decoded.source_command.message_id).pipe(
							Effect.flatMap(
								Option.match({
									onNone: () =>
										Effect.succeed(Option.none<HostedProjectCloneAcceptance>()),
									onSome: (row) =>
										Effect.gen(function* () {
											const approval = yield* DecodeApproval(row);
											const command = yield* ReadCommand(
												transaction,
												row.source_command_id,
											);
											const request_matches =
												row.thread_id === decoded.thread_id &&
												row.request_fingerprint ===
													decoded.request_fingerprint &&
												json_equals(
													approval.repository,
													public_repository(decoded.request),
												) &&
												command_matches(
													command,
													decoded.source_command,
													decoded.thread_id,
													"hosted.project.clone.request",
													request_payload({
														approval_id: row.approval_id,
														destination_path: row.destination_path,
														request_fingerprint:
															decoded.request_fingerprint,
													}),
												);

											if (!request_matches) {
												return yield* conflict("request_conflict");
											}

											if (approval.state !== "reused") {
												const artifact = yield* ReadArtifact(
													transaction,
													row,
													approval,
												);

												if (
													!json_equals(artifact.request, decoded.request)
												) {
													return yield* conflict("request_conflict");
												}
											}

											const acceptance = yield* ValidateStoredBinding(
												transaction,
												row,
											);

											return Option.some({
												...acceptance,
												status: "duplicate" as const,
											});
										}),
								}),
							),
						),
					),
				),
				Effect.mapError(normalize_error),
			);
		const ReadBySourceCommand = (source_command_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(source_command_id).pipe(
				Effect.mapError(() => invariant("Hosted clone request command id is invalid")),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						FindBySourceCommand(transaction, decoded).pipe(
							Effect.flatMap(
								Option.match({
									onNone: () =>
										Effect.succeed(Option.none<HostedProjectCloneAcceptance>()),
									onSome: (row) =>
										ValidateStoredBinding(transaction, row).pipe(
											Effect.map((acceptance) =>
												Option.some({
													...acceptance,
													status: "duplicate" as const,
												}),
											),
										),
								}),
							),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListApproved = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select({
							approval_id: HostedProjectCloneApprovals.approval_id,
							thread_id: HostedProjectCloneApprovals.thread_id,
						})
						.from(HostedProjectCloneApprovals)
						.where(eq(HostedProjectCloneApprovals.state, "approved"))
						.orderBy(
							asc(HostedProjectCloneApprovals.created_at),
							asc(HostedProjectCloneApprovals.approval_id),
						);

					yield* Effect.forEach(
						rows,
						(row) => EnsureLiveThread(transaction, row.thread_id),
						{ discard: true },
					);

					return rows;
				}),
			)
			.pipe(Effect.mapError(normalize_error));

		const ListExecuting = Effect.gen(function* () {
			const now = yield* metadata.Now;

			return yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(HostedProjectCloneApprovals)
						.where(eq(HostedProjectCloneApprovals.state, "executing"))
						.orderBy(
							asc(HostedProjectCloneApprovals.created_at),
							asc(HostedProjectCloneApprovals.approval_id),
						);

					return yield* Effect.forEach(rows, (row) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, row.thread_id);

							const approval = yield* DecodeApproval(row);
							const artifact = yield* ReadArtifact(transaction, row, approval);
							const claim = yield* ReadClaim(transaction, row, artifact);
							const expired = yield* LeaseExpired(claim.lease_expires_at, now);
							const safe =
								claim.execution_started_at === null ||
								artifact.clone_result !== undefined;
							const owned = claim.owner_instance_id === metadata.instance_id;
							const recovery: HostedProjectCloneDispatch["recovery"] = safe
								? owned
									? "owned"
									: expired
										? "recoverable"
										: "waiting"
								: expired
									? "quarantine"
									: "waiting";

							return {
								approval_id: row.approval_id,
								recovery,
								thread_id: row.thread_id,
							};
						}),
					);
				}),
			);
		}).pipe(Effect.mapError(normalize_error));

		const AbandonOwnedExecutions = Effect.gen(function* () {
			const now = yield* metadata.Now;

			yield* DecodeDateTime(now, "Hosted clone lease clock");
			yield* RetrySqliteWrite(
				database.client
					.update(HostedProjectCloneClaims)
					.set({ lease_expires_at: now })
					.where(eq(HostedProjectCloneClaims.owner_instance_id, metadata.instance_id)),
			);
		}).pipe(Effect.mapError(normalize_error));

		const ActiveClaimsForThread = (thread_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(thread_id).pipe(
				Effect.mapError(() => new HostedProjectCloneUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client
						.select({ approval_id: HostedProjectCloneClaims.approval_id })
						.from(HostedProjectCloneClaims)
						.where(eq(HostedProjectCloneClaims.thread_id, decoded))
						.limit(1),
				),
				Effect.map((rows) => rows.length === 1),
				Effect.mapError(normalize_error),
			);

		return {
			AbandonOwnedExecutions,
			ActiveClaimsForThread,
			ClaimRecovery,
			Decide,
			ExecuteClaimed,
			ListApproved,
			ListExecuting,
			MarkExecuting,
			Query,
			QuarantineInterrupted,
			ReadBySourceCommand,
			ReplayRequest,
			ReadExecution,
			RecordCloneResult,
			RecordRegisteredProject,
			RecordReused,
			RenewLease,
			Request,
			Settle,
		};
	}),
);
