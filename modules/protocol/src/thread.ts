import { Schema } from "effect";

import { Identifier, IsoDateTime, StreamSequence } from "./common";

/** Enumerates meaningful actions that advance thread retention activity. */
export const ThreadActivityKind = Schema.Literals([
	"user_message",
	"run_started",
	"run_completed",
	"run_failed",
	"file_attached",
	"diff_attached",
	"terminal_attached",
	"process_attached",
	"renamed",
	"pinned",
	"unpinned",
	"archived",
	"restored",
]);

export type ThreadActivityKind = typeof ThreadActivityKind.Type;

/** Identifies how the current thread title was established. */
export const ThreadTitleSource = Schema.Literals(["initial", "automatic", "manual"]);

export type ThreadTitleSource = typeof ThreadTitleSource.Type;

/** Describes the durable thread projection sent to sidebar clients. */
export const ThreadListItem = Schema.Struct({
	activity_version: StreamSequence,
	archived_at: Schema.optional(IsoDateTime),
	created_at: IsoDateTime,
	current_goal: Schema.optional(Schema.NonEmptyString),
	last_activity_at: IsoDateTime,
	live_status: Schema.NonEmptyString,
	metadata_version: StreamSequence,
	pinned: Schema.Boolean,
	rename_suggestion: Schema.optional(Schema.NonEmptyString),
	thread_id: Identifier,
	title: Schema.NonEmptyString,
	title_locked: Schema.Boolean,
	title_source: ThreadTitleSource,
	updated_at: IsoDateTime,
});

export type ThreadListItem = typeof ThreadListItem.Type;

/** Creates a durable thread with an initial auto-managed identity. */
export const ThreadCreateCommand = Schema.Struct({
	type: Schema.Literal("thread.create"),
	title: Schema.NonEmptyString,
});

export type ThreadCreateCommand = typeof ThreadCreateCommand.Type;

/** Permanently locks a thread title to the user's chosen value. */
export const ThreadRenameCommand = Schema.Struct({
	type: Schema.Literal("thread.rename"),
	title: Schema.NonEmptyString,
});

export type ThreadRenameCommand = typeof ThreadRenameCommand.Type;

/** Applies asynchronous auto-managed metadata against explicit projection versions. */
export const ThreadMetadataRefineCommand = Schema.Struct({
	basis_activity_version: StreamSequence,
	basis_metadata_version: StreamSequence,
	current_goal: Schema.optional(Schema.NonEmptyString),
	live_status: Schema.NonEmptyString,
	rename_suggestion: Schema.optional(Schema.NonEmptyString),
	title: Schema.optional(Schema.NonEmptyString),
	type: Schema.Literal("thread.metadata.refine"),
});

export type ThreadMetadataRefineCommand = typeof ThreadMetadataRefineCommand.Type;

/** Records one meaningful activity that resets the retention horizon. */
export const ThreadActivityRecordCommand = Schema.Struct({
	activity_kind: ThreadActivityKind,
	type: Schema.Literal("thread.activity.record"),
});

export type ThreadActivityRecordCommand = typeof ThreadActivityRecordCommand.Type;

/** Exempts a thread from automatic retention deletion. */
export const ThreadPinCommand = Schema.Struct({ type: Schema.Literal("thread.pin") });

/** Removes a thread's automatic-retention exemption. */
export const ThreadUnpinCommand = Schema.Struct({ type: Schema.Literal("thread.unpin") });

/** Moves a thread out of the active sidebar while preserving normal retention. */
export const ThreadArchiveCommand = Schema.Struct({ type: Schema.Literal("thread.archive") });

/** Returns an archived thread to the active sidebar. */
export const ThreadRestoreCommand = Schema.Struct({ type: Schema.Literal("thread.restore") });

/** Describes the intentionally small global inactive-thread retention setting. */
export const ThreadRetentionInactivityDays = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(1),
	Schema.isLessThanOrEqualTo(3650),
);

export const ThreadRetentionPolicy = Schema.Struct({
	enabled: Schema.Boolean,
	inactivity_days: ThreadRetentionInactivityDays,
});

export type ThreadRetentionPolicy = typeof ThreadRetentionPolicy.Type;

/** Updates the global inactive-thread retention policy through the durable command path. */
export const ThreadRetentionUpdateCommand = Schema.Struct({
	type: Schema.Literal("thread.retention.update"),
	...ThreadRetentionPolicy.fields,
});

export type ThreadRetentionUpdateCommand = typeof ThreadRetentionUpdateCommand.Type;

/** Defines the event payload emitted when a thread is created. */
export const ThreadCreatedEvent = Schema.Struct({
	type: Schema.Literal("thread.created"),
	title: Schema.NonEmptyString,
});

export type ThreadCreatedEvent = typeof ThreadCreatedEvent.Type;

/** Projects one durable thread identity, lifecycle, or meaningful activity change. */
export const ThreadMetadataUpdatedEvent = Schema.Struct({
	activity_kind: Schema.optional(ThreadActivityKind),
	change: Schema.Literals([
		"activity",
		"archive",
		"metadata",
		"pin",
		"rename",
		"restore",
		"unpin",
	]),
	thread: ThreadListItem,
	type: Schema.Literal("thread.metadata.updated"),
});

export type ThreadMetadataUpdatedEvent = typeof ThreadMetadataUpdatedEvent.Type;

/** Records an accepted refinement result that lost its projection-version race. */
export const ThreadRefinementIgnoredEvent = Schema.Struct({
	basis_activity_version: StreamSequence,
	basis_metadata_version: StreamSequence,
	type: Schema.Literal("thread.refinement.ignored"),
});

export type ThreadRefinementIgnoredEvent = typeof ThreadRefinementIgnoredEvent.Type;

/** Records a durable update to the global inactive-thread retention policy. */
export const ThreadRetentionPolicyUpdatedEvent = Schema.Struct({
	policy: ThreadRetentionPolicy,
	type: Schema.Literal("thread.retention.updated"),
});

export type ThreadRetentionPolicyUpdatedEvent = typeof ThreadRetentionPolicyUpdatedEvent.Type;

/** Replaces erased historical thread content while preserving replay coordinates. */
export const ThreadContentErasedEvent = Schema.Struct({
	type: Schema.Literal("thread.content_erased"),
});

export type ThreadContentErasedEvent = typeof ThreadContentErasedEvent.Type;

/** Marks the durable terminal point of an erased thread stream. */
export const ThreadErasedEvent = Schema.Struct({
	type: Schema.Literal("thread.erased"),
});

export type ThreadErasedEvent = typeof ThreadErasedEvent.Type;
