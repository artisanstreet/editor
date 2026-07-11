import { Schema } from "effect";

import { Identifier, IsoDateTime, JournalSequence, PositiveInt, RawOrigin } from "./common";

const text_encoder = new TextEncoder();

/** Defines the V1 control-frame ceiling for workspace source text at four MiB of UTF-8 data. */
export const workspace_text_maximum_bytes = 4 * 1024 * 1024;

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
