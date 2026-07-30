import { Context, Data, Effect, Schema } from "effect";

import {
	ContentIdentity,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspacePath,
} from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreAuthorizationError } from "../../filesystem/workspace-bounded-regular-file-store-registry";
import { type BoundedRegularFileStore } from "../../filesystem/bounded-regular-file-store";
import { CommandIdConflict } from "../../persistence/journal-store";
import {
	WorkspaceChanges,
	WorkspaceChangeOperations,
	WorkspaceMutationAuthorities,
} from "../../persistence/tables";
import {
	WorkspaceChangeIdConflict,
	WorkspaceChangeTransitionError,
	type ClaimRollback,
	type ClaimReplace,
	type WorkspaceChangeClaim,
} from "../changes/repository";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));

export const WorkspaceMutationClaimReplace = Schema.Struct({
	_tag: Schema.Literal("replace"),
	agent_id: Identifier,
	change_id: Identifier,
	expected_before: ContentIdentity,
	intended_after: ContentIdentity,
	message_id: Identifier,
	path: WorkspacePath,
	raw_origin: Schema.optional(RawOrigin),
	request_fingerprint: RequestFingerprint,
	run_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

export const WorkspaceMutationClaimRollback = Schema.Struct({
	_tag: Schema.Literal("rollback"),
	change_id: Identifier,
	expected_after: ContentIdentity,
	message_id: Identifier,
	request_fingerprint: RequestFingerprint,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const StoredAuthorityBase = {
	agent_id: Identifier,
	change_id: Identifier,
	created_at: IsoDateTime,
	message_id: Identifier,
	run_id: Identifier,
	thread_id: Identifier,
	working_directory: Schema.NonEmptyString,
	workspace_id: Identifier,
};

const StoredWorkspaceMutationAuthority = Schema.Union([
	Schema.Struct({
		...StoredAuthorityBase,
		approval: Schema.Null,
		assignment_id: Schema.Null,
		authority_kind: Schema.Literal("base_run"),
		group_id: Schema.Null,
		scope_kind: Schema.Null,
		scope_value: Schema.Null,
	}),
	Schema.Struct({
		...StoredAuthorityBase,
		approval: Schema.Literals(["never", "on_request", "always"]),
		assignment_id: Identifier,
		authority_kind: Schema.Literal("graph_run"),
		group_id: Identifier,
		scope_kind: Schema.Literals(["repo", "files"]),
		scope_value: Schema.NonEmptyString,
	}),
]);

/** Identifies a controlled replacement claim. */
export type WorkspaceMutationClaimReplace = typeof WorkspaceMutationClaimReplace.Type;

/** Identifies a user-initiated rollback against one committed controlled replacement. */
export type WorkspaceMutationClaimRollback = typeof WorkspaceMutationClaimRollback.Type;

/** Describes the durable authority that admitted a controlled mutation. */
export type WorkspaceMutationAuthorityGrant =
	| {
			readonly _tag: "base_run";
			readonly agent_id: string;
			readonly run_id: string;
			readonly thread_id: string;
			readonly workspace_id: string;
	  }
	| {
			readonly _tag: "graph_run";
			readonly agent_id: string;
			readonly approval: "never" | "on_request" | "always";
			readonly assignment_id: string;
			readonly group_id: string;
			readonly run_id: string;
			readonly scope: "repo" | "files";
			readonly thread_id: string;
			readonly workspace_id: string;
	  };

/** Returns an atomic mutation claim with its authorized bounded file capability. */
export interface WorkspaceMutationAdmission {
	readonly authority: WorkspaceMutationAuthorityGrant;
	readonly claim: WorkspaceChangeClaim;
	readonly store: typeof BoundedRegularFileStore.Service;
}

/** Carries the immutable source data validated before rollback admission. */
export interface WorkspaceMutationRollbackSource {
	readonly after_identity: typeof ContentIdentity.Type;
	readonly before_identity: typeof ContentIdentity.Type;
	readonly path: string;
	readonly workspace_id: string;
}

/** Returns a rollback claim bound to its validated immutable source data. */
export type WorkspaceMutationRollbackAdmission =
	| {
			readonly _tag: "authorized";
			readonly authority: WorkspaceMutationAuthorityGrant;
			readonly claim: Extract<
				WorkspaceChangeClaim,
				{ readonly _tag: "claimed" | "incomplete_retry" }
			>;
			readonly source: WorkspaceMutationRollbackSource;
			readonly store: typeof BoundedRegularFileStore.Service;
	  }
	| {
			readonly _tag: "duplicate";
			readonly authority: WorkspaceMutationAuthorityGrant;
			readonly claim: Extract<WorkspaceChangeClaim, { readonly _tag: "duplicate" }>;
			readonly source: WorkspaceMutationRollbackSource;
	  }
	| {
			readonly _tag: "rejected";
			readonly authority: WorkspaceMutationAuthorityGrant;
			readonly claim: Extract<WorkspaceChangeClaim, { readonly _tag: "rejected" }>;
			readonly source: WorkspaceMutationRollbackSource;
	  };

export type WorkspaceMutationAuthorityDenialReason =
	| "identity_mismatch"
	| "path_outside_scope"
	| "run_not_active"
	| "thread_unavailable"
	| "unsupported_scope"
	| "workspace_mismatch"
	| "workspace_unavailable"
	| "write_not_allowed";

/** Reports a malformed controlled-mutation request without echoing its content. */
export class WorkspaceMutationAuthorityInvalid extends Data.TaggedError(
	"WorkspaceMutationAuthorityInvalid",
)<{ readonly operation: "claim_replace" | "claim_rollback" }> {}

/** Reports a live authority proof that does not permit the requested mutation. */
export class WorkspaceMutationAuthorityDenied extends Data.TaggedError(
	"WorkspaceMutationAuthorityDenied",
)<{ readonly reason: WorkspaceMutationAuthorityDenialReason }> {}

/** Reports a reused operation whose durable state prevents mutation admission. */
export class WorkspaceMutationAuthorityConflict extends Data.TaggedError(
	"WorkspaceMutationAuthorityConflict",
)<{
	readonly reason:
		| "authority_conflict"
		| "operation_conflict"
		| "operation_rejected"
		| "unpinned_operation";
}> {}

/** Conceals corrupt state and unexpected persistence failures from mutation callers. */
export class WorkspaceMutationAuthorityFailure extends Data.TaggedError(
	"WorkspaceMutationAuthorityFailure",
)<{ readonly reason: "invalid_persisted_state" | "persistence_failure" }> {}

export type WorkspaceMutationAuthorityError =
	| WorkspaceMutationAuthorityConflict
	| WorkspaceMutationAuthorityDenied
	| WorkspaceMutationAuthorityFailure
	| WorkspaceMutationAuthorityInvalid;

/** Atomically proves run authority and claims one controlled workspace mutation. */
export class WorkspaceMutationAuthority extends Context.Service<
	WorkspaceMutationAuthority,
	{
		readonly ClaimReplace: (
			input: WorkspaceMutationClaimReplace,
		) => Effect.Effect<WorkspaceMutationAdmission, WorkspaceMutationAuthorityError>;
		readonly ClaimRollback: (
			input: WorkspaceMutationClaimRollback,
		) => Effect.Effect<WorkspaceMutationRollbackAdmission, WorkspaceMutationAuthorityError>;
	}
>()("Artisan/WorkspaceMutationAuthority") {}

export type StoredAuthority = typeof StoredWorkspaceMutationAuthority.Type;

export interface AuthorityProof {
	readonly authority: WorkspaceMutationAuthorityGrant;
	readonly store: typeof BoundedRegularFileStore.Service;
	readonly stored: Omit<typeof WorkspaceMutationAuthorities.$inferInsert, "created_at">;
}

export function invalid_state() {
	return new WorkspaceMutationAuthorityFailure({ reason: "invalid_persisted_state" });
}

export function denied(reason: WorkspaceMutationAuthorityDenialReason) {
	return new WorkspaceMutationAuthorityDenied({ reason });
}

export function DecodeReplaceClaim(input: unknown) {
	return Schema.decodeUnknownEffect(WorkspaceMutationClaimReplace, {
		onExcessProperty: "error",
	})(input).pipe(
		Effect.mapError(
			() => new WorkspaceMutationAuthorityInvalid({ operation: "claim_replace" }),
		),
	);
}

export function DecodeRollbackClaim(input: unknown) {
	return Schema.decodeUnknownEffect(WorkspaceMutationClaimRollback, {
		onExcessProperty: "error",
	})(input).pipe(
		Effect.mapError(
			() => new WorkspaceMutationAuthorityInvalid({ operation: "claim_rollback" }),
		),
	);
}

export function DecodeStoredJson<A>(schema: Schema.Codec<A, A>, value: string) {
	return Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError(invalid_state),
	);
}

function DecodeRequiredStoredJson<A>(schema: Schema.Codec<A, A>, value: string | null) {
	if (value === null) {
		return Effect.fail(invalid_state());
	}

	return DecodeStoredJson(schema, value);
}

function DecodeOptionalStoredJson<A>(schema: Schema.Codec<A, A>, value: string | null) {
	return value === null ? Effect.succeed(undefined) : DecodeStoredJson(schema, value);
}

function DecodeRequiredStoredValue<A>(schema: Schema.Codec<A, A>, value: unknown) {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(invalid_state),
	);
}

export function DecodeStoredAuthority(row: unknown) {
	return Schema.decodeUnknownEffect(StoredWorkspaceMutationAuthority, {
		onExcessProperty: "error",
	})(row).pipe(Effect.mapError(invalid_state));
}

export function grant_from_stored(authority: StoredAuthority): WorkspaceMutationAuthorityGrant {
	if (authority.authority_kind === "base_run") {
		return {
			_tag: "base_run",
			agent_id: authority.agent_id,
			run_id: authority.run_id,
			thread_id: authority.thread_id,
			workspace_id: authority.workspace_id,
		};
	}

	return {
		_tag: "graph_run",
		agent_id: authority.agent_id,
		approval: authority.approval,
		assignment_id: authority.assignment_id,
		group_id: authority.group_id,
		run_id: authority.run_id,
		scope: authority.scope_kind,
		thread_id: authority.thread_id,
		workspace_id: authority.workspace_id,
	};
}

export function authority_matches_claim(
	authority: StoredAuthority,
	claim: WorkspaceMutationClaimReplace,
) {
	return (
		authority.agent_id === claim.agent_id &&
		authority.change_id === claim.change_id &&
		authority.message_id === claim.message_id &&
		authority.run_id === claim.run_id &&
		authority.thread_id === claim.thread_id &&
		authority.workspace_id === claim.workspace_id
	);
}

function identities_match(left: typeof ContentIdentity.Type, right: typeof ContentIdentity.Type) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

function raw_origins_match(
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

export function ValidateSourceAuthority(
	authority: StoredAuthority,
	operation: typeof WorkspaceChangeOperations.$inferSelect,
	change: typeof WorkspaceChanges.$inferSelect,
) {
	return Effect.gen(function* () {
		const decoded = yield* Effect.all({
			change_after: DecodeStoredJson(ContentIdentity, change.after_identity_json),
			change_before: DecodeStoredJson(ContentIdentity, change.before_identity_json),
			change_path: DecodeRequiredStoredValue(WorkspacePath, change.path),
			change_raw_origin: DecodeOptionalStoredJson(RawOrigin, change.raw_origin_json),
			operation_after: DecodeRequiredStoredJson(
				ContentIdentity,
				operation.result_identity_json,
			),
			operation_before: DecodeRequiredStoredJson(
				ContentIdentity,
				operation.expected_identity_json,
			),
			operation_path: DecodeRequiredStoredValue(WorkspacePath, operation.path),
			operation_raw_origin: DecodeOptionalStoredJson(RawOrigin, operation.raw_origin_json),
		});

		if (
			operation.action !== "replace" ||
			operation.agent_id !== authority.agent_id ||
			operation.change_id !== authority.change_id ||
			operation.lifecycle !== "committed" ||
			operation.message_id !== authority.message_id ||
			decoded.operation_path !== decoded.change_path ||
			operation.run_id !== authority.run_id ||
			operation.thread_id !== authority.thread_id ||
			operation.workspace_id !== authority.workspace_id ||
			change.agent_id !== authority.agent_id ||
			change.change_id !== authority.change_id ||
			change.run_id !== authority.run_id ||
			change.source_command_id !== authority.message_id ||
			change.thread_id !== authority.thread_id ||
			change.workspace_id !== authority.workspace_id ||
			!identities_match(decoded.operation_before, decoded.change_before) ||
			!identities_match(decoded.operation_after, decoded.change_after) ||
			!raw_origins_match(decoded.operation_raw_origin, decoded.change_raw_origin)
		) {
			return yield* Effect.fail(invalid_state());
		}

		return yield* Schema.decodeUnknownEffect(WorkspaceMutationClaimReplace, {
			onExcessProperty: "error",
		})({
			_tag: "replace",
			agent_id: authority.agent_id,
			change_id: authority.change_id,
			expected_before: decoded.operation_before,
			intended_after: decoded.operation_after,
			message_id: authority.message_id,
			path: decoded.operation_path,
			...(decoded.operation_raw_origin === undefined
				? {}
				: { raw_origin: decoded.operation_raw_origin }),
			request_fingerprint: operation.request_fingerprint,
			run_id: authority.run_id,
			sent_at: operation.sent_at,
			thread_id: authority.thread_id,
			workspace_id: authority.workspace_id,
		}).pipe(Effect.mapError(invalid_state));
	});
}

export function file_scope_contains(scope: string, path: string) {
	return path === scope || path.startsWith(`${scope}/`);
}

export function repository_claim_from(claim: WorkspaceMutationClaimReplace): ClaimReplace {
	return {
		_tag: "replace",
		agent_id: claim.agent_id,
		change_id: claim.change_id,
		expected_before: claim.expected_before,
		intended_after: claim.intended_after,
		message_id: claim.message_id,
		path: claim.path,
		...(claim.raw_origin === undefined ? {} : { raw_origin: claim.raw_origin }),
		request_fingerprint: claim.request_fingerprint,
		run_id: claim.run_id,
		sent_at: claim.sent_at,
		thread_id: claim.thread_id,
		workspace_id: claim.workspace_id,
	};
}

export function rollback_repository_claim_from(
	claim: WorkspaceMutationClaimRollback,
): ClaimRollback {
	return {
		_tag: "rollback",
		change_id: claim.change_id,
		expected_after: claim.expected_after,
		message_id: claim.message_id,
		request_fingerprint: claim.request_fingerprint,
		sent_at: claim.sent_at,
		thread_id: claim.thread_id,
	};
}

export function rollback_source_from(
	claim: WorkspaceMutationClaimReplace,
): WorkspaceMutationRollbackSource {
	return {
		after_identity: claim.intended_after,
		before_identity: claim.expected_before,
		path: claim.path,
		workspace_id: claim.workspace_id,
	};
}

export function conceal_error(error: unknown): WorkspaceMutationAuthorityError {
	if (
		error instanceof WorkspaceMutationAuthorityConflict ||
		error instanceof WorkspaceMutationAuthorityDenied ||
		error instanceof WorkspaceMutationAuthorityFailure ||
		error instanceof WorkspaceMutationAuthorityInvalid
	) {
		return error;
	}

	if (error instanceof WorkspaceBoundedRegularFileStoreAuthorizationError) {
		return denied("workspace_unavailable");
	}

	if (error instanceof CommandIdConflict || error instanceof WorkspaceChangeIdConflict) {
		return new WorkspaceMutationAuthorityConflict({ reason: "operation_conflict" });
	}

	if (error instanceof WorkspaceChangeTransitionError) {
		return denied("thread_unavailable");
	}

	return new WorkspaceMutationAuthorityFailure({ reason: "persistence_failure" });
}

/** Supplies the SQLite-backed transactional mutation authority. */
