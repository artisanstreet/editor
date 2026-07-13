import { Schema } from "effect";

import { Identifier, IsoDateTime, JournalSequence, PositiveInt, RawOrigin } from "./common";

const text_encoder = new TextEncoder();

/** Defines the V1 control-frame ceiling for workspace source text at four MiB of UTF-8 data. */
export const workspace_text_maximum_bytes = 4 * 1024 * 1024;

/** Defines the maximum UTF-8 size of one workspace diff patch. */
export const workspace_diff_maximum_bytes = 16 * 1024 * 1024;

/** Defines the maximum number of added or removed lines in one workspace diff. */
export const workspace_diff_maximum_lines_per_side = 100_000;

/** Defines the maximum rendered line count for one workspace diff. */
export const workspace_diff_maximum_rendered_lines = 250_000;

/** Defines the number of context lines emitted around workspace diff changes. */
export const workspace_diff_context_lines = 3;

/** Defines the maximum visible character count for one workspace approval reason. */
export const workspace_replace_approval_reason_maximum_characters = 4096;

/** Identifies the immutable unified workspace-diff representation emitted by V1. */
export const workspace_diff_format_version = 1;

const has_control_character = (value: string) => /[\p{Cc}]/u.test(value);

/** Validates one bounded, visible reason supplied with a requested replacement approval. */
export const WorkspaceReplaceApprovalReason = Schema.String.check(
	Schema.makeFilter<string>((reason) => {
		const normalized = reason.trim().replace(/\s+/g, " ");

		return reason.length > workspace_replace_approval_reason_maximum_characters
			? `Expected at most ${workspace_replace_approval_reason_maximum_characters} input characters`
			: has_control_character(reason)
				? "Expected visible text without control characters"
				: normalized.length === 0
					? "Expected a non-empty visible approval reason"
					: undefined;
	}),
);

export type WorkspaceReplaceApprovalReason = typeof WorkspaceReplaceApprovalReason.Type;

/** Validates a canonical slash-separated relative path that identifies one workspace file. */
export const WorkspacePath = Schema.String.check(
	Schema.makeFilter<string>((path) => {
		const has_invalid_segment = path
			.split("/")
			.some((segment) => segment.length === 0 || segment === "." || segment === "..");
		const has_control_character = /[\p{Cc}]/u.test(path);
		const has_windows_drive_prefix = /^[a-z]:/iu.test(path);
		const byte_count = text_encoder.encode(path).byteLength;

		return path.length === 0 ||
			byte_count > 4096 ||
			has_control_character ||
			has_windows_drive_prefix ||
			path.includes("\\") ||
			path.startsWith("/") ||
			path.endsWith("/") ||
			has_invalid_segment
			? "Expected a non-empty canonical relative file path without traversal or control characters"
			: undefined;
	}),
);

export type WorkspacePath = typeof WorkspacePath.Type;

/** Validates V1 replacement text within the negotiated four MiB UTF-8 control-frame bound. */
export const WorkspaceTextContent = Schema.String.check(
	Schema.makeFilter<string>((content) =>
		text_encoder.encode(content).byteLength <= workspace_text_maximum_bytes
			? undefined
			: `Expected at most ${workspace_text_maximum_bytes} UTF-8 bytes`,
	),
);

export type WorkspaceTextContent = typeof WorkspaceTextContent.Type;

/** Identifies UTF-8 workspace content without carrying that content in a durable projection. */
export const ContentIdentity = Schema.Struct({
	algorithm: Schema.Literal("sha256"),
	byte_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(workspace_text_maximum_bytes),
	),
	content_hash: Schema.String.check(
		Schema.isPattern(/^[a-f0-9]{64}$/, { message: "Expected a lowercase SHA-256 hash" }),
	),
});

export type ContentIdentity = typeof ContentIdentity.Type;

/** Validates a UTF-8 unified workspace diff patch within the V1 byte bound. */
export const WorkspaceDiffPatch = Schema.String.check(
	Schema.makeFilter<string>((patch) =>
		text_encoder.encode(patch).byteLength <= workspace_diff_maximum_bytes
			? undefined
			: `Expected at most ${workspace_diff_maximum_bytes} UTF-8 bytes`,
	),
);

export type WorkspaceDiffPatch = typeof WorkspaceDiffPatch.Type;

/** Identifies one bounded UTF-8 workspace diff patch. */
export const WorkspaceDiffIdentity = Schema.Struct({
	algorithm: Schema.Literal("sha256"),
	byte_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(workspace_diff_maximum_bytes),
	),
	content_hash: Schema.String.check(
		Schema.isPattern(/^[a-f0-9]{64}$/, { message: "Expected a lowercase SHA-256 hash" }),
	),
});

export type WorkspaceDiffIdentity = typeof WorkspaceDiffIdentity.Type;

/** Represents the human review lifecycle for one recorded workspace change. */
export const WorkspaceChangeReviewState = Schema.Literals([
	"needs_review",
	"reviewed",
	"rolled_back",
]);

export type WorkspaceChangeReviewState = typeof WorkspaceChangeReviewState.Type;

/** Represents whether one recorded workspace change can still be rolled back. */
export const WorkspaceChangeRollbackState = Schema.Literals(["available", "consumed"]);

export type WorkspaceChangeRollbackState = typeof WorkspaceChangeRollbackState.Type;

/** Projects an attributed, reviewable replacement of one existing UTF-8 workspace file. */
export const WorkspaceChange = Schema.Struct({
	after_identity: ContentIdentity,
	agent_id: Identifier,
	before_identity: ContentIdentity,
	change_id: Identifier,
	created_at: IsoDateTime,
	path: WorkspacePath,
	raw_origin: Schema.optional(RawOrigin),
	review_state: WorkspaceChangeReviewState,
	reviewed_at: Schema.optional(IsoDateTime),
	rollback_state: WorkspaceChangeRollbackState,
	rolled_back_at: Schema.optional(IsoDateTime),
	run_id: Identifier,
	source_command_id: Identifier,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	version: PositiveInt,
	workspace_id: Identifier,
});

export type WorkspaceChange = typeof WorkspaceChange.Type;

/** Announces a recorded, reviewed, or rolled-back workspace change projection. */
export const WorkspaceChangeUpdatedEvent = Schema.Struct({
	action: Schema.Literals(["recorded", "reviewed", "rolled_back"]),
	change: WorkspaceChange,
	type: Schema.Literal("workspace.change.updated"),
});

export type WorkspaceChangeUpdatedEvent = typeof WorkspaceChangeUpdatedEvent.Type;

/** Describes the assignment policy that made a workspace replacement require approval. */
export const WorkspaceReplaceApprovalPolicy = Schema.Literals(["on_request", "always"]);

export type WorkspaceReplaceApprovalPolicy = typeof WorkspaceReplaceApprovalPolicy.Type;

const WorkspaceReplaceApprovalBase = {
	after_identity: ContentIdentity,
	agent_id: Identifier,
	approval_id: Identifier,
	before_identity: ContentIdentity,
	change_id: Identifier,
	created_at: IsoDateTime,
	path: WorkspacePath,
	policy: WorkspaceReplaceApprovalPolicy,
	reason: WorkspaceReplaceApprovalReason,
	run_id: Identifier,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	workspace_id: Identifier,
};

const WorkspaceReplaceApprovalDecision = {
	decided_at: IsoDateTime,
	decision_message_id: Identifier,
};

/** Projects a pending workspace replacement approval without source or patch bytes. */
export const WorkspaceReplaceApprovalRequested = Schema.Struct({
	...WorkspaceReplaceApprovalBase,
	state: Schema.Literal("requested"),
});

export type WorkspaceReplaceApprovalRequested = typeof WorkspaceReplaceApprovalRequested.Type;

/** Projects an approved workspace replacement awaiting execution. */
export const WorkspaceReplaceApprovalApproved = Schema.Struct({
	...WorkspaceReplaceApprovalBase,
	...WorkspaceReplaceApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("approved"),
});

export type WorkspaceReplaceApprovalApproved = typeof WorkspaceReplaceApprovalApproved.Type;

/** Projects an approved workspace replacement while the replacement is executing. */
export const WorkspaceReplaceApprovalExecuting = Schema.Struct({
	...WorkspaceReplaceApprovalBase,
	...WorkspaceReplaceApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("executing"),
});

export type WorkspaceReplaceApprovalExecuting = typeof WorkspaceReplaceApprovalExecuting.Type;

/** Projects a workspace replacement that was denied by an approver. */
export const WorkspaceReplaceApprovalDenied = Schema.Struct({
	...WorkspaceReplaceApprovalBase,
	...WorkspaceReplaceApprovalDecision,
	decision: Schema.Literal("denied"),
	state: Schema.Literal("denied"),
});

export type WorkspaceReplaceApprovalDenied = typeof WorkspaceReplaceApprovalDenied.Type;

/** Projects an approved workspace replacement that was applied successfully. */
export const WorkspaceReplaceApprovalApplied = Schema.Struct({
	...WorkspaceReplaceApprovalBase,
	...WorkspaceReplaceApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("applied"),
});

export type WorkspaceReplaceApprovalApplied = typeof WorkspaceReplaceApprovalApplied.Type;

/** Projects an approved workspace replacement that could not be completed. */
export const WorkspaceReplaceApprovalRejected = Schema.Struct({
	...WorkspaceReplaceApprovalBase,
	...WorkspaceReplaceApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("rejected"),
});

export type WorkspaceReplaceApprovalRejected = typeof WorkspaceReplaceApprovalRejected.Type;

/** Represents every source-free lifecycle state for one workspace replacement approval. */
export const WorkspaceReplaceApproval = Schema.Union([
	WorkspaceReplaceApprovalRequested,
	WorkspaceReplaceApprovalApproved,
	WorkspaceReplaceApprovalExecuting,
	WorkspaceReplaceApprovalDenied,
	WorkspaceReplaceApprovalApplied,
	WorkspaceReplaceApprovalRejected,
]);

export type WorkspaceReplaceApproval = typeof WorkspaceReplaceApproval.Type;

/** Announces one source-free workspace replacement approval lifecycle update. */
export const WorkspaceReplaceApprovalUpdatedEvent = Schema.Struct({
	approval: WorkspaceReplaceApproval,
	type: Schema.Literal("workspace.replace.approval.updated"),
});

export type WorkspaceReplaceApprovalUpdatedEvent = typeof WorkspaceReplaceApprovalUpdatedEvent.Type;

/** Requests the current UTF-8 content and identity for one canonical workspace file. */
export const WorkspaceFileReadQuery = Schema.Struct({
	path: WorkspacePath,
	workspace_id: Identifier,
});

export type WorkspaceFileReadQuery = typeof WorkspaceFileReadQuery.Type;

/** Returns the current UTF-8 content and identity for one canonical workspace file. */
export const WorkspaceFileReadQueryResult = Schema.Struct({
	content: WorkspaceTextContent,
	identity: ContentIdentity,
	path: WorkspacePath,
	workspace_id: Identifier,
});

export type WorkspaceFileReadQueryResult = typeof WorkspaceFileReadQueryResult.Type;

/** Requests an attributed replacement of one existing UTF-8 regular workspace file. */
export const WorkspaceFileReplaceRequest = Schema.Struct({
	approval_request: Schema.optional(
		Schema.Struct({
			reason: WorkspaceReplaceApprovalReason,
		}),
	),
	change_id: Identifier,
	content: WorkspaceTextContent,
	expected_before: ContentIdentity,
	path: WorkspacePath,
	workspace_id: Identifier,
});

export type WorkspaceFileReplaceRequest = typeof WorkspaceFileReplaceRequest.Type;

/** Requests that one workspace change be marked as reviewed. */
export const WorkspaceChangeReviewRequest = Schema.Struct({
	change_id: Identifier,
});

export type WorkspaceChangeReviewRequest = typeof WorkspaceChangeReviewRequest.Type;

/** Requests a guarded rollback of one workspace change. */
export const WorkspaceChangeRollbackRequest = Schema.Struct({
	change_id: Identifier,
	expected_after: ContentIdentity,
});

export type WorkspaceChangeRollbackRequest = typeof WorkspaceChangeRollbackRequest.Type;

/** Requests workspace changes attributed to one thread, optionally within one workspace. */
export const WorkspaceChangeListQuery = Schema.Struct({
	thread_id: Identifier,
	workspace_id: Schema.optional(Identifier),
});

export type WorkspaceChangeListQuery = typeof WorkspaceChangeListQuery.Type;

/** Returns the workspace-change projection at one durable journal position. */
export const WorkspaceChangeListQueryResult = Schema.Struct({
	changes: Schema.Array(WorkspaceChange),
	journal_sequence: JournalSequence,
});

export type WorkspaceChangeListQueryResult = typeof WorkspaceChangeListQueryResult.Type;

/** Requests the unified diff for one recorded workspace change. */
export const WorkspaceChangeDiffQuery = Schema.Struct({
	change_id: Identifier,
	thread_id: Identifier,
});

export type WorkspaceChangeDiffQuery = typeof WorkspaceChangeDiffQuery.Type;

const WorkspaceChangeDiffQueryResultBase = Schema.Struct({
	added_line_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(workspace_diff_maximum_lines_per_side),
	),
	after_identity: ContentIdentity,
	before_identity: ContentIdentity,
	change_id: Identifier,
	context_lines: Schema.Literal(workspace_diff_context_lines),
	format: Schema.Literal("unified"),
	format_version: Schema.Literal(workspace_diff_format_version),
	patch: WorkspaceDiffPatch,
	patch_identity: WorkspaceDiffIdentity,
	path: WorkspacePath,
	removed_line_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(workspace_diff_maximum_lines_per_side),
	),
	thread_id: Identifier,
	truncated: Schema.Literal(false),
	workspace_id: Identifier,
});

/** Returns one bounded, content-addressed unified diff for a workspace change. */
export const WorkspaceChangeDiffQueryResult = WorkspaceChangeDiffQueryResultBase.check(
	Schema.makeFilter<typeof WorkspaceChangeDiffQueryResultBase.Type>((result) =>
		text_encoder.encode(result.patch).byteLength === result.patch_identity.byte_count
			? undefined
			: "Expected patch_identity.byte_count to equal the patch UTF-8 byte count",
	),
);

export type WorkspaceChangeDiffQueryResult = typeof WorkspaceChangeDiffQueryResult.Type;

/** Requests one source-free workspace replacement approval and its private unified diff. */
export const WorkspaceReplaceApprovalQuery = Schema.Struct({
	approval_id: Identifier,
	thread_id: Identifier,
});

export type WorkspaceReplaceApprovalQuery = typeof WorkspaceReplaceApprovalQuery.Type;

/** Returns one approval projection together with its bounded private unified diff. */
export const WorkspaceReplaceApprovalQueryResult = Schema.Struct({
	approval: WorkspaceReplaceApproval,
	diff: WorkspaceChangeDiffQueryResult,
});

export type WorkspaceReplaceApprovalQueryResult = typeof WorkspaceReplaceApprovalQueryResult.Type;

/** Records an explicit frontend approval or denial decision for one workspace replacement. */
export const WorkspaceReplaceApprovalResponseRequest = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
});

export type WorkspaceReplaceApprovalResponseRequest =
	typeof WorkspaceReplaceApprovalResponseRequest.Type;
