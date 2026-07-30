import { Effect, Schema } from "effect";

import {
	ContentIdentity,
	RawOrigin,
	WorkspaceChange,
	WorkspaceConflict,
	type RawOrigin as RawOriginValue,
	type WorkspaceChange as WorkspaceChangeValue,
} from "@artisan/protocol";

import {
	JournalEvents,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceConflicts,
} from "../../persistence/tables";
import { JournalInvariantError } from "../../persistence/journal-store";
import {
	WorkspaceChangeJournalEvent,
	WorkspaceChangeOperationSchema,
	type WorkspaceChangeOperation,
} from "./model";

const DecodeJson = Schema.decodeUnknownEffect(Schema.UnknownFromJsonString);

const optional_fields = <T extends Readonly<Record<string, unknown>>>(input: T) =>
	Object.fromEntries(Object.entries(input).filter(([, value]) => value != null));

const operation_state_is_valid = (operation: WorkspaceChangeOperation) => {
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
};

const change_state_is_valid = (change: WorkspaceChangeValue) => {
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
};

export const DecodeStoredRawOrigin = (raw_origin_json: string | null, message: string) => {
	if (raw_origin_json === null) {
		return Effect.succeed<RawOriginValue | undefined>(undefined);
	}

	return DecodeJson(raw_origin_json).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(RawOrigin)),
		Effect.map((raw_origin): RawOriginValue | undefined => raw_origin),
		Effect.mapError(() => new JournalInvariantError({ message })),
	);
};

export const DecodeOperation = (row: typeof WorkspaceChangeOperations.$inferSelect) =>
	Effect.all({
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
			Schema.decodeUnknownEffect(WorkspaceChangeOperationSchema, {
				onExcessProperty: "error",
			})(
				optional_fields({
					action: row.action,
					agent_id: row.agent_id,
					change_id: row.change_id,
					diff_format_version: row.diff_format_version,
					evidence_recorded: row.evidence_recorded,
					expected_identity: identities.expected_identity,
					journal_sequence: row.journal_sequence,
					lifecycle: row.lifecycle,
					message_id: row.message_id,
					path: row.path,
					raw_origin: identities.raw_origin,
					reviewer_kind:
						row.action === "review" ? (row.reviewer_kind ?? "user") : undefined,
					assignment_id: row.reviewer_assignment_id,
					comment: row.review_comment,
					group_id: row.reviewer_group_id,
					outcome: row.review_outcome,
					reviewer_agent_id: row.reviewer_agent_id,
					reviewer_run_id: row.reviewer_run_id,
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

export const DecodeChange = (row: typeof WorkspaceChanges.$inferSelect) =>
	Effect.all({
		reviewer_raw_origin:
			row.reviewer_raw_origin_json === null
				? Effect.succeed(undefined)
				: DecodeJson(row.reviewer_raw_origin_json).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(RawOrigin)),
					),
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
					review:
						row.review_source_command_id === null
							? undefined
							: optional_fields({
									assignment_id: row.reviewer_assignment_id,
									comment: row.review_comment,
									group_id: row.reviewer_group_id,
									outcome: row.review_outcome,
									reviewer_kind: row.reviewer_kind ?? "user",
									raw_origin: json.reviewer_raw_origin,
									reviewer_agent_id: row.reviewer_agent_id,
									reviewer_run_id: row.reviewer_run_id,
									reviewed_at: row.reviewed_at,
									source_command_id: row.review_source_command_id,
								}),
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

export const DecodeConflict = (row: typeof WorkspaceConflicts.$inferSelect) =>
	Effect.all({
		expected_identity: DecodeJson(row.expected_identity_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
		),
		observed_identity:
			row.observed_identity_json === null
				? Effect.succeed(undefined)
				: DecodeJson(row.observed_identity_json).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
					),
		raw_origin: DecodeStoredRawOrigin(row.raw_origin_json, "Stored conflict origin is invalid"),
	}).pipe(
		Effect.flatMap((decoded) =>
			Schema.decodeUnknownEffect(WorkspaceConflict)(optional_fields({ ...row, ...decoded })),
		),
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Stored workspace conflict ${row.conflict_id} is invalid`,
				}),
		),
	);

export const DecodeEvent = (row: typeof JournalEvents.$inferSelect) =>
	DecodeJson(row.payload_json).pipe(
		Effect.flatMap((payload) =>
			Schema.decodeUnknownEffect(WorkspaceChangeJournalEvent, {
				onExcessProperty: "error",
			})({
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
