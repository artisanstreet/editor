import { Schema } from "effect";

import {
	workspace_diff_format_version,
	type ContentIdentity as ContentIdentityValue,
	type RawOrigin as RawOriginValue,
	type WorkspaceChange as WorkspaceChangeValue,
} from "@artisan/protocol";

import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
} from "../../persistence/journal-store";
import {
	WorkspaceChangeIdConflict,
	WorkspaceChangeTransitionError,
	type ClaimReplace,
	type ClaimReview,
	type ClaimRollback,
	type WorkspaceChangeEvent,
	type WorkspaceChangeOperation,
	type WorkspaceChangeRepositoryError,
} from "./model";

export {
	WorkspaceChangeIdConflict,
	WorkspaceChangeRepository,
	WorkspaceChangeTransitionError,
	type ClaimReplace,
	type ClaimReview,
	type ClaimRollback,
	type ReconcileWorkspaceChange,
	type WorkspaceChangeClaim,
	type WorkspaceChangeCommit,
	type WorkspaceChangeEvent,
	type WorkspaceChangeOperation,
	type WorkspaceChangeReconciliation,
	type WorkspaceChangeRepositoryError,
} from "./model";

export const DecodeJson = Schema.decodeUnknownEffect(Schema.UnknownFromJsonString);

export function normalize_error(error: unknown): WorkspaceChangeRepositoryError {
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

export function normalize_commit_error(error: unknown): WorkspaceChangeRepositoryError {
	if (
		error instanceof CommandIdConflict ||
		error instanceof JournalInvariantError ||
		error instanceof WorkspaceChangeIdConflict ||
		error instanceof WorkspaceChangeTransitionError
	) {
		return error;
	}

	return new JournalStoreFailure({
		cause: new Error("Workspace change commit persistence failed"),
	});
}

export function bytes_match(left: Uint8Array, right: Uint8Array) {
	return (
		left.byteLength === right.byteLength && left.every((byte, index) => byte === right[index])
	);
}

export function command_payload_json(operation: WorkspaceChangeOperation) {
	return JSON.stringify({
		action: operation.action,
		change_id: operation.change_id,
		request_fingerprint: operation.request_fingerprint,
		type: "workspace.change.command",
	});
}

export function identities_match(left: ContentIdentityValue, right: ContentIdentityValue) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

export function raw_origins_match(
	left: RawOriginValue | undefined,
	right: RawOriginValue | undefined,
) {
	return (
		(left === undefined && right === undefined) ||
		(left !== undefined &&
			right !== undefined &&
			left.provider === right.provider &&
			left.reference === right.reference)
	);
}

export function event_action_for(operation: WorkspaceChangeOperation) {
	return operation.action === "replace"
		? "recorded"
		: operation.action === "review"
			? "reviewed"
			: "rolled_back";
}

export function change_identity_matches(left: WorkspaceChangeValue, right: WorkspaceChangeValue) {
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

export function event_matches_operation(
	event: WorkspaceChangeEvent,
	operation: WorkspaceChangeOperation,
) {
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

export function operation_from_claim(
	input: ClaimReplace | ClaimReview | ClaimRollback,
): WorkspaceChangeOperation {
	const base = {
		change_id: input.change_id,
		diff_format_version: workspace_diff_format_version as 1,
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

	return {
		...base,
		action: "review",
		reviewer_kind: input.reviewer_kind ?? "user",
		...(input.assignment_id === undefined ? {} : { assignment_id: input.assignment_id }),
		...(input.comment === undefined ? {} : { comment: input.comment }),
		...(input.group_id === undefined ? {} : { group_id: input.group_id }),
		...(input.outcome === undefined ? {} : { outcome: input.outcome }),
		...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
		...(input.reviewer_agent_id === undefined
			? {}
			: { reviewer_agent_id: input.reviewer_agent_id }),
		...(input.reviewer_run_id === undefined ? {} : { reviewer_run_id: input.reviewer_run_id }),
	};
}

export function immutable_operations_match(
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
		return (
			stored.reviewer_kind === claimed.reviewer_kind &&
			stored.assignment_id === claimed.assignment_id &&
			stored.comment === claimed.comment &&
			stored.group_id === claimed.group_id &&
			stored.outcome === claimed.outcome &&
			raw_origins_match(stored.raw_origin, claimed.raw_origin) &&
			stored.reviewer_agent_id === claimed.reviewer_agent_id &&
			stored.reviewer_run_id === claimed.reviewer_run_id
		);
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
